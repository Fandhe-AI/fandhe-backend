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

E・F の静的確認も、説明文・コメントの言及による偽陽性を避けるため範囲を限定する:
E は hub-wiring `Cargo.toml` の `[dependencies]` 表のキーのみ、F は `fuzz.sh` の
`FUZZ_DIRS=(...)` 代入行（コメント除外）と `ci.yml` の `fuzz-smoke` ジョブ内の
コメント以外の `run:` 行のみを見る。`scripts/tests/run-jwt-verify-accept-tests.sh` が、
実ファイルから本物の行だけを除いたコピーで FAIL する陰性対照を常設している。

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
bash scripts/accept/jwt-verify-accept.sh            # 基準 A〜H（cargo audit/deny は DB 更新でネットワークを使う）
JWT_ACCEPT_RUN_FUZZ=1 bash scripts/accept/jwt-verify-accept.sh   # fuzz 実行を含める（任意）
cargo test -p fandhe-backend-plugin-jwt --features jwks-refresh --test refresh_gate   # H のみ（jwks-refresh）
```

---

# 追補: `jwks-refresh` の受け入れ結果（イシュー #897）

追加要件 `jwks-refresh`（設計 `docs/design/jwt-verify-plugin.md` 8 節。実装は #894〜#896）の受け入れ基準を、
実際の利用形（`JwtGate` を `Server::gate` に登録し `handle_connection` を通す経路）で、tokio の仮想時間により
決定的に検証した結果。本追補は `crates/**/src` の production コードを変更していない。

## 追加の受け入れ基準

| 記号 | 基準 | 検証方法 |
|------|------|------|
| G | 取り直しタスクの時刻制御テスト（周期・`max-age`・下限・失敗時の保持・`max_stale`・未知 `kid` のまとめ） | `--features jwks-refresh --test refresh_task` の 13 件を名前単位で照合 |
| H-R1 | JWKS に無い `kid` を大量に送っても、期間 T の取得の開始回数が「T ÷ 下限 の切り上げ + 定期の取得回数」以下で、偽トークンの受理が 0 件 | `--test refresh_gate` の `r1_*` 5 件 |
| H-R2 | 事前公開なしで鍵を切り替えた後、切替の時点ではその場で 401、「下限 + 取得にかかる時間」以内に受理し始める | 同 `r2_*` 3 件 |
| H-R3（補足） | 事前公開 → 署名鍵の切替 → 旧鍵の退役という正しい運用では、正当なトークンの拒否が 0 件で未知 `kid` による追加の取得もない | 同 `r3_*` 1 件 |
| C・D（見直し） | 同期・I/O なし（NFR-7）と依存ゼロ | 下記「検証器の見直し」 |

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日 | 2026-10-27 |
| 対象コミット | `origin/main` `4af28148e84d9706f36b520b6ecfe3f7cbd37c29` + 本変更 |
| rustc / cargo | 1.98.1 (2026-09-01) / 1.98.1 |
| cargo-audit / cargo-deny | 0.22.2 / 0.19.4 |
| 未導入ツール | `oha`（NFR-6 の HTTP 計測は未計測） |

## 判定サマリー（`scripts/accept/jwt-verify-accept.sh`、終了コード 0）

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A・B・E・F | 既存と同じ判定（A 12 件・B 7 件が名前単位で ok、E は hub 配線の移行と再実行、F は fuzz 対象の静的確認） |
| PASS | C | `check` は同期 fn。ゲート側 `src` に I/O・async の痕跡 0 件。`src/refresh/` は走査から除外し、`refresh` モジュールが `jwks-refresh` の cfg の内側にあることを別に確認。tokio は `jwks-refresh` 限定の optional 依存。gate テスト 5 件 PASS |
| PASS | D | 許可集合（tokio は optional + `dep:` + default 外のときのみ許可）。設計 8.2 の (a)(b) を `cargo tree` で判定して PASS。`forbid(unsafe_code)`・`unsafe-baseline` は 0 |
| PASS | D(e) | `cargo audit` 既知脆弱性 0 件／`cargo deny check` 違反 0 件 |
| PASS | G | `refresh_task` の 13 件が名前単位で ok |
| PASS | H | `refresh_gate` の 9 件が名前単位で ok |
| SKIP | F 実行 | opt-in。CI の `fuzz-smoke` で担保（未検証） |
| WARN | NFR-6 | `oha` 未導入のため未計測 |

## 検証器の見直し（C・D）

`scripts/accept/jwt-verify-accept.sh` の C・D は #823 の時点（取り直しがなく、本クレートが tokio に直接依存しない）の
前提で書かれていた。#894 以降の構成では、`src/refresh/task.rs` の `async fn`・`.await`・`tokio::` が C の
禁止パターンに、optional な `tokio` が D の許可集合に当たり、旧基準のままでは `jwks-refresh` と両立しない。
そのため設計 8.1・8.2 に沿って次のように直した。

- C: NFR-7 は `RequestGate::check` の契約であり、取り直しタスクは設計上ゲートの外（8.1）。走査対象から
  `src/refresh/` だけを除く。ゲート側（`gate.rs`・`auth.rs` ほか）の検査は弱めていない。除外が安全であるよう、
  `lib.rs` で `refresh` が `#[cfg(feature = "jwks-refresh")]` の直後にあることを別に判定する（外れれば FAIL）。
- D: tokio は `[dependencies]` で `optional = true`、`[features]` の `jwks-refresh` が `dep:tokio` を含み、
  `default` に入っていないときに限り許可する。あわせて設計 8.2 の (a)(b) を機械判定に加えた
  （(c) は下記の手動確認と A・B・G の cargo test で確かめる）。`cargo tree` の失敗や空出力は 0 件扱いにせず判定不能とする。
- いずれも `scripts/tests/run-jwt-verify-accept-tests.sh` に陽性・陰性（実 `lib.rs` の cfg 除去、実 `Cargo.toml` の
  `optional` 除去、除外外の違反）のケースを追加した。

## 基準ごとの結果

### R1: 取得回数の上限（`r1_*`）

数え方は設計 8.7 のとおり半開区間 `[a, a + T)`。攻撃者の鍵で正しく署名したトークンと署名が出鱈目なトークンを、
未知の `kid` で毎秒送り続けた。全応答が 401（偽トークンの受理 0 件）。応答はステータスだけでなくヘッダ（`Content-Length`・`Content-Type`）とボディも毎回検証する（200 は本文 `ok`、401 は固定の JSON `{"error":"invalid_token"}`）。

| テスト | 条件 | 結果 |
|------|------|------|
| `r1_unknown_kid_flood_is_bounded_by_min_interval` | T=60 s、L=10 s、毎秒 20 件 | 開始は 0・10・20・30・40・50 s の 6 回。式の上限は ceil(60/10) + 定期 1 = 7 |
| `r1_bound_holds_over_every_half_open_window` | 300 s 連続、全開始時刻を左端とする窓（T = 2.5L〜5.5L） | 開始 30 回、連続する間隔はすべて L 以上、どの窓でも開始 ≤ ceil(T/L) |
| `r1_bound_holds_with_failures_and_slow_fetches` | 通信エラー・時間切れ・遅延 3 s の各 100 s | 開始 ≤ ceil(100/10) + 1、間隔は L 以上 |
| `r1_bound_holds_with_non_default_min_interval` | L=7 s、周期 60 s、130 s | 開始 ≤ ceil(130/7) + 3 = 22、間隔は L 以上 |
| `r1_shared_trigger_across_two_gates_keeps_one_bound` | 同じ合図を共有する 2 つのゲートへ交互に 100 s | 全体で 1 つの上限（設計 8.6） |

### R2: 事前公開なしの切替への追従（`r2_*`）

下限 L=10 s、取得にかかる時間 d=3 s（`fetch_timeout` 5 s 以内）、上限は L + d = 13 s。取得関数は呼ばれた時点の
本文を写し取るため、切替の直後に始まった取得は旧本文を得る（最悪のケース）。

| テスト | 結果 |
|------|------|
| `r2_unannounced_rotation_is_followed_within_min_interval_plus_fetch_time` | 切替の時点で 401（取り直しを待たない）。切替から受理までは約 12 s で、上限 13 s 以内、取得の完了より前には受理されない |
| `r2_follow_bound_for_various_switch_offsets` | 取得が進行中でない状態で、切替が 3.05・6.05・9.95 s のいずれでも、受理までは 13 s 以内 |
| `r2_switch_during_in_flight_fetch_is_followed_by_next_fetch` | 5 s の未知 `kid` で取得を予約し、10 s に始まった旧本文の取得（10〜13 s）の最中（切替 10.05・10.5・12.9 s）に切り替える。その取得は旧本文のまま完了するので受理されず、取得中に出た合図による次回の取得（20 s 開始、23 s 完了）で追従する。切替 10.05 s からは約 12.95 s（100 ms 刻みの観測で 13.0 s 前後）で、上限 13 s に最も近い最悪ケース |
| `r2_attacker_noise_does_not_delay_following` | 切替の直前に未知 `kid` の大量送信で利用者の通知口が間引かれていても、合図は独立に働き 13 s 以内に追従 |

### R3: 通常の回転（`r3_*`、補足）

周期 60 s で、100 s に事前公開、200 s に署名鍵を切替、300 s に旧鍵を退役させる運用を 420 s 流した。正当なトークンの
拒否は 0 件。取得の開始は定期の予定（0・60・…・360 s）の 7 回だけで、未知 `kid` による追加の取得は起きない。
退役が反映された後は旧鍵のトークンが 401 になる（期待どおり）。

## 依存ゼロの確認（基準 4 の `jwks-refresh` 部分、設計 8.2）

`docs/dep-impact/records.md` の #894 節と矛盾しない（重複は参照で済ませる）。

- (a) `cargo tree -p fandhe-backend-plugin-jwt -e normal --depth 1`: 無効時は `tokio` への直接の辺なし、
  `--features jwks-refresh` で現れる（陽性対照）。
- (b) 同 `-e normal` の全体のパッケージ集合は有効・無効で同一。`cargo tree -e features -i tokio` の tokio の
  feature 集合も同一（`core` が既に `time`・`sync` を有効にしているため）。`jwt-verify-accept.sh` が自動判定する。
- (c) feature なし・`--features jwks-refresh`・`--all-features` の 3 構成で `cargo build -p fandhe-backend-plugin-jwt`
  と `RUSTDOCFLAGS="-D warnings" cargo doc -p fandhe-backend-plugin-jwt --no-deps` が成功。feature なしと
  `--all-features` の `cargo test -p fandhe-backend-plugin-jwt` も全件成功（feature なしでは `refresh_gate` は
  cfg により空になる）。

## 前提と既知の限界

- 仮想時間のテストであり、実際の経過時間は計測していない。
- `exp` の判定と利用者通知口の間引き（`NotifyThrottle`）は `SystemTime` に依存し、仮想時間では進まない。
  トークンの `exp` は壁時計に対して十分先に置いた。
- 「下限 + 取得時間」の保証は、`fetch_timeout <= min_interval` の構築時検査と、合図ハンドルを
  `jwks_refresh_trigger` に登録する形が前提。`unknown_key_notifier` 経由で取り直しを起動すると間引きの分だけ遅れる。
- NFR-6 の HTTP 計測は `oha` 未導入のため未計測のまま。
- ガイド・CHANGELOG・クレート doc への追随は #898 の担当。
