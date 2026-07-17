# REQ-6 受け入れ検証レポート — 陰性対照 CI 型検査整備・受け入れテスト（TASK-6.2、#55）

`docs/spec/04-requirements.md` REQ-6（openapi-typescript 連携）の受け入れ基準のうち
TASK-6.2 が担う「陰性対照の CI 常設化・受け入れテスト」を
`scripts/accept/openapi-ts-accept.sh` で検証した結果。TASK-6.1（#54、PR #150、
`origin/main` マージ済み）で確立したパイプライン・`openapi-ts` ジョブを前提とし、
production コードの追加変更は行っていない（`ts/src/negative/**`・
`scripts/openapi-ts-negative.sh`・`scripts/accept/openapi-ts-accept.sh`・
`scripts/tests/run-openapi-ts-negative-tests.sh` はいずれも test/tooling スコープの
新規追加）。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-17 |
| 対象コミット（作業ブランチ起点、`origin/main`） | `05a0370998daba72684cd9989844b62daf452a38` |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| node | v24.13.0 |
| npm | 11.6.2 |
| typescript | 5.9.3（`ts/package.json` 固定版） |
| openapi-typescript | 7.13.0（`ts/package.json` 固定版） |

## 判定サマリー（`scripts/accept/openapi-ts-accept.sh`）

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A: 陽性対照（tsc --noEmit 通過） | `scripts/openapi-ts.sh` が成功（stage 1〜3: gen-openapi --check・schema.d.ts 鮮度検証・5 エンドポイント呼び出しの tsc --noEmit） |
| PASS | B: 陰性対照（意図的な型不一致の検出） | `scripts/openapi-ts-negative.sh` が成功（N1: 4 類型すべて期待エラーコード検出・N2: openapi.json 境界伝搬、陽性対照も同時に成功） |
| PASS | C: Rust 定義変更の伝搬 | `docs.rs` の `/users/{id}` `id: u64→String` 一時変更が `gen-openapi --update`・`npm run gen:types` のみで `schema.d.ts` に伝搬し、`usage.ts` の型検査が `TS2322` で失敗することを確認。実行後 `trap` により復元済み（`git status` クリーンを確認） |

**終了コード: 0（FAIL なし）**

## 基準 B（陰性対照）の詳細

### N1: TS 側陰性対照（`ts/src/negative/type-mismatch.ts`）

`npm run typecheck:negative`（`tsc --noEmit -p tsconfig.negative.json`）は exit 2 で
終了し、4 類型すべてで期待した TS エラーコードを検出した（実測、2026-07-17）:

| 類型 | 該当箇所 | 期待エラーコード | 実測 |
|------|---------|-----------------|------|
| パスパラメータ型不一致（`/users/{id}` の `id` に文字列を渡す） | `type-mismatch.ts(34,23)` | TS2322 | `error TS2322: Type 'string' is not assignable to type 'number'.` |
| レスポンス型誤代入（`health.data` を `number` へ代入） | `type-mismatch.ts(46,9)` | TS2322 | `error TS2322: Type 'string \| undefined' is not assignable to type 'number'.` |
| 存在しないエンドポイント呼び出し（`/does-not-exist`） | `type-mismatch.ts(62,16)` | TS2554 | `error TS2554: Expected 2 arguments, but got 1.` |
| リクエスト body 型不一致（`POST /echo` の `message` に `number` を渡す） | `type-mismatch.ts(73,13)` | TS2322 | `error TS2322: Type 'number' is not assignable to type 'string'.` |

discrimination（誤った理由での失敗を PASS と誤認しないこと）は
`scripts/tests/run-openapi-ts-negative-tests.sh` で fixture ベースに検証済み
（module 解決エラー TS2307 の fixture・4 類型中 1 類型欠落の fixture はいずれも
FAIL 相当と判定されることを確認）。

### N2: スキーマ側陰性対照（openapi.json 境界からの伝搬）

`crates/plugin-openapi/openapi.json` の一時コピーへ `/users/{id}` の `id` を
`integer`→`string` へ変更し、一時ディレクトリで `schema.d.ts` を再生成した上で
既存（無改変）の `ts/src/usage.ts` を型検査した結果、期待どおり `TS2322` で
失敗した（`usage.ts` は `params: { path: { id: 42 } }` と `number` を渡しているため）。
一時ディレクトリ・注入したコピーはいずれもリポジトリの実ファイルに影響しない。

## 基準 C（Rust 定義変更の伝搬）の詳細

前提条件（対象パスクリーン）を満たしたため実行:

1. `crates/plugin-openapi/src/docs.rs` の `("id" = u64, Path, ...)` を
   `("id" = String, Path, ...)` へ一時変更
2. `cargo run -p bf-plugin-openapi --features gen-cli --bin gen-openapi -- --update`
   で `crates/plugin-openapi/openapi.json` を再生成
3. `npm run gen:types`（`ts/` 配下）で `ts/src/generated/schema.d.ts` を再生成 →
   `git diff` で差分が現れることを確認（PASS）
4. `npm run typecheck`（`ts/` 配下）で既存 `usage.ts`（無改変）を型検査 → 期待どおり
   非 0 終了、出力に `TS2322` を確認（`user.data` の `id` フィールドが
   `string` になったことに伴う伝搬）
5. `trap` により `docs.rs`・`openapi.json`・`schema.d.ts` を `git checkout --` で
   復元。実行後 `git status --porcelain crates/plugin-openapi/src/docs.rs
   crates/plugin-openapi/openapi.json ts/src/generated/schema.d.ts` が空であることを
   確認済み（作業ツリー整合性維持、`.claude/rules/security.md`）

この結果は「Rust 定義変更が型再生成のみ（コード生成コマンド 2 本）で TypeScript 側
の型検査に反映される」ことを、実際の `docs.rs` 変更を通じて実証する
（N2 はスキーマ境界からの縮小確認、C は Rust ソース起点のフル確認）。

## セルフテスト実行結果

```
$ bash scripts/tests/run-openapi-ts-tests.sh
===== 結果: PASS=12 FAIL=0 =====

$ bash scripts/tests/run-openapi-ts-negative-tests.sh
===== 結果: PASS=19 FAIL=0 =====
```

いずれも cargo・ネットワーク非依存の判定ロジック検証（引数検証・fail-closed 挙動・
fixture ベースの discrimination・CI ジョブ/ステップ存在確認）。

## 結論

REQ-6 の TASK-6.2 が担う 3 基準（陽性対照・陰性対照の CI 常設化・Rust 定義変更の
伝搬確認）はすべて PASS で受け入れ済み。

## 参照

- 検証スクリプト: `scripts/accept/openapi-ts-accept.sh`
- 陰性対照本体: `scripts/openapi-ts-negative.sh`
- 陰性対照ファイル: `ts/src/negative/type-mismatch.ts`
- セルフテスト: `scripts/tests/run-openapi-ts-negative-tests.sh`
- 設計ドキュメント: `docs/design/openapi-typescript-pipeline.md`
- タスク定義: `docs/spec/05-tasks.md` TASK-6.2
- 前提タスク（TASK-6.1）レポート相当の設計: `docs/design/openapi-typescript-pipeline.md`（上部節）
