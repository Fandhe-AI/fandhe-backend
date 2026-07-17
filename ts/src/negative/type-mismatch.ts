/**
 * TASK-6.2（#55、REQ-6）陰性対照（negative control）: 意図的に誤った型で
 * `src/client.ts` を呼び出す集合。`tsc --noEmit -p ../tsconfig.negative.json`
 * （`npm run typecheck:negative`）の被検体とし、生成型（`schema.d.ts`）が
 * 「見かけ上 `tsc --noEmit` が通るだけ」ではなく実際に制約として機能している
 * ことを CI で常設検証する（PoC-8、docs/spec/03-poc/trpc-contract/README.md
 * で有効性が実証された陰性対照パターンの CI 常設化）。
 *
 * `src/usage.ts`（陽性対照、5 エンドポイント呼び出しが型検査を通ること）とは
 * 逆に、本ファイルは「`tsc --noEmit` がエラーで失敗すること」自体が期待結果。
 * `tsconfig.json` の `exclude` で通常の `npm run typecheck` からは除外し、
 * 専用の `tsconfig.negative.json` からのみ include する
 * （`scripts/openapi-ts-negative.sh` 参照）。
 *
 * 各呼び出しには期待される TS エラーコードをコメントで注記する。エラーコードは
 * 実際に `npx tsc --noEmit -p tsconfig.negative.json` を実行して得られた実測値
 * （2026-07-16 時点、typescript@5.9.3 / openapi-typescript@7.13.0 固定版）。
 * `scripts/openapi-ts-negative.sh` はこのコメント規約に依存せず、出力に含まれる
 * エラーコードを直接 grep することで判定する（コメントはあくまで人間向けの文書）。
 */
import { createBackendFrameworkClient } from "../client.js";

const client = createBackendFrameworkClient("http://127.0.0.1:8080");

/**
 * 類型 1: パスパラメータ型不一致。`/users/{id}` の `id` は `schema.d.ts` 上
 * `number`（`crates/plugin-openapi/src/docs.rs` の utoipa 属性由来）だが、
 * 文字列を渡す。
 *
 * 期待エラー: TS2322（Type 'string' is not assignable to type 'number'）。
 */
async function pathParamTypeMismatch(): Promise<void> {
  await client.GET("/users/{id}", {
    params: { path: { id: "not-a-number" } },
  });
}

/**
 * 類型 2: レスポンス型の誤代入。`GET /health` のレスポンス body は
 * `string | undefined`（`health.data`）だが、`number` 型の変数へ代入する。
 *
 * 期待エラー: TS2322（'string | undefined' is not assignable to 'number'）。
 */
async function responseTypeMisassignment(): Promise<void> {
  const health = await client.GET("/health");
  const wrongType: number = health.data;
  void wrongType;
}

/**
 * 類型 3: 存在しないエンドポイント呼び出し。`schema.d.ts` の `paths` に
 * `/does-not-exist` は存在しないため、`GET` のオーバーロード解決に失敗する
 * （`openapi-fetch` の型定義上、未知パスは `never` 系の呼び出しシグネチャに
 * フォールバックし引数個数不一致となる）。
 *
 * 期待エラー: TS2554（Expected 2 arguments, but got 1）。
 */
async function nonexistentEndpointCall(): Promise<void> {
  // 抑制ディレクティブ（ts-expect-error 等）はあえて使わない。本ファイル自体が
  // エラー検出の被検体であり、抑制すると tsc がエラーを報告しなくなるため
  // （本コメントで当該ディレクティブの文字列を直接書かないのもそのため）。
  await client.GET("/does-not-exist");
}

/**
 * 類型 4: リクエスト body の型不一致。`POST /echo` の body は
 * `{ message: string }` だが、`message` に `number` を渡す。
 *
 * 期待エラー: TS2322（Type 'number' is not assignable to type 'string'）。
 */
async function requestBodyTypeMismatch(): Promise<void> {
  await client.POST("/echo", {
    body: { message: 123 },
  });
}

void pathParamTypeMismatch;
void responseTypeMisassignment;
void nonexistentEndpointCall;
void requestBodyTypeMismatch;
