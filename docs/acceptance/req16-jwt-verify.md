# REQ-16 受け入れ検証レポート — 汎用 JWT 検証プラグイン（イシュー #823）

`docs/spec/04-requirements.md` REQ-16「汎用 JWT 検証プラグイン」（`fandhe-backend-plugin-jwt`）の
受け入れ基準 1〜6 を `scripts/accept/jwt-verify-accept.sh` で検証した結果。仕様は submodule
`docs/spec` の `261741e`（本リポジトリが pin、REQ-16 の文言は REQ-16 追加時の pin `1862571` と
同一）。設計は `docs/design/jwt-verify-plugin.md`。前提イシュー #810（SetCookie 接頭辞）・
#817（網羅テスト）・#818（fuzz）・#820（hub-wiring 移行）・#821（性能計測）・#822（文書追随）は
いずれもマージ済みで、本タスクは `crates/**/src` の production コードを変更していない。

## 受け入れ基準（計画対応表）

| 記号 | REQ-16 基準 | 検証方法 |
|------|------|------|
| A | 1. `iss`・`aud`・`typ` 不一致、`nbf` 前、`exp` 後、未知 `kid`、許可リスト外 `alg`、鍵種別と `alg` の不一致、`alg: none`、HS 系を拒否するテスト | `cargo test -p fandhe-backend-plugin-jwt --test req16_acceptance`。`c1_*` 11 件 + `baseline_is_accepted` を**テスト名単位で `ok` 照合** |
| B | 2. RS256・ES256 の正しいトークン受理と、回転中の新旧鍵での検証 | 同テストバイナリの `c2_*` 7 件を名前単位で照合 |
| C | 3. ゲートが同期で I/O を行わない（`RequestGate` 契約、NFR-7） | `RequestGate::check` が同期 `fn`／`crates/plugin-jwt/src` のコメント除外ソースに `async fn`・`.await`・`tokio::`・`std::fs`・`std::net`・`thread::sleep`・`std::process` が 0 件／`tokio` は `[dependencies]` に無い／`--test gate` 全件 PASS |
| D | 4. 新規外部依存なし（暗号は `ring` のみ）・未使用構成で依存ツリー・バイナリに 0 件 | `[dependencies]` の許可集合照合／外部依存 4 種が着手前から既存／`cargo tree -p fandhe-backend-core` の 3 構成で 0 件／`forbid(unsafe_code)`／`cargo audit`・`cargo deny check` |
| E | 5. REQ-9 hub 共通配線が本プラグインの検証部を使う形へ移行し、既存受け入れ基準を維持 | hub-wiring が plugin-jwt に依存・旧 `jwt.rs`/`jwks.rs` 不在／`scripts/dep-direction-check.sh`／`hub-wiring-accept.sh` 再実行 |
| F | 6. JWT 解析部が fuzz の対象（NFR-11） | `fuzz_targets/` の 3 target／`scripts/fuzz.sh` の `FUZZ_DIRS`／`ci.yml` の `fuzz-smoke` |

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日 | 2026-10-16 |
| 対象コミット（`origin/main`） | `03d8d5feb6c3146525c1168a5afa3b228b7bbc25` |
| rustc / cargo | 1.99.0 (2026-09-28) / 1.99.0 |
| cargo-audit / cargo-deny | 0.22.2 / 0.20.2 |
| 未導入ツール | `oha`、`cargo-geiger`（後述のとおり該当項目は未計測・未実施） |

## 判定サマリー（`scripts/accept/jwt-verify-accept.sh`、終了コード 0）

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A | 指定 12 件のテストが名前単位で ok |
| PASS | B | 指定 7 件のテストが名前単位で ok |
| PASS | C | `check` は同期 fn・src に I/O/async 痕跡 0 件（コメント除外）・tokio は dev のみ・gate テスト 5 件 PASS |
| PASS | D | 外部依存 `ring`/`base64`/`serde`/`serde_json` は着手前 `e9ceffd` の hub-wiring に既存。core の `cargo tree` に `fandhe-backend-plugin-jwt` が 0 件（`--no-default-features`／既定／`--all-features`）。`lib.rs` に `#![forbid(unsafe_code)]`、`unsafe-baseline` の `plugin-jwt` は 0 |
| PASS | D(e) | `cargo audit` 既知脆弱性 0 件／`cargo deny check` 違反 0 件 |
| PASS | E | hub-wiring は plugin-jwt に依存・旧 `jwt.rs`/`jwks.rs` なし・依存方向検査 PASS |
| PASS | E | `hub-wiring-accept.sh` 再実行は終了コード 0（FAIL なし） |
| PASS | F | target `jwks_from_json`/`verify_claims`/`verify_token`・`FUZZ_DIRS` 登録・`ci.yml` `fuzz-smoke` 経由（静的確認） |
| SKIP | F 実行 | opt-in（`JWT_ACCEPT_RUN_FUZZ=1`）。本実行では fuzz を回していない（未検証）。CI の `fuzz-smoke` で担保 |
| WARN | NFR-6 | HTTP 計測は `oha` が必要なため未計測 |

## 基準ごとの詳細

### A・B（基準 1・2）

テストは `crates/plugin-jwt/tests/req16_acceptance.rs`（対応表はモジュール doc）。各拒否テストは
拒否原因を 1 点に絞り、`baseline_is_accepted` で「その 1 点以外は受理される」ことを担保する。
スクリプトは件数一致ではなく**テスト名ごとの `... ok`** を照合する（名前の取り違え・欠落を検出）。

- A: `baseline_is_accepted`／`c1_rejects_{iss_mismatch, aud_mismatch, typ_mismatch_and_missing, before_nbf, after_exp, unknown_kid, alg_outside_allowlist, key_kind_alg_mismatch, alg_none, hs_family, der_encoded_ecdsa_signature}`
- B: `c2_accepts_{rs256,es256}_with_fixture_key`、`c2_accepts_mixed_rs_es_keyset`、`c2_rotation_same_kid_{rs256,es256}`、`c2_rotation_distinct_kids`、`c2_rotation_lifecycle_via_shared_key_set`

### C（基準 3）

`RequestGate::check` は同期 `fn`（`crates/core/src/extension.rs`）。`gate.rs` の module doc は
「`.await` なし」と述べるため、静的検査は**行コメントを除いた行**のみを対象にする
（コメントを含めると偽陽性になる。REQ-1 レポート 2026-08-26 再検証節の基準 E・F と同型の前例）。
署名検証中に保持するロックはない設計（短時間の `RwLock` のみ）。性能面は
`benches/reports/issue821-jwt-verify-performance.md` を参照。**HTTP の NFR-6 計測
（`benches/jwt-nfr6-bench.sh`）は `oha` 未導入のため未計測**であり、PASS に含めていない。

### D（基準 4）

本プラグインは Gate 型で core に feature を持たない（設計 2.1・2.7 節）。そのため「feature 無効時」は
「本 crate を依存に加えない構成」と読み替えた。この解釈は設計 2.7 節で承認済みとは記録されていない
ため、承認済みとは扱わず、検証結果のみを記録する。

- `Cargo.lock` は本リポジトリで追跡されない（`git ls-files` に無い）ため、lock 差分では比較できない。
  代わりに外部依存 4 種が着手前コミット `e9ceffd` の `plugin-hub-wiring/Cargo.toml` に宣言済みであることを確認した
- `--all-features` の core ツリーで `ring` が現れる場合は webrtc 経由であり、plugin-jwt 経由ではない
  （`docs/dep-impact/records.md` #822 節と同じ手法。plugin-jwt 自体は 0 件）
- `cargo-geiger` による二重検証は未導入のため**未実施**（`forbid(unsafe_code)` と `unsafe-baseline` で代替確認）

### E（基準 5）

詳細な移行内容・実測は `docs/acceptance/req9-hub-wiring.md` の #820 節を参照（重複記載しない）。
本実行では `hub-wiring-accept.sh` を再実行して FAIL なしを確認した。同スクリプトの基準 D（NFR-6）は
`oha` 未導入のため SKIP であり、移行後の REQ-9 基準 D の再判定は**未実施**。

### F（基準 6）

3 target はそれぞれ JWKS 解析（`jwks_from_json`）、クレーム検証（`verify_claims`）、トークン全体
（`verify_token`）を対象とする。`scripts/fuzz.sh` の `FUZZ_DIRS` に `crates/plugin-jwt/fuzz` が登録され、
`ci.yml` の `fuzz-smoke` ジョブから実行される。

## 未計測・フォローアップ

- HTTP の NFR-6（`benches/jwt-nfr6-bench.sh`）と REQ-9 基準 D の移行後再判定: `oha` 導入済みの専有環境で実施
- fuzz の手元実行: `JWT_ACCEPT_RUN_FUZZ=1 bash scripts/accept/jwt-verify-accept.sh`（nightly 必須）
- `dep-audit-accept.sh` 3a への plugin-jwt の fuzz target 追加は #843・#846 で対象外と記録済み

## セキュリティ補足

脅威モデル（設計 5 節 A01〜A09）は再掲しない。fixture の鍵はテスト専用
（`crates/plugin-jwt/tests/fixtures/README.md`）で、鍵・トークンの実値は本レポートに含めない。

## 検証コマンド一覧（再現手順）

```bash
bash scripts/tests/run-jwt-verify-accept-tests.sh   # 判定ロジックのセルフテスト（オフライン）
bash scripts/accept/jwt-verify-accept.sh            # 基準 A〜F（cargo audit/deny は DB 更新でネットワークを使う）
JWT_ACCEPT_RUN_FUZZ=1 bash scripts/accept/jwt-verify-accept.sh   # fuzz 実行を含める（任意）
```
