# TASK-12.4-1 タスク定義（N=10、事前確定）

`docs/design/third-party-verification.md`（3 節・4 節）のプロトコルに従い、被験実行に
**先立って**確定・コミットするタスクセット。以後変更しない（同書「後出し防止」）。

設計者（タスク設計者役、3 節 (A)）は本タスクセットの設計にあたり `docs/spec/03-poc/`
配下の PoC-9 タスク（T-01〜T-15）を参照していない。対象は `crates/core` / `crates/http`
の公開 API（doc comment・既存テストのみから把握できる範囲）に限定した。

各タスクの完遂判定は `scripts/third-party-verify.sh` による機械ゲート（`fmt --check` /
`clippy --all-features -- -D warnings` / `cargo test --workspace --all-features`、起点
コミットとの突合によるリグレッション 0 件確認）を一次判定、受け入れ基準列の充足確認を
二次判定として行う（`third-party-verification.md` 5 節）。

## タスク一覧

| ID | 対象クレート | 内容 | 受け入れ基準 |
|----|-------------|------|--------------|
| T-01 | `crates/core` | `Middleware` trait（`extension.rs`）に、既存 doc comment の契約説明を踏襲した doc test 付き公開ヘルパー関数を 1 つ追加する（例: no-op middleware を返すファクトリ） | 新規 doc test が `cargo test` で PASS。既存 `extension.rs` のテストにリグレッションなし |
| T-02 | `crates/core` | `GateOutcome` enum（`extension.rs`）に対する `impl` ブロックで、許可/拒否を判定するヘルパーメソッド（例: `is_allowed`）を追加する | 新規メソッドの単体テストが PASS。既存 enum のバリアント数・シリアライズ形式を変更しない |
| T-03 | `crates/http` | `RequestHead::header`（`request.rs`）を使い、複数ヘッダ値を大文字小文字区別なく検索する公開ヘルパー関数を `request.rs` に追加する | 新規関数の単体テストが PASS（大文字小文字混在ケースを含む）。既存 `header`/`headers` の挙動を変更しない |
| T-04 | `crates/http` | `HttpVersion` enum（`request.rs`）に `Display` トレイトを実装する（`HTTP/1.1` 等の文字列表現） | `Display` 実装の doc test または単体テストが PASS。既存の `derive` 属性・バリアントを変更しない |
| T-05 | `crates/http` | `BodyLength` enum（`body.rs`）に対し、既知長かどうかを判定する公開ヘルパーメソッド（例: `is_known`）を追加する | 新規メソッドの単体テストが PASS。`body_length` 関数の既存挙動・テストを変更しない |
| T-06 | `crates/http` | `ParseError`（`request.rs`）に対し `std::error::Error` を実装する（未実装の場合）。実装済みの場合は `Display` メッセージの網羅性を検証する単体テストを追加する | `cargo test` で新規テストが PASS。既存エラー型のバリアント・メッセージ文言を変更しない |
| T-07 | `crates/http` | `should_keep_alive`（`connection.rs`）の HTTP バージョン別 keep-alive 判定を検証する境界値テスト（HTTP/1.0・HTTP/1.1・`Connection` ヘッダ有無の組み合わせ）を追加する | 追加した境界値テストが全て PASS。既存 `should_keep_alive` の実装は変更しない（テスト追加のみ） |
| T-08 | `crates/core` | `RequestGate` trait（`extension.rs`）の doc comment に契約説明を追記し、対応する doc test（trait を実装する最小サンプル）を追加する | doc test が `cargo test` で PASS。既存 trait のシグネチャを変更しない |
| T-09 | `crates/http` | `parse_request_head`（`request.rs`）に対し、不正入力（境界を越えたヘッダ数・空行のみのバッファ等）を与えた際に `ParseError` を返すことを確認する単体テストを追加する | 追加テストが全て PASS。`parse_request_head` の実装は変更しない（テスト追加のみ） |
| T-10 | `crates/core` | `version()`（`lib.rs`）の戻り値が `Cargo.toml` の `package.version` と一致することを確認する doc test または単体テストを追加する | 新規テストが PASS。`version()` の実装を変更しない |

## 完遂判定の記録欄（被験実行後に記入）

被験実行前は空欄。`scripts/third-party-verify.sh` の出力・評価者の受け入れ基準確認結果を
`docs/reports/task-12-4-1-completion-rate-verification.md` に転記する。

| ID | 判定 | 備考 |
|----|------|------|
| T-01〜T-10 | （未実施） | 実施状況は `task-12-4-1-completion-rate-verification.md` を参照 |
