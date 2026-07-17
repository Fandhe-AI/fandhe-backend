/**
 * TASK-6.1（#54、REQ-6）受け入れ基準「最低 1 つのエンドポイント呼び出しが型検査を
 * 通る」の検証コード。`crates/plugin-openapi/openapi.json` の 5 エンドポイント
 * （パスパラメータ・クエリパラメータ・リクエスト body・パラメータなしの全形態）を
 * すべて呼び出し、`tsc --noEmit`（`npm run typecheck`）の被検体とする。
 *
 * `src/client.ts` の型安全クライアントを介した呼び出しのみを対象とし、実際の
 * ネットワーク疎通は行わない（PoC-4 で確認済みのためスコープ外、
 * docs/design/openapi-typescript-pipeline.md 参照）。本ファイル自体は実行せず、
 * 型検査を通ることのみを確認する。
 */
import { createBackendFrameworkClient } from "./client.js";

const client = createBackendFrameworkClient("http://127.0.0.1:8080");

/**
 * 5 エンドポイントすべてを一巡し、生成型（`schema.d.ts`）が実際に入出力を
 * 制約していることを型検査で確認する呼び出しサイト。
 */
async function callAllEndpoints(): Promise<void> {
  // GET /health（パラメータなし）: レスポンスは text/plain の string。
  const health = await client.GET("/health");
  const healthBody: string | undefined = health.data;
  void healthBody;

  // GET /hello/{name}（パスパラメータ 1 件）。
  const hello = await client.GET("/hello/{name}", {
    params: { path: { name: "world" } },
  });
  const helloBody: string | undefined = hello.data;
  void helloBody;

  // GET /users/{id}（パスパラメータ + 400 応答定義。id は number）。
  const user = await client.GET("/users/{id}", {
    params: { path: { id: 42 } },
  });
  if (user.data) {
    const label: string = `${user.data.id}: ${user.data.name}`;
    void label;
  }

  // GET /search（クエリパラメータ 2 件。limit は省略可能）。
  const search = await client.GET("/search", {
    params: { query: { q: "rust", limit: 5 } },
  });
  const firstResult: string | undefined = search.data?.results[0];
  void firstResult;

  // POST /echo（リクエスト body・レスポンス body）。
  const echoed = await client.POST("/echo", {
    body: { message: "hi" },
  });
  const echoedMessage: string | undefined = echoed.data?.message;
  void echoedMessage;
}

void callAllEndpoints;

// --- 型検査で失敗すべき呼び出し例（コメントアウト、意図的な型不一致の確認用）。 ---
// 以下のコメントを外すと `tsc --noEmit` がエラーを報告することを確認できる
// （TASK-6.1 検証手順「陰性対照のローカルスモーク」、docs/design/
// openapi-typescript-pipeline.md 参照。CI 常設化は TASK-6.2 #55 のスコープ）。
// client.GET("/users/{id}", { params: { path: { id: "not-a-number" } } }); // 型エラー: id は number
// const wrongType: number = health.data; // 型エラー: health.data は string | undefined
