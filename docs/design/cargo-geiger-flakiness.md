# cargo-geiger の非決定的パニック対応（#212）

対応: Issue #212「cargo-geiger の上流バグ（version 固定・既知 issue）を調査し、pin
または代替手段の要否を docs/design/ に記録する」。関連: `docs/design/pay-for-what-you-use-check.md`
（検証スクリプト本体）、`.claude/rules/ci.md`（self-hosted runner 運用）。

## 1. 事象

CI の `pay-for-what-you-use` 検証ジョブで、cargo-geiger 実行時に非決定的に以下の
panic が発生する。

```
assertion failed: self.pending_ids.insert(id)
  at src/cargo/core/package.rs:736 in cargo 0.86.0
```

実績: 2026-07-19 だけで 4 回発生（PR #206/#207/#208、main push CI run
29668037975・29667450326）。それ以前にも PR #188/#192、PR #164/#24 で同一 panic を
確認。いずれもドキュメント変更のみ・依存や実装への変更なしの差分で発生し、再実行に
よって全件回復する。

## 2. 根本原因

上流調査（cargo-geiger GitHub issues）から以下が判明した。

### 2.1 cargo 側の assertion 箇所

panic は cargo crate 内部 `Downloads::start_inner` の
`assert!(self.pending_ids.insert(id))` で発生（ダウンロード中パッケージ集合の不変条件
assertion）。

### 2.2 cargo-geiger 側の非公開 API 直接呼び出し

cargo-geiger は `cargo-geiger/src/scan/rs_file.rs::resolve_rs_file_deps` で
**非公開ライセンス内部 API** の `cargo::ops::clean` → `PackageSet::get_many` を
直接呼び出している。cargo crate の docs.rs では「外部ツールでの利用は意図しない」と
明記されており、外部ツール側の誤用に起因するバグと位置づけられる。

### 2.3 レースコンディション

self-hosted runner で `CARGO_HOME` がジョブ間で共有されている構成下で、以下のタイミング
衝突により重複 `PackageId` 挿入が発生する。

1. cargo-geiger が `PackageSet::get_many` へ package ID を追加（保留中として記録）
2. **並行実行中の他ジョブが同一レジストリキャッシュを同時更新**
3. 別パッケージのダウンロードが同じ ID で `PackageSet` へ挿入を試行
4. 既存 ID の重複挿入 assertion で panic

これは self-hosted runner 環境固有の問題（GitHub ホストランナーでは
`CARGO_HOME` が分離されるため回避される）。

## 3. version pin および代替手段の検討

### 3.1 バージョン pin

CI は既に `cargo-geiger@0.13.0`（最新リリース、2025-08-31）を `.github/workflows/ci.yml`
の「Ensure cargo-geiger 0.13.0」ステップで `--locked` により pin 済み。

しかし 0.13.0 でも上記 panic は未解消。加えて上流で以下が報告されている:

- **cargo-geiger issue #559**（open、2025-11-XX 起票、2026-06 時点で更新継続）
  - 報告者も同一の assertion panic
- **cargo-geiger PR #558**（cargo 0.86.0 → 0.89.0 bump）
  - 未マージ・非公開 API 依存の根本構造は変わらず
  - 0.89.0 への bump 後にも**同一 panic の再現報告あり**（issues 内コメント）

**結論**: version bump では恒久回避できない。non-public API への依存が根本原因であり、
cargo-geiger がこの依存を外すまで panic は再発する可能性が続く。

### 3.2 代替ツール

- **cargo-count**: 長期未メンテで不適。unsafe 集計機能が段階的に劣化・廃止方向
- **cargo-vet / cargo-crev**: 監査レコード目的で unsafe 集計が本来機能ではなく、不適
- **cargo-geiger の代替**: unsafe 集計で同等レベルの積極メンテを続ける代替ツールは
  2026 年 7 月時点で存在しない

**結論**: 現実的な代替ツールなし。

### 3.3 有界リトライ（採用方式）

**唯一の現実的ワークアラウンド**: `scripts/pay-for-what-you-use-check.sh` 内で
cargo-geiger の実行を有界リトライ対象とする。

実装済み（TASK-2.2 対応時、`docs/design/pay-for-what-you-use-check.md` 3.3 節参照）:

- リトライ上限: **3 回**（1 度目失敗→2 度目リトライ→3 度目最終判定の往復）
- バックオフ: 線形バックオフあり（試行 N 後に N×基準秒 sleep。基準は環境変数
  `PFWU_GEIGER_RETRY_WAIT`、既定 5 秒。セルフテストでは 0 を指定して待機を省略する）
- 失敗判定: 全 3 回失敗時は `FAIL` で終了コード非 0（ci-complete に含まれ緑にならない）
- **unsafe 検出結果の FAIL は リトライ対象外**（panic ではなく検出結果であり、
  揉み消さない。fail-closed 原則）

## 4. 結論と現在の実装

### 4.1 判断

- version pin: 既に最新 0.13.0 で pin 済みだが、根本的解決にならない
- 代替ツール: 現状なし
- **リトライ**: `scripts/pay-for-what-you-use-check.sh` に実装完了

### 4.2 運用上の注意

1. **リトライ実装の根拠**: CI panic はジョブを赤化してユーザー判定を中断させる。
   一過性のレース障害を恒久的な CI 失敗に変換しないため、有界リトライは最小限の
   ノイズ低減措置
2. **safe な検出は揺るがない**: unsafe 検出が true 正とは別に、cargo-geiger 自体の
   panic を区別して扱い、FAIL はリトライ対象外にすることで、unsafe ダイレクトの見落とし
   を防ぐ
3. **self-hosted runner 環境固有**: 他の CI ランナーサービス移行時には本対応の必要性
   を再評価する（GitHub ホストランナーでは `CARGO_HOME` 分離により自然回避される）

## 5. 上流監視

### 5.1 定期確認項目

以下の上流リポジトリの進捗を依存更新時（`update-external` ワークフロー等）に確認する。

- **cargo-geiger issue #559**: https://github.com/geiger-rs/cargo-geiger/issues/559
- **cargo-geiger PR #558** (cargo 0.89.0 bump): https://github.com/geiger-rs/cargo-geiger/pull/558

### 5.2 リトライ不要化の条件

以下いずれかが成立した場合、本スクリプトのリトライ実装を不要化できる可能性がある
（再評価必須）:

1. cargo-geiger が非公開 API 依存を外し、独立した unsafe 集計実装へ移行
2. cargo がバグを完全修正し、同一 panic が再発しなくなることを上流確認（複数の
   stable リリースサイクルに渡る実績）
3. self-hosted runner から GitHub ホストランナーへ移行し、`CARGO_HOME` 分離により
   レースそのものが起きなくなる

再評価時は本ファイルを更新し、判定根拠を記録する。

### 5.3 version bump 候補

cargo-geiger 側での修正リリースまたは PR #558 のマージが実現した場合は、
以下の手順で version update を検討する。

1. ローカル環境で cargo-geiger version を bump
2. `scripts/pay-for-what-you-use-check.sh` を複数回実行（10+ 回）してリトライ需要を
   確認
3. リトライ不要（全実行で一度も panic が起きない）ことを確認してから bump を PR に
   反映する

## 6. 参考

- cargo-geiger #559: https://github.com/geiger-rs/cargo-geiger/issues/559
- cargo-geiger PR #558: https://github.com/geiger-rs/cargo-geiger/pull/558
- cargo-geiger latest release: https://github.com/geiger-rs/cargo-geiger/releases/tag/cargo-geiger-0.13.0
- cargo src (panic 箇所): https://github.com/rust-lang/cargo/blob/rust-1.86.0/src/cargo/core/package.rs#L736
- docs.rs cargo crate: https://docs.rs/cargo/latest/cargo/
- `docs/design/pay-for-what-you-use-check.md` 3.3 節（リトライ実装の詳細）
- `.claude/rules/ci.md`（self-hosted runner 運用）
