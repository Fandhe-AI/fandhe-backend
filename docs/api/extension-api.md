# 同期 3 拡張点契約リファレンス（Middleware / UpgradeHandler / RequestGate）

## 1. 目的と位置づけ

本書は `fandhe-backend-core` の `extension` モジュールが公開する 3 種の拡張点 trait
（`Middleware` / `UpgradeHandler` / `RequestGate`）の全体像・契約・呼び出しタイミングを
俯瞰する読み物である。個々のシグネチャ・doc test を含む一次情報源は rustdoc
（`crates/core/src/extension.rs` の doc comment）であり、本書と記述が食い違う場合は
rustdoc を正とする。

- 設計原則「拡張点は 4 種 trait に集約」の実体のうち、`extension` モジュールが公開する
  同期 3 trait（`Middleware` / `UpgradeHandler` / `RequestGate`）を扱う。4 種目の
  `Interceptor`（`interceptor` モジュール、リダイレクト・確定済みレスポンスの改変を
  担う feature ゲート不要のレスポンダ系シーム）は本書のスコープ外であり、
  [./interceptor-api.md](./interceptor-api.md) を参照する。プラグイン
  （`crates/plugin-*`）はこれらの trait を実装する側であり、コアがプラグイン
  固有シンボルに依存することはない
- 3 trait はクレート直下にも re-export される（`fandhe_backend_core::Middleware` 等）
- trait 自体は feature によらず無条件で公開されるが、実装ゼロなら実行時コストもゼロ
  （pay-for-what-you-use に反しない）
- 登録は `Server::middleware` / `Server::gate` / `Server::upgrade_handler`
  （[server-api.md](./server-api.md) 参照）
- 自作手順・実装例は [../guide/extension-points.md](../guide/extension-points.md) を参照

## 2. 公開 API 一覧

### 2.1 `Middleware` — 観測専用フック

| メソッド | シグネチャ概略 | 説明 |
|---------|---------------|------|
| `name` | `fn (&self) -> &'static str` | 診断・ログ表示用の静的識別名 |
| `on_request` | `fn (&self, &RequestHead)` | リクエストヘッド受理後・ルーティング前に呼ばれる |
| `on_response` | `fn (&self, &RequestHead, Duration)` | レスポンス送出後に呼ばれる。`Duration` は受理から送出までの経過時間 |

ロギング・メトリクス等の横断的関心事向け。**レスポンスへの参照を持たない**
（`on_response` の引数はリクエストヘッドと経過時間のみ）。レスポンスの読み取り・
書き換えはできない。

### 2.2 `UpgradeHandler` — 長時間接続への委譲判定

| メソッド | シグネチャ概略 | 説明 |
|---------|---------------|------|
| `name` | `fn (&self) -> &'static str` | 診断・ログ表示用の静的識別名 |
| `matches` | `fn (&self, &RequestHead) -> bool` | このリクエストが自分の担当するアップグレードプロトコルに該当するかを判定 |

**判定のみ**の拡張点。`matches` が `true` を返すと以降の接続処理はプラグイン側へ
委譲され、フレーミング・プロトコルアップグレード後の読み書きは本 trait の責務外
（`crates/plugin-websocket` 等に閉じる）。

### 2.3 `RequestGate` — 早期拒否ゲート

| メソッド | シグネチャ概略 | 説明 |
|---------|---------------|------|
| `name` | `fn (&self) -> &'static str` | 診断・ログ表示用の静的識別名 |
| `check` | `fn (&self, &RequestHead) -> GateOutcome` | リクエストヘッドを検査し許可/拒否を判定 |

### 2.4 `GateOutcome` — 判定結果

| variant | フィールド | 説明 |
|---------|-----------|------|
| `Allow` | なし | 許可。以降の処理（ルーティング等）を続行 |
| `Reject` | `status: u16` / `body: Vec<u8>` | 拒否。`status` がレスポンスのステータスコード、`body` がボディの生バイト列になる |

許可/拒否の判定結果のみを運び、JWT クレーム・`org_id` 等のプラグイン固有データを
コアへ持ち込まない。`Reject` の `status` を数値（`u16`）に限定するのは、任意文字列を
ステータス行へ書き出す設計を避け、レスポンス分割・ヘッダインジェクションを型レベルで
排除するため（reason phrase の付与はコア側の責務）。

## 3. 呼び出しタイミング比較

コアのリクエスト処理パイプライン（1 リクエストあたり）における評価位置。

| 順序 | ステップ | 拡張点 | 備考 |
|------|---------|--------|------|
| 1 | `Middleware::on_request` | Middleware | 登録順に全件呼び出し |
| 2 | `RequestGate::check` | RequestGate | 登録順に評価、最初の `Reject` を優先。拒否時は以降のステップへ進まない |
| 3 | `UpgradeHandler::matches` | UpgradeHandler | 登録順に評価。マッチしたら接続ごとプラグインへ委譲（以降のステップなし） |
| 4 | パスインターセプト型プラグイン | （拡張点外） | WebRTC・GraphQL・OpenAPI・静的配信等。`Some(response)` なら 5 をスキップ |
| 5 | `Handler::handle` | （既定ハンドラ） | 未登録時は 404 |
| 6 | レスポンス後処理型プラグイン | （拡張点外） | CORS ヘッダ付与 → gzip 圧縮の順で逐次適用（4・5 双方の応答が対象） |
| 7 | レスポンス書き込み → `Middleware::on_response` | Middleware | 登録順に全件呼び出し |

| 観点 | Middleware | UpgradeHandler | RequestGate |
|------|-----------|----------------|-------------|
| 目的 | 観測（ロギング・計測） | 長時間接続への委譲判定 | 早期拒否（認証・認可・同意） |
| 戻り値 | なし（副作用のみ） | `bool` | `GateOutcome` |
| リクエストへの影響 | なし（変更禁止契約） | `true` で接続ごと委譲 | `Reject` で即時拒否応答 |
| レスポンスへのアクセス | なし（経過時間のみ） | なし | 拒否応答の status/body を自ら生成 |
| 複数登録時 | 全件呼び出し（登録順） | 最初のマッチで確定 | 最初の `Reject` で確定 |

`RequestGate` を `UpgradeHandler`・パスインターセプト型より**先**に評価するのは、
認可ゲートの既定拒否が WebSocket アップグレードやプラグイン応答にも漏れなく及ぶ
ようにするため。**ゲートを迂回してプラグインへ到達する経路は存在しない**
（バイパス不可）。

## 4. 契約・不変条件

1. **3 trait とも同期 API**: `async fn` を持ち込むと `Box<dyn Middleware>` 等の
   trait object 保持（dyn 互換性）が壊れるため。既定ハンドラ `Handler::handle` のみ
   async という非対称設計（[server-api.md](./server-api.md) 参照）。
2. **`Send + Sync` 必須**: 実装は `Arc<Server>` 経由で複数コネクションタスクから
   共有参照される。
3. **Middleware は同期ブロッキング I/O 禁止**: 実測でスループットが最大 25% 劣化
   する。ロギング等で I/O が必要な実装は非同期チャネルへの送信に留め、実際の I/O は
   別タスクで行う（詳細規約は `AGENTS.md`）。`crates/plugin-tracing` が本契約に
   従う参照実装（非同期・バッファ済み writer）。
4. **Middleware は `head` を変更しない**: 観測専用の契約。型では強制されないため
   実装者が守る規約として明記されている。
5. **`name()` に機密を含めない**: 静的識別名であり、リクエスト内容（トークン・PII）を
   含めてはならない。
6. **RequestGate はフェイルクローズ**: 判定に必要な情報が欠落・不正・判定不能な場合は
   必ず `GateOutcome::Reject` を返す（疑わしきは通過させない）。
7. **UpgradeHandler は判定のみ**: `matches` はヘッダ検査等の軽量判定に留める。
   ハンドシェイク検証・フレーミングはプラグイン側の責務。

## 5. セキュリティ観点

- **ゲート最優先・バイパス不可**: `RequestGate` は Upgrade・パスインターセプトを含む
  すべての応答生成経路より先に評価される。認可の既定拒否をコアの評価順序で保証する。
- **フェイルクローズ**: 情報欠落時に `Allow` へ倒す実装は契約違反。複数ゲート登録時も
  最初の `Reject` が優先され、後段ゲートの `Allow` で覆せない。
- **インジェクション耐性**: `GateOutcome::Reject` の `status: u16` 設計により、
  拒否応答のステータス行へ任意文字列が流入する経路を型レベルで排除している。
- **観測ログの機密混入防止**: `Middleware` 実装はログに機密（トークン・PII）を
  出さないこと。`name()` も同様。

## 6. スコープ外・関連ドキュメント

- **拡張点に載らないプラグイン配線は利用者実装不可**: パスインターセプト型
  （`try_intercept`）・レスポンス後処理型（`finalize_response`）・Upgrade 委譲の
  実体（`try_handle_upgrade`）は、いずれもコアの**非公開** `plugin` モジュール内の
  シームであり、公開拡張点ではない。利用者が独自プラグインをこれらのシームへ配線する
  ことはできない（設計判断は `docs/design/plugin-boundary.md` を参照）。利用者が
  実装できる拡張点は本書の 3 trait・`Interceptor`（本書スコープ外、
  [./interceptor-api.md](./interceptor-api.md) 参照）・`Handler` trait に限られる。
- サーバへの登録 API・リクエスト処理全体の設定: [server-api.md](./server-api.md)
- `RequestHead` の読み取り API（ヘッダ・パス・クエリ・Cookie）: [http-api.md](./http-api.md)
- 4 拡張点の自作ガイド・実装例: [../guide/extension-points.md](../guide/extension-points.md)
- feature 構成別サンプル: [../guide/feature-samples.md](../guide/feature-samples.md)
- 一次情報源（rustdoc）: `crates/core/src/extension.rs`
  （GitHub: `https://github.com/Fandhe-AI/fandhe-backend/blob/main/crates/core/src/extension.rs`）
