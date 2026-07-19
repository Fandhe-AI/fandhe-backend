# NFR-8 注入リグレッション検知率 実装フェーズ確定検証 — 実測レポート（#238）

`docs/spec/04-requirements.md` NFR-8「AI 生成テストによる注入リグレッションの検知率が
90% 以上」の実装フェーズ確定計測結果。ケース定義・選定基準は
`docs/reports/nfr8-injection-case-definitions.md` を参照（本レポートは実測値の記録に
責務を限定し、ケース定義は改変しない）。

## 実施環境

| 項目 | 値 |
|------|-----|
| 実測日 | 2026-07-19 |
| 起点コミット（origin/main HEAD） | `54e87a7`（ケース定義コミット時点と同一） |
| ケース定義コミット | `docs/reports/nfr8-injection-case-definitions.md` 本体（本 PR 内、実測より先行コミット） |
| OS | Linux 7.0.0-27-generic x86_64 |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| cargo-nextest | 0.9.137 (75ddba7e9 2026-05-26) |
| 実行コマンド | `bash scripts/regression-injection-verify.sh` |

ベースライン突合（起点コミット時点で既に FAIL しているテストの除外）は今回不要だった。
12 ケースとも起点コミットではクリーンな状態（`cargo clippy` / `cargo nextest` /
`cargo test --doc` 全通過）から出発し、環境依存の既知 FAIL テストは対象クレート
（`http` / `routes` / `core` / `plugin-websocket` / `plugin-graphql` /
`plugin-hub-wiring` / `plugin-tracing` / `plugin-webrtc-proxy`）のいずれにも存在しない。

## ケース別結果

| ID | 結果 | 検知チャネル | 備考 |
|----|------|-------------|------|
| R-01 | DETECTED | `cargo nextest`（`too_many_headers`） | |
| R-02 | DETECTED | `cargo clippy`（未使用ローカル変数 warning が `-D warnings` で検知）＋ `cargo nextest`（追加検証で確認、下記） | 本番計測では該当行削除に伴う clippy warning が先に検知した（`GATE=clippy`）。clippy 単独の検知では「意味論的な破壊を既存テストが捕捉できるか」を証明しないため、追加で `cargo nextest run -p fandhe-backend-http`（clippy を経由しない単独実行）を実施したところ `chunked::tests::single_chunk_exceeding_max_body_bytes_is_rejected`・`chunked::tests::total_decoded_across_chunks_exceeding_max_body_bytes_is_rejected` の 2 件が実際に FAIL することを確認した（ビルド自体は成功、`unused_imports` warning のみで打ち切られていない）。ケース定義の想定どおり意味論的検知が機能している |
| R-03 | DETECTED | `cargo clippy`（`clippy::overly_complex_bool_expr`、`if false && ...`） | ケース定義で想定した通り |
| R-04 | DETECTED | `cargo nextest`（`http11_connection_close_disables_keep_alive` 等・doc test） | |
| R-05 | DETECTED | `cargo nextest`（`match_segments_rejects_dot_and_dotdot_path_traversal`） | |
| R-06 | DETECTED | `cargo nextest`（`crates/core/src/server.rs` の `RequestGate` 拒否系テスト） | |
| R-07 | DETECTED | `cargo nextest`（`websocket_upgrade_disabled.rs`・`plugin_boundary*.rs`） | |
| R-08 | DETECTED | `cargo nextest`（`validate_rejects_malformed_key_length`） | |
| R-09 | DETECTED | `cargo nextest`（`falls_through_on_unrelated_path`・`falls_through_on_wrong_method`） | |
| R-10 | DETECTED | `cargo clippy`（`now_unix` 未使用引数 warning）＋ `cargo nextest`（追加検証で確認、下記） | 本番計測では `now_unix` 引数が未使用になったことで clippy が先行して検知した。R-02 と同じ理由で追加検証を行い、`cargo nextest run -p fandhe-backend-plugin-hub-wiring`（clippy を経由しない単独実行）で `gate::tests::expired_token_is_401`・`jwt::tests::expired_token_is_rejected`・`jwt::tests::exp_equal_to_now_is_rejected`・`hub_acceptance::expired_token_is_rejected_before_handler` 等 7 件が実際に FAIL することを確認した。ケース定義の想定どおり意味論的検知が機能している |
| R-11 | DETECTED | `cargo nextest`（doc test・`interval_hundred_samples_one_in_hundred`） | |
| R-12 | DETECTED | `cargo nextest`（`oversized_offer_is_rejected`） | |

生ログ（`GATE=<clippy|nextest|doctest>` の記録を含む）は計測実行時の一時ディレクトリに
出力されるが、全件 PASS（検知）のため `regression-injection-verify.sh` の設計どおり
保持していない（`third-party-verify.sh` と同じ「失敗時のみログを残す」方針は本ハーネスの
用途では逆方向 — 全件検知が成功条件のため、恒常的なログ保持は行わない。再現は下記コマンド
で可能）。

## 検知率

```
metric=injection_detection_rate pass=12 fail=0 pending=0 total=12
```

**検知率 12/12（100%）。NFR-8 の閾値（90%）を上回った。**

初回計測で 90% を上回ったため、`docs/reports/nfr8-injection-case-definitions.md` に定めた
「90% 未満ならテスト追加のうえ再計測する」手順は発動していない（テスト追加・再計測は
不要だった）。

## 追加検証: clippy 先行検知 2 件（R-02・R-10）の意味論的検知の裏付け

`regression-injection-verify.sh` は「いずれか 1 つのゲートが失敗すれば検知」と
定義しており、本番計測では R-02・R-10 の 2 件が `cargo clippy` で先に検知された
（該当パッチが未使用ローカル変数・未使用引数を残す実装だったため）。これは
NFR-8 の文言「AI 生成テストによる注入リグレッションの検知率」に照らすと
「テストによる検知」ではなく「lint による検知」であり、検知率の解釈にあいまいさが
残る（`clippy -D warnings` 自体は `.github/workflows/ci.yml` test ジョブ相当の
既存ゲートの一部だが、テストスイートそのものではない）。

このあいまいさを解消するため、R-02・R-10 それぞれについて `cargo clippy` を経由
しない `cargo nextest run -p <crate>` 単独実行を追加で行った（起点コミット
`54e87a7` に対して同一パッチを適用、ビルド自体が失敗していないことも確認）。

- **R-02**（`crates/http/src/chunked.rs`）: `cargo nextest run -p fandhe-backend-http`
  で `chunked::tests::single_chunk_exceeding_max_body_bytes_is_rejected`・
  `chunked::tests::total_decoded_across_chunks_exceeding_max_body_bytes_is_rejected`
  の 2 件が FAIL（138 passed, 2 failed / 140 件）
- **R-10**（`crates/plugin-hub-wiring/src/jwt.rs`）: `cargo nextest run -p
  fandhe-backend-plugin-hub-wiring` で `gate::tests::expired_token_is_401`・
  `jwt::tests::expired_token_is_rejected`・`jwt::tests::exp_equal_to_now_is_rejected`・
  `hub_acceptance::expired_token_is_rejected_before_handler`・
  `tenant_gate::expired_token_is_rejected_before_handler` 等 7 件が FAIL
  （102 passed, 7 failed / 109 件）

両ケースとも意味論的な破壊を既存の `#[test]` が確実に捕捉することを確認した。
したがって **12/12（100%）は「テストによる検知率」の厳密な解釈でも成立する**
（clippy 先行検知はあくまで実測実行順序の結果であり、テストが検知しないことを
意味しない）。

## 再現手順

```bash
# 実測本体（起点コミットの HEAD から使い捨て worktree を作り 12 ケースを計測する）
bash scripts/regression-injection-verify.sh

# 判定ロジックのセルフテスト（cargo 非依存、スタブゲートで集計ロジックのみ検証する。
# green であることは実測検知率そのものの保証ではない点に注意）
bash scripts/tests/run-regression-injection-tests.sh
```

## 既知の限界

- **検知ゲートの範囲**: 各ケースはパッチが変更したファイルが属するクレート
  ディレクトリ内でのみ `clippy` / `nextest` / doc test を実行する（workspace 全体では
  ない）。12 ケースは互いに独立したファイルへの単一注入であるため対象クレート内の
  検知力を計測すれば十分と判断したが、クレート横断で初めて顕在化する退行の検知力は
  本計測の対象外である（`ci-complete`（workspace 全体）が別途 PR 上で担う）
- **検知チャネルの実態**: ケース定義時点で想定した「検知チャネル」（主に `#[test]` /
  doc test）と実測の検知チャネル（`cargo clippy` が先行してヒットしたケースが 2 件、
  R-02・R-10）が一部異なった。これは検知ゲートを「いずれか 1 つでも失敗すれば検知」と
  定義した設計上の帰結であり、検知率の算出自体には影響しない（`clippy -D warnings` も
  `.github/workflows/ci.yml` の test ジョブ相当のゲートに含まれる既存テストスイートの
  一部のため）
- **母数 12 件は「AI 生成テストによる検知」の代表サンプル**であり、あらゆる将来の
  破壊的変更を代表するものではない。選定基準（ケース定義文書参照）に基づき、コア/
  プラグイン横断・バグ分類分散・セキュリティ後退を含む 12 件を選定した
