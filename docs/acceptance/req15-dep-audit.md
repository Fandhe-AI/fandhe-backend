# REQ-15 受け入れ検証レポート — 依存監査基盤（TASK-15.4、#52）

`docs/spec/04-requirements.md` REQ-15（依存監査基盤）の受け入れ基準を
`scripts/accept/dep-audit-accept.sh` で検証した結果。依存監査基盤系列
（TASK-15.1〜15.3）の最終受け入れテストとして、既存成果物（`deny.toml`・
`scripts/dep-audit.sh`・`scripts/fuzz.sh`・fuzz 本実行結果）を機械検証する。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-17 |
| 対象コミット（origin/main 先端。本ブランチは未 push） | `ab8d66a`（`test(plugin-openapi): TASK-3.3 OpenAPI 自動生成受け入れテスト (#141)`） |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| cargo-deny | 0.19.8 |
| cargo-audit | 0.22.2 |
| jq | 導入済み（`/usr/bin/jq`） |
| cargo-fuzz | 0.13.2 相当（導入済み。基準 3d 実測に使用） |

## 判定サマリー

`bash scripts/accept/dep-audit-accept.sh` の実行結果（終了コード 0、FAIL なし）。

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | 1a: deny.toml 存在確認 | リポジトリルートに `deny.toml` が存在する |
| PASS | 1b: 許可ライセンスリスト | 仕様記載 5 ライセンス（MIT / Apache-2.0 / Apache-2.0 WITH LLVM-exception / Unicode-3.0 / BSD-3-Clause）すべてが `[licenses] allow` に含まれる。TASK-8.1（#26）で `ISC` が追加済みだが許可リスト方式の拡張であり必須要件を損なわない |
| PASS | 1c: `[graph] all-features` | `all-features = true` が設定され、全 feature 構成が常に監査対象に含まれる |
| PASS | 1d: `[advisories] ignore` | `ignore = []`（無視リスト空維持）を確認 |
| PASS | 2: 全 feature 構成の依存監査 | `scripts/dep-audit.sh` を no-default-features / default / 各 feature（`graphql`・`webrtc`・`webrtc-proxy`・`websocket`）/ all-features の全構成で実行し、`cargo audit`（`audit-triage.sh` 経由）既知脆弱性 0 件・`cargo deny check`（`advisories ok, bans ok, licenses ok, sources ok`）違反 0 件を確認 |
| PASS | 3a: fuzz target 存在確認 | `crates/http/fuzz/fuzz_targets/` に期待する 2 target（`parse_request_head`・`head_semantics`）が存在する |
| PASS | 3b: CI fuzz-smoke ジョブ存在確認 | `.github/workflows/ci.yml` に `fuzz-smoke` ジョブが存在し `scripts/fuzz.sh` を呼び出す（TASK-15.3-1、#87 実装済み） |
| PASS | 3c: fuzz 本実行結果の記録確認 | `docs/design/fuzzing.md` に「#88（TASK-15.3-2）fuzz 本実行結果」節と crash/hang 未検出の記述が存在する |
| PASS | 3d: fuzz smoke 実測（任意） | `scripts/fuzz.sh --max-total-time 60` を実行し、両 target とも crash/hang 0 件で正常終了（各 60 秒、約 1,200 万〜1,900 万実行）。実行後 `crates/http/fuzz/corpus/` への新規追加分は非追跡分として `git clean -fd` で除去済み（`docs/design/fuzzing.md`「corpus・artifacts の取り扱い」節の既存方針） |

## 参照した既存成果物

- **TASK-15.1（deny.toml ベースライン）**: `deny.toml`（許可リスト方式・`[graph] all-features = true`・`[advisories] ignore = []`）
- **TASK-15.2（#17、全 feature 構成の依存監査）**: `scripts/dep-audit.sh`（feature 動的列挙・`audit-triage.sh` 経由の `cargo audit` トリアージ・`cargo deny check --metadata-path`）
- **TASK-15.3-1（#87）/TASK-15.3-2（#88）（fuzz スクリーニング基盤・本実行）**: `scripts/fuzz.sh`（pinned nightly + cargo-fuzz）・`.github/workflows/ci.yml` `fuzz-smoke` ジョブ・`docs/design/fuzzing.md`「#88（TASK-15.3-2）fuzz 本実行結果」節（240 秒/target、`parse_request_head` 約 4,600 万実行・`head_semantics` 約 4,700 万実行、crash/hang 0 件）

## 再実行手順

```bash
bash scripts/accept/dep-audit-accept.sh
```

前提ツール（`cargo-deny`・`cargo-audit`・`jq`）未導入の環境では基準 2 が SKIP として
記録され、終了コードには影響しない。`cargo-fuzz` 未導入の環境では基準 3d のみ SKIP
される（基準 3a〜3c の静的証跡確認は継続して実行される）。

セルフテスト（判定ロジックのみ、ネットワーク・cargo ビルド不要）:

```bash
bash scripts/tests/run-dep-audit-accept-tests.sh
```

## セキュリティ考慮（OWASP Top 10）

- **A06 脆弱な依存（サプライチェーン）**: `cargo-deny`・`cargo-audit`・`cargo-fuzz` は
  いずれもバージョン pin 付き導入コマンドを案内するのみで、本スクリプトが自動
  インストールすることはない（`check_tool` の既存方針）。`cargo audit` 既知脆弱性 0 件・
  `deny.toml` の `ignore = []` 維持を機械検証し、違反時はフェイルクローズ（非 0 終了）
- **A05 設定ミス**: SKIP は「ツール未導入・実測任意」の場合にのみ用い、実測結果の FAIL を
  隠蔽しない。`deny.toml` の許可リスト方式（デフォルト拒否）を弱める変更は行っていない
- **A09 ログと監視**: 出力（PASS/FAIL/SKIP/WARN 詳細）にはライセンス名・crate 名・
  target 名のみを含み、シークレット・PII は含まない
- **リソース枯渇**: 基準 3d の任意実測は `--max-total-time 60`（既存 `scripts/fuzz.sh`
  の smoke 既定値）に限定し、self-hosted runner・ローカル環境を長時間占有しない

## スコープ外（本イシューで変更しない）

- `deny.toml` の許可ライセンス追加・変更（TASK-15.1 の改訂は別イシュー）
- fuzz target の追加・長時間スクリーニングの再実行（#88 完了済み。chunked
  Transfer-Encoding 対応後の target 追加は `docs/design/fuzzing.md`
  「スコープ外（out-of-scope-tracking）」節に既記録）
- `scripts/dep-audit.sh` / `scripts/fuzz.sh` 本体のロジック変更（本イシューはそれらを
  再利用する受け入れ検証スクリプトの追加に限定）
