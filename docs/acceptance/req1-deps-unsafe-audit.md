# REQ-1 受け入れ検証レポート — 依存数・unsafe・監査（TASK-1.6-2、#72 / TASK-1.6-4、#169）

`docs/spec/04-requirements.md` REQ-1（最小コア）の受け入れ基準のうち、性能計測を除く
非性能系の基準を `scripts/accept/core-deps-unsafe-audit.sh` で検証した結果。
性能計測（RPS・レイテンシ・RSS・バイナリサイズ・起動時間）は姉妹イシュー
TASK-1.6-1（#71）のスコープであり、本レポートには含まない。

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-18（#169 是正後の現行 main 再実行） |
| 対象コミット（origin/main 先端） | `0cdc7280f73b342955da4e7fb2f1147923dc74c4` |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| cargo-audit | 0.22.2 |
| cargo-deny | 0.19.8 |
| cargo-geiger | 導入済みだが実行失敗（参考値のため判定に影響しない） |
| tokei | 未導入（参考値なし） |

## 判定サマリー

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A: 依存クレート数比 <=50% | core=9 種類 / axum-ref=50 種類（比率 18%） |
| PASS | B: unsafe 0件/根拠明記 | 対象コアクレート（crates/core, crates/http, crates/routes）の src/ に unsafe 0 件 |
| WARN | B補足: workspace lint | ルート Cargo.toml で `unsafe_code = "warn"` を設定済み。CI の `clippy -D warnings` と組み合わせ実質 deny として機能 |
| WARN | B補足: cargo geiger | 実行に失敗（参考値のため受け入れ判定に影響しない） |
| PASS | C: cargo audit 既知脆弱性 0件 | 1166 件の advisory DB に対しスキャンし、workspace 全体（340 crate dependencies）で検出 0 件 |
| PASS | C: cargo deny check | `deny.toml`（TASK-15.1 #16 で整備済み）による全項目（advisories/bans/sources/licenses）チェックで違反 0 件 |
| PASS | D: コア実質コード行数 <=5000 | 2478 行（空行・`//` コメント行除外、対象: crates/core, crates/http, crates/routes） |
| PASS | E: 3拡張点 trait 定義 | Middleware / UpgradeHandler / RequestGate すべて crates/core/src に定義あり |
| **PASS** | **E: コアループの feature 非分岐** | コアループ関数 3 件（`BoundServer::run`・`handle_connection`・`handle_connection_with_permit`）の非コメント行に `#[cfg(feature ...)]` なし。`Server` ビルダーの cfg-gated 設定・`plugin.rs` シームは `docs/design/plugin-boundary.md` §3-5 の許容領域のため対象外（#169 是正、下記詳細） |
| **PASS** | **F: プラグイン非依存（http/routes）** | 対象（crates/http, crates/routes）にプラグイン固有シンボル・依存を検出せず |

**終了コード: 0（FAIL なし）**

前回レポート（#72 時点）から保留だった基準 E・F の SKIP は、TASK-1.4-2（#70）・
TASK-1.5（#14）の後続マージにより両方とも PASS へ解消済み。今回の #169 対応は、
基準 E の判定ロジック自体の誤検出を修正するものであり、判定対象パスの新規出現
（前回の SKIP 解消）とは別の変更である（下記「基準 E チェックの誤検出修正（#169）」
節を参照）。

## 基準 E チェックの誤検出修正（#169）

### 発端（イシュー記載のバグ）

#70・#14 マージ直後の診断ベースで、基準 E の「コアループの feature 非分岐」チェック
（旧実装、`check_extension_points` 後半）は、コアループ実装ファイルを
`xargs grep -l '#\[cfg(feature'` で**ファイル単位**に検査していたため、
`#[cfg(feature = "...")]` を「使っていないこと」を説明する doc comment 内の引用
（`crates/core/src/server.rs:14`「`handle_connection` 内に `#[cfg(feature = "...")]`
を一切持たない」等）を実コードの feature 分岐と誤認し FAIL していた。基準 F の
プラグイン非依存チェックには #72 でコメント除外（`grep -rn` の `file:line:` 出力
形式を踏まえた `^[^:]*:[0-9]+:[[:space:]]*//` 除外）が実装済みだったが、基準 E には
未適用だった。

### 追加で判明した論点（検査粒度の陳腐化）

上記のコメント除外を単純に基準 E へ移植するだけでは、現行 main では依然として
FAIL する。#70/#14 マージ後、TASK-2.1（#129）・TASK-4.1（#137）・TASK-8.1（#138）の
マージで `crates/core/src/server.rs`（`Server` の cfg-gated 設定フィールド・
`Default` 実装・ビルダーメソッド・`WebSocketUpgradeAdapter` 等）と
`crates/core/src/plugin.rs`（cfg 集約シームモジュール）に、実コードの
`#[cfg(feature` が計 20 箇所存在するようになったため（ファイル単位検査では
これらもすべて「ループの feature 分岐」として誤検出する）。

これらは `docs/spec/04-requirements.md` の基準 E 本文（各拡張点は
`try_handle_*` ヘルパーとして `#[cfg(feature)]` で丸ごと出し分け、**ループ内には**
`#[cfg]` 分岐を持たせない）と `docs/design/plugin-boundary.md` §3-5
（「コアループは cfg-free を維持する」「feature 分岐が必要な場合もコアループ側は
ヘルパーのシグネチャを変えずに済む」）が明示的に許容する設計であり、基準 E が
本来検証すべき対象は「コアループ本体（`BoundServer::run` /
`handle_connection` / `handle_connection_with_permit`）」に限定すべきと判断した。

### 修正内容

1. SKIP 判定を「lib.rs/extension.rs 以外のファイル有無」から、コアループの所在を
   `docs/design/plugin-boundary.md` §3 どおり `crates/core/src/server.rs` に固定し、
   同ファイル不在時のみ SKIP とするよう変更。
2. 検査本体を awk（POSIX 構文のみ）でコアループ 3 関数
   （`run` / `handle_connection` / `handle_connection_with_permit`）の範囲を抽出し、
   範囲内の非コメント行にある `#[cfg(feature` のみを検出するよう変更。関数範囲は
   開始行と同一インデントの `}` のみの行までとする（CI が `cargo fmt --check` を
   強制するため rustfmt 整形済みを前提にできる）。
3. コメント除外は基準 F と同じ「行頭 `//`（`///`・`//!` 含む）」方式。
4. 抽出できた関数数が 0 件の場合は誤 PASS を避け、計測不能として明示的に FAIL する
   （基準 A の `core_deps==0` ガードと同じフェイルクローズ方針）。

### 検証

- 現行 main の対象 3 関数の範囲内に非コメントの `#[cfg(feature` は 0 件であることを
  確認済み（`awk` 抽出結果、下記生ログ参照）。
- 負検出テスト（スクラッチパッドで実施、リポジトリ非汚染）:
  `handle_connection_with_permit` 内に `#[cfg(feature = "test")]` を注入した検査対象
  コピーで FAIL・`file:line` 出力を確認。
  関数名を全てリネームした検査対象コピーで `FN_COUNT=0` → フェイルクローズ FAIL に
  なることを確認。
  doc comment 中の `#[cfg(feature` 引用（現行 main の line 14 等）は 3 関数の範囲外
  にあるため、コメント除外を経ずとも範囲限定だけで誤検出しないことを確認。

本修正は `scripts/accept/core-deps-unsafe-audit.sh` の `check_extension_points` と
`scripts/accept/README.md` の該当節にのみ影響し、Rust 実装コード・`Cargo.toml` の
変更は伴わない。

## スクリプト実行結果（生ログ）

```text
=== REQ-1 受け入れ検証（依存数・unsafe・監査） ===
workspace root: <repo root>

[PASS] A: 依存クレート数比 <=50%: core=9 種類 / axum-ref=50 種類（比率 18%、自クレート含む同一手法での cargo tree -e normal 集計）
[PASS] B: unsafe 0件/根拠明記: 対象コアクレート（crates/core crates/http crates/routes）の src/ に unsafe 0 件
[WARN] B補足: workspace lint: ルート Cargo.toml で unsafe_code="warn" を設定済み。CI の clippy -D warnings と組み合わせ実質 deny として機能（.claude/rules/security.md）
[WARN] B補足: cargo geiger: 実行に失敗（参考値のため受け入れ判定に影響しない）
[PASS] C: cargo audit 既知脆弱性 0件:       Loaded 1166 security advisories (from <local advisory-db cache>)     Updating crates.io index     Scanning Cargo.lock for vulnerabilities (340 crate dependencies)
[PASS] C: cargo deny check: deny.toml による全項目チェックで違反 0 件
情報: tokei が見つかりません。導入する場合は:
[PASS] D: コア実質コード行数 <=5000: 実質コード行数（空行・// コメント行除外、/* */ ブロックコメントは未除外のため参考値に上振れの可能性あり）: 2478 行（対象: crates/core crates/http crates/routes）
[PASS] E: 3拡張点 trait 定義: Middleware / UpgradeHandler / RequestGate すべて crates/core/src に定義あり
[PASS] E: コアループの feature 非分岐: コアループ関数 3 件（run/handle_connection/handle_connection_with_permit）の非コメント行に #[cfg(feature ...)] なし。Server ビルダーの cfg-gated 設定・plugin.rs シームは docs/design/plugin-boundary.md §3-5 の許容領域のため対象外
[PASS] F: プラグイン非依存（http/routes）: 対象（crates/http crates/routes）にプラグイン固有シンボル・依存を検出せず

=== 受け入れ検証サマリー（REQ-1、TASK-1.6-2 / #72） ===
結果: FAIL なし（PASS / SKIP / WARN のみ）。
```

## 検証手法の注記

- **基準 A（依存クレート数比）**: `cargo tree -p <crate> -e normal --prefix none` を
  crate 名部分のみに正規化して重複排除した種類数を、core と axum-ref の両方に
  同一手法で適用して比較した。自クレート自身（`backend-framework-core` /
  `axum-ref`）を含めて数えているが、両側とも同じ扱いのため比率への影響はない。
  前回レポート（core=5 種類、比率 10%）からの増分（core=9 種類、比率 18%）は
  websocket/graphql/openapi/webrtc/tracing 系プラグインの feature 経由 dev-dependency
  等が原因であり、依然として axum-ref の 50% 以下を満たす。
- **基準 B（unsafe）**: `grep -rn -E '\bunsafe\b'` を対象クレート（`crates/core`,
  `crates/http`, `crates/routes`）の `src/` に適用。検出時は直前行に
  `// SAFETY:` があるかを機械検査する。今回は検出 0 件のため該当なし。ワークスペース
  lint `unsafe_code = "warn"` と CI の `clippy -- -D warnings` の組み合わせにより、
  新規混入時も CI が検知する体制。
- **基準 C（audit/deny）**: `cargo audit` は workspace 全体（Cargo.lock 経由、
  axum-ref を含む 340 crate dependencies）をスキャンする。axum-ref は比較専用の
  参照実装でありコアの受け入れ基準の対象外だが、advisory が検出された場合は
  混入元を明記した上で報告する方針（今回は検出 0 件のため該当なし）。
  `deny.toml`（TASK-15.1 #16 で整備済み）による全項目チェック（advisories/bans/
  sources/licenses）を実行し、違反 0 件を確認した。
- **基準 D（LoC）**: 空行・`//` 行コメントを除いた実質行数。`/* */` ブロック
  コメントは未対応のため、実際の実質行数はこの値以下になり得る（上振れ方向の
  参考値）。tokei 導入時はより正確な参考値を併記できる。前回レポート
  （1136 行、crates/core・crates/http のみ）からの増分（2478 行）は
  `crates/routes` 追加分と各プラグイン配線コードの増加によるもので、依然として
  閾値 5000 行以内。
- **基準 E**: 上記「基準 E チェックの誤検出修正（#169）」節を参照。コアループ
  3 関数への範囲限定 + コメント除外 grep で判定する。
- **基準 F**: `crates/http` に加え `crates/routes`（TASK-1.5 #14 で追加済み）も
  検証対象に含む。識別子パターン（`[A-Za-z_]*[Pp]lugin`）grep + `Cargo.toml` の
  `plugin-` 依存確認、行頭 `//` コメント除外（#72 是正）。

## 保留項目

前回レポート（#72 時点）の保留項目（基準 E・F の SKIP）は、#70（TASK-1.4-2）・
#14（TASK-1.5）の後続マージによりいずれも解消し、本レポートでは両方とも PASS。

| 項目 | 状態 | 備考 |
|------|------|------|
| B補足: cargo geiger（参考値） | WARN（実行失敗） | 基準ではなく参考値のため受け入れ判定に影響しない。cargo-geiger 自体は導入済みだが実行時にエラーとなり出力を得られていない。詳細調査はスコープ外（下記「スコープ外」参照） |
| D参考値: tokei | SKIP（未導入） | 基準ではなく参考値のため受け入れ判定に影響しない |
| audit/deny の CI 組み込み | CI 組み込み済み（TASK-15.2 #17・#108） | 完了（参考記録） |

これらは基準そのものの未達ではなく、参考値の欠落・未達に留まるため、REQ-1 の
受け入れ判定（終了コード 0・FAIL なし）には影響しない。

## スコープ外（本レポートに含まないもの）

- 性能計測（RPS・レイテンシ・RSS・バイナリサイズ・起動時間、axum-ref 比）→ #71（TASK-1.6-1）
- 基準 E の関数範囲抽出を grep/awk からより堅牢な構文解析（syn ベースの検査バイナリ等）
  へ置き換える改善、`/* */` ブロックコメント対応（基準 B/D と共通の既知限界） → 本イシュー
  （#169）の対象外。ユーザー承認のうえ別途起票を検討する
- `cargo geiger` 実行失敗の原因調査・修正 → 参考値であり受け入れ判定に影響しないため
  本イシューのスコープ外
- 性能計測レポート・accept スクリプトの CI 常時実行化 → 対象外
