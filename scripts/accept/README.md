# scripts/accept — 受け入れ検証スクリプト

`core-deps-unsafe-audit.sh`（REQ-1）・`plugin-mechanism-accept.sh`（REQ-2、TASK-2.4）・
`dep-audit-accept.sh`（REQ-15、TASK-15.4）・`req13-change-impact-accept.sh`（REQ-13、TASK-13.2）・
`webrtc-accept.sh`（REQ-8、TASK-8.4）・`graphql-accept.sh`（REQ-5、TASK-5.2）・
`openapi-ts-accept.sh`（REQ-6、TASK-6.2）・`tracing-accept.sh`（REQ-10、TASK-10.4 / TASK-10.5）・
`hub-wiring-accept.sh`（REQ-9、TASK-9.5）・`websocket-accept.sh`（REQ-4、TASK-4.4）
の 10 スクリプトを収録する。以下はまず REQ-1 側の詳細、他は本ファイル末尾の各節を参照。

`docs/spec/04-requirements.md` REQ-1（最小コア）の受け入れ基準のうち、**性能計測を除く**
非性能系の受け入れ基準（依存クレート数比・unsafe 根拠・audit / deny・実質コード行数・
拡張点・プラグイン非依存）を機械的に検証する（TASK-1.6-2、#72）。

性能計測（RPS・レイテンシ・RSS・バイナリサイズ・起動時間、axum-ref 比）は
姉妹イシュー TASK-1.6-1（#71）が `benches/` で担当する。

## 検証する受け入れ基準

| # | 基準 | 検証手段 |
|---|------|---------|
| A | コアの推移的依存クレート数が axum ベース実装の 50% 以下 | `cargo tree` の同一手法比較 |
| B | 自コード `unsafe` 0 件、または各箇所に `// SAFETY:` 根拠 100% 記述 | grep + workspace lint 確認 + `cargo geiger`（導入済みなら参考値） |
| C | `cargo audit` 既知脆弱性 0 件・`cargo deny check` ライセンス/出所違反 0 件 | cargo-audit / cargo-deny 実行 |
| D | コア実質コード行数 5,000 行以内 | 空行・コメント行を除いた行数集計 |
| E | 3 種拡張点（`Middleware`/`UpgradeHandler`/`RequestGate`）が trait 定義され、コアループ本体が feature 有無で分岐しない | trait 存在の grep + `crates/core/src/server.rs` のコアループ 3 関数（`BoundServer::run`・`handle_connection`・`handle_connection_with_permit`）を awk で範囲抽出し、コメント除外付きで `#[cfg(feature` 不在を grep（#169 是正） |
| F | `routes`・`http/` がプラグイン固有シンボルへ依存しない | 識別子パターン grep + `Cargo.toml` 依存確認 |

## 前提ツール

スクリプトはツールを自動ダウンロードしない（サプライチェーン考慮、
`.claude/rules/security.md`）。未導入の場合は SKIP として記録し、導入コマンドを案内する。

| ツール | 用途 | 導入コマンド |
|--------|------|-------------|
| `cargo-audit` | 基準 C（既知脆弱性） | `cargo install cargo-audit` |
| `cargo-deny` | 基準 C（ライセンス・出所） | `cargo install cargo-deny` |
| `cargo-geiger`（任意） | 基準 B の参考値 | `cargo install cargo-geiger` |
| `tokei`（任意） | 基準 D の参考値 | 各 OS のパッケージマネージャ等 |

`cargo audit` / `cargo deny check` はネットワークアクセス（advisory DB 取得・
crates.io index 更新）を伴う。

## 実行方法

リポジトリルートから実行する。

```bash
./scripts/accept/core-deps-unsafe-audit.sh
```

終了コード 0 = FAIL なし（PASS / SKIP / WARN のみ）、非 0 = 1 件以上 FAIL。
再実行可能（べき等）で、前提タスク（下記）マージ後に再実行すれば判定が更新される。

## 前提タスク未完時の挙動（SKIP）

並列実行中の以下のタスクが本スクリプト実行時点で未マージの場合、該当チェックは
FAIL ではなく SKIP として記録され、終了コードには影響しない。

- **TASK-1.4-2（#70、コアループ）未マージ**: 基準 E の「ループ本体が feature 分岐しない」
  部分のみ SKIP（`crates/core/src/server.rs` 自体が存在しない場合。trait 定義の存在
  確認は独立して実行される）。TASK-1.4-2（#70）は既にマージ済みで現行 main では
  SKIP は発生しない
- **TASK-1.5（#14、`crates/routes`）未作成**: 基準 F の routes 部分のみ SKIP
  （`crates/http` の検証は独立して実行される）。TASK-1.5（#14）は既にマージ済みで
  現行 main では SKIP は発生しない
- **TASK-15.1（#16、`deny.toml` 整備）未完**: `deny.toml` 不在時は
  `cargo deny check advisories bans sources` を既定設定で実行し、licenses チェックは
  WARN として保留を記録する

## 基準 E・基準 B/D の grep/awk 検証の制約事項

- **基準 E（コアループの feature 非分岐、#169 是正）**: 検査対象は
  `crates/core/src/server.rs` の `BoundServer::run`・`handle_connection`・
  `handle_connection_with_permit` の 3 関数本体に限定する
  （`docs/design/plugin-boundary.md` §3「コアループは cfg-free を維持する」の定義）。
  同ファイル内でも `Server` のビルダーメソッド・cfg-gated 設定フィールド・
  `plugin.rs` の cfg 集約シームは §4-5 が明示許容する領域であり検査対象外。
  awk による関数範囲抽出は **rustfmt 整形済み**（開始行と終端 `}` が同一インデント）
  であることを前提にする。CI が `cargo fmt --check` を強制するため通常は成立するが、
  手元で未整形のまま本スクリプトを実行すると関数範囲を誤検出しうる。抽出できた
  関数数が 0 件（関数名変更等）の場合は誤 PASS を避け明示的に FAIL とする
  （フェイルクローズ）
- **基準 B/D（既存、#72 由来）と同様の限界**: 行頭 `//`（`///`・`//!` 含む）の除外
  のみ対応し、`/* ... */` ブロックコメント内の記述は除外できない。基準 E も同一の
  行頭 `//` 除外手法のため同じ限界を持つ

## 出力の読み方

- `PASS`: 基準を満たした
- `FAIL`: 基準を満たさなかった（受け入れ未達。終了コードを非 0 にする）
- `SKIP`: 前提タスク未完・対象クレート不在等により検証対象が存在しない（保留）
- `WARN`: 参考情報・暫定運用の記録（判定には影響しない）

実行結果レポートは `docs/acceptance/req1-deps-unsafe-audit.md` に記録する。

## `plugin-mechanism-accept.sh` — REQ-2（プラグイン機構）受け入れ検証（TASK-2.4、#21）

`docs/spec/04-requirements.md` REQ-2 の受け入れ基準のうち TASK-2.4 が担う 3 点
（2 種以上のプラグインの feature flag 着脱・コンパイル時 vs 動的ロードのトレードオフ
設計文書・受け入れテストスクリプトと結果）を検証する。`core-deps-unsafe-audit.sh`
と同じ `lib/common.sh`（PASS/FAIL/SKIP/WARN 集計）を共有する。

```bash
./scripts/accept/plugin-mechanism-accept.sh
```

検証内容:

1. `webrtc-proxy`・`graphql` の 2 feature が `backend-framework-core` に存在すること
   （`cargo metadata` + `jq`）
2. `scripts/pay-for-what-you-use-check.sh`（TASK-2.2）を呼び出し、feature 無効時の
   依存・unsafe・バイナリサイズ完全除外を確認（動的列挙のため graphql 追加時も
   同スクリプトの変更は不要）
3. 4 通りの feature 構成（無効・graphql 単独・webrtc-proxy 単独・全 feature）で
   `cargo build` / `cargo test` が成功すること
4. `docs/design/plugin-loading-tradeoffs.md`（安全性トレードオフ設計文書）の存在

両 feature 無効時のコア性能（REQ-1 基準維持）は axum-ref 等価計測用バイナリが
TASK-1.6-1（#71）BLOCKED のため自動検証対象外（SKIP として記録、フェイルクローズで
PASS を偽らない）。手動計測手順・実行結果は
`docs/acceptance/req2-plugin-mechanism.md` を参照。

## `dep-audit-accept.sh` — REQ-15（依存監査基盤）受け入れ検証（TASK-15.4、#52）

`docs/spec/04-requirements.md` REQ-15 の受け入れ基準を、依存監査基盤系列
（TASK-15.1〜15.3）の最終受け入れテストとして検証する。`core-deps-unsafe-audit.sh` と
同じ `lib/common.sh`（PASS/FAIL/SKIP/WARN 集計）を共有する。

```bash
./scripts/accept/dep-audit-accept.sh
```

### 検証する受け入れ基準

| # | 基準 | 検証手段 |
|---|------|---------|
| 1 | `deny.toml` ベースライン設定（許可ライセンスリスト・`[graph] all-features = true`・`[advisories] ignore = []`）がリポジトリに存在する（TASK-15.1） | `deny.toml` の静的検査（grep/awk、ツール不要） |
| 2 | 全 feature 構成（no-default / default / 各 feature / all-features）で `cargo audit` 既知脆弱性 0 件・`cargo deny check` 違反 0 件（TASK-15.2） | 既存 `scripts/dep-audit.sh` を再利用（重複実装しない。feature 動的列挙のためプラグイン追加時も本スクリプトの変更は不要） |
| 3 | コアパーサ（`crates/http/src`）への fuzz スクリーニング実施（TASK-15.3、Conditional Go 条件(4)） | fuzz target・CI `fuzz-smoke` ジョブ配線・`docs/design/fuzzing.md` の本実行結果記録の存在確認。pinned nightly + cargo-fuzz 導入済み環境では任意で 60 秒 smoke 実測も行う |

### 前提ツール

| ツール | 用途 | 導入コマンド |
|--------|------|-------------|
| `cargo-deny` | 基準 2（ライセンス・出所） | `cargo install --locked cargo-deny@0.19.8` |
| `cargo-audit` | 基準 2（既知脆弱性） | `cargo install --locked cargo-audit@0.22.2` |
| `jq` | 基準 2（feature 動的列挙） | OS のパッケージマネージャで導入（例: `apt install jq`） |
| `cargo-fuzz`（任意） | 基準 3d（fuzz smoke 実測） | `cargo install --locked cargo-fuzz@0.13.2` |

基準 1・3a〜3c はツール不要（静的検査のみ）。基準 2・3d はネットワークアクセス
（advisory DB 取得・crates.io index 更新）を伴う。前提ツール未導入時は SKIP として記録し、
終了コードには影響しない（フェイルクローズだが判定不能を FAIL と混同しない、
`.claude/rules/security.md` のサプライチェーン方針を踏襲）。

### 出力の読み方

`core-deps-unsafe-audit.sh` と同一（PASS/FAIL/SKIP/WARN、終了コードは FAIL 1 件以上で
非 0）。実行結果レポートは `docs/acceptance/req15-dep-audit.md` に記録する。

### セルフテスト

`scripts/tests/run-dep-audit-accept-tests.sh`（ネットワーク・cargo ビルド・監査ツールに
非依存。判定ロジックのみを `scripts/tests/fixtures/dep-audit-accept/` の fixture で
検証する）。`.github/workflows/ci.yml` の `unsafe-triage` ジョブから呼ばれる。

## `req13-change-impact-accept.sh` — REQ-13（変更影響範囲の機械判定構造）受け入れ検証（TASK-13.2、#50）

`docs/spec/04-requirements.md` REQ-13 の受け入れ基準（拡張点への閉包可否・閉じない場合の
理由明記、依存方向の機械可読明示）を基準 A〜F で検証する
（`docs/design/dependency-graph-contract.md` 対応）。`core-deps-unsafe-audit.sh` と同じ
`lib/common.sh`（PASS/FAIL/SKIP/WARN 集計）を共有する。

```bash
./scripts/accept/req13-change-impact-accept.sh
```

検証内容（基準 A〜F）:

- A. 依存方向一方向性の機械検証（`scripts/dep-direction-check.sh` の呼び出し）
- B. プラグイン全クレートの拡張点対応宣言（`crates/plugin-*/src/lib.rs` の
  `//! 拡張点対応: <値>` 統一形式・許可語彙・参照先設計文書の存在）
- C. 契約ドキュメント（`docs/design/dependency-graph-contract.md`）の存在・必須セクション
- D. 実例 3 コミット（WebSocket/GraphQL/WebRTC）の閉包判定再現
- E. 閉包違反（WebRTC の E ファイル `crates/http/src/response.rs`）の理由明記照合
- F. `scripts/extension-closure-check.sh` / `scripts/extension-closure-gate.sh` の
  セルフテスト

`--crates-dir <dir>` / `--contract-doc <file>` でそれぞれ基準 B・基準 C の検証対象を
差し替え可能（`scripts/tests/run-req13-accept-tests.sh` のセルフテスト用注入口、
`dep-direction-check.sh` の `--crates-dir` 慣例を踏襲）。

実行結果レポートは `docs/acceptance/req13-change-impact.md` に記録する。

## `webrtc-accept.sh` — REQ-8（WebRTC）受け入れ検証（TASK-8.4、#29）

`docs/spec/04-requirements.md` REQ-8・NFR-6 の受け入れ基準のうち TASK-8.4 が担う
「依存・バイナリ・unsafe を再評価し audit / deny を確認する」を検証する。
`lib/common.sh`（PASS/FAIL/SKIP/WARN 集計）と `lib/nfr6-ratio.sh`（NFR-6 判定ロジック
単体、オフラインテスト用に分離）を共有する。

```bash
./scripts/accept/webrtc-accept.sh
```

検証内容:

1. `webrtc` feature 無効時、`backend-framework-core` の依存ツリーに webrtc 系依存が
   一切現れない（`cargo tree`）
2. `crates/plugin-webrtc` 自コードの unsafe が 0 件（grep。依存側 `webrtc-rs` の
   unsafe 増分は PoC-5 実測値を参考記録として引用するのみ）
3. 全 feature 構成で `cargo audit` / `cargo deny check` 違反 0 件（`scripts/dep-audit.sh`
   呼び出し）
4. `webrtc`・`webrtc-proxy` の 2 feature が `backend-framework-core` に存在し、
   `crates/plugin-webrtc`・`crates/plugin-webrtc-proxy` がクレート境界で相互非依存
   であること
5. NFR-6（無関係パスへの RPS・レイテンシ影響）。計測用バイナリ
   （`target/release/examples/minimal`・`target/release/examples/webrtc_nfr6`）と
   `oha` が揃っていれば `benches/webrtc-nfr6-bench.sh` で empirical 計測し、
   実務許容帯 [95%, 105%]（FAIL 境界）・狭義 NFR-6 帯 [100.3%, 100.8%]（PASS/WARN 境界）
   と照合する。揃っていなければ SKIP + 実行手順を案内する

前提: `cargo build --release -p backend-framework-core --example minimal
--no-default-features` と `... --example webrtc_nfr6 --features webrtc` を事前実行
（本スクリプトは自動ビルドしない）。

判定ロジックのオフライン・セルフテスト（cargo・ネットワーク非依存）は
`scripts/tests/run-webrtc-accept-tests.sh` を参照。実行結果レポートは
`docs/acceptance/req8-webrtc-attack-surface.md`、NFR-6 の詳細計測結果は
`benches/reports/task-8.4-webrtc-nfr6.md` に記録する。

## `websocket-accept.sh` — REQ-4（WebSocket）受け入れ検証（TASK-4.4、#25）

`docs/spec/05-tasks.md` TASK-4.4「WebSocket プラグイン受け入れテスト」が担う受け入れ
基準を検証する。`lib/common.sh`（PASS/FAIL/SKIP/WARN 集計）と `lib/nfr6-ratio.sh`
（NFR 判定ロジック単体、`webrtc-accept.sh`/`graphql-accept.sh` と共有）を使う
`graphql-accept.sh` 同型のオーケストレータ。

```bash
./scripts/accept/websocket-accept.sh
```

検証内容:

1. **A: `websocket` feature 無効時の依存完全除外** — `cargo tree -p
   backend-framework-core -e normal --no-default-features` に
   `tokio-tungstenite` / `tungstenite` / `bf-plugin-websocket` 系依存が一切現れない
   こと。`scripts/pay-for-what-you-use-check.sh`（動的列挙のため websocket feature も
   自動検証対象）も併走し、依存・unsafe・バイナリサイズ完全除外を二重に確認する
2. **A': `crates/plugin-websocket` 自コードの unsafe が 0 件**（grep）
3. **B: 回帰テスト** — `cargo test -p backend-framework-core --features websocket`
   （境界テスト `websocket_upgrade.rs`・`websocket_respawn.rs`）・`cargo test -p
   bf-plugin-websocket`（RFC 6455 ハンドシェイク契約テスト）・`cargo test -p
   backend-framework-core --no-default-features`（フォールスルー陰性対照
   `websocket_upgrade_disabled.rs`）がすべて成功すること
4. **C: レイテンシ計測（p95・劣化定量化）** — `WEBSOCKET_ACCEPT_RESULT_JSON` env に
   `benches/bench-ws-load.sh` の `RESULT_JSON` 出力パスを指定すると、ティア別
   心拍 RTT p95・接続数増による劣化率（最小ティア比）の記録が存在することを検証する。
   未指定・バイナリ未ビルド時は SKIP + 実行手順を案内する（本スクリプト自体は長時間
   負荷試験を自動実行しない）
5. **D: NFR-6（無関係パスへの RPS・レイテンシ影響）** — 計測用バイナリ
   （`target/release/examples/minimal`・`target/release/examples/ws_nfr6`）と `oha`
   が揃っていれば `benches/ws-nfr6-bench.sh` で empirical 計測し、
   `webrtc-accept.sh`/`graphql-accept.sh` と同じ実務許容帯 [95%, 105%]（FAIL 境界）・
   狭義帯 [100.3%, 100.8%]（PASS/WARN 境界）と照合する。揃っていなければ SKIP + 実行
   手順を案内する

前提: `cargo build --release -p backend-framework-core --example minimal
--no-default-features` と `... --example ws_nfr6 --features websocket` を事前実行
（基準 D）。基準 C は追加で `cargo build --release -p backend-framework-core
--features websocket --example ws_echo`・`cargo build --release -p axum-ref --features
ws --target-dir target/ws-bench`・`cargo build --release -p ws-load-client` の後、
`benches/bench-ws-load.sh` を `RESULT_JSON` 指定で実行しておく（本スクリプトは自動
ビルド・自動長時間負荷試験を行わない）。

`examples/ws_nfr6.rs`（NFR-6 専用、`current_thread` ランタイム）は `examples/ws_echo.rs`
（TASK-4.3 の 10,000 同時接続負荷試験専用、`multi_thread` ランタイム）とは別の
example である点に注意。ベースライン `examples/minimal.rs` も `current_thread` の
ため、ランタイム構成を揃えないと NFR-6 比較が「feature の処理コスト」ではなく
「ランタイムのスレッド数差」を計測してしまう（詳細は `crates/core/examples/ws_nfr6.rs`
の doc comment・`benches/reports/task-4.4-ws-latency.md` を参照）。

判定ロジックのオフライン・セルフテスト（cargo・ネットワーク非依存）は
`scripts/tests/run-websocket-accept-tests.sh` を参照。実行結果レポートは
`docs/acceptance/req4-websocket.md`、レイテンシ・NFR-6 の詳細計測結果は
`benches/reports/task-4.4-ws-latency.md` に記録する。

## `graphql-accept.sh` — REQ-5（GraphQL）受け入れ検証（TASK-5.2、#53）

`docs/spec/04-requirements.md` REQ-5 の受け入れ基準のうち TASK-5.2「GraphQL 受け入れ
テスト」が担う 3 点を検証する。`lib/common.sh`（PASS/FAIL/SKIP/WARN 集計）と
`lib/nfr6-ratio.sh`（NFR 判定ロジック単体、`webrtc-accept.sh` と共有）を使う
`webrtc-accept.sh` と同型のオーケストレータ。

```bash
./scripts/accept/graphql-accept.sh
```

検証内容:

1. `graphql` feature 無効時、`backend-framework-core` の依存ツリーに
   `async-graphql` / `bf-plugin-graphql` 系依存が一切現れない
   （`cargo tree -p backend-framework-core -e normal --no-default-features`。
   `-e normal --no-default-features` は `crates/core` がテスト専用に持つ
   `async-graphql` dev-dependency を除外するために必須）。
   `scripts/pay-for-what-you-use-check.sh`（動的列挙のため graphql feature も自動
   検証対象）も併走し、依存・unsafe・バイナリサイズ完全除外を二重に確認する
2. `crates/plugin-graphql` 自コードの unsafe が 0 件（grep）
3. 最小疎通（クエリ実行と結果 JSON の返却）。`cargo test -p backend-framework-core
   --features graphql`（境界テスト `plugin_graphql_boundary.rs`）・
   `cargo test -p bf-plugin-graphql`（契約テスト）に加え、ビルド済み
   `graphql_nfr6` バイナリがあれば curl で `POST /graphql` に実際にクエリを送り
   `data.hello == "world"` を live 検証する
4. NFR（無関係パスへの RPS・レイテンシ影響）。計測用バイナリ
   （`target/release/examples/minimal`・`target/release/examples/graphql_nfr6`）と
   `oha` が揃っていれば `benches/graphql-nfr6-bench.sh` で empirical 計測し、
   `webrtc-accept.sh` と同じ実務許容帯 [95%, 105%]（FAIL 境界）・狭義帯
   [100.3%, 100.8%]（PASS/WARN 境界）と照合する。揃っていなければ SKIP + 実行手順を
   案内する

前提: `cargo build --release -p backend-framework-core --example minimal
--no-default-features` と `... --example graphql_nfr6 --features graphql` を事前実行
（本スクリプトは自動ビルドしない）。

判定ロジックのオフライン・セルフテスト（cargo・ネットワーク非依存）は
`scripts/tests/run-graphql-accept-tests.sh` を参照。実行結果レポートは
`docs/acceptance/req5-graphql.md`、NFR の詳細計測結果は
`benches/reports/task-5.2-graphql-performance.md` に記録する。

## `tracing-accept.sh` — REQ-10（可観測性）サンプリング適用後性能再検証（TASK-10.4、#59）+
依存インパクト記録・文書化（TASK-10.5、#60）

`docs/spec/05-tasks.md` TASK-10.4「サンプリング適用後性能再検証」・TASK-10.5「依存
インパクト記録・文書化・受け入れテスト」の受け入れ基準を検証する `lib/common.sh` 利用の
`graphql-accept.sh` 同型オーケストレータ。

```bash
./scripts/accept/tracing-accept.sh
```

検証内容:

1. **A: `tracing` feature 無効時の依存完全除外** — `cargo tree -p
   backend-framework-core -e normal --no-default-features` に `bf-plugin-tracing` /
   `tracing-appender` / `tracing-subscriber` が一切現れないこと（pay-for-what-you-use）
2. **B: テスト回帰** — `cargo test -p backend-framework-core`（feature 無効/`tracing`
   有効の両方）・`cargo test -p bf-plugin-tracing` が成功すること
3. **C: NFR** — TASK-10.1〜10.3 の全緩和策適用後、`GET /health` への RPS 劣化 5% 以内
   （RPS 比 ≥95%）・p95 悪化 110% 以内（p95 比 ≤110%）を `benches/tracing-nfr-bench.sh`
   の実測（シナリオ A）で確認する。`webrtc-accept.sh` / `graphql-accept.sh` の NFR-6
   判定帯（狭義 100.3〜100.8%）とは別の帯（REQ-10 の成功基準そのもの）。ビルド済み
   バイナリ・`oha` が揃っていなければ SKIP + 実行手順を案内する
4. **D（TASK-10.5）: 依存インパクト記録・連携方式設計文書の存在検証** —
   `docs/dep-impact/records.md` の plugin-tracing エントリ・`docs/design/
   tracing-integration.md` の存在を grep で機械検証する
5. **E（TASK-10.5）: 依存クレート数増分の機械検証** — `cargo tree -p
   backend-framework-core --features tracing` の union 展開差分件数を算出し、
   `docs/dep-impact/records.md` 記録値（+24 クレート）と許容帯（±5）で突合する

前提: `cargo build --release -p backend-framework-core --example minimal
--no-default-features` と `... --example tracing_nfr --features tracing` を事前実行
（本スクリプトは自動ビルドしない）。

基準未達（FAIL）でも `docs/spec/06-roadmap.md` の分岐どおり「デフォルト無効・
明示的 opt-in feature」を維持する結論自体は成立する（現状 `default = []`）。本
スクリプトは実測を PASS/WARN/FAIL として機械記録するのみで、分岐判断そのものは
実行結果レポート側の役割とする。実行結果レポートは
`benches/reports/task-10.4-tracing-performance.md` に記録する。

## `openapi-ts-accept.sh` — REQ-6（openapi-typescript 連携）受け入れ検証（TASK-6.2、#55）

`docs/spec/05-tasks.md` TASK-6.2「陰性対照 CI 型検査整備・受け入れテスト」の受け入れ
基準を検証する `lib/common.sh` 利用の `graphql-accept.sh` 同型オーケストレータ。

```bash
./scripts/accept/openapi-ts-accept.sh
```

検証内容:

1. **A: 陽性対照** — 最低 1 つのエンドポイント呼び出し（`ts/src/usage.ts` の 5
   エンドポイント一巡）が `tsc --noEmit` を通ること。`scripts/openapi-ts.sh`
   （TASK-6.1、#54）の成功で検証する
2. **B: 陰性対照** — 意図的な型不一致が `tsc --noEmit` のエラーとして確実に検出
   されること。`scripts/openapi-ts-negative.sh`（N1: `ts/src/negative/type-mismatch.ts`
   の 4 類型・N2: openapi.json 境界からの型不一致伝搬）の成功で検証する
3. **C: Rust 定義変更の伝搬** — `crates/plugin-openapi/src/docs.rs` の
   `/users/{id}` の `id` を `u64`→`String` へ一時的に変更 →
   `gen-openapi --update` → `npm run gen:types` のみで `ts/src/generated/schema.d.ts`
   に差分が現れ、既存 `usage.ts` の型検査が `TS2322` で失敗することを確認する。
   `trap` で必ず元の状態へ復元する。対象パス（`docs.rs` / `openapi.json` /
   `schema.d.ts`）に未コミット変更がある場合は SKIP し、勝手に破棄しない
   （`.claude/rules/security.md` 作業ツリー整合性）

前提: `node`（>=24）・`npm`・`cargo`。いずれか未導入の場合は該当基準を SKIP する
（自動ダウンロードしない既存規約）。

判定ロジックのオフライン・セルフテスト（cargo・ネットワーク非依存）は
`scripts/tests/run-openapi-ts-negative-tests.sh` を参照（`openapi-ts-negative.sh`
本体の判定ロジックを fixture で検証。`openapi-ts-accept.sh` 自体の A/B/C 判定分岐は
軽量なため専用セルフテストは設けず、本スクリプトの実行結果と
`docs/acceptance/req6-typescript-types.md` の記録で確認する）。実行結果レポートは
`docs/acceptance/req6-typescript-types.md` に記録する。

## `hub-wiring-accept.sh` — REQ-9（hub 共通配線）受け入れ検証（TASK-9.5、#65）

`docs/spec/05-tasks.md` TASK-9.5「hub 共通配線受け入れテスト」の受け入れ基準を検証する
`lib/common.sh` 利用の `webrtc-accept.sh` 同型オーケストレータ。

```bash
./scripts/accept/hub-wiring-accept.sh
```

検証内容:

1. **A: 越境遮断・フェイルクローズ受け入れテスト** — `cargo test -p
   bf-plugin-hub-wiring --test hub_acceptance`（PoC-6 相当の実データ入りマルチ
   テナントハンドラで、越境クエリ全件遮断・JWT 欠落/不正時のフェイルクローズ・
   鍵ローテーション・検証結果キャッシュ共有を固定する 16 テスト）が全件 PASS
   すること
2. **B: 配線コード削減率** — `examples/hub_service_demo.rs` のマーカー区間
   （`// --- wiring:begin --- 〜 // --- wiring:end ---`）の実 LOC を PoC-6 基準
   （3 エンドポイント・207 行）に対して評価し、削減率 90% 以上で PASS
   （`scripts/accept/lib/hub-wiring-loc.sh`）。ハンドラ領域（`build_router`）に
   手書き JWT 検証・JWKS パース等の配線シンボルが現れないことも同ライブラリで
   grep 検証する
3. **C: 依存方向・pay-for-what-you-use** — `cargo tree -p backend-framework-core`
   に `bf-plugin-hub-wiring` が一切現れないこと（依存逆転型プラグインの維持）
4. **D: NFR-6** — `bf-plugin-hub-wiring` をリンクした hub サービス（`BF_HUB_GATE=off`
   で `TenantGate` 未登録）が無関係パス（`GET /`）へ与える影響を
   `benches/hub-nfr6-bench.sh` の実測で確認する。`webrtc-accept.sh` /
   `graphql-accept.sh` と同一の NFR-6 判定帯（狭義 100.3〜100.8%・実務 [95%,105%]）を
   使う。ビルド済みバイナリ・`oha` が揃っていなければ SKIP + 実行手順を案内する

前提: `cargo build --release -p backend-framework-core --example minimal
--no-default-features` と `cargo build --release -p bf-plugin-hub-wiring --example
hub_service_demo` を事前実行（本スクリプトは自動ビルドしない）。

判定ロジックのオフライン・セルフテスト（cargo・ネットワーク非依存）は
`scripts/tests/run-hub-wiring-accept-tests.sh` を参照。実行結果レポートは
`docs/acceptance/req9-hub-wiring.md` に記録する。
