# scripts/accept — 受け入れ検証スクリプト

`core-deps-unsafe-audit.sh`（REQ-1）・`plugin-mechanism-accept.sh`（REQ-2、TASK-2.4）・
`req13-change-impact-accept.sh`（REQ-13、TASK-13.2）の 3 スクリプトを収録する。
以下はまず REQ-1 側の詳細、REQ-2・REQ-13 側は本ファイル末尾を参照。

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
| E | 3 種拡張点（`Middleware`/`UpgradeHandler`/`RequestGate`）が trait 定義され、コアループ本体が feature 有無で分岐しない | trait 存在の grep + ループ本体への `#[cfg(feature` 不在 grep |
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
  部分のみ SKIP（trait 定義の存在確認は独立して実行される）
- **TASK-1.5（#14、`crates/routes`）未作成**: 基準 F の routes 部分のみ SKIP
  （`crates/http` の検証は独立して実行される）
- **TASK-15.1（#16、`deny.toml` 整備）未完**: `deny.toml` 不在時は
  `cargo deny check advisories bans sources` を既定設定で実行し、licenses チェックは
  WARN として保留を記録する

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
