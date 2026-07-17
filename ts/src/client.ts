/**
 * TASK-6.1（#54、REQ-6）: `openapi-fetch` ベースの型安全クライアント薄ラッパー。
 *
 * 型引数 `paths` は `src/generated/schema.d.ts`（`npm run gen:types` の生成物。
 * `crates/plugin-openapi/openapi.json` が唯一の一次スキーマ源、docs/design/
 * openapi-typescript-pipeline.md 参照）から供給される。本ファイルはその型を
 * `createClient<paths>()` に束縛するだけの薄いラッパーであり、独自の型演算・
 * ランタイム変換は持たない（PoC-8 の自作 `Proxy` クライアントで確認された型推論の
 * 限界を避けるため、実績のあるクライアントライブラリを採用した経緯は同ドキュメント
 * のクライアントライブラリ選定結果を参照）。
 *
 * 呼び出し元: `src/usage.ts`（`tsc --noEmit` の型検査被検体）。実サーバーへの
 * HTTP 疎通確認は本タスクのスコープ外（PoC-4 で確認済み、TASK-6.1 は契約面のみ）。
 */
import createClient from "openapi-fetch";
import type { paths } from "./generated/schema";

/**
 * backend-framework サーバーへの型安全クライアント。
 *
 * `baseUrl` は呼び出し側（統合先アプリケーション）が用途に応じて指定する前提のため、
 * ここでは固定値を持たずファクトリ関数として公開する（シークレット・環境依存値を
 * 本パッケージにハードコードしない、.claude/rules/security.md A05/シークレット管理）。
 */
export function createBackendFrameworkClient(baseUrl: string) {
  return createClient<paths>({ baseUrl });
}
