# openapi-typescript 連携パイプライン（TASK-6.1、#54、REQ-6）

## 背景・目的

REQ-6（TypeScript 型安全性提供、Should）に基づき、「`utoipa` 属性 → `openapi.json` →
`openapi-typescript` → TS 型 → 型安全クライアント」の一方向パイプラインを構築する
（`docs/spec/05-tasks.md` TASK-6.1、`docs/spec/04-requirements.md` REQ-6）。

前提タスク TASK-3.2（#31、CLOSED）で `fandhe-backend-plugin-openapi` の `gen-openapi` CLI
（`--check`/`--update`）・コミット済み `crates/plugin-openapi/openapi.json`（`/health`・
`/hello/{name}`・`/users/{id}`・`/echo`・`/search` の 5 エンドポイント）・CI
`openapi-two-stage` ジョブが整備済みであり、本タスクはこの投資をそのまま入力として
再利用する（追加の Rust 実装は不要）。

PoC-8（`docs/spec/03-poc/trpc-contract/`）で `openapi-typescript` による型生成・
`tsc --noEmit` 型検査・陰性対照の有効性が実証済み。本タスクは PoC の使い捨て
プロトタイプを本実装（コミット対象のパイプライン + 実績あるクライアントライブラリ）へ
昇格させる。

## パイプライン全体図

```
crates/plugin-openapi/src/docs.rs（utoipa::path 属性）
        │  cargo run -p fandhe-backend-plugin-openapi --features gen-cli --bin gen-openapi
        ▼
crates/plugin-openapi/openapi.json（コミット対象、TASK-3.2 で確立済み）
        │  npm run gen:types（ts/、openapi-typescript）
        ▼
ts/src/generated/schema.d.ts（コミット対象、手動編集禁止）
        │  createClient<paths>()（ts/src/client.ts、openapi-fetch）
        ▼
ts/src/usage.ts（型安全な呼び出しサイト、tsc --noEmit の被検体）
```

一方向性の原則: 矢印は常に Rust 側 → TS 側にのみ流れる。TS 側から Rust 側の型・
スキーマへ逆流させる経路は設けない（生成物 `schema.d.ts` を手動編集しない、
`ts/` から `crates/**` への書き込みを行うスクリプトを作らない）。Rust 側の
エンドポイント定義（`crates/plugin-openapi/src/docs.rs`）を変更すれば
`gen-openapi --update` → `openapi-ts.sh --update` の再実行のみで TS 側に反映され、
手動同期の余地がない。

## クライアントライブラリ選定結果

| 案 | 概要 | 判定 |
|---|---|---|
| **`openapi-fetch`（採用）** | 生成された `paths` 型を `createClient<paths>()` の型引数に渡すだけで、メソッド・パス・パラメータ・レスポンス型が静的に解決される薄いクライアント。ランタイム実装は `fetch` のラッパーのみで依存は最小（`openapi-typescript` の姉妹プロジェクト、`5.9k+ stars`・活発なメンテナンス実績）。 | 採用。TASK-6.1 本文・REQ-6 詳細が「`openapi-fetch` 等の実績あるクライアントライブラリの採用」を明示推奨している。 |
| 自作 `Proxy` ベースクライアント | PoC-8 `ts/src/client.ts` で実装した tRPC 風の `Proxy` ラッパー。依存追加を避けられるが、`{ [K in ProcedureName]: ... }` という型注釈に完全依存するため型推論の限界がある（PoC-8 発見事項）。 | 不採用。PoC-8 の結論として本番実装では実績あるクライアントライブラリの採用を推奨すると明記されている。 |
| `openapi-generator`（Java 製コード生成ツール） | OpenAPI 定義から丸ごとクライアントコードを生成する汎用ツール。多言語対応だが Java ランタイム前提・生成量が過多（ドキュメント・モデルクラス・API クラスを大量生成）で TypeScript プロジェクトの軽量性と噛み合わない。 | 不採用。`openapi-typescript` + 軽量クライアントの組み合わせに対し、導入コスト（Java 依存）・生成物の保守コストが見合わない。 |

**採用理由の要約**: `openapi-fetch` は `openapi-typescript` が生成する型をそのまま
型引数として受け取れるため、二重の型定義・変換層が不要で「一方向パイプライン」の
原則と整合する。依存は `openapi-fetch` 本体のみで pay-for-what-you-use（ビルド時
専用・Rust バイナリに一切影響しない）とも整合する。

## ディレクトリ構成

```
ts/
├── package.json          # devDeps 完全固定（openapi-typescript 7.13.0・typescript 5.9.3）、
│                          # dependencies に openapi-fetch 0.17.0、engines/volta で Node 24 系固定
├── package-lock.json      # コミット対象（npm ci の単一真実源）
├── tsconfig.json          # strict / noEmit（PoC-8 構成を踏襲）
└── src/
    ├── generated/schema.d.ts  # openapi-typescript 生成物（コミット対象・手動編集禁止）
    ├── client.ts               # openapi-fetch ベースの型安全クライアント薄ラッパー
    └── usage.ts                # 5 エンドポイント呼び出し例（tsc --noEmit の被検体）
```

## `--check` / `--update` 運用

`scripts/openapi-ts.sh` が `gen-openapi` CLI（TASK-3.2 確立済み）と同一パターンの
2 モードで運用する（詳細は同スクリプトのヘッダコメント参照）。

- **`--check`（既定、CI ジョブ `openapi-ts` が使用）**: stage 1 で `gen-openapi --check`
  により `openapi.json` 自体の鮮度を検証し、stage 2 で一時ディレクトリへ
  `schema.d.ts` を再生成してコミット済みのものと `diff` する。乖離時は差分を表示して
  非 0 終了する（fail-closed、OWASP A08 対策）。stage 3 で `tsc --noEmit` を実行する。
- **`--update`（開発者向け）**: stage 1 で `openapi.json` を、stage 2 で
  `schema.d.ts` を in-place 再生成してから stage 3 の型検査を行う。

## サプライチェーン対策（OWASP A06/A08）

- `ts/package.json` の devDependencies/dependencies は完全固定バージョン（`^` なし）
  とし、`ts/package-lock.json` をコミット対象とする。
- 依存の導入・再現は `npm ci --ignore-scripts` に限定し、`postinstall` 等の
  lifecycle script による任意コード実行を遮断する。
- 生成物（`schema.d.ts`）はコミット対象とし、PR レビューで差分監査可能にする。
  `--check` の fail-closed 鮮度検証で改ざん・ドリフトを CI 検知する（`gen-openapi`
  と同一パターン）。
- CI（`.github/workflows/ci.yml` の `openapi-ts` ジョブ）はスクリプト実行のみで
  新規サードパーティ Action を導入しない。

## TASK-6.2（#55）: 陰性対照の CI 常設化・受け入れテスト（実装済み）

TASK-6.1（本ドキュメント上部）はパイプライン構築とローカルスモーク（コメントアウト
した陰性対照例の手動確認）までを対象とし、CI 常設化・受け入れテストスクリプト化は
TASK-6.2（#55）で実装した。

### 陰性対照の 2 段構成

`tsc --noEmit` が**成功するだけ**では、生成型が `any` 混入等で実質的な制約を失って
いても見かけ上は通ってしまう可能性が残る（PoC-8、`docs/spec/03-poc/trpc-contract/
README.md` で有効性が実証された「陰性対照」の考え方）。TASK-6.2 では次の 2 段で
「意図的な型不一致が確実にエラー検出されること」を CI 常設で検証する
（`scripts/openapi-ts-negative.sh`）。

- **N1: TS 側陰性対照** — `ts/src/negative/type-mismatch.ts` に 4 類型
  （パスパラメータ型不一致・レスポンス型誤代入・存在しないエンドポイント呼び出し・
  リクエスト body 型不一致）の意図的に誤った呼び出しを集約し、専用の
  `ts/tsconfig.negative.json`（通常の `tsconfig.json` は `exclude` で本ディレクトリを
  除外）経由で `tsc --noEmit`（`npm run typecheck:negative`）にかける。
- **N2: スキーマ側陰性対照** — `crates/plugin-openapi/openapi.json` の一時コピーへ
  `/users/{id}` の `id` を `integer`→`string` に変える node ワンライナーで型不一致を
  注入し、一時ディレクトリへ `schema.d.ts` を再生成した上で、既存（無改変）の
  `ts/src/usage.ts` の型検査が失敗することを確認する（openapi.json 境界からの
  伝搬確認）。

### fail-closed 判定

「非 0 終了」だけでは tsconfig 不備・ファイル欠落等の環境破損による失敗を陰性対照
PASS と誤認しうる（`.claude/rules/security.md` A08）。`scripts/openapi-ts-negative.sh`
は次の 3 条件すべてを満たした場合のみ PASS とする。

1. 同一実行内で陽性対照（`npm run typecheck`）が成功する
2. N1 が非 0 終了し、4 類型すべての期待 TS エラーコード（TS2322/TS2554）が出力に
   含まれる
3. N2 が非 0 終了し、期待 TS エラーコード（TS2322）が出力に含まれる

### Rust 定義変更の伝搬（受け入れテスト）

N2 はスキーマ（`openapi.json`）境界からの伝搬確認に留まる。「Rust 側の utoipa 属性
変更が型再生成のみで TypeScript 側に反映される」ことのフル確認は、`crates/
plugin-openapi/src/docs.rs` を一時変更して `gen-openapi --update` → `npm run
gen:types` を実際に実行する `scripts/accept/openapi-ts-accept.sh` 基準 C が受け持つ
（`trap` による復元・未コミット変更時 SKIP を含む詳細は `scripts/accept/README.md`
参照）。Rust→openapi.json 方向の一致は既存の `gen-openapi --check`（stage 1、
fail-closed）が CI 常設で検証しているため、CI 常設対象は N1・N2 に限定し、C は
人間/ローカル実行の受け入れテストとして位置付ける。

### スコープ外（変更なし）

- **TS クライアントから Rust サーバーへの実 HTTP 疎通テスト**: PoC-4/PoC-8 の
  切り分けどおり型契約面のみを対象とする。ネットワーク層の疎通確認は
  `docs/spec/03-poc/openapi-generation/` で実施済み。
- **`openapi.json` への恒久的なエンドポイント追加・変更**: 受け入れテスト基準 C の
  変更は一時注入 + 復元のみで、既存 5 エンドポイントの恒久変更は行わない。

## 検証コマンド

```bash
# パイプライン全体（--check、CI 既定と同一、TASK-6.1）
bash scripts/openapi-ts.sh

# schema.d.ts・openapi.json を再生成（開発者向け）
bash scripts/openapi-ts.sh --update

# 判定ロジックのセルフテスト（ネットワーク・cargo・npm 不要、TASK-6.1）
bash scripts/tests/run-openapi-ts-tests.sh

# 陰性対照（N1: TS 側 + N2: openapi.json 境界、TASK-6.2）
bash scripts/openapi-ts-negative.sh

# 陰性対照の判定ロジックのセルフテスト（ネットワーク・cargo 不要、TASK-6.2）
bash scripts/tests/run-openapi-ts-negative-tests.sh

# REQ-6 受け入れテスト一式（基準 A/B/C、TASK-6.2）
bash scripts/accept/openapi-ts-accept.sh
```

## 参照

- タスク定義: `docs/spec/05-tasks.md` TASK-6.1・TASK-6.2
- 要件定義: `docs/spec/04-requirements.md` REQ-6
- PoC: `docs/spec/03-poc/trpc-contract/README.md`（PoC-8）
- 前提タスク: TASK-3.2（#31）、`scripts/openapi-two-stage.sh`
- 受け入れテスト: `scripts/accept/openapi-ts-accept.sh`・`docs/acceptance/req6-typescript-types.md`
- セキュリティ規約: `.claude/rules/security.md`
- pay-for-what-you-use: `.claude/rules/pay-for-what-you-use.md`
