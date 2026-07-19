# REQ-3 受け入れ検証レポート — OpenAPI 自動生成（TASK-3.3、#32）

> 注記: 本レポートの初回検証（2026-07-17）記録は 2026-07 の crate・import 一括改名
> （#202）以前の実測であり、旧クレート名（`backend-framework-core` / `bf-http` /
> `bf-routes` / `bf-plugin-*` 等）表記のまま保持している。実測値本文は改変しない
> （`docs/design/framework-naming.md` 7 節）。
>
> **最終判定は「再判定（2026-07-19、#259）」節を参照。** 初回検証時の FAIL（基準 1）・
> SKIP（基準 2b・3・4）は、前提だった `openapi` feature 配線（#256）・クエリ文字列
> 分離（#258）・5 エンドポイント実サービング（#257）の完了を受けて再実測し、
> 全基準 PASS で確定した。以下の初回検証記録（判定サマリー・手動突合表・
> フォローアップ節を含む）は経緯の記録としてそのまま保持する。

`docs/spec/04-requirements.md` REQ-3（OpenAPI 自動生成）の受け入れ基準を
`scripts/accept/openapi-accept.sh` および手動突合で検証した結果。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-17 |
| 対象コミット（origin/main 先端。本ブランチは未 push） | `08445f0`（`test(global): TASK-2.4 プラグイン機構受け入れテストを整備 (#139)`） |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| openapi-spec-validator | 未導入（後述） |

## 前提ギャップ（実装着手前に確認した既知事項）

REQ-3 の受け入れ基準 3・4（`openapi` feature 無効時の依存完全除外、生成有無での
`GET /health` 性能有意差なし）は、`crates/core` にサーバ側 `openapi` feature
（`openapi = ["dep:bf-plugin-openapi"]` 相当）が存在することを前提とする。しかし
本イシュー着手時点（2026-07-17）で当該配線は存在しない。

- `crates/plugin-openapi/src/lib.rs`・`src/embed.rs` の doc comment は「サーバ側
  feature による配線は TASK-2.1（#18、並列進行中）に接続点を委ねる」と明記しているが、
  TASK-2.1（#18）は `webrtc-proxy` feature のみを確立してクローズしており、`openapi`
  feature の配線は行われていない。後継 Issue も `gh issue list --search openapi
  --state open` で確認した限り存在しない。
- `crates/core/Cargo.toml` の feature 一覧は `webrtc-proxy` / `websocket` / `graphql`
  の 3 件のみで、`openapi` は含まれない（`cargo metadata` で確認）。

**本イシューはテストタスク（`test(plugin-openapi)`）であり、`.claude/rules/
out-of-scope-tracking.md` は pay-for-what-you-use 違反の是正・別タスクの機能を現在の
変更に混ぜないことを明示している。TASK-2.1（#18）のスコープであった配線をテスト
タスク側で先取り実装すると、スコープ外の機能追加をテスト PR に混入させることになる
ため見送った。** 配線が存在しない前提のもとで機械検証可能な範囲を最大限実施し、
配線依存の基準は BLOCKED として本レポートに正直に記録する（`benches/reports/
task-1.6-1-performance.md` の前例に倣う、判定不能を PASS と偽らないフェイルクローズ原則
`.claude/rules/security.md`）。

## 判定サマリー

`bash scripts/accept/openapi-accept.sh` の実行結果（終了コード 1。理由は基準 1 参照）。

| 判定 | 基準 | 詳細 |
|------|------|------|
| FAIL | 1: openapi.json 構文妥当性 | `openapi-spec-validator` が本実行環境に未導入のためフェイルクローズ FAIL。CI（`.github/workflows/ci.yml` `openapi-two-stage` ジョブ）にはバージョン pin 付きインストールステップを追加済みで、CI では実行される |
| PASS | 2a: ApiDoc/openapi.json 内部整合（機械検証） | `cargo test -p bf-plugin-openapi`（`tests/openapi_consistency.rs` 含む）が全 19 テスト PASS |
| SKIP | 2b: 実装（`crates/routes`）との突合 | 下記「手動突合表」参照。`GET /health` 以外の 4 エンドポイントは実サービング未実装のため機械検証不能 |
| SKIP | 3: `openapi` feature 存在・依存除外検証 | 前提ギャップにより配線が存在せず判定不能。配線後は `scripts/pay-for-what-you-use-check.sh` の動的列挙（`dep:bf-plugin-*` パターン）が自動的に検証対象へ含める（同スクリプトの変更は不要と確認済み） |
| SKIP | 4: `GET /health` 性能有意差（±5% 以内） | 節 3 と同じ理由で A/B 計測不能。詳細は `benches/reports/task-3.3-openapi-performance.md` |
| PASS | 5: CI 2 段階ビルド順序 | `openapi-two-stage` ジョブが `ci.yml` に存在し `scripts/openapi-two-stage.sh`（`gen-openapi --check` → `cargo build --all-features`）を呼び出す（TASK-3.2、#31 実装済み）。本イシューでバリデータステップ（基準 1 の CI 側実行）を追加した |

基準 1 の FAIL はローカル実行環境（本 worktree）に `openapi-spec-validator` が未導入
であることによる。`pip install --user` は本環境が `externally-managed-environment`
（PEP 668）のため素の `pip install --user` では拒否され、システム全体への影響を
考慮し自動導入しなかった（`.claude/rules/security.md` のサプライチェーン方針、
`scripts/accept/lib/common.sh` の `check_tool` は「導入コマンドを案内するのみで
自動インストールしない」既定方針を踏襲）。CI（self-hosted ランナー）側は
`.github/workflows/ci.yml` の `openapi-two-stage` ジョブにバージョン pin 付き
（`openapi-spec-validator==0.7.1`）インストールステップを追加済みで、CI 常設実行では
本基準を実際に検証する。

## 手動突合表（受け入れ基準 2「生成された定義とエンドポイント実装の齟齬が手動突合で
0 件である」への対応）

REQ-3 の受け入れ基準 2 は仕様上「手動突合」を要求している（`docs/spec/
04-requirements.md` 129〜131 行）。`bf-routes::Router` は method + target の完全一致
のみを扱い、パスパラメータ・クエリ文字列分離を持たない（`crates/routes/src/lib.rs`
doc comment）。このため本イシュー着手時点で対象 5 エンドポイントのうち実サービング
されているのは 0 件、`crates/core/examples/minimal.rs` に登録されているのは
`GET /` のみで、対象 5 エンドポイントはいずれも未登録である。

| # | path | method | ApiDoc 宣言（`crates/plugin-openapi/src/docs.rs`） | `crates/routes` 実装 | 突合結果 |
|---|------|--------|-------------------------------------------------|---------------------|---------|
| 1 | `/health` | GET | パラメータなし、200 応答（`String`） | 未登録（`GET /` のみ登録済み、`/health` は未登録） | **BLOCKED**（実装が存在せず突合不能） |
| 2 | `/hello/{name}` | GET | パスパラメータ `name`（string, 必須）、200 応答（`String`） | 未実装（`Router` はパスパラメータ非対応） | **BLOCKED** |
| 3 | `/users/{id}` | GET | パスパラメータ `id`（integer/int64, 必須）、200/400 応答 | 未実装（同上） | **BLOCKED** |
| 4 | `/echo` | POST | リクエスト/レスポンス body `EchoBody`、200/400 応答 | 未実装 | **BLOCKED** |
| 5 | `/search` | GET | クエリパラメータ `q`（必須）・`limit`（任意）、200/400 応答 | 未実装（`Router` はクエリ文字列分離非対応） | **BLOCKED** |

**結論**: 「齟齬 0 件」を意味のある形で主張できるのは、宣言と実装の両方が存在し
比較可能な場合のみである。現状は比較対象（実装）そのものが 5 件中 5 件とも存在しない
ため、「齟齬 0 件」の判定を PASS と記録することは判定の空洞化（vacuous truth）になり
`.claude/rules/security.md` のフェイルクローズ原則に反する。よって本レポートでは
「齟齬なし」ではなく「突合不能・BLOCKED」として正直に記録する。

## フォローアップ（要 Issue 化、本イシューでは起票せず PR 側に明記）

自動運転モードでは `.claude/rules/out-of-scope-tracking.md` の「ユーザーの承認なしに
勝手に Issue を起票しない」を優先し、以下は本イシューで起票せず PR 本文・本レポートへの
記録に留める。ユーザー承認後、後続 Issue として起票することを推奨する。

1. **`crates/core` への `openapi` feature 配線**（TASK-2.1、#18 で計画されたが未実施の
   接続点。`webrtc-proxy`/`websocket`/`graphql` と同型の `dep:bf-plugin-openapi` +
   `GET /openapi.json` 静的サービングハンドラ）。配線後に基準 3・4 の再検証が可能になる
2. **`bf-routes::Router` へのパスパラメータ・クエリ文字列対応の追加**、および対象
   5 エンドポイント（`/health`・`/hello/{name}`・`/users/{id}`・`/echo`・`/search`）の
   実サービング実装。実装後に手動突合表を PASS に更新できる（コア設計変更を伴うため
   本テストイシューのスコープ外と判断）
3. 上記 1・2 が完了した時点で `scripts/accept/openapi-accept.sh` を再実行し、
   本レポートの SKIP/BLOCKED 項目を PASS に更新すること

## セキュリティ考慮（OWASP Top 10）

- `GET /openapi.json`（将来配線予定）は `include_str!` 定数の返却のみを想定しており、
  リクエスト入力を実行時生成やインジェクションに反映しない設計が `embed.rs` の doc
  comment で既に規定されている（配線実装時にレビューで確認する）
- `openapi.json` の内容確認: 公開前提の API 記述（path/method/パラメータ/レスポンス
  スキーマ）のみを含み、内部パス・秘密情報・環境情報の混入は確認されなかった
  （`crates/plugin-openapi/openapi.json` を目視確認）
- CI に追加した `openapi-spec-validator` インストールステップはバージョン pin
  （`==0.7.1`）でサプライチェーンリスクを抑制した

---

## 再判定（2026-07-19、#259）

初回検証（2026-07-17、上記）で FAIL/SKIP/BLOCKED だった基準を、前提 3 イシューの完了
（#256: `crates/core` への `openapi` feature 配線、#258: `RequestHead` のクエリ文字列
分離、#257: 対象 5 エンドポイントの実サービング `crates/core/examples/
openapi_endpoints.rs`）を受けて再実行し、判定を確定した。

### 実行環境（再判定）

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-19 |
| 対象コミット（origin/main 先端） | `12dbdc3`（`feat(routes): REQ-3 対象 5 エンドポイントの実サービングを実装する (#257) (#272)`） |
| rustc / cargo | 1.96.0（stable, 2026-05-25） |
| openapi-spec-validator | 0.7.1（CI の pin `==0.7.1` と同版、`python3 -m` モジュール実行） |
| CPU コア数 | 12（`nproc`） |

### 判定サマリー（再判定・最終）

| 判定 | 基準 | 実測根拠 |
|------|------|---------|
| PASS | 1: openapi.json 構文妥当性 | `python3 -m openapi_spec_validator crates/plugin-openapi/openapi.json` → `OK`（exit 0、エラー 0 件）。バージョン 0.7.1 は CI（`.github/workflows/ci.yml` `openapi-two-stage` ジョブ）の pin と同版 |
| PASS | 2a: ApiDoc/openapi.json 内部整合（機械検証） | `cargo test -p fandhe-backend-plugin-openapi` 全 PASS（unit 8 + doc test 6 を含む全テスト、`tests/openapi_consistency.rs` 含む） |
| PASS | 2b: 実装との突合（手動突合表 + 機械検証） | 下記「手動突合表（再判定）」で 5 件すべて齟齬 0 件。機械検証は `cargo test -p fandhe-backend-core --example openapi_endpoints`（15 テスト PASS、Content-Type アサート含む） |
| PASS | 3: `openapi` feature 存在・依存除外検証 | `cargo metadata` の feature 一覧に `openapi` が存在。`cargo tree -p fandhe-backend-core -e normal --no-default-features` で `fandhe-backend-plugin-openapi` 0 件（default 構成でも 0 件）、`--features openapi` でのみ出現。`scripts/pay-for-what-you-use-check.sh` exit 0（依存・unsafe・バイナリシンボル・全構成ビルドの各検証 PASS、`openapi` feature は動的列挙で検証対象に含まれることをログで確認） |
| PASS | 4: `GET /health` 性能有意差（±5% 以内） | 専有計測枠（`benches/lib/exclusive.sh` の flock + 静穏確認、開始時 loadavg1=0.89）で RUNS=5・交互ペア・中央値方式の A/B 計測を実施。中央値差 **RPS +0.58%（baseline 143142.89 → openapi 143973.18）・p95 +1.59%（0.000908s → 0.000923s）** でいずれも ±5% 以内（baseline 中央値は下側中央値。`benches/lib/common.sh` `median()` 規約の中央 2 値平均で再計算しても RPS −1.96%・p95 +0.64% で判定不変）。詳細・全 run 実測値・baseline run 1 無効化の注記は `benches/reports/task-3.3-openapi-performance.md` の「再計測（#259）」節 |
| PASS | 5: CI 2 段階ビルド順序 | 初回検証から変更なし（`openapi-two-stage` ジョブ + `scripts/openapi-two-stage.sh` の存在を `scripts/accept/openapi-accept.sh` 節 5 が継続検証） |

`bash scripts/accept/openapi-accept.sh`（#259 で節 2b・3・4 を再判定に追随させて更新）は
全基準 PASS・exit 0。

### 手動突合表（再判定。受け入れ基準 2「生成された定義とエンドポイント実装の齟齬が
手動突合で 0 件である」への対応）

宣言側: `crates/plugin-openapi/src/docs.rs`（`ApiDoc`）および生成物
`crates/plugin-openapi/openapi.json`。実装側: `crates/core/examples/
openapi_endpoints.rs`（#257）。突合観点はメソッド・パラメータ（名前/位置/必須性/型）・
応答（ステータス/スキーマ）・Content-Type の 4 点。

| # | path | method | ApiDoc 宣言 | 実装（openapi_endpoints.rs） | 突合結果 |
|---|------|--------|------------|------------------------------|---------|
| 1 | `/health` | GET | パラメータなし、200（`String`、`text/plain`） | `route("GET", "/health")`、200 固定文字列 `OK`、`Content-Type: text/plain` 明示 | **一致** |
| 2 | `/hello/{name}` | GET | パスパラメータ `name`（string, 必須）、200（`String`、`text/plain`） | `route_param("GET", "/hello/{name}")`、`params.get("name")` を挨拶文へ埋め込み 200、`text/plain` 明示 | **一致** |
| 3 | `/users/{id}` | GET | パスパラメータ `id`（integer/int64, 必須）、200（`UserResponse`）/400（`ErrorBody`）、`application/json` | `route_param("GET", "/users/{id}")`、`id` の `u64` パース成功で 200 `UserResponse` 同一フィールド構成、失敗で 400 `ErrorBody`、`application/json` | **一致** |
| 4 | `/echo` | POST | request body `EchoBody`、200（`EchoBody`）/400（`ErrorBody`）、`application/json` | `route("POST", "/echo")`、body を `EchoBody` としてパースし 200 で再シリアライズ、不正 JSON は 400 `ErrorBody`、`application/json` | **一致** |
| 5 | `/search` | GET | クエリパラメータ `q`（string, 必須）・`limit`（u32, 任意）、200（`SearchResponse`）/400（`ErrorBody`）、`application/json` | `route("GET", "/search")`、`RequestHead::query()`（#258）から `q` 必須・`limit` 任意（既定 10、非負整数のみ）を解析、200 `SearchResponse`/400 `ErrorBody`、`application/json` | **一致** |

**結論**: 齟齬 0 件。宣言・実装の両方が存在する状態での実質的な突合であり、初回検証時の
「突合不能・BLOCKED」（vacuous truth の回避）は解消した。Content-Type は openapi.json の
各応答 `content` キー（`text/plain` × 2、`application/json` × 3 パス）と実装の
`with_content_type` 指定を突合し、`openapi_endpoints.rs` のテストが `text/plain` の
アサートを含むことも確認した。

### 基準 4 の A/B 計測条件（再判定）

- 計測対象: `crates/core/examples/openapi_endpoints.rs` を 2 構成でビルド
  - baseline: `cargo build --release --example openapi_endpoints -p fandhe-backend-core`
    （feature なし。`GET /openapi.json` は 404）
  - openapi: 同 `--features openapi`（#259 で example に追加した
    `#[cfg(feature = "openapi")] let server = server.openapi();` により
    `GET /openapi.json` を実サービング。事前に baseline=404 / openapi=200 を curl で確認）
- 計測方法: `oha -z 15s -c 128`（ウォームアップ 2s を各 run 前に実施・記録対象外）で
  `GET /health` を RUNS=5、host contention によるドリフト対策として baseline/openapi を
  run ごとに交互（ペア）で計測し、RPS・p95 それぞれの中央値の相対差で判定
- 専有性: `benches/lib/exclusive.sh` の `acquire_exclusive_lock`（flock）+
  `wait_for_quiescence`（loadavg1 ≤ 1.0・cargo/rustc/oha 不在）を計測開始前に確認
- 実測値・判定: `benches/reports/task-3.3-openapi-performance.md` の「再計測（#259）」節
  （中央値差 RPS +0.58%・p95 +1.59% で PASS。baseline run 1 は残存プロセス整理の巻き添え
  で無効となり baseline は有効 4 run で中央値を算出、最低 3 run の規約は充足。判定初回の
  試行は他セッションのビルドによる静穏未達（loadavg1=2.61、cargo/rustc 稼働検知）で
  BLOCKED となり、当該ビルド終了後の再試行で静穏を確認して確定した）

### フォローアップ（初回検証節）の消化状況

| 初回検証のフォローアップ | 消化先 |
|--------------------------|--------|
| 1. `crates/core` への `openapi` feature 配線 | #256（PR #266）で完了 |
| 2. Router のパスパラメータ・クエリ対応 + 5 エンドポイント実サービング | パスパラメータは #176、クエリ分離は #258（PR #265）、実サービングは #257（PR #272）で完了 |
| 3. 配線・実装後の `openapi-accept.sh` 再実行と SKIP/BLOCKED の更新 | 本再判定（#259）で完了 |
