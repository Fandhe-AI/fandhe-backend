# REQ-4 受け入れ検証レポート — WebSocket 受け入れテスト（TASK-4.4、#25）

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` / `bf-plugin-*` 等）
> 表記のまま保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。

`docs/spec/04-requirements.md` REQ-4（WebSocket）の受け入れ基準のうち TASK-4.4 が担う
「WebSocket プラグイン受け入れテスト」を `scripts/accept/websocket-accept.sh` で
検証した結果。TASK-4.1（#22、RFC 6455 ハンドシェイク・`UpgradeHandler` 拡張点配線）・
TASK-4.2（委譲後の専用タスク再 spawn + permit 引き継ぎ最適化）・TASK-4.3（#24、
10,000 同時接続負荷試験・RSS 再計測、PR #164）は前提タスクとして `origin/main` へ
マージ済み（本レポートはそれらの変更を前提とし、production コードの追加変更は
行っていない。`crates/core/examples/ws_nfr6.rs`・`benches/ws-nfr6-bench.sh`・
`scripts/accept/websocket-accept.sh`・`benches/bench-ws-load.sh` への RTT percentile
抽出追加はいずれも test スコープの新規追加・拡張）。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-18 |
| 対象コミット（作業ブランチ起点、`origin/main`） | `85b65ffdb08f861a8cf7712d9372a61adc496784` |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| oha | 1.15.0 |
| jq | 1.8.1 |
| OS | Linux 7.0.0-27-generic x86_64（Ubuntu） |
| CPU コア数 | 12（`nproc`） |
| `ulimit -n` | 524288 |
| `/proc/sys/net/ipv4/ip_local_port_range` | `32768-60999`（幅 28,231） |

## 判定サマリー（`scripts/accept/websocket-accept.sh`）

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A: websocket 無効時の依存完全除外 | `cargo tree -p backend-framework-core -e normal --no-default-features \| grep -c -E 'tokio-tungstenite\|tungstenite\|bf-plugin-websocket'` = 0 |
| WARN | A補足: websocket 有効時の依存インパクト（陽性対照） | 同条件 `--features websocket` = 3（`bf-plugin-websocket`・`tokio-tungstenite`・`tungstenite` が出現、配線切れでないことの確認が目的で PASS/FAIL 判定には使わない） |
| PASS | A補足: pay-for-what-you-use-check.sh | 全プラグイン feature（websocket 含む、動的列挙）の依存・unsafe・バイナリサイズ完全除外を確認（a〜e の全段階 PASS） |
| PASS | A': plugin-websocket 自コード unsafe 0件 | `crates/plugin-websocket/src` に unsafe 0 件（テキストベース走査） |
| PASS | B: `cargo test -p backend-framework-core --features websocket` | `websocket_upgrade.rs`（6 件）・`websocket_respawn.rs`（2 件）を含め成功 |
| PASS | B補足: `cargo test -p backend-framework-core --no-default-features` | `websocket_upgrade_disabled.rs`（2 件、feature 無効時のフォールスルー陰性対照）を含め成功 |
| PASS | B: `cargo test -p bf-plugin-websocket` | RFC 6455 ハンドシェイク契約テスト（16 件）・E2E テスト（2 件）・doc test（2 件）が成功 |
| PASS | C: レイテンシ計測（p95・劣化定量化） | `CONNECTION_TIERS="1000 5000 10000" HOLD_SECS=30 RUNS=3` で実測。詳細は下記・`benches/reports/task-4.4-ws-latency.md` |
| WARN〜PASS | D: NFR-6 無関係パス影響 | RPS 比・p95 比とも実務許容帯 [95%, 105%] 内に安定。詳細下記 |

**終了コード: 0（FAIL なし。PASS / WARN / SKIP のみ）**

## 基準 C（レイテンシ計測・劣化定量化）の詳細

`benches/bench-ws-load.sh`（`CONNECTION_TIERS="1000 5000 10000" HOLD_SECS=30 RUNS=3`）
を実行し、`crates/ws-load-client` が算出する心拍 RTT percentile（`heartbeat_rtt_us`）を
ティア別に中央値評価した。詳細な生ログ・接続あたり RSS 増分・axum 比較は
`benches/reports/task-4.4-ws-latency.md` を参照。

**結論（fullscratch 側）**: 接続数 1,000→5,000→10,000 で心拍 RTT p95 中央値は
915us→915us→1024us（詳細は上記レポート参照）と、10,000 接続時点でもマイクロ秒
オーダーに収まり、桁が変わるような劣化は観測されなかった。「接続数増による劣化
度合いの定量化」（受け入れ基準）を満たす実測データを記録した。

## 基準 D（NFR-6）の詳細

`benches/ws-nfr6-bench.sh`（`examples/minimal.rs` = ベースライン、`examples/ws_nfr6.rs` =
`websocket` feature 有効・`Server::websocket` 登録済み、ともに `current_thread`
ランタイム、RUNS=5・DURATION=5s・CONNECTIONS=32）を 3 回実行した結果:

| 実行 | RPS 比（ws_nfr6 / baseline） | p95 比（ws_nfr6 / baseline） | `evaluate_nfr6_ratio` |
|------|------|------|------|
| 1 回目 | 98.67% | 101.94% | WARN（実務帯内・狭義帯外） |
| 2 回目 | 100.36% | 99.92% | PASS（狭義帯内） |
| 3 回目（採用値） | 101.49% | 97.71% | WARN（実務帯内・狭義帯外） |

3 回とも実務許容帯 [95%, 105%]（RPS）・[0, 105%]（p95）に安定して収まり、FAIL は
1 度も観測されなかった。狭義帯（100.3〜100.8%相当）は 2 回目のみ達成。

**重要な前提修正（実装過程で判明した問題と対処）**: 当初計画では NFR-6 比較対象に
`examples/ws_echo.rs`（TASK-4.3 の 10,000 同時接続負荷試験専用、
`#[tokio::main(flavor = "multi_thread")]`）をそのまま流用する想定だったが、
ベースライン `examples/minimal.rs`（`current_thread`）と組み合わせて実測したところ
RPS 比が baseline 比 約190%・約190%（2 回とも再現）という説明のつかない値になった。
原因を調査した結果、`ws_echo.rs` の `multi_thread` ランタイム（全コア使用）と
`minimal.rs` の `current_thread` ランタイム（単一スレッド）というランタイム構成の
違いが RPS 差を支配しており、`websocket` feature 自体の処理コストを計測できて
いなかったことが判明した。対処として `examples/ws_nfr6.rs`（`graphql_nfr6.rs` /
`webrtc_nfr6.rs` と同型の `current_thread` 専用 NFR-6 計測用 example）を新設し、
ベースラインとランタイム構成を揃えたところ、上記の妥当な比率（95〜105%帯）が
安定して得られた。`examples/ws_echo.rs` 自体は TASK-4.3 の用途（10,000 同時接続
負荷試験）に変更なく引き続き使用する。

## 受け入れ条件チェックリストとの対応

- [x] 依存除外: 基準 A・A' が PASS（`cargo tree` 0 件・`pay-for-what-you-use-check.sh`
      PASS・unsafe 0 件）
- [x] レイテンシ計測・劣化定量化: 基準 C が PASS（1,000/5,000/10,000 接続の心拍 RTT
      percentile を実測・記録。桁が変わるような劣化は観測されず）
- [x] NFR-6（無関係パスへの性能影響誤差範囲）: 基準 D が実務許容帯内で安定
      （3 回中 FAIL 0 回、狭義帯は 1/3 回のみ達成。`graphql-accept.sh`（req5-graphql.md）
      が記録したような大きな振れ幅・FAIL は本タスクでは観測されなかった）
- [x] 成果物（スクリプト・実行結果レポート）が揃っている

## 検証コマンド一覧（再現手順）

```bash
# 事前ビルド
cargo build --release -p backend-framework-core --features websocket --example ws_echo
cargo build --release -p backend-framework-core --example minimal --no-default-features
cargo build --release -p backend-framework-core --features websocket --example ws_nfr6
cargo build --release -p axum-ref --features ws --target-dir target/ws-bench
cargo build --release -p ws-load-client

# レイテンシ計測（基準 C の前提。本計測は数分〜十数分かかる）
CONNECTION_TIERS="1000 5000 10000" HOLD_SECS=30 RUNS=3 \
    RESULT_JSON=/tmp/ws-bench-full.json bash benches/bench-ws-load.sh

# NFR-6 計測（基準 D の前提）
bash benches/ws-nfr6-bench.sh

# A・B・C・D をまとめて実行（C は WEBSOCKET_ACCEPT_RESULT_JSON 指定時のみ検証）
WEBSOCKET_ACCEPT_RESULT_JSON=/tmp/ws-bench-full.json bash scripts/accept/websocket-accept.sh

# 判定ロジックのオフライン・セルフテスト（cargo 非依存）
bash scripts/tests/run-websocket-accept-tests.sh

# pay-for-what-you-use ゲート
bash scripts/pay-for-what-you-use-check.sh

# 依存インパクトの個別確認
cargo tree -p backend-framework-core -e normal --no-default-features | grep -c -E 'tokio-tungstenite|tungstenite|bf-plugin-websocket'                    # 0
cargo tree -p backend-framework-core -e normal --no-default-features --features websocket | grep -c -E 'tokio-tungstenite|tungstenite|bf-plugin-websocket'  # 3

# 最小疎通・回帰テスト
cargo test -p backend-framework-core --features websocket
cargo test -p backend-framework-core --no-default-features
cargo test -p bf-plugin-websocket
```
