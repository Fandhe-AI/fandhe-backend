# REQ-3 受け入れ検証レポート — OpenAPI 自動生成（TASK-3.3、#32）

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
