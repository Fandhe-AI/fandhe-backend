# `RequestGate::check` へ実 peer `SocketAddr` を渡す API（イシュー #486）

## 1. 背景・目的

`RequestGate::check(&self, head: &RequestHead) -> GateOutcome` には接続元の実
peer `SocketAddr` を参照する手段がなく、ダウンストリーム（
Fandhe-AI/local-llm-server）は実 peer IP の CIDR 照合（allowed_cidrs）を gate
上で実装できなかった。このため利用側は自前 accept のフロントリスナー
（`TlsFront`/`PlainFront`）+ 内部ループバック `Server` 中継 + 固定値リゾルバ
（`FixedSourceAddr`）という回避構成を強いられていた（local-llm-server
ADR-0013）。

本イシューは、accept したソケットの peer address を gate 層まで伝搬し、
`RequestGate` 実装から参照可能にする恒久解を提供する。

## 2. 現状の実装（変更前）

- `crates/core/src/server.rs` の accept ループ（`BoundServer::run_until`）は
  `listener.accept()` が返す peer addr をその場で破棄していた
  （`Ok((stream, _peer_addr)) => ...`）。
- ゲート評価点は `first_rejection(&server.gates, &request.head)` の 1 箇所の
  み（ルーティング・Upgrade 評価より前）。
- 接続処理 `handle_connection_with_permit<S: AsyncRead + AsyncWrite>` は
  ストリームをジェネリックに受け、`tokio::io::duplex` によるソケット不要
  テストを許容している。実ソケットでない経路では peer addr が原理的に存在
  しない。

## 3. 設計判断

### 3.1 API 形状の比較と採用案

| 案 | 内容 | 評価 |
|---|---|---|
| **A（採用）**: `check` へコンテキスト引数追加（breaking） | `fn check(&self, head: &RequestHead, ctx: &GateContext) -> GateOutcome` | 正準メソッドが 1 つでフェイルクローズ契約が曖昧にならない。#424 の前例（BREAKING CHANGE + CHANGELOG 移行手順）に倣える |
| B: 既定メソッド追加（非破壊） | `check_with_context` を provided method で追加しコアはそちらを呼ぶ | 実装者が 2 メソッドを意識する必要があり、「どちらが呼ばれるか」の契約が二重化する。peer 依存 gate でも旧 `check` の実装を強制され、呼び分けミスの余地が残る |
| C: `RequestHead` へ peer を持たせる | — | `RequestHead` はバイト列から構築される sans-IO 型（`crates/http`）であり、接続層の情報を混入させると責務境界（依存方向 `server → routes → http`）が崩れる。不採用 |

案 A を採用した。単一の正準メソッドを維持することがフェイルクローズ契約
（AI ファースト保守性）に最も適合し、リポジトリ内の追随箇所は小規模で
機械的に完結する。

### 3.2 `GateContext` の形状

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateContext {
    peer_addr: Option<SocketAddr>,
}

impl GateContext {
    pub fn new(peer_addr: Option<SocketAddr>) -> Self;
    pub fn peer_addr(&self) -> Option<SocketAddr>;
}
```

- `peer_addr` を `Option` にする理由: `handle_connection` は
  `AsyncRead + AsyncWrite` ジェネリックで duplex ストリーム（統合テスト）
  を受けるため、peer が存在しない経路が正当に存在する。**peer addr を判定
  に必要とする gate 実装は `None` 時に必ず `Reject` を返す**（フェイル
  クローズ）ことを trait doc に契約として明記した。
- `Copy` 型・ヒープ割当なしで、gate 未登録時・登録時いずれも追加コストは
  実質ゼロ（pay-for-what-you-use。feature ゲート不要・外部依存ゼロ）。
- 公開コンストラクタを設けたのは、ダウンストリームが gate 単体テストで
  コンテキストを組み立てられるようにするため。
- フィールドは非公開とし、将来の項目追加（`local_addr` 等）を非破壊にする。

### 3.3 伝搬経路と公開 API

1. accept ループ（`server.rs`）: `Ok((stream, peer_addr)) => Some((stream, peer_addr, permit))` とし、spawn するタスクへ move する。
2. `handle_connection_with_permit` に `peer_addr: Option<SocketAddr>` 引数を
   追加し、接続ループ先頭で `GateContext` を 1 回構築する（`Copy` なので
   保持コストなし）。
3. `first_rejection(gates, head, ctx)` へ引数追加し、`gate.check(head, ctx)`
   を呼ぶ。
4. 既存公開 API `handle_connection(server, stream)` は `None` を渡す薄い
   ラッパーとして無変更（非破壊）。
5. 新公開 API `handle_connection_with_peer_addr(server, stream, peer_addr)`
   を追加し `lib.rs` から再エクスポート。duplex テスト・カスタム accept
   ループから peer を注入可能にする。

TLS 終端はフレームワーク v1 スコープ外（リバースプロキシ前提、
`docs/design/v1-scope-tls-multipart.md`）のため、本実装が伝搬するのは常に
TCP accept 時の peer address であり、TLS の有無による分岐は生じない。

## 4. セキュリティ考慮（OWASP Top 10 観点）

- **A01 アクセス制御**: 本 API はダウンストリームの CIDR 許可リストを gate
  層で正しく実装可能にする（回避構成の内部 listener 削減 = 攻撃表面縮小に
  寄与）。`peer_addr` は TCP accept したソケットの peer であり、
  `X-Forwarded-For` / `Forwarded` ヘッダ（クライアント申告値・偽装可能）
  とは別物。IP ベース認可では偽装可能ヘッダではなく `ctx.peer_addr()` を
  使うこと。
- **フェイルクローズ（A04/A07）**: peer addr を判定に必要とする gate は
  `peer_addr() == None` で必ず `Reject` を返す契約を trait doc に明記した
  （既存の「判定不能時は Reject」契約の具体化）。コア側で `None` を実
  アドレスに偽装する経路は作らない。
- **プロキシ配下の意味論**: リバースプロキシ/LB 配下では peer はプロキシ
  のアドレスになる（v1 の TLS 終端方針 `docs/design/
  v1-scope-tls-multipart.md` と整合）。この前提を doc・本設計ドキュメント
  に明記し、誤用（プロキシ配下で CIDR 照合が常にプロキシ IP に一致する
  等）を防ぐ。
- **インジェクション（A03）**: `SocketAddr` は型付き値で文字列パース・
  ヘッダ由来の入力を含まず、新たな注入経路は生じない。
- **ログ・PII**: コアは peer addr をログ出力しない（現状維持）。gate
  実装が peer IP をログへ出す場合は PII 相当としての取り扱い注意を
  `GateContext` の doc に一言添えている。
- **DoS（リソース枯渇）**: `GateContext` は `Copy`・割当なしで接続あたりの
  追加コストは定数。既存の同時接続上限・タイムアウト機構に影響しない。

## 5. 影響範囲

- `crates/core`（`extension.rs` / `server.rs` / `lib.rs`）: `RequestGate::
  check` シグネチャ変更（BREAKING）、`GateContext` 新設、
  `handle_connection_with_peer_addr` 新設。
- `crates/plugin-hub-wiring`（`TenantGate`）: シグネチャ追随のみ。判定
  ロジック（`org_id` ベースのテナント境界）は無変更、`ctx` は未使用
  （`_ctx`）。
- `templates/` / `examples/`: `RequestGate` の独自実装なし（grep 確認
  済み）のため変更なし。
- 依存追加なし（`std::net::SocketAddr` のみ）。

## 6. 検証方法

```bash
cargo build -p fandhe-backend-core --all-features
cargo build -p fandhe-backend-core --no-default-features   # feature なし構成の依存無変化確認
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --check
cargo doc -p fandhe-backend-core -p fandhe-backend-plugin-hub-wiring --all-features --no-deps
```

- 実 peer address 伝搬の e2e 検証: `crates/core/src/server.rs` の
  `request_gate_receives_real_peer_addr_over_tcp`（`127.0.0.1:0` へ実 bind
  → `run()` → 実 TCP 接続 → gate が観測した `peer_addr` がクライアント側
  `local_addr()` と一致することを確認）。
- duplex 経路の `None` 契約: `handle_connection_duplex_path_yields_none_peer_addr`。
- 注入 API の契約: `handle_connection_with_peer_addr_injects_supplied_addr`。
- フェイルクローズ規約（`peer_addr() == None` で `Reject`）: `crates/core/
  src/extension.rs` の `request_gate_peer_required_rejects_when_peer_addr_missing`。

## 7. スコープ外（`.claude/rules/out-of-scope-tracking.md` 対象候補）

- `Middleware` / `UpgradeHandler` / `Interceptor` への peer addr 伝搬（本
  イシューは `RequestGate` のみ。必要になった時点で `GateContext` と同型の
  パターンを適用可能）。
- `X-Forwarded-For` 等の信頼済みプロキシ解決（trusted proxies）機構。
- local-llm-server 側の回避構成撤去（ダウンストリームの作業）。
