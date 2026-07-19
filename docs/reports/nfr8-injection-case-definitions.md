# NFR-8 注入リグレッション検知率 実装フェーズ確定検証 — 注入ケース定義（#238）

`docs/spec/04-requirements.md` NFR-8「AI 生成テストによる注入リグレッションの検知率が
90% 以上」の実装フェーズ確定計測に使う注入ケース一覧。計測実施（後出し改変防止）より
**先に本定義・パッチ実体をコミットする**（`docs/reports/task-12-4-1-task-definitions.md`
の「タスク定義先行コミット」方式を踏襲）。

- 定義コミット時点の起点コミット（origin/main HEAD）: `54e87a7`
- パッチ実体: `docs/reports/nfr8-injection-patches/R-01.diff`〜`R-12.diff`
  （`git apply` 可能な unified diff。各パッチは対応する実クレートのソースファイルを
  1 ファイルのみ変更する）
- 計測ハーネス: `scripts/regression-injection-verify.sh`
  （使い捨て `git worktree` へ 1 件ずつ適用し、clippy / cargo-nextest / doc test を
  実行して検知可否を判定する）

## 選定基準

1. コア（`crates/http` / `crates/routes` / `crates/core`）とプラグイン
   （`crates/plugin-*`）の両方を跨ぐ
2. バグ分類（境界値・条件反転・上限撤廃・検証スキップ・状態管理・フォールスルー破壊）
   を分散させる
3. 原則コンパイルが通る変更に限る（型不整合による自明なビルドエラーのみでの検知は
   避け、意味論的な破壊を実既存テストが捕捉できるかを計測する）
4. セキュリティ関連の後退（DoS 上限撤廃・検証省略・認可バイパス）を必ず含める
   （NFR-8 が守るべき退行の代表例のため。R-02・R-06・R-08・R-10・R-12 が該当）

12 件すべてコンパイルは通る変更である（コンパイルエラーのみで検知されるケースは 0 件。
検知は既存の `#[test]` / doc test / clippy lint による意味論的検知に限定される）。

## ケース一覧

| ID | 対象ファイル | バグ分類 | 注入内容 | なぜ破壊的か（利用者影響） | 期待検知チャネル |
|----|-------------|---------|---------|--------------------------|-----------------|
| R-01 | `crates/http/src/request.rs` | 境界値（off-by-one） | ヘッダ本数上限チェックを `headers.len() >= MAX_HEADER_COUNT` から `>` に変更 | `MAX_HEADER_COUNT` ちょうどの上限值ではなく `+1` 件まで受理してしまい、DoS 上限（リソース枯渇対策）の境界がずれる | `cargo nextest`（`too_many_headers`） |
| R-02 | `crates/http/src/chunked.rs` | 上限撤廃（DoS） | chunked デコード後総量が `MAX_BODY_BYTES` を超えても `BodyTooLarge` を返さず素通りさせる | chunked transfer-coding 経由でボディサイズ上限を完全に迂回でき、メモリ枯渇 DoS に直結する | `cargo nextest`（`decoded_body_exceeding_max_returns_body_too_large` 相当） |
| R-03 | `crates/http/src/body.rs` | 検証スキップ | `Content-Length` と `Transfer-Encoding: chunked` の共存拒否を `if false && has_content_length` へ無効化 | リクエストスマグリングの典型的な入口（CL/TE 共存）を許してしまう | `cargo nextest`（`content_length_with_chunked_transfer_encoding_is_rejected` 相当）・`cargo clippy`（`clippy::overly_complex_bool_expr`） |
| R-04 | `crates/http/src/connection.rs` | 条件反転 | HTTP/1.1 の keep-alive 判定 `!has_close()` を `has_close()` へ反転 | `Connection: close` を指定したのに接続を維持し、`Connection` ヘッダなしの通常リクエストは逆に即座に切断される（意味論の完全反転） | doc test（`should_keep_alive` の doc 例）・`cargo nextest`（`http11_connection_close_disables_keep_alive` 等） |
| R-05 | `crates/routes/src/pattern.rs` | 境界値（パス走査対策の欠落） | `{name}` パラメータ照合から `.` / `..` の拒否条件を削除 | パストラバーサル対策として意図的に拒否していた `.`/`..` セグメントがパラメータ値として束縛可能になる | `cargo nextest`（`match_segments_rejects_dot_and_dotdot_path_traversal`） |
| R-06 | `crates/core/src/server.rs`（`first_rejection`） | 検証スキップ（認可バイパス） | `RequestGate::check` の結果を握りつぶして常に `None`（許可）を返す | `plugin-hub-wiring` 等が登録する認証・認可・同意ゲートが完全に無効化され、拒否すべきリクエストを全通しする | `cargo nextest`（`crates/core/src/server.rs` 内の `RequestGate` テスト群） |
| R-07 | `crates/core/src/server.rs`（`handle_connection_with_permit`） | フォールスルー破壊 | `upgrade_handlers.iter().any(...)` の判定結果を無視し常に `true` を返す | `UpgradeHandler` が 1 つでも登録されていると、対象外の通常 HTTP リクエストまで長時間接続経路へ誤委譲され 501 を返す（フォールスルー条件の破壊） | `cargo nextest`（`crates/core/tests/websocket_upgrade_disabled.rs`・`plugin_boundary*.rs` 系） |
| R-08 | `crates/plugin-websocket/src/handshake.rs` | 検証スキップ | `Sec-WebSocket-Key` の base64/24 文字検証（`is_valid_base64_key`）の呼び出し結果を握りつぶす | RFC 6455 が要求するハンドシェイクキー形式検証が無効化され、不正な鍵でもハンドシェイクが成立してしまう | `cargo nextest`（`validate_rejects_malformed_key_length`） |
| R-09 | `crates/plugin-graphql/src/lib.rs`（`try_handle_graphql`） | フォールスルー破壊（条件反転） | 対象外判定 `method != "POST" \|\| target != GRAPHQL_PATH` を `&&` に変更 | スキーマ未登録時・無関係パス・誤ったメソッドでのフォールスルー契約（crate doc 「スキーマ未登録時はフォールスルー」）が崩れ、無関係な POST リクエストが GraphQL 実行経路に誤って取り込まれる | `cargo nextest`（`falls_through_on_unrelated_path`・`falls_through_on_wrong_method`） |
| R-10 | `crates/plugin-hub-wiring/src/jwt.rs`（`verify_token`） | 検証スキップ（認可バイパス） | 期限切れ判定 `payload.exp <= now_unix` を `payload.exp == 0` に弱体化 | `exp` が 0 以外であれば期限切れトークンでも常に検証を通過してしまい、JWT の期限失効機構が実質的に無効化される | `cargo nextest`（`gate.rs` の `expired_token_is_401`）・`cargo clippy`（`now_unix` 未使用引数の warning） |
| R-11 | `crates/plugin-tracing/src/sampler.rs`（`should_sample`） | 条件反転 | サンプリング判定 `n.is_multiple_of(interval)` を `!...` へ反転 | `interval` 件に 1 件のみ記録する契約が「1 件だけ間引く」に反転し、サンプリングによる負荷抑制（PoC-10 実測根拠）が実質機能しなくなる | doc test（`should_sample` の doc 例）・`cargo nextest`（`interval_hundred_samples_one_in_hundred`・`concurrent_access_preserves_exact_ratio`） |
| R-12 | `crates/plugin-webrtc-proxy/src/handler.rs`（`try_handle_rtc_offer`） | 上限撤廃（DoS） | SDP offer サイズ上限 `body.len() > config.max_offer_bytes()` チェックを削除 | WebRTC シグナリングプロキシの offer サイズ上限（DoS 対策）が迂回可能になる | `cargo nextest`（`oversized_offer_is_rejected`） |

## 検知ゲート

`scripts/regression-injection-verify.sh` は各パッチ適用後、パッチが変更した
ファイルが属するクレートディレクトリ内で次のゲートを順に実行する（`.github/workflows/ci.yml`
の test ジョブ・`.claude/rules/coding-rust.md` に整合。ワークスペース全体ではなく
対象クレートのみに限定するのは計測時間短縮のためであり、判定単位を欠くものではない
— 12 ケースは互いに独立したファイルへの単一注入のため、対象クレート内のテストで
意味論的検知が可能かどうかが本計測の関心事である）:

1. `cargo clippy --all-targets --all-features -- -D warnings`
2. `cargo nextest run --all-features --profile ci`（`cargo-nextest` 未導入環境では
   `cargo test --all-features` にフォールバック）
3. `cargo test --doc --all-features`

いずれか 1 つでも失敗、またはタイムアウト（既定 600 秒、`REGRESSION_INJECTION_TIMEOUT`
で上書き可）した場合を「検知（DETECTED）」、全ゲート通過を「検知漏れ（MISSED）」とする。

## 実測結果

実測日・起点コミット・ケース別結果・検知率・再現手順は
`docs/reports/nfr8-injection-detection-verification.md` に記録する（本ドキュメントは
ケース定義の確定に責務を限定し、実測値は改変しない）。
