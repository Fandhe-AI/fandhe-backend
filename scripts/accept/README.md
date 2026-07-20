# scripts/accept — 受け入れ検証スクリプト

`core-deps-unsafe-audit.sh`（REQ-1）・`plugin-mechanism-accept.sh`（REQ-2、TASK-2.4）・
`dep-audit-accept.sh`（REQ-15、TASK-15.4）・`req13-change-impact-accept.sh`（REQ-13、TASK-13.2）・
`webrtc-accept.sh`（REQ-8、TASK-8.4）・`graphql-accept.sh`（REQ-5、TASK-5.2）・
`openapi-ts-accept.sh`（REQ-6、TASK-6.2）・`tracing-accept.sh`（REQ-10、TASK-10.4 / TASK-10.5）・
`hub-wiring-accept.sh`（REQ-9、TASK-9.5）・`websocket-accept.sh`（REQ-4、TASK-4.4）・
`ai-autonomy-accept.sh`（REQ-12/NFR-8、TASK-12.7）・`hub-e2e-accept.sh`（REQ-9 後続、#97）
の 12 スクリプトを収録する。以下はまず REQ-1 側の詳細、他は本ファイル末尾の各節を参照。

`docs/spec/04-requirements.md` REQ-1（最小コア）の受け入れ基準のうち、**性能計測を除く**
非性能系の受け入れ基準（依存クレート数比・unsafe 根拠・audit / deny・実質コード行数・
拡張点・プラグイン非依存）を機械的に検証する（TASK-1.6-2、#72）。

性能計測（RPS・レイテンシ・RSS・バイナリサイズ・起動時間、axum-ref 比）は
姉妹イシュー TASK-1.6-1（#71）が `benches/` で担当する。

## 検証する受け入れ基準

| # | 基準 | 検証手段 |
|---|------|---------|
| A | コアの推移的依存クレート数が axum ベース実装の 50% 以下 | `cargo tree` の同一手法比較 |
| B | 自コード `unsafe` 0 件、または各箇所に `// SAFETY:` 根拠 100% 記述 | grep + workspace lint 確認 + `cargo geiger`（独立ツールによる二重検証。#284。成功時は PASS/FAIL 判定、実行失敗時のみ WARN） |
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
| `cargo-geiger`（任意） | 基準 B の二重検証（#284） | `cargo install --locked cargo-geiger@0.13.0` |
| `jq`（任意） | 基準 B の geiger JSON 出力解析（#284。未導入時は geiger 二重検証のみ SKIP） | 各 OS のパッケージマネージャ（例: `apt install jq`） |
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
  （フェイルクローズ）。**PR #171 レビュー是正**: (1) 合計関数数のみのガードでは
  3 関数のうち 1 つがリネーム等で検出できなくなっても残り 2 関数のマッチで PASS
  し得たため、3 関数それぞれの検出有無を個別に追跡し、1 つでも未検出なら FAIL と
  する。(2) 対象関数のシグネチャ**直前**（関数本体の外側）に付与される
  `#[cfg(feature = "...")]` は関数本体のみの走査では検出できなかったため、関数外の
  実属性行を追跡し対象関数のシグネチャ行到達時にヒットとして合流させる（doc
  comment 中の `#[cfg(feature` という文字列引用は従来どおり誤検出しない）
- **基準 B/D（既存、#72 由来）と同様の限界**: 行頭 `//`（`///`・`//!` 含む）の除外
  のみ対応し、`/* ... */` ブロックコメント内の記述は除外できない。基準 E も同一の
  行頭 `//` 除外手法のため同じ限界を持つ

## 基準 B の cargo geiger 二重検証（#284）

従来は workspace ルート（仮想マニフェスト）に対して `cargo geiger -p
fandhe-backend-core` を実行しており、cargo-geiger 0.13.0 が仮想マニフェスト越しの
`-p` パッケージ選択に対応しないため常に失敗し、`2>/dev/null` で stderr が握り潰され
「B補足: cargo geiger」が参考値 WARN のまま固定化していた（`docs/acceptance/
req1-deps-unsafe-audit.md` 既知 WARN）。`--manifest-path crates/core/Cargo.toml` で
実パッケージを起点に指定すれば workspace 内の推移的依存（core → routes → http）を
含めて解決できる（`scripts/pay-for-what-you-use-check.sh` と同じ呼び出し方）。

- **`--manifest-path` は絶対パスで渡す**: cargo-geiger 0.13.0 は `--manifest-path`
  に相対パスを渡すと `manifest_path:"..." is not an absolute path` で確定的に
  失敗する（プレーンな `cargo` コマンドは相対パスを許容するため cargo-geiger
  固有の制約。リトライしても回復しない、#212 の非決定的 panic とは別種の失敗。
  `docs/acceptance/req1-deps-unsafe-audit.md` 再検証節で実測確認）。
  `${WORKSPACE_ROOT}`（`scripts/dep-impact.sh` では `${REPO_ROOT}`）を前置した
  絶対パスを渡すこと
- 専用 `CARGO_TARGET_DIR`（`target/accept-geiger`）で共有 `target/` のビルド
  キャッシュ破損・並列実行中の他ジョブとの競合を避ける
- イシュー #212（cargo-geiger の非決定的 panic、`docs/design/
  cargo-geiger-flakiness.md`）を踏まえ、最大 3 回の簡易リトライを行う
- 判定: geiger 出力（JSON、`jq` で解析）から対象コアクレート（`fandhe-backend-core`・
  `fandhe-backend-http`・存在すれば `fandhe-backend-routes`）の used unsafe
  （`functions`/`exprs`/`item_impls`/`item_traits`/`methods` の `unsafe_` 合算）が
  全て 0 なら PASS。非 0（grep ベースの基準 B 本体と矛盾）または対象クレートが出力に
  現れない（判定不能）なら FAIL（フェイルクローズ）。リトライ後も geiger 実行自体が
  失敗した場合のみ WARN とし、stderr 要約を詳細に残す（基準 B 本体（grep + workspace
  lint）が主判定を担うため、geiger 実行失敗そのものは FAIL にしない）

## 出力の読み方

- `PASS`: 基準を満たした
- `FAIL`: 基準を満たさなかった（受け入れ未達。終了コードを非 0 にする）
- `SKIP`: 前提タスク未完・対象クレート不在等により検証対象が存在しない（保留）
- `WARN`: 参考情報・暫定運用の記録（判定には影響しない）

実行結果レポートは `docs/acceptance/req1-deps-unsafe-audit.md` に記録する。

## `plugin-mechanism-accept.sh` — REQ-2（プラグイン機構）受け入れ検証（TASK-2.4、#21。実ペア再検証、#261）

`docs/spec/04-requirements.md` REQ-2 の受け入れ基準（**WebSocket・GraphQL** の
少なくとも 2 種のプラグインを feature flag で着脱できること）を、既定では
仕様が名指しする実プラグインペア（`websocket` + `graphql`）で検証する。
`core-deps-unsafe-audit.sh` と同じ `lib/common.sh`（PASS/FAIL/SKIP/WARN 集計）を
共有する。

対象 feature ペアは環境変数 `REQ2_FEATURES`（空白区切り、既定
`"websocket graphql"`）でパラメータ化できる。TASK-2.4（#21）実施当時の代替ペア
（実 WebSocket プラグインが並行実装中だったため使用した `webrtc-proxy` +
`graphql`）は `REQ2_FEATURES="webrtc-proxy graphql"` で再現できる（後方互換）。
入力は許可リスト正規表現（`[a-z0-9-]+`）+ `cargo metadata` 実在確認で検証し、
未知の文字列・存在しない feature 名は即 FAIL する（コマンドインジェクション
防止・判定不能を PASS と偽らないフェイルクローズ、`.claude/rules/security.md`）。

```bash
# 既定（websocket + graphql 実ペア）
./scripts/accept/plugin-mechanism-accept.sh

# 旧代替ペアを再現する場合
REQ2_FEATURES="webrtc-proxy graphql" ./scripts/accept/plugin-mechanism-accept.sh
```

検証内容:

1. 対象 feature ペア（既定: `websocket`・`graphql`）が `fandhe-backend-core` に
   存在すること（`cargo metadata` + `jq`）。未検出・入力不正の場合はここで
   即 FAIL し、以降の重い cargo build/test ステップは実行しない
2. `scripts/pay-for-what-you-use-check.sh`（TASK-2.2）を呼び出し、feature 無効時の
   依存・unsafe・バイナリサイズ完全除外を確認（動的列挙のため対象追加時も
   同スクリプトの変更は不要）。加えて対象ペア限定の直接証跡として `cargo tree` で
   当該プラグインクレートの無効時不出現・有効時出現（ポジティブコントロール）を確認
3. 各 feature 構成（無効・各 feature 単独・ペア同時有効・全 feature）で
   `cargo build` / `cargo test` が成功すること（実プラグインの統合テスト、例:
   `websocket_upgrade.rs`・`plugin_graphql_boundary.rs` による動作確認を兼ねる）。
   加えて対象プラグインクレート単体の契約テストも実行する
4. `docs/design/plugin-loading-tradeoffs.md`（安全性トレードオフ設計文書）の存在

5. 両 feature 無効時のコア性能（REQ-1 基準維持）: `benches/reports/
   task-2.4-plugin-accept.md` の「## 結論」セクション内の「総合判定」行を
   `lib/plugin-mechanism-conclusion-verdict.awk` で判定（他セクションへの過去実測の
   引用は無視し、セクションが複数存在する場合はレポート末尾に近い方＝直近の
   再計測結果を採用する、TASK-260 / #260）。`benches/bench-accept-exclusive.sh`
   （`REPORT_MD=` 指定）で再計測するたびにレポートへ「## 結論（自動記録）」
   セクションが追記され、手編集なしにゲートへ反映される。レポート不在・
   「## 結論」セクションに記録なし・BLOCKED のみの場合は SKIP（フェイルクローズで
   PASS を偽らない）。awk ロジックは `scripts/tests/
   run-plugin-mechanism-accept-tests.sh` で回帰検証する。手動計測手順・実行結果は
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

1. `webrtc` feature 無効時、`fandhe-backend-core` の依存ツリーに webrtc 系依存が
   一切現れない（`cargo tree`）
2. `crates/plugin-webrtc` 自コードの unsafe が 0 件（grep。依存側 `webrtc-rs` の
   unsafe 増分は PoC-5 実測値を参考記録として引用するのみ）
3. 全 feature 構成で `cargo audit` / `cargo deny check` 違反 0 件（`scripts/dep-audit.sh`
   呼び出し）
4. `webrtc`・`webrtc-proxy` の 2 feature が `fandhe-backend-core` に存在し、
   `crates/plugin-webrtc`・`crates/plugin-webrtc-proxy` がクレート境界で相互非依存
   であること
5. NFR-6（無関係パスへの RPS・レイテンシ影響）。計測用バイナリ
   （`target/release/examples/minimal`・`target/release/examples/webrtc_nfr6`）と
   `oha` が揃っていれば `benches/webrtc-nfr6-bench.sh` で empirical 計測し、
   実務許容帯 [95%, 105%]（FAIL 境界）・狭義 NFR-6 帯 [100.3%, 100.8%]（PASS/WARN 境界）
   と照合する。揃っていなければ SKIP + 実行手順を案内する

前提: `cargo build --release -p fandhe-backend-core --example minimal
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
   fandhe-backend-core -e normal --no-default-features` に
   `tokio-tungstenite` / `tungstenite` / `fandhe-backend-plugin-websocket` 系依存が一切現れない
   こと。`scripts/pay-for-what-you-use-check.sh`（動的列挙のため websocket feature も
   自動検証対象）も併走し、依存・unsafe・バイナリサイズ完全除外を二重に確認する
2. **A': `crates/plugin-websocket` 自コードの unsafe が 0 件**（grep）
3. **B: 回帰テスト** — `cargo test -p fandhe-backend-core --features websocket`
   （境界テスト `websocket_upgrade.rs`・`websocket_respawn.rs`）・`cargo test -p
   fandhe-backend-plugin-websocket`（RFC 6455 ハンドシェイク契約テスト）・`cargo test -p
   fandhe-backend-core --no-default-features`（フォールスルー陰性対照
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

前提: `cargo build --release -p fandhe-backend-core --example minimal
--no-default-features` と `... --example ws_nfr6 --features websocket` を事前実行
（基準 D）。基準 C は追加で `cargo build --release -p fandhe-backend-core
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

1. `graphql` feature 無効時、`fandhe-backend-core` の依存ツリーに
   `async-graphql` / `fandhe-backend-plugin-graphql` 系依存が一切現れない
   （`cargo tree -p fandhe-backend-core -e normal --no-default-features`。
   `-e normal --no-default-features` は `crates/core` がテスト専用に持つ
   `async-graphql` dev-dependency を除外するために必須）。
   `scripts/pay-for-what-you-use-check.sh`（動的列挙のため graphql feature も自動
   検証対象）も併走し、依存・unsafe・バイナリサイズ完全除外を二重に確認する
2. `crates/plugin-graphql` 自コードの unsafe が 0 件（grep）
3. 最小疎通（クエリ実行と結果 JSON の返却）。`cargo test -p fandhe-backend-core
   --features graphql`（境界テスト `plugin_graphql_boundary.rs`）・
   `cargo test -p fandhe-backend-plugin-graphql`（契約テスト）に加え、ビルド済み
   `graphql_nfr6` バイナリがあれば curl で `POST /graphql` に実際にクエリを送り
   `data.hello == "world"` を live 検証する
4. NFR（無関係パスへの RPS・レイテンシ影響）。計測用バイナリ
   （`target/release/examples/minimal`・`target/release/examples/graphql_nfr6`）と
   `oha` が揃っていれば `benches/graphql-nfr6-bench.sh` で empirical 計測し、
   `webrtc-accept.sh` と同じ実務許容帯 [95%, 105%]（FAIL 境界）・狭義帯
   [100.3%, 100.8%]（PASS/WARN 境界）と照合する。揃っていなければ SKIP + 実行手順を
   案内する

前提: `cargo build --release -p fandhe-backend-core --example minimal
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
   fandhe-backend-core -e normal --no-default-features` に `fandhe-backend-plugin-tracing` /
   `tracing-appender` / `tracing-subscriber` が一切現れないこと（pay-for-what-you-use）
2. **B: テスト回帰** — `cargo test -p fandhe-backend-core`（feature 無効/`tracing`
   有効の両方）・`cargo test -p fandhe-backend-plugin-tracing` が成功すること
3. **C: NFR** — TASK-10.1〜10.3 の全緩和策適用後、`GET /health` への RPS 劣化 5% 以内
   （RPS 比 ≥95%）・p95 悪化 110% 以内（p95 比 ≤110%）を `benches/tracing-nfr-bench.sh`
   の実測（シナリオ A）で確認する。`webrtc-accept.sh` / `graphql-accept.sh` の NFR-6
   判定帯（狭義 100.3〜100.8%）とは別の帯（REQ-10 の成功基準そのもの）。ビルド済み
   バイナリ・`oha` が揃っていなければ SKIP + 実行手順を案内する
4. **D（TASK-10.5）: 依存インパクト記録・連携方式設計文書の存在検証** —
   `docs/dep-impact/records.md` の plugin-tracing エントリ・`docs/design/
   tracing-integration.md` の存在を grep で機械検証する
5. **E（TASK-10.5）: 依存クレート数増分の機械検証** — `cargo tree -p
   fandhe-backend-core --features tracing` の union 展開差分件数を算出し、
   `docs/dep-impact/records.md` 記録値（+24 クレート）と許容帯（±5）で突合する

前提: `cargo build --release -p fandhe-backend-core --example minimal
--no-default-features` と `... --example tracing_nfr --features tracing` を事前実行
（本スクリプトは自動ビルドしない）。

基準未達（FAIL）でも `docs/spec/06-roadmap.md` の分岐どおり「デフォルト無効・
明示的 opt-in feature」を維持する結論自体は成立する（現状 `default = []`）。本
スクリプトは実測を PASS/WARN/FAIL として機械記録するのみで、分岐判断そのものは
実行結果レポート側の役割とする。実行結果レポートは
`benches/reports/task-10.4-tracing-performance.md` に記録する。

TASK-10.5 分（D/E チェック）の受け入れ記録は `docs/acceptance/req10-tracing.md` に
記録する（一次記録: `docs/reports/task-10-5-acceptance.md`）。

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
   fandhe-backend-plugin-hub-wiring --test hub_acceptance`（PoC-6 相当の実データ入りマルチ
   テナントハンドラで、越境クエリ全件遮断・JWT 欠落/不正時のフェイルクローズ・
   鍵ローテーション・検証結果キャッシュ共有を固定する 16 テスト）が全件 PASS
   すること
2. **B: 配線コード削減率** — `examples/hub_service_demo.rs` のマーカー区間
   （`// --- wiring:begin --- 〜 // --- wiring:end ---`）の実 LOC を PoC-6 基準
   （3 エンドポイント・207 行）に対して評価し、削減率 90% 以上で PASS
   （`scripts/accept/lib/hub-wiring-loc.sh`）。ハンドラ領域（`build_router`）に
   手書き JWT 検証・JWKS パース等の配線シンボルが現れないことも同ライブラリで
   grep 検証する
3. **C: 依存方向・pay-for-what-you-use** — `cargo tree -p fandhe-backend-core`
   に `fandhe-backend-plugin-hub-wiring` が一切現れないこと（依存逆転型プラグインの維持）
4. **D: NFR-6** — `fandhe-backend-plugin-hub-wiring` をリンクした hub サービス（`FANDHE_BACKEND_HUB_GATE=off`
   で `TenantGate` 未登録）が無関係パス（`GET /`）へ与える影響を
   `benches/hub-nfr6-bench.sh` の実測で確認する。`webrtc-accept.sh` /
   `graphql-accept.sh` と同一の NFR-6 判定帯（狭義 100.3〜100.8%・実務 [95%,105%]）を
   使う。ビルド済みバイナリ・`oha` が揃っていなければ SKIP + 実行手順を案内する

前提: `cargo build --release -p fandhe-backend-core --example minimal
--no-default-features` と `cargo build --release -p fandhe-backend-plugin-hub-wiring --example
hub_service_demo` を事前実行（本スクリプトは自動ビルドしない）。

判定ロジックのオフライン・セルフテスト（cargo・ネットワーク非依存）は
`scripts/tests/run-hub-wiring-accept-tests.sh` を参照。実行結果レポートは
`docs/acceptance/req9-hub-wiring.md` に記録する。

## `ai-autonomy-accept.sh` — REQ-12（AI 自律改修支援機構）・NFR-8 受け入れ検証（TASK-12.7、#48）

`docs/spec/05-tasks.md` TASK-12.7「AI 自律改修支援機構受け入れテスト」の受け入れ基準を
基準 A〜F で検証する。TASK-12.4〜12.6 の第三者再検証結果に基づき確定した値を
`docs/reports/task-12-7-metrics.summary`（機械可読台帳）から読み取り、`lib/common.sh`
（PASS/FAIL/SKIP/WARN 集計）を使う `req13-change-impact-accept.sh` 同型のオーケストレータ。

```bash
./scripts/accept/ai-autonomy-accept.sh
```

検証内容（基準 A〜F）:

- A. 自律完遂率 ≥60% かつリグレッション 0 件（確定値台帳突合）
- B. 可否判定正解率 ≥80% かつ誤判定破壊 0 件（確定値台帳突合。判定記録
  `docs/reports/task-12-4-2-records/` が残っていれば `third-party-feasibility-verify.sh`
  で再採点し台帳値との一致も確認する）
- C. エスカレーション時の判断根拠提示 ≥80%（確定値台帳突合）
- D-1（機械）. `scripts/audit-triage.sh` が改善提案の必須 5 項目（背景・根拠データ／
  影響範囲（crate 列）／対応方針（推奨アクション）／検証方法／リスク、
  `docs/design/improvement-proposal-flow.md` 4 節）の全欄を fixture 実行で生成することの
  機械検証（検証方法・リスク欄はコミット `becf0e0` で追加）
- D-2（人手）. 受け入れレポート内の人手評価台帳（`docs/reports/task-12-7-acceptance.md`）
  の集計。未記入・PENDING 行が残る場合は PASS と偽らず SKIP
- E. NFR-8（自動修正でテストが通る修正を得られる割合）≥70%（確定値台帳突合）
- E-2. NFR-8（AI 生成テストによる注入リグレッション検知率）≥90%（確定値台帳突合、
  #238。実測は `scripts/regression-injection-verify.sh` が担い、
  `docs/reports/nfr8-injection-detection-verification.md` に記録した値を台帳へ転記する）
- F. TASK-12.5 試行 2・3・TASK-12.6 グレーゾーン実測の状態。試行サマリ
  （`docs/reports/trial-*.summary`）・判定記録（`docs/reports/task-12-6-records/`）が
  存在すれば `third-party-stability-aggregate.sh`・`third-party-feasibility-verify.sh`
  をそれぞれ呼び出して検証し、なければ SKIP + 実施手順を案内する

確定値台帳（`docs/reports/task-12-7-metrics.summary`）は被験由来の実測値を転記した
信頼できない入力として扱い、metric 名 allowlist + 非負整数のみを受理する fail-closed
パースを行う（`third-party-stability-aggregate.sh` のパーサ設計を踏襲。`eval`・
コマンド置換への値展開は行わない）。

`--ledger <file>` / `--audit-fixtures-dir <dir>` / `--acceptance-doc <file>` /
`--reports-dir <dir>` でそれぞれ台帳・D-1 fixture・D-2 人手評価台帳・F の試行サマリ
探索先を差し替え可能（`scripts/tests/run-ai-autonomy-accept-tests.sh` のセルフテスト
注入口、`req13-change-impact-accept.sh` の `--crates-dir` 慣例を踏襲）。

判定ロジックのオフライン・セルフテスト（cargo・ネットワーク非依存）は
`scripts/tests/run-ai-autonomy-accept-tests.sh` を参照。実行結果レポートは
`docs/acceptance/req12-ai-autonomy.md`、確定版測定値・PENDING 事項の詳細は
`docs/reports/task-12-7-acceptance.md` に記録する。

## `hub-e2e-accept.sh` — REQ-9 後続 E2E 統合検証（Outbox Relay 完了待ち、#97）

イシュー #97「MS-6 後続 E2E 統合検証（Outbox Relay 完了待ち）」の受け入れ検証。
`docs/design/outbox-consent-integration.md` 11.2 節が定める 4 検証項目を、実
PostgreSQL・実 `micro-service-hub` サービスとの結線で確認する。`hub-wiring-accept.sh`
とは異なり `crates/plugin-hub-wiring` 単体の受け入れテストではなく、実データモデル・
実外部サービスとの E2E 結線検証を担う（TASK-9.5（#65）の後続、TASK-9.4（#64）の
「設計完了」と「E2E 統合検証完了」の 2 段階分割のうち後者）。

```bash
HUB_E2E_PG_URI="postgres://user:pass@host:5432/dbname" \
HUB_E2E_CONSENT_API="https://consent.example.internal" \
HUB_E2E_ORG_A="org-a-uuid" \
HUB_E2E_ORG_B="org-b-uuid" \
./scripts/accept/hub-e2e-accept.sh
```

### 前提（micro-service-hub 側）

- Outbox Relay（MS-5、目標 2026-09-30）・同意管理サービス（MS-3、目標 2026-08-31）が
  稼働していること。2026-07-18 時点ではいずれも未完了見込みであり、環境変数未設定で
  実行すると前提チェック段で `exit 2`（規約違反ではなく実行前提エラー、
  `scripts/feasibility-check.sh` の exit 規約と同型）を返す
- `docs/design/outbox-consent-integration.md` 11.1 節の未決事項（`outbox` テーブルの
  実カラム定義・`consent_grants` 実スキーマ/アクセス方式・`consent_revoked` ペイロード
  形式）が確定していること。未確定の間は基準 B（同意フィルタ）を SKIP、基準 C
  （Outbox Relay 配送）は enqueue 時点のカラム名不一致で FAIL しうる

### 検証する受け入れ基準（11.2 節）

| 記号 | 検証項目 | 検証手段 |
|------|---------|---------|
| A | 越境アクセス時の 0 行（RLS フェイルクローズ） | テナント A コンテキスト・コンテキスト未設定の双方でテナント B 行への `SELECT count(*)` が 0 件であることをパラメータ化クエリで確認 |
| B | 同意フィルタの実データ整合 | 同意管理サービスへの到達性確認 + オプトイン/未設定/取り消し済み 3 状態の判定結果照合（API 契約確定待ちのため現状は到達性確認までを実装、SKIP） |
| C | Outbox Relay 配送 | 検証用イベントを `enqueue` し、Relay によるポーリング配送（配送状態列の遷移）をタイムアウト付きで確認 |
| D | RLS ポリシー・`SET LOCAL` 適用漏れ検知 | `pg_class.relforcerowsecurity` カタログ照会で `outbox` テーブルの `FORCE ROW LEVEL SECURITY` 適用を確認 |

### 環境変数（すべて必須。既定値・実接続文字列はコミットしない）

| 変数 | 用途 |
|------|------|
| `HUB_E2E_PG_URI` | 検証用 PostgreSQL への接続文字列 |
| `HUB_E2E_CONSENT_API` | 同意管理サービスの検証用ベース URL |
| `HUB_E2E_ORG_A` / `HUB_E2E_ORG_B` | 検証 A（越境アクセス）用のテナント ID 2 件 |
| `HUB_E2E_RELAY_TIMEOUT_SEC`（任意、既定 30） | 検証 C の配送待ちタイムアウト秒数 |

### 前提ツール

| ツール | 用途 | 導入コマンド |
|--------|------|-------------|
| `psql` | 検証 A/C/D（PostgreSQL クライアント） | `apt install postgresql-client` 等 |
| `curl` | 検証 B（同意管理サービス API 呼び出し） | 各 OS のパッケージマネージャ |

### 出力の読み方

`core-deps-unsafe-audit.sh` と同一（PASS/FAIL/SKIP/WARN、終了コードは FAIL 1 件以上で
非 0）。ただし本スクリプトは**前提環境変数未設定時に `exit 2`** を返す点が他の
`scripts/accept/*.sh` と異なる（前提チェック段が判定サマリー出力前に非 0 終了する）。

### 実行前提・着手条件（重要）

本スクリプトは着手条件（micro-service-hub 側 Outbox Relay・同意管理サービスの完了確認、
11.1 節未決事項の確定情報入手、検証用接続情報の安全な受け渡し）が成立し、かつ
ユーザー承認を得てから実行する（`.claude/rules/feasibility-guardrail.md` 6 節、条件付き
可の着手ゲート）。実行結果レポートは `docs/acceptance/req9-hub-e2e.md` に記録する。
