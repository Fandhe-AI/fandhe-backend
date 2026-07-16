# REQ-1 受け入れ検証レポート — 依存数・unsafe・監査（TASK-1.6-2、#72）

`docs/spec/04-requirements.md` REQ-1（最小コア）の受け入れ基準のうち、性能計測を除く
非性能系の基準を `scripts/accept/core-deps-unsafe-audit.sh` で検証した結果。
性能計測（RPS・レイテンシ・RSS・バイナリサイズ・起動時間）は姉妹イシュー
TASK-1.6-1（#71）のスコープであり、本レポートには含まない。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-16 |
| 対象コミット（origin/main） | `92a371d95804795069651ac01f9afa3e6b390b2d` |
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
| PASS | C: cargo audit 既知脆弱性 0件 | 1160 件の advisory DB に対しスキャンし、workspace 全体（55 crate dependencies）で検出 0 件 |
| WARN | C: cargo deny check（既定設定） | `deny.toml` 未整備のため既定設定で `advisories bans sources` のみ実行し違反 0 件。licenses チェックは #16（TASK-15.1）待ち |
| PASS | D: コア実質コード行数 <=5000 | 988 行（空行・`//` コメント行除外、対象: crates/core, crates/http） |
| PASS | E: 3拡張点 trait 定義 | Middleware / UpgradeHandler / RequestGate すべて crates/core/src に定義あり |
| SKIP | E: コアループの feature 非分岐 | コアループ実装（TASK-1.4-2 #70）が未マージのため検証対象なし |
| SKIP | F: routes のプラグイン非依存 | `crates/routes`（TASK-1.5 #14）が未作成のため検証対象なし |
| PASS | F: プラグイン非依存（http） | crates/http にプラグイン固有シンボル・依存を検出せず |

**終了コード: 0（FAIL なし）**

## スクリプト実行結果（生ログ）

```text
=== REQ-1 受け入れ検証（依存数・unsafe・監査） ===
workspace root: <repo root>

[PASS] A: 依存クレート数比 <=50%: core=5 種類 / axum-ref=50 種類（比率 10%、自クレート含む同一手法での cargo tree -e normal 集計）
[PASS] B: unsafe 0件/根拠明記: 対象コアクレート（crates/core crates/http）の src/ に unsafe 0 件
[WARN] B補足: workspace lint: ルート Cargo.toml で unsafe_code="warn" を設定済み。CI の clippy -D warnings と組み合わせ実質 deny として機能（.claude/rules/security.md）
[SKIP] B補足: cargo geiger: cargo-geiger 未導入のため参考値なし（導入: cargo install cargo-geiger）
[PASS] C: cargo audit 既知脆弱性 0件:       Loaded 1160 security advisories (from /home/fandhe/.cargo/advisory-db)     Updating crates.io index     Scanning Cargo.lock for vulnerabilities (55 crate dependencies)
[WARN] C: cargo deny check（既定設定）: deny.toml 未整備のため既定設定で advisories/bans/sources のみ実行し違反 0 件。licenses は #16（TASK-15.1）待ち
[PASS] D: コア実質コード行数 <=5000: 実質コード行数（空行・// コメント行除外、/* */ ブロックコメントは未除外のため参考値に上振れの可能性あり）: 988 行（対象: crates/core crates/http）
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
  axum-ref を含む 55 crate dependencies）をスキャンする。axum-ref は比較専用の
  参照実装でありコアの受け入れ基準の対象外だが、advisory が検出された場合は
  混入元を明記した上で報告する方針（今回は検出 0 件のため該当なし）。
  `deny.toml` は TASK-15.1（#16）で整備予定のため、本レポート時点では既定設定
  （`advisories bans sources`）のみ実行し、licenses チェックは保留とする。
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
| 基準 C: licenses チェック（`cargo deny check licenses`） | WARN（既定設定のみ実行） | #16（TASK-15.1） |
| audit/deny の CI 組み込み | 未着手（本イシューのスコープ外） | #17（TASK-15.2） |

これらは本イシュー（#72）のスコープ外であり、対応する既存イシューで別途扱う
（`.claude/rules/out-of-scope-tracking.md`）。前提タスクマージ後、本スクリプトの
再実行により保留を解消できる。

## スコープ外（本レポートに含まないもの）

- 性能計測（RPS・レイテンシ・RSS・バイナリサイズ・起動時間、axum-ref 比）→ #71（TASK-1.6-1）
- `deny.toml` ベースライン整備 → #16（TASK-15.1）
- audit / deny の CI 組み込み・全 feature 構成マトリクス → #17（TASK-15.2）・TASK-2.2
- コアループ・routes 実装そのもの → #70・#14
