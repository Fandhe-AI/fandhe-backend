# AGENTS.md

## 文書の位置づけ

本リポジトリで作業するすべての AI エージェント・開発者が従う設計規約集。二つの役割を持つ:

1. 実装コード（`crates/**`）から直接参照される横断的な設計規約（例:
   「規約: ミドルウェア非同期 I/O 必須化」節）。`CLAUDE.md` / `.claude/rules/` と
   内容を重複させない
2. AI エージェントが安全に改修するための変更ガイド（「AI エージェント向け変更ガイド」
   節、TASK-11.3・#35）。REQ-11 が要求する機械可読性のため、モジュール境界・変更手順・
   完了判定・エスカレーション基準等の要点を本書に集約するが、各項目の一次情報源
   （`docs/design/*.md`・`.claude/rules/*.md`）を正とし、詳細はそちらを参照する

全体の運用ガイドは `CLAUDE.md`、Rust コーディング規約の詳細は `.claude/rules/`
（特に [coding-rust.md](.claude/rules/coding-rust.md)）を参照する。

## 規約: ミドルウェア非同期 I/O 必須化

TASK-2.3（`docs/spec/05-tasks.md`、Phase 1 / MS-1、親 Issue #4、前提 TASK-2.1 #18）
対応。`docs/spec/04-requirements.md` REQ-2 受け入れ基準・NFR-7 を満たす規約文書。

### 規約本文

全リクエストに介入する `Middleware` 実装（`crates/core/src/extension.rs` の
`Middleware` trait、`on_request` / `on_response`）は**非同期・バッファ済み I/O を
既定**とする。同期ブロッキング I/O 実装（同期 `eprintln!`・同期ファイル書き込み・
`std::net` 直接利用等）は**不採用**とする。

`Middleware` trait 自体は dyn 互換性（`Box<dyn Middleware>` としてコアループが
拡張点を保持する構成）を保つため `async fn` を持たない同期 API として定義される
（`crates/core/src/extension.rs` モジュール doc「非同期・I/O に関する規約」節）。
本規約はこの同期 API の**制約下で守るべき実装契約**であり、trait のシグネチャ変更
を求めるものではない。

### 実装パターン

I/O が必要な実装は、フック（`on_request` / `on_response`）内では非同期チャネルへの
送信、またはアトミックカウンタの更新等の**非ブロッキング操作に留め**、実際の I/O
（ファイル書き込み・ネットワーク送信等）は別タスク（バックグラウンドタスク・
`tracing-appender` の non-blocking writer 等）に委譲する。

### 根拠（PoC-3 実測、`docs/spec/03-poc/plugin-mechanism/README.md`）

全リクエストに介入するミドルウェア型プラグイン（ロギング）を素朴な同期 I/O
（リクエストごとの同期 `eprintln!`）で実装すると、`/health` の RPS が
**725,024 → 44,108 RPS（無効時比 25.0%）** まで劣化した。同一の `Middleware`
trait 実装のまま I/O を停止し、アトミックカウンタの更新のみに切り替えて計測
（`ACCESS_LOG_QUIET=1`）すると **177,549 RPS（無効時比 100.5%）** まで回復した。

この切り分けにより、劣化要因は「`Middleware` trait 呼び出し（動的束縛）のコスト
自体」ではなく「プラグインが選んだ I/O 実装の質（同期か非同期か）」であることが
実証された。

補足として、PoC-10（`docs/spec/04-requirements.md` REQ-10）でも同旨の実測がある。
可観測性ミドルウェアを同期 writer で実装した場合に RPS が **63.0% 劣化**すること
に加え、非同期 writer に切り替えても span/event 生成の CPU コストにより RPS が
31.6% 劣化する事例が確認されており、**非同期 I/O 化だけでは pay-for-what-you-use
の性能目標を満たさない場合がある**（サンプリング・イベント数削減・高頻度パス除外
等の追加対策は REQ-10 側のスコープであり、本規約は「同期 I/O の不採用」という
最小限の必須要件を定めるものである）。

### 出典リンク

- `docs/spec/03-poc/plugin-mechanism/README.md`（PoC-3 性能比較表・発見事項）
- `docs/spec/02-poc-plan.md`（PoC-3 計画）
- `docs/spec/04-requirements.md`（REQ-2・NFR-7、参考: REQ-10・PoC-10）
- `docs/spec/05-tasks.md`（TASK-2.3）
- `crates/core/src/extension.rs`（`Middleware` trait 定義・同旨の契約を doc comment に記載）

### 適用範囲と検証責務

標準提供ミドルウェア有効化時のコア RPS 劣化は 5% 以内を維持する（NFR-7 受け入れ
基準）。レビュー時の本規約準拠確認は `reviewer` / `plugin-builder`、性能検証は
`bench-builder` が担う（[delegation-impl.md](.claude/rules/delegation-impl.md)）。

### 可用性・可観測性に関する注記

- **リソース枯渇（DoS）耐性**: 全リクエストのホットパスに載るミドルウェアが同期
  I/O を行うと、スロー I/O（ディスク詰まり・パイプブロック等）発生時にワーカー
  スレッドが枯渇し、サービス全体が応答不能に陥りうる。本規約はこのリスクを構造的
  に排除する（[security.md](.claude/rules/security.md) の「リソース枯渇（DoS）」
  観点）。
- **ログ欠落の許容可否**: 非同期・バッファ済みログはバックプレッシャ時にイベント
  欠落（drop）が起こりうる。セキュリティ監査イベント等、欠落を許容できないログの
  扱いは、標準ロギング／トレーシング実装（REQ-10・`plugin-tracing` 系タスク）側
  の設計事項として別途定める。本規約はこの論点を暗黙に決定しない。

## AI エージェント向け変更ガイド

TASK-11.3（#35、`docs/spec/05-tasks.md` Phase 3 / MS-3、`docs/spec/04-requirements.md`
REQ-11）対応。AI がこのリポジトリを安全に改修するための、モジュール境界・変更手順・
完了判定・アサーション規約・安全性方針・エスカレーション基準を機械可読な形でまとめる。
運用・委譲の詳細は `CLAUDE.md`、Rust コーディング規約の詳細は
[coding-rust.md](.claude/rules/coding-rust.md) を正とし、本節は重複させず要点と
一次情報源への参照のみを記載する。

### モジュール境界

workspace 内クレート間の依存方向は次の一方向を維持する（`crates/core/src/lib.rs`
モジュール doc・`scripts/dep-direction-check.sh` と同一の宣言）。

```text
server → routes → http::*
```

- `crates/core` はこの依存グラフの末端に位置し、`crates/plugin-*` の固有シンボルには
  一切依存しない（pay-for-what-you-use、
  [pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md)）。
  プラグインは feature 経由でコアの拡張点を実装する側であり、コアからプラグインへの
  依存は発生しない設計とする
- **既知の例外（是正中）**: `crates/core` → `bf-plugin-webrtc-proxy`
  （`webrtc-proxy` feature 経由）は現状の依存グラフで許可リスト化された例外であり、
  是正は Issue #136（`fix(core): crates/core が bf_plugin_webrtc_proxy に直接依存し
  依存方向一方向性に違反`）で追跡する。新規変更でこの例外を拡大しない
- 機械検証: `bash scripts/dep-direction-check.sh`（`cargo metadata` の依存エッジを
  許可リストと照合、循環依存検出、コアへのプラグイン固有シンボル混入を grep 検出）

crates 一覧と責務（`crates/` 直下、`ls` で最新を確認できる）:

| クレート | 責務 |
|---------|------|
| `core` | HTTP/1.1 パーサ・keep-alive・3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）を持つ最小コア |
| `http` | sans-IO な HTTP プリミティブ（`bf-http`）。workspace 内で最下層 |
| `routes` | ルーティング（`bf-routes`）。`server → routes → http::*` の中間層 |
| `plugin-websocket` | WebSocket（RFC 6455 ハンドシェイク・`UpgradeHandler` 拡張点） |
| `plugin-graphql` | GraphQL プラグイン境界 |
| `plugin-openapi` | OpenAPI ドキュメント生成 |
| `plugin-webrtc` | in-process WebRTC（`webrtc-rs` 直接依存） |
| `plugin-webrtc-proxy` | WebRTC シグナリングプロキシ（別プロセス切り出し型） |
| `plugin-hub-wiring` | hub 共通配線（`RequestGate` 上の `TenantGate`。JWT (RS256 + JWKS) 検証 → `org_id` 抽出 → フェイルクローズ。依存逆転型プラグイン、`docs/design/plugin-boundary.md` 5.6 節）。越境アクセス監査ログ（`audit` モジュール、`cross_tenant_attempt` カテゴリ。「正当な 404」と「越境 404」を外部応答同一のまま監査ログのみで区別、TASK-9.6・#89） |
| `axum-ref` | 性能比較用参照実装 |

### 変更手順

拡張点変更は、まず 3 種 trait（`Middleware` / `UpgradeHandler` / `RequestGate`、
`crates/core/src/extension.rs`）のいずれかに載るか判定することから入る
（[coding-rust.md](.claude/rules/coding-rust.md)）。feature の新規追加・変更は
[pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md) と
[feature-modification-flow.md](docs/design/feature-modification-flow.md) に従う。

#### 新規エンドポイント追加手順

1. `bf_routes::Router::route()` へのルート登録
2. ハンドラ実装（対象クレートは「モジュール境界」節の crates 一覧・
   [delegation-impl.md](.claude/rules/delegation-impl.md) のパスベース委譲に従い判断する）
3. doc コメント + doc test（`# Examples`）を付与する
   （[code-comment-style.md](.claude/rules/code-comment-style.md)）
4. 「アサーション網羅性」節に従う網羅的アサーション付きテストを追加する
5. **本 AGENTS.md の更新をサブタスクとして必ず含める**。エンドポイント・拡張点追加時に
   本書が古びていないかを確認し、必要な追随を行う（本節が確立する運用。
   [feature-modification-flow.md](docs/design/feature-modification-flow.md) 8 節が
   参照する追随先）

### 変更完了の判定基準

変更ごとに以下をすべて満たすことを確認する。コマンドの正確な集合・CI ジョブ構成は
[ci-completion-criteria.md](docs/design/ci-completion-criteria.md) を正とし、本節では
二重管理しない（ジョブ追加・改名時は同書側が更新される）。

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`（`.config/nextest.toml` の
  `slow-timeout` 設定によりテスト単位タイムアウト付き。`cargo nextest run` でも可）
  + `cargo test --doc`
- `cargo doc`（`RUSTDOCFLAGS="-D warnings"`）
- CI 集約ゲート `ci-complete` が green
- ドキュメント追随が完了していること
  （[feature-modification-flow.md](docs/design/feature-modification-flow.md) 8 節の
  変更種別 → 追随ドキュメントのマッピングに従う）
- 受け入れ基準を充足していること（人間判断によるレビューゲート。
  [feature-modification-flow.md](docs/design/feature-modification-flow.md) 9 節）

上記のいずれかが未充足のまま「変更完了」とみなさない（fail-closed）。

### アサーション網羅性

PoC-9（`docs/spec/03-poc/ai-first-maintainability/README.md`）では、HTTP レスポンスの
ボディ内容のみを検証しステータス行・`Content-Type` を検証しないテストがバグを見逃す
事例が確認された。この教訓に基づき、HTTP レスポンスを検証するテストは次を**すべて**
検証する:

- ステータス行（ステータスコード）
- ヘッダ（少なくとも `Content-Type` / `Content-Length`）
- ボディ

ボディ内容の一致のみで「テストが通った」と判断しない。新規エンドポイント追加・既存
エンドポイント変更のテストはこの規約に従う（「変更手順」節・
[feature-modification.md](.claude/rules/feature-modification.md) の「実装にはテスト
追加を伴う」と併せて適用する）。

### 安全性方針

- `unsafe` は最小限に留め、使う場合は `// SAFETY:` コメントで不変条件と安全性の根拠を
  必ず書く（[coding-rust.md](.claude/rules/coding-rust.md)）
- workspace lints は 2 層防御を敷く（詳細は
  [unsafe-deny-lints.md](docs/design/unsafe-deny-lints.md)）: 第 1 層は `forbid`
  （`#[allow]` による抑制自体を禁止）、第 2 層は `deny`（正当理由があれば局所
  `#[allow]` + レビューで例外化可能）
- OWASP Top 10 観点（入力検証・認証認可・インジェクション・リソース枯渇・
  暗号/シークレット管理・可観測性）は
  [security.md](.claude/rules/security.md) を正とする
- pay-for-what-you-use（feature 無効時の依存・コード・`unsafe`・バイナリ増ゼロ）は
  [pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md) を正とする
- WebRTC（`plugin-webrtc` / `plugin-webrtc-proxy`）の安全性方針の詳細（プロセス分離等）は
  [webrtc-process-isolation.md](docs/design/webrtc-process-isolation.md)、および攻撃表面と
  「使う/使わない」サービスの安全性方針の詳細は本書「規約: WebRTC の攻撃表面と
  「使う/使わない」サービスの安全性方針」節（TASK-8.4、#29）を参照

### エスカレーション基準

対応可否の自律判断は「可 / 条件付き可 / 不可・要エスカレーション / 不可（明確な拒否）」
の 4 値で判定する（詳細は
[feasibility-guardrail.md](docs/design/feasibility-guardrail.md)、運用規約は
[feasibility-guardrail.md（rules）](.claude/rules/feasibility-guardrail.md)）。

判定の 3 軸（いずれか 1 つでも不充足なら「可」と判定しない、fail-closed）:

1. 実施可能か（検証可能な受け入れ基準に落ちるか）
2. 安全か（[security.md](.claude/rules/security.md)・OWASP Top 10 と整合するか）
3. 影響範囲が許容内か（クレート・feature・利用者への影響が特定・限定できるか）

不可判定 4 カテゴリ（代表例、網羅列挙ではない）:

| カテゴリ | 判定条件 | 判定区分 |
|---------|---------|---------|
| 曖昧要求 | 受け入れ基準がなく曖昧語のみで完遂を測定不能 | 不可・要エスカレーション |
| 未定義依存 | 依存・接続情報・方式が未定義 | 不可・要エスカレーション |
| 安全性方針との衝突 | 既存安全性方針（DoS 耐性・境界検証等）を後退させる | 不可・要エスカレーション |
| 明確な脆弱性を招く要求 | OWASP Top 10 に直結する脆弱性（RCE・インジェクション等）が明白 | 不可（明確な拒否） |

判断不能な場合は安全側に倒し、実装を進めずエスカレーションする（fail-closed 原則。
判定記録の形式検証は `bash scripts/feasibility-check.sh --input <record>` で行う）。

## 規約: WebRTC の攻撃表面と「使う/使わない」サービスの安全性方針

TASK-8.4（`docs/spec/05-tasks.md`、Phase 2 / MS-2、#29）対応。`docs/spec/04-requirements.md`
REQ-8（WebRTC）受け入れ基準・NFR-6（拡張の非侵襲性）を満たす運用規約文書。

### 背景: 2 クレートの対照

backend-framework は WebRTC を 2 つの独立クレートで提供し、**クレート境界で完全に
分離**する（相互 path 依存なし。`docs/dep-impact/records.md` の TASK-8.4 エントリで
機械検証済み）。

| クレート | feature | 依存モデル | 攻撃表面 |
|---------|---------|-----------|---------|
| `crates/plugin-webrtc` | `webrtc` | `webrtc-rs`（0.17.1 系）を**プロセス内**に直接組み込む（in-process） | 大（`webrtc` feature 単体で `cargo tree -p backend-framework-core --features webrtc` に webrtc 系依存 23 件、release バイナリサイズ約 11 倍、TASK-8.4 実測。`docs/dep-impact/records.md`） |
| `crates/plugin-webrtc-proxy` | `webrtc-proxy` | `webrtc-rs` に**一切依存しない**軽量シグナリングプロキシ。重い WebRTC サービスは別プロセスへ切り出す | 小（`webrtc-rs` 依存が本体プロセスに一切現れない） |

`crates/core/src/plugin.rs` の `try_intercept` は両 feature が同時に有効な場合
（`--all-features` CI 構成）、`webrtc-proxy` を先に評価する（REQ-8 の MVP 推奨方式を
優先する運用判断。両方を `Server` に登録した場合は `webrtc-proxy` が優先され、
`webrtc` 側の設定は評価されない）。

### 安全性方針

- **WebRTC を使わないサービス**: `webrtc`・`webrtc-proxy` のどちらの feature も有効化
  しない。依存・`unsafe`・バイナリ増をゼロに保つ（pay-for-what-you-use、
  [pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md)）。`cargo tree -p
  backend-framework-core` にいずれの feature 無効時も webrtc 系依存が一切現れないこと
  を維持する。
- **WebRTC を使うサービス**: 可能な限り `plugin-webrtc-proxy`（`webrtc-proxy` feature）
  による**別プロセス切り出し**を第一選択とする。`webrtc-rs` の巨大な依存グラフ・
  パーサ群をコアプロセスから隔離し、脆弱性発生時の影響範囲・監査対象を限定できる。
- **in-process `plugin-webrtc`（`webrtc` feature）を選ぶ場合**: 別プロセス切り出しの
  運用コスト（プロセス間通信・デプロイ構成の複雑化）が許容できない場合に限り検討する。
  有効化すると `webrtc-rs` の巨大な依存グラフ・パーサ群がコアプロセスに直接組み込まれ、
  ICE 接続性チェックはクライアント SDP 由来のアドレスへ UDP 送信を発生させ得る（WebRTC
  の構造上不可避）。STUN/TURN は既定で設定しない（`RTCConfiguration::default()`）。
  Offer サイズ上限・接続数上限（503 フェイルクローズ）・シグナリングタイムアウト
  （504）は維持されている（`crates/plugin-webrtc/tests/attack_surface.rs` で受け入れ
  観点から再アサート済み）が、依存グラフそのものの大きさは変わらない。

### NFR-6（無関係パスへの性能影響）に関する留意事項

NFR-6 は「パス一致時のみ介入する拡張点は、無関係なパスへの RPS・レイテンシ影響が
誤差範囲内（100.3〜100.8%相当）である」ことを求める。この帯は GraphQL（PoC-3、依存
インパクトが軽微なパスインターセプト型）由来の実測に基づく。TASK-8.4 の empirical
計測（`benches/webrtc-nfr6-bench.sh`、`benches/reports/task-8.4-webrtc-nfr6.md`）では、
`webrtc` feature 有効時の無関係パス（`GET /`）RPS が baseline 比おおむね 94〜95%、
p95 レイテンシがおおむね 106〜108% となり、狭義の 100.3〜100.8% 帯には収まらなかった。
`try_intercept` 自体は対象外パスに対して 1 回のパス比較のみでフォールスルーするため
（`crates/core/src/plugin.rs`）、この差は拡張点の呼び出しコストではなく、バイナリ
サイズが約 11 倍に達すること（icache/TLB 圧迫等）に起因すると考えられる。**WebRTC を
使うサービスがこの性能影響を避けたい場合も、`plugin-webrtc-proxy` による別プロセス
切り出しが有効な緩和策となる**（プロキシプロセスとコアプロセスが分離するため、コア
プロセスのバイナリサイズ・性能特性は影響を受けない）。

### 出典リンク

- `docs/design/webrtc-process-isolation.md`（別プロセス切り出しの設計判断）
- `docs/design/webrtc-rs-version-strategy.md`（`webrtc-rs` バージョン戦略、TASK-8.3）
- `docs/acceptance/req8-webrtc-attack-surface.md`（TASK-8.4 攻撃表面評価・受け入れ判定）
- `docs/dep-impact/records.md`（依存インパクト計測記録）
- `docs/spec/04-requirements.md`（REQ-8・NFR-6）
- `docs/spec/05-tasks.md`（TASK-8.1〜TASK-8.4）

### 適用範囲と検証責務

`webrtc`/`webrtc-proxy` 両 feature の依存完全除外・クレート境界分離の機械検証は
`scripts/accept/webrtc-accept.sh`、NFR-6 の empirical 計測は `bench-builder` が担う
（[delegation-impl.md](.claude/rules/delegation-impl.md)）。
