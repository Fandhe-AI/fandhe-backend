# REQ-17 受け入れ検証レポート — CSRF 検査ゲート（イシュー #832）

`docs/spec/04-requirements.md` REQ-17「CSRF 検査ゲート」（`fandhe-backend-plugin-csrf`）の
受け入れ基準 1〜4 を検証した結果。仕様は submodule `docs/spec` の `261741e`（本リポジトリが pin）。
設計は `docs/design/plugin-boundary.md` 5.6.6 節と `crates/plugin-csrf/src/lib.rs` の module doc。
前提イシュー #825（クレート新設）・#826（網羅受け入れテスト）はいずれもマージ済みで、本タスクは
`crates/**/src` の production コードを変更していない（変更は `publish` メタデータの解除と文書・
台帳・ベースラインの追随のみ）。

## 受け入れ基準の対応表

| 記号 | REQ-17 基準 | 検証方法 |
|------|------|------|
| A | 1. `Sec-Fetch-Site` の各値、`Origin` の一致・不一致・`null`、両ヘッダなし、安全なメソッドの素通し、パスの除外を確かめるテストがある | `cargo test -p fandhe-backend-plugin-csrf --test req17_acceptance`。`tests/req17_acceptance.rs` の対応表にある 12 件を**テスト名ごとに `ok` 照合** |
| B | 2. 拒否の応答が 403 で本文を含まない | 単体 `c1_rejection_is_bare_403`、および `tests/gate.rs` の実接続（`tokio::io::duplex` 経由の `handle_connection`）テスト |
| C | 3. 同期・I/O なしの契約（`RequestGate`、NFR-7） | `RequestGate::check` が同期 `fn`／`crates/plugin-csrf/src` の行コメント除外ソースの静的検査／`tokio` は `[dev-dependencies]` のみ／全テスト PASS |
| D | 4. 新規外部依存なし・登録しない構成で依存ツリー・バイナリへの増加 0 件 | `[dependencies]` の照合／`cargo tree` の複数構成／`strings` によるバイナリ確認／`cargo audit`・`cargo deny check` |

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日 | 2026-10-16 |
| 対象コミット（`origin/main`） | `89de5ae`（本変更はその上の文書・メタデータ追随） |
| rustc / cargo | 1.99.0 (2026-09-28) / 1.99.0 |
| cargo-audit / cargo-deny | 0.22.2 / 0.20.2 |
| 未導入ツール | `cargo-geiger`（`unsafe` 件数は `#![forbid(unsafe_code)]` と `scripts/unsafe-triage.sh` のベースライン照合で代替） |

## 判定サマリー

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A | `req17_acceptance` 12 件が名前単位で ok（`cargo test -p fandhe-backend-plugin-csrf` 全体は単体 15 件・`gate` 11 件・`req17_acceptance` 12 件・doc test 4 件、すべて PASS） |
| PASS | B | 単体 1 件と実接続 2 件が ok（403・本文長 0・ハンドラ未到達） |
| PASS | C | `check` は同期 fn。src に非同期・I/O の痕跡 0 件（コメント除外、後述の注記 1 件を除く）。`tokio` は dev のみ |
| PASS | D | `[dependencies]` は workspace 内 2 クレートのみ。`cargo tree` の全構成で 0 件。バイナリ内の該当シンボル 0 件。`cargo audit` 脆弱性 0 件・`cargo deny check` 違反 0 件 |
| WARN | D(unsafe) | `cargo-geiger` が未導入のため geiger による集計は未実施。`forbid(unsafe_code)` とベースライン（`plugin-csrf` は 0）で代替 |

## 基準ごとの詳細

### A（基準 1）

`tests/req17_acceptance.rs` のモジュール doc の対応表を正とする。各拒否テストは「その 1 点だけを
変えた」要求で行い、変える前の基準要求が許可されること（`baseline_is_allowed`）で拒否原因を特定する。
名前ごとに `... ok` を確認した（件数ではなく名前の照合。名前の取り違え・欠落を検出するため）。

| 仕様の項目 | テスト名 |
|------|------|
| 基準要求の許可 | `baseline_is_allowed` |
| `Sec-Fetch-Site: same-origin` / `none`（通す） | `c1_sec_fetch_site_same_origin_allowed` / `c1_sec_fetch_site_none_allowed` |
| `Sec-Fetch-Site: same-site` / `cross-site`（拒否） | `c1_sec_fetch_site_same_site_rejected` / `c1_sec_fetch_site_cross_site_rejected` |
| `Origin` 一致 | `c1_origin_match_allowed` |
| `Origin` 不一致 | `c1_origin_mismatch_rejected` |
| `Origin: null` | `c1_origin_null_rejected` |
| 両ヘッダなし | `c1_both_headers_absent_rejected` |
| 安全なメソッドの素通し | `c1_safe_methods_pass_through` |
| パスの除外（完全一致のみ） | `c1_exempt_path_exact_match_only` |
| 拒否は一様な素の 403 | `c1_rejection_is_bare_403` |

### B（基準 2）

- 単体: `c1_rejection_is_bare_403`。拒否の各原因が同一の応答（ステータス 403、本文なし、ヘッダ値を
  反映しない）になることを確認する
- 実接続: `tests/gate.rs` の `cross_site_post_is_bare_403_and_handler_not_called` と
  `origin_null_and_missing_headers_are_403`。ワイヤ上で 403・本文なしとなり、ハンドラへ到達しないことを確認する
- 単体テスト `gate::tests::all_rejections_are_identical_bare_403` も同じ契約を固定する

### C（基準 3）

`RequestGate::check` は同期 `fn`（`crates/core/src/extension.rs`）。`lib.rs` の module doc は
「I/O・ロック・`.await`・ヒープ確保を行わない」と述べるため、静的検査は**行コメントを除いた行**のみを
対象にした（コメントを含めると偽陽性になる。`docs/acceptance/req16-jwt-verify.md` の C と同じ手法）。
検索語は `async fn`・`.await`・`tokio::`・`std::fs`・`std::net`・`thread::sleep`・`std::process`。

- 該当は `crates/plugin-csrf/src/config.rs` の `inner.parse::<std::net::Ipv6Addr>()` の 1 件のみ。
  これは構築時（`CsrfGateBuilder::build`）の Origin 形式検証で IPv6 アドレス文字列を構文解析する
  純関数であり、ソケット・DNS・ファイル等の I/O は行わない。`check`（要求ごとの判定）の経路には現れない
- `tokio` は `crates/plugin-csrf/Cargo.toml` の `[dev-dependencies]` にのみある（通常依存に現れない）
- `cargo test -p fandhe-backend-plugin-csrf` は全件 PASS

### D（基準 4）

本プラグインは Gate 型で core に feature を持たない（`docs/design/plugin-boundary.md` 5.6 節）。そのため
「登録しない構成」は「本クレートを依存に加えない構成」と読み替えた。この読み替えは承認済みとは記録されて
いないため、承認済みとは扱わず、検証結果のみを記録する（`req16-jwt-verify.md` の D と同じ扱い）。

- `[dependencies]` は `fandhe-backend-core`・`fandhe-backend-http` の 2 つだけ（workspace 内）。
  `cargo tree -p fandhe-backend-plugin-csrf -e normal` に現れる外部 crate は core / http が既に引く
  `tokio` / `bytes` / `libc` / `memchr` / `mio` / `pin-project-lite` / `rustc-hash` / `socket2` のみ。
  `Cargo.lock` に差分はない
- `cargo tree -e normal --prefix none` で `fandhe-backend-plugin-csrf` が 0 件であることを確認した対象:
  core・http・routes・plugin-jwt・plugin-hub-wiring（`--no-default-features`・既定・`--all-features` の
  3 構成）、他の plugin 9 種（`--all-features`）、`templates/app`（standalone workspace）
- バイナリ: plugin-csrf に依存しない `cargo build --release -p fandhe-backend-core --example minimal
  --all-features` の成果物に対し `strings` で `fandhe_backend_plugin_csrf` を検索して 0 件。依存に加えない構成では
  リンクされないため構造上当然の結果であり、直接の証跡として記録する
- 依存数: `bash scripts/dep-impact.sh` は `--no-default-features` 281・既定 281・`--all-features` 283 で、
  plugin-jwt 公開準備時点から増分 0（`docs/dep-impact/records.md`）
- `unsafe`: `lib.rs` に `#![forbid(unsafe_code)]`、`scripts/unsafe-baseline.json` の `plugin-csrf` は 0・0。
  `bash scripts/unsafe-triage.sh` は「baseline から変化なし」
- `cargo audit` は既知脆弱性 0 件（終了コード 0）、`cargo deny check` は advisories / bans / licenses /
  sources すべて ok

### 補足: 登録順の文書化（受け入れ基準の外）

REQ-17 の詳細にある「JWT 検証ゲート（REQ-16）などと併用するときの登録順の文書化」は、`lib.rs` の doc と
`docs/design/plugin-boundary.md` 5.6.6 節にあり、挙動は `tests/gate.rs` の
`csrf_first_short_circuits_later_gate`・`csrf_first_then_auth_gate_evaluated_when_origin_ok`・
`decision_is_order_independent`・`reverse_order_exposes_auth_challenge`（および除外パスで後続ゲートが
評価される `exempt_path_still_runs_later_auth_gate`）で固定されている。これらは受け入れ基準 4 件には
含まれない。

## 未計測・フォローアップ

- REQ-17 専用の受け入れスクリプト（`scripts/accept/` 配下）は作っていない。本レポートは手で実行した
  コマンド（下記）の結果である。必要なら別イシューで常設化する
- `cargo-geiger` による `unsafe` 集計は未実施（未導入）
- 実運用でのクライアント互換性（`Sec-Fetch-Site` を送らない古いブラウザ等）は本レポートの範囲外

## セキュリティ補足

脅威モデル（`Sec-Fetch-*` の優先、`same-site` を拒否する理由、除外パスを完全一致に限る理由、重複
ヘッダの拒否、`Origin` のバイト完全一致）は `crates/plugin-csrf/src/lib.rs` の module doc と
`docs/design/plugin-boundary.md` 5.6.6 節を参照し、ここでは再掲しない。拒否応答はヘッダ値を反映せず、
本文も理由も返さない（基準 B）。

## 検証コマンド一覧（再現手順）

```bash
cargo test -p fandhe-backend-plugin-csrf
cargo test -p fandhe-backend-plugin-csrf --test req17_acceptance -- --list
cargo tree -p fandhe-backend-plugin-csrf -e normal --prefix none
cargo tree -p fandhe-backend-core -e normal --prefix none --all-features | grep -c '^fandhe-backend-plugin-csrf '
cargo tree --manifest-path templates/app/Cargo.toml -e normal --prefix none | grep -c 'plugin-csrf'
cargo build --release -p fandhe-backend-core --example minimal --all-features
strings target/release/examples/minimal | grep -c 'fandhe_backend_plugin_csrf'
bash scripts/dep-impact.sh
bash scripts/unsafe-triage.sh
bash scripts/dep-direction-check.sh
cargo audit
cargo deny check
cargo publish --workspace --dry-run --allow-dirty
```
