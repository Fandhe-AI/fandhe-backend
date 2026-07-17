# REQ-13（変更影響範囲の機械判定構造）受け入れ検証結果（TASK-13.2 / #50）

`docs/spec/04-requirements.md` REQ-13 の受け入れ基準
(1) 新規プロトコル・機能の追加が既存 3 拡張点のいずれかに閉じるか、閉じない場合は
その理由が設計文書に明記される、(2) モジュール境界・依存方向が `lib.rs` 等の doc
コメントで機械可読に明示されている、を `scripts/accept/req13-change-impact-accept.sh`
（基準 A〜F）で検証した結果を記録する。

**結論（要約）**: 基準 A〜F すべて PASS（FAIL 0 件、SKIP 0 件）。REQ-13 の 2 つの
受け入れ基準はいずれも本タスクの成果物（`docs/design/dependency-graph-contract.md`・
`scripts/extension-closure-gate.sh`・プラグイン各クレートの機械可読宣言）により満たされた。

## 1. 検証コマンド・再現手順

```bash
bash scripts/accept/req13-change-impact-accept.sh
```

前提: `git cat-file -e <sha>^{commit}` で 3 節の実例コミット 3 件が解決できること
（shallow clone 環境では基準 D が SKIP になる。CI では `.github/workflows/ci.yml`
`unsafe-triage` ジョブの Checkout に `fetch-depth: 0` を設定済みのため実検証される）。

セルフテスト（判定ロジック単体の回帰確認）:

```bash
bash scripts/tests/run-extension-closure-tests.sh        # 拡張点閉包判定エンジン
bash scripts/tests/run-extension-closure-gate-tests.sh   # PR ゲート
bash scripts/tests/run-req13-accept-tests.sh              # 受け入れスクリプト自体
bash scripts/tests/run-dep-direction-tests.sh              # 依存方向一方向性（非破壊確認）
```

## 2. 実行結果（実行ログ）

```
[PASS] A: 依存方向一方向性の機械検証: scripts/dep-direction-check.sh が PASS（詳細: /tmp/req13-accept-dep-direction.log）
[PASS] B: プラグイン拡張点対応宣言: crates 直下 5 プラグインクレート全てに統一形式・許可語彙の宣言あり
[PASS] C: 契約ドキュメントの存在・必須セクション: docs/design/dependency-graph-contract.md が存在し必須見出し 4 件全て確認
[PASS] D: 実例 3 コミットの閉包判定再現: websocket(3ae6d11): 期待どおり; graphql(6a6fb9c): 期待どおり; webrtc(1877cfa): 期待どおり;
[PASS] E: 閉包違反の理由明記照合: WebRTC の E ファイル crates/http/src/response.rs と sha 1877cfa が docs/design/extension-closure-verification.md に記載済み
[PASS] F: run-extension-closure-tests.sh セルフテスト: PASS
[PASS] F: run-extension-closure-gate-tests.sh セルフテスト: PASS

=== 受け入れ検証サマリー（REQ-13、TASK-13.2 / #50） ===
判定 | 基準                                   | 詳細
-------+------------------------------------------+-----------------------------------------
PASS   | A: 依存方向一方向性の機械検証 | scripts/dep-direction-check.sh が PASS
PASS   | B: プラグイン拡張点対応宣言  | crates 直下 5 プラグインクレート全てに統一形式・許可語彙の宣言あり
PASS   | C: 契約ドキュメントの存在・必須セクション | docs/design/dependency-graph-contract.md が存在し必須見出し 4 件全て確認
PASS   | D: 実例 3 コミットの閉包判定再現 | websocket(3ae6d11): 期待どおり; graphql(6a6fb9c): 期待どおり; webrtc(1877cfa): 期待どおり;
PASS   | E: 閉包違反の理由明記照合     | WebRTC の E ファイル crates/http/src/response.rs と sha 1877cfa が docs/design/extension-closure-verification.md に記載済み
PASS   | F: run-extension-closure-tests.sh セルフテスト | PASS
PASS   | F: run-extension-closure-gate-tests.sh セルフテスト | PASS

結果: FAIL なし（PASS / SKIP / WARN のみ）。
```

セルフテスト実行結果（`scripts/tests/run-extension-closure-gate-tests.sh`: 22 passed,
0 failed／`scripts/tests/run-req13-accept-tests.sh`: 13 passed, 0 failed）。既存の
`scripts/tests/run-extension-closure-tests.sh`（19 passed）・
`scripts/tests/run-dep-direction-tests.sh`（19 passed）も非破壊であることを確認した
（プラグイン `lib.rs` への doc comment 追加が既存の依存方向宣言検査に影響しないことの
回帰確認）。

## 3. 基準別の詳細

### A. 依存方向一方向性の機械検証

`scripts/dep-direction-check.sh` を呼び出し、3 チェック（依存エッジホワイトリスト照合・
エントリポイント宣言の存在・core/http/routes のプラグイン非依存）すべて PASS を確認した。

### B. プラグイン全クレートの拡張点対応宣言

`crates/plugin-{websocket,graphql,webrtc,webrtc-proxy,openapi}/src/lib.rs` の 5 クレート
全てに `//! 拡張点対応: <値>` 統一形式の宣言があり、値は許可語彙内であることを確認した。

| クレート | 宣言値 |
|---|---|
| `bf-plugin-websocket` | `UpgradeHandler（try_handle_upgrade）` |
| `bf-plugin-graphql` | `パスインターセプト型（try_intercept）`（参照先: `extension-closure-verification.md` 3.4 節） |
| `bf-plugin-webrtc` | `パスインターセプト型（try_intercept）`（同上） |
| `bf-plugin-webrtc-proxy` | `パスインターセプト型（try_intercept）`（同上） |
| `bf-plugin-openapi` | `非該当（理由の参照: docs/design/dependency-graph-contract.md）` |

### C. 契約ドキュメントの存在・必須セクション

`docs/design/dependency-graph-contract.md` が存在し、必須見出し
（1. 正準依存グラフ・2. 契約一覧・3. 機械可読宣言の規約・4. 非該当時の理由明記運用）を
すべて確認した。

### D. 実例 3 コミットの閉包判定再現

`docs/design/extension-closure-verification.md` の実例検証結果（WebSocket=PASS・
GraphQL=PASS・WebRTC=FAIL、E 1 件）を機械的に再現し、期待どおりであることを確認した。

### E. 閉包違反の理由明記照合

WebRTC（`1877cfa`）の E ファイル `crates/http/src/response.rs` と対象コミット sha が
`docs/design/extension-closure-verification.md` に記載済みであることを確認した。

### F. ゲート・判定エンジンのセルフテスト

`scripts/tests/run-extension-closure-tests.sh`・`scripts/tests/run-extension-closure-gate-tests.sh`
がいずれも PASS することを確認した。

## 4. 補足: PR ゲートの実差分検証

本タスクの変更差分（プラグイン `lib.rs` 5 件 + `docs/**` + `scripts/**` + `.github/**`）に
対して `scripts/extension-closure-gate.sh --base origin/main` を実行し、A/D カテゴリのみで
構成され E（閉包違反候補）が 0 件であること、すなわち本タスク自体が拡張点への変更影響範囲
閉包を満たすことを確認した（コミット後に再現可能: `bash scripts/extension-closure-gate.sh
--base origin/main`）。

## 5. 既知の制約・スコープ外

- `scripts/extension-closure-gate.sh` の理由明記照合は `docs/design/*.md`（1 階層のみ）を
  対象とする。サブディレクトリを持つ設計文書の追加時は対象拡大を検討する
  （現状 `docs/design/` はフラット構成のため影響なし）
- `docs/design/dependency-graph-contract.md` 1 節の正準依存グラフは
  `scripts/dep-direction-check.sh` の `allowed_edge_patterns` からの手動転記であり、
  乖離検知の機械化は本タスクのスコープ外（`dependency-graph-contract.md` 7 節参照）
