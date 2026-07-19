# イシュー #136 検証レポート — crates/core → bf_plugin_webrtc_proxy 依存方向一方向性

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` / `bf-plugin-*` 等）
> 表記のまま保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。

イシュー #136（`fix(core): crates/core が bf_plugin_webrtc_proxy に直接依存し依存方向
一方向性に違反`）の非再現・是正済みであることを `scripts/dep-direction-check.sh` /
`scripts/tests/run-dep-direction-tests.sh` / `cargo tree` / CI 実行結果で検証した記録。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-18 |
| 対象コミット（origin/main 先端） | `e8aefb3`（`ci(global): update-external.yml の全ジョブに timeout-minutes を追加 (#196)`） |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |

## 背景

イシュー起票時点（`ffc6c76`、2026-07-17 18:33 JST）では、検証 1（エッジホワイト
リスト）に `backend-framework-core:bf-plugin-webrtc-proxy` エッジは既に許可済みだった
一方、検証 3（プラグイン非依存の grep 検査）に例外がなく `crates/core/src/plugin.rs:43`
の `bf_plugin_webrtc_proxy` シンボル参照が FAIL していた。

その後 `3ae6d11`（TASK-4.1、PR #137）で検証 3 に `webrtc_proxy_exception_file` /
`webrtc_proxy_exception_symbol_pattern` の個別例外が導入され、本イシューの FAIL は
解消済み。以降 `1877cfa`（#138 webrtc）・`6a6fb9c`（#144 graphql）・`85d066c`
（#151 tracing）で同型の個別例外が拡張され、いずれもレビュー + CI を経て main へ
マージ済み。

イシューが提示した 2 択（(1) 依存除去 / (2) ホワイトリスト・許容ルールの正式更新）の
うち、**選択肢 2 が既に採用・main へマージ済み**であることを本レポートで実測確認する。

## 判定サマリー

| 検証 | コマンド | 結果 |
|------|---------|------|
| 依存方向検証本体（検証 1〜3） | `bash scripts/dep-direction-check.sh` | PASS（3 検証すべて PASS、exit 0） |
| セルフテスト | `bash scripts/tests/run-dep-direction-tests.sh` | 19 passed, 0 failed |
| 依存除外（`webrtc-proxy` feature 無効） | `cargo tree -p backend-framework-core -e normal \| grep bf-plugin-webrtc-proxy` | 非該当（0 件、pay-for-what-you-use 維持） |
| 依存有効化（`webrtc-proxy` feature 有効） | `cargo tree -p backend-framework-core -e normal --features webrtc-proxy \| grep bf-plugin-webrtc-proxy` | 出現（1 件） |
| main 最新 CI | `gh run view 29655090382 --json jobs`（headSha `a019fdc3`） | `unsafe 追加の検知トリアージ`・`ci-complete` とも success |

**コード変更なし。検証のみで是正済みであることを確認したため、本イシューは実装対象外
（是正済みの記録のみ）として扱う。**

### 検証 1〜3 の実測出力

```
[PASS] 1: 依存エッジホワイトリスト照合 — 循環なし・全エッジが許可リストに合致
[PASS] 2: エントリポイント依存方向宣言 — crates 直下 12 クレート全てのエントリポイントに統一形式の宣言あり
[PASS] 3: プラグイン非依存（core/routes/http） — crates/core・crates/http・crates/routes にプラグイン固有シンボル・依存を検出せず

=== 依存方向一方向性検証: PASS ===
```

## 設計判断の記録先（イシューの選択肢 2 の要件充足）

- `scripts/dep-direction-check.sh` 内コメント: REQ-2（feature flag + `dep:` 構文の
  コンパイル時プラグイン機構）と、3 拡張点（`Middleware` / `UpgradeHandler` /
  `RequestGate`）が dyn 互換性のため同期 API 限定であり非同期パスインターセプト型
  プラグインを依存逆転で表現できない、という例外根拠を明記。許可エッジは
  `bf-plugin-*` ワイルドカードに一般化せず個別列挙（新規プラグインは明示追加 +
  レビュー必須）。
- `docs/design/plugin-boundary.md` 6.1 節「`scripts/dep-direction-check.sh` ホワイト
  リストの例外」: PR #129 の指摘を起点とした経緯・4 件の個別例外（webrtc-proxy /
  webrtc / websocket / tracing）の根拠を記録。

## pay-for-what-you-use 維持の根拠

`crates/core/Cargo.toml` の `optional = true` + `dep:` 構文により、`webrtc-proxy`
feature 無効時は `bf-plugin-webrtc-proxy` が依存グラフから完全除外されることを
`cargo tree` で実測確認済み（上表参照）。CI の pay-for-what-you-use 検証ジョブでも
機械検証されている。

## 結論

イシュー #136 が報告した FAIL は `3ae6d11`（PR #137）以降の main で既に解消済みで
あり、現行 main（`e8aefb3`）で再現しない。設計判断（選択肢 2: ホワイトリスト・許容
ルールの正式更新）は `scripts/dep-direction-check.sh` のコメントおよび
`docs/design/plugin-boundary.md` 6.1 節に記録済み。追加のコード変更は不要と判定する。
