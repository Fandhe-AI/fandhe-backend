# REQ-1 受け入れ検証レポート — 依存数・unsafe・監査（TASK-1.6-2、#72）

`docs/spec/04-requirements.md` REQ-1（最小コア）の受け入れ基準のうち、性能計測を除く
非性能系の基準を `scripts/accept/core-deps-unsafe-audit.sh` で検証した結果。
性能計測（RPS・レイテンシ・RSS・バイナリサイズ・起動時間）は姉妹イシュー
TASK-1.6-1（#71）のスコープであり、本レポートには含まない。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-17（origin/main 再追随後の再実行） |
| 対象コミット（origin/main 先端。実行時点で本ブランチは未 push） | `255377dcac50dec2cb6ac6076d001bb5b4310210` |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| cargo-audit | 0.22.2 |
| cargo-deny | 0.19.8 |
| cargo-geiger | 未導入（参考値なし） |
| tokei | 未導入（参考値なし） |

## 判定サマリー

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A: 依存クレート数比 <=50% | core=5 種類 / axum-ref=50 種類（比率 10%） |
| PASS | B: unsafe 0件/根拠明記 | 対象コアクレート（crates/core, crates/http）の src/ に unsafe 0 件 |
| WARN | B補足: workspace lint | ルート Cargo.toml で `unsafe_code = "warn"` を設定済み。CI の `clippy -D warnings` と組み合わせ実質 deny として機能 |
| SKIP | B補足: cargo geiger | cargo-geiger 未導入のため参考値なし（導入: `cargo install cargo-geiger`） |
| PASS | C: cargo audit 既知脆弱性 0件 | 1160 件の advisory DB に対しスキャンし、workspace 全体（56 crate dependencies）で検出 0 件 |
| PASS | C: cargo deny check | `deny.toml`（TASK-15.1 #16 で整備済み）による全項目（advisories/bans/sources/licenses）チェックで違反 0 件 |
| PASS | D: コア実質コード行数 <=5000 | 1136 行（空行・`//` コメント行除外、対象: crates/core, crates/http） |
| PASS | E: 3拡張点 trait 定義 | Middleware / UpgradeHandler / RequestGate すべて crates/core/src に定義あり |
| SKIP | E: コアループの feature 非分岐 | コアループ実装（TASK-1.4-2 #70）が未マージのため検証対象なし |
| SKIP | F: routes のプラグイン非依存 | `crates/routes`（TASK-1.5 #14）が未作成のため検証対象なし |
| PASS | F: プラグイン非依存（http） | crates/http にプラグイン固有シンボル・依存を検出せず |

**終了コード: 0（FAIL なし）**

## origin/main 再追随に伴う変更点

前回実行（対象コミット `92a371d`、2026-07-16）から origin/main が 25 コミット進み、
本ブランチをリベースして再実行した。差分として判定が変化した項目:

- **基準 C（cargo deny check）**: `deny.toml`（TASK-15.1、#16、コミット `bcbd34d`）が
  整備されたため、既定設定の `advisories bans sources` のみの WARN から、licenses を
  含む全項目チェックの PASS に変わった。
- **基準 D（LoC）**: コア実装が進んだため 988 行 → 1136 行に増加（引き続き閾値
  5000 行以内で PASS）。
- **基準 A（依存数）**: `plugin-webrtc-proxy` クレート等が追加されたが、
  `crates/core` 自体の推移的依存には影響せず、比率は 10% で不変。

基準 E（コアループ、#70）・基準 F（routes、#14）は origin/main 追随後も該当パスが
未存在のため SKIP のまま。前提タスクマージ後の再実行で解消する（下記「保留項目」）。

## スクリプトのバグ修正（本レポート作成中に検出）

再実行で基準 F（プラグイン非依存・http）が **FAIL** した。原因を調査した結果、
`check_plugin_independence` 内の除外フィルタ

```sh
grep -rn --include='*.rs' -E '[A-Za-z_]*[Pp]lugin' "${dir}/src" | grep -v -E '^\s*//'
```

が、`grep -rn` の出力形式 `file:line:content` に対して `^\s*//`（行頭空白 + `//`）を
適用していたため、`file:line:` プレフィックスにより常に不一致となり、コメント行
（`crates/http/src/connection.rs:420` の日本語コメント中の「plugin-webrtc-proxy」）を
一切除外できていなかった（除外フィルタが機能しない実装バグ）。

除外パターンを `file:line:` プレフィックスを踏まえた `^[^:]*:[0-9]+:[[:space:]]*//` に
修正し、再実行で FAIL が解消（コメント中の誤検出のみで、実コード上の plugin 依存は
存在しないことを確認済み）。本バグ修正は本イシュー（#72）が構築対象とする受け入れ
検証スクリプト自体の欠陥であり、本イシューのスコープ内として本コミットに含める。

## スクリプト実行結果（生ログ）

```text
=== REQ-1 受け入れ検証（依存数・unsafe・監査） ===
workspace root: <repo root>

[PASS] A: 依存クレート数比 <=50%: core=5 種類 / axum-ref=50 種類（比率 10%、自クレート含む同一手法での cargo tree -e normal 集計）
[PASS] B: unsafe 0件/根拠明記: 対象コアクレート（crates/core crates/http）の src/ に unsafe 0 件
[WARN] B補足: workspace lint: ルート Cargo.toml で unsafe_code="warn" を設定済み。CI の clippy -D warnings と組み合わせ実質 deny として機能（.claude/rules/security.md）
[SKIP] B補足: cargo geiger: cargo-geiger 未導入のため参考値なし（導入: cargo install cargo-geiger）
[PASS] C: cargo audit 既知脆弱性 0件:       Loaded 1160 security advisories (from /home/fandhe/.cargo/advisory-db)     Updating crates.io index     Scanning Cargo.lock for vulnerabilities (56 crate dependencies)
[PASS] C: cargo deny check: deny.toml による全項目チェックで違反 0 件
[PASS] D: コア実質コード行数 <=5000: 実質コード行数（空行・// コメント行除外、/* */ ブロックコメントは未除外のため参考値に上振れの可能性あり）: 1136 行（対象: crates/core crates/http）
[PASS] E: 3拡張点 trait 定義: Middleware / UpgradeHandler / RequestGate すべて crates/core/src に定義あり
[SKIP] E: コアループの feature 非分岐: コアループ実装（TASK-1.4-2 #70）が本 worktree 未マージのため検証対象なし。マージ後に再実行すること
[SKIP] F: routes のプラグイン非依存: crates/routes（TASK-1.5 #14）が本 worktree 未作成のため検証対象なし。作成後に再実行すること
[PASS] F: プラグイン非依存（http）: 対象（crates/http）にプラグイン固有シンボル・依存を検出せず

=== 受け入れ検証サマリー（REQ-1、TASK-1.6-2 / #72） ===
結果: FAIL なし（PASS / SKIP / WARN のみ）。
```

## 検証手法の注記

- **基準 A（依存クレート数比）**: `cargo tree -p <crate> -e normal --prefix none` を
  crate 名部分のみに正規化して重複排除した種類数を、core と axum-ref の両方に
  同一手法で適用して比較した。自クレート自身（`backend-framework-core` /
  `axum-ref`）を含めて数えているが、両側とも同じ扱いのため比率への影響はない。
- **基準 B（unsafe）**: `grep -rn -E '\bunsafe\b'` を対象クレートの `src/` に適用。
  検出時は直前行に `// SAFETY:` があるかを機械検査する。今回は検出 0 件のため
  該当なし。ワークスペース lint `unsafe_code = "warn"` と CI の
  `clippy -- -D warnings` の組み合わせにより、新規混入時も CI が検知する体制。
- **基準 C（audit/deny）**: `cargo audit` は workspace 全体（Cargo.lock 経由、
  axum-ref を含む 56 crate dependencies）をスキャンする。axum-ref は比較専用の
  参照実装でありコアの受け入れ基準の対象外だが、advisory が検出された場合は
  混入元を明記した上で報告する方針（今回は検出 0 件のため該当なし）。
  `deny.toml`（TASK-15.1 #16 で整備済み）による全項目チェック（advisories/bans/
  sources/licenses）を実行し、違反 0 件を確認した。
- **基準 D（LoC）**: 空行・`//` 行コメントを除いた実質行数。`/* */` ブロック
  コメントは未対応のため、実際の実質行数はこの値以下になり得る（上振れ方向の
  参考値）。tokei 導入時はより正確な参考値を併記できる。
- **基準 E / F**: 前提タスク（#70 コアループ、#14 routes クレート）が本検証時点
  で未マージのため、該当部分は SKIP。マージ後に `scripts/accept/core-deps-unsafe-audit.sh`
  を再実行すれば完全な受け入れ判定になる（スクリプトは再実行可能・べき等）。

## 保留項目（前提タスク待ち）

| 項目 | 状態 | 前提イシュー |
|------|------|-------------|
| 基準 E: コアループの feature 非分岐検証 | SKIP | #70（TASK-1.4-2） |
| 基準 F: routes のプラグイン非依存検証 | SKIP | #14（TASK-1.5） |
| audit/deny の CI 組み込み | CI 組み込み済み（TASK-15.2 #17・#108） | 完了（参考記録） |

これらは本イシュー（#72）のスコープ外であり、対応する既存イシューで別途扱う
（`.claude/rules/out-of-scope-tracking.md`）。前提タスクマージ後、本スクリプトの
再実行により保留を解消できる。

## スコープ外（本レポートに含まないもの）

- 性能計測（RPS・レイテンシ・RSS・バイナリサイズ・起動時間、axum-ref 比）→ #71（TASK-1.6-1）
- コアループ・routes 実装そのもの → #70・#14
