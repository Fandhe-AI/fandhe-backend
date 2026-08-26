# REQ-1 受け入れ検証レポート — 依存数・unsafe・監査（TASK-1.6-2、#72 / TASK-1.6-4、#169）

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` / `bf-plugin-*` 等）
> 表記のまま保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。

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
| cargo-geiger | 導入済み（0.13.0）。実行失敗の原因は #284 で解消済み（下記「再検証（#284、2026-07-20）」節を参照。cargo-geiger 自体の一過性失敗（#212）は残存し得るが WARN 経路で吸収する） |
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
| B補足: cargo geiger（参考値） | → #284 で解消済み（下記「再検証（#284、2026-07-20）」節を参照） | 実行失敗の原因（仮想マニフェスト越しの `-p` パッケージ選択非対応）を特定し、`--manifest-path` 指定 + JSON 判定へ修正。二重検証として正式に PASS/FAIL 判定を持つようになった（実行自体が失敗した場合のみ WARN） |
| D参考値: tokei | SKIP（未導入） | 基準ではなく参考値のため受け入れ判定に影響しない |
| audit/deny の CI 組み込み | CI 組み込み済み（TASK-15.2 #17・#108） | 完了（参考記録） |

これらは基準そのものの未達ではなく、参考値の欠落・未達に留まるため、REQ-1 の
受け入れ判定（終了コード 0・FAIL なし）には影響しない。

## スコープ外（本レポートに含まないもの）

- 性能計測（RPS・レイテンシ・RSS・バイナリサイズ・起動時間、axum-ref 比）→ #71（TASK-1.6-1）
- 基準 E の関数範囲抽出を grep/awk からより堅牢な構文解析（syn ベースの検査バイナリ等）
  へ置き換える改善、`/* */` ブロックコメント対応（基準 B/D と共通の既知限界） → 本イシュー
  （#169）の対象外。ユーザー承認のうえ別途起票を検討する
- ~~`cargo geiger` 実行失敗の原因調査・修正~~ → #284 で解消済み（下記「再検証
  （#284、2026-07-20）」節を参照）
- 性能計測レポート・accept スクリプトの CI 常時実行化 → 対象外

## 再検証（#284、2026-07-20）

### 背景・失敗原因

上記「B補足: cargo geiger」は前回レポート時点で WARN のまま長期放置されていた。
原因を特定した結果、`scripts/accept/core-deps-unsafe-audit.sh` が workspace ルート
（仮想マニフェスト）に対して

```bash
cargo geiger --output-format Ascii -p fandhe-backend-core 2>/dev/null | tail -5 ...
```

を実行しており、cargo-geiger 0.13.0 は仮想マニフェスト越しの `-p` パッケージ選択に
対応しないため

```
manifest path <repo>/Cargo.toml is a virtual manifest, but this command requires
running against an actual package in this workspace
```

で即失敗していた（バージョン・edition 互換の問題ではない）。`2>/dev/null` で stderr
を握り潰し、`set -o pipefail` によりコマンド置換全体が失敗 → `|| echo '実行に失敗
...'` に落ちて WARN 固定化していた。

### 修正内容

`--manifest-path crates/core/Cargo.toml --no-default-features` で実パッケージを
起点に指定し（`scripts/pay-for-what-you-use-check.sh` と同じ呼び出し方に統一）、
専用 `CARGO_TARGET_DIR`（`target/accept-geiger`）で共有 `target/` のビルドキャッシュ
破損を回避。イシュー #212（cargo-geiger の非決定的 panic）を踏まえ最大 3 回の簡易
リトライを実装し、stderr は握り潰さず失敗時に要約を記録するよう変更した。判定は
geiger JSON 出力（`jq` で解析）から対象コアクレート（`fandhe-backend-core`・
`fandhe-backend-http`・`fandhe-backend-routes`）の used unsafe
（`functions`/`exprs`/`item_impls`/`item_traits`/`methods` の `unsafe_` 合算）を
集計し、全て 0 なら PASS、非 0 または対象クレート欠落なら FAIL、geiger 実行自体が
リトライ後も失敗した場合のみ WARN とする（詳細は `scripts/accept/README.md`
「基準 B の cargo geiger 二重検証（#284）」節を参照）。同一原因の随伴事象として
`scripts/dep-impact.sh` の geiger 呼び出しも同じ修正を適用した。

**追加修正（同一イシュー #284 内、フル実行での再検証で判明）**: 上記修正直後の
実装は `--manifest-path crates/core/Cargo.toml` を**相対パス**のまま渡していたが、
`scripts/accept/core-deps-unsafe-audit.sh` を実際にフル実行したところ 3 回の
リトライすべてが失敗し、stderr に
`error: manifest_path:"crates/core/Cargo.toml" is not an absolute path. Please
provide an absolute path.` を確認した。これは cargo-geiger 0.13.0 固有の制約
（`--manifest-path` に絶対パスを要求する。プレーンな `cargo` コマンドは相対パスを
許容するため cargo-geiger 特有の挙動）であり、同一コマンドを複数回・複数の
独立した `CARGO_TARGET_DIR` で実行しても毎回同じエラーで確定的に失敗した
（#212 の非決定的 panic とは異なる、リトライで回復しない性質の失敗）。よって
当初の再実行結果に記載していた「単独実行で相対パス指定のまま成功した」という
記述は誤りだったと判明したため、本節を上書き訂正する。`scripts/pay-for-what-you-use-check.sh`
の `CORE_MANIFEST="${WORKSPACE_ROOT}/crates/core/Cargo.toml"` に倣い、
`core-deps-unsafe-audit.sh`・`dep-impact.sh` の両方で `--manifest-path` を
`${WORKSPACE_ROOT}`（`${REPO_ROOT}`）を前置した絶対パスに修正した。

### 再実行結果（絶対パス修正後）

実行環境: cargo-geiger 0.13.0 / rustc 1.96.0（本レポート冒頭の実行環境と同一）。

`scripts/accept/core-deps-unsafe-audit.sh` をフル実行し、「B補足: cargo geiger
（二重検証）」が PASS となることを確認した（対象 3 コアクレート
`fandhe-backend-core` / `fandhe-backend-http` / `fandhe-backend-routes` の
used unsafe（`functions.unsafe_`/`exprs.unsafe_`/`item_impls.unsafe_`/
`item_traits.unsafe_`/`methods.unsafe_` 合算）がいずれも 0）。これは基準 B 本体
（grep 検証）の「unsafe 0 件」判定と一致する。同一構成で 3 回連続実行し、いずれも
1 回目のリトライで成功・WARN 経路には落ちなかったことも確認した。`dep-impact.sh`
の geiger 呼び出し（絶対パス修正後）も単独実行し、正常に Utf8 形式の集計表が
出力されることを確認した。

## 再検証（2026-08-26、v0.4.0 系 main `a4192b5`）

紹介記事（Fandhe-AI/articles PR #61）で本監査値を引用するにあたり、初回実行（2026-07-18、v0.1.0 公開前のコード）から乖離がないかを確認するため、`scripts/accept/core-deps-unsafe-audit.sh` を main（コミット `a4192b5`、v0.4.0 以降のコード）でフル再実行した。

実行環境: macOS 26.6.2（Apple Silicon）/ rustc 1.96.0 / cargo-geiger 0.13.0。

### 結果サマリー

```text
[PASS] A: 依存クレート数比 <=50%: core=11 種類 / axum-ref=50 種類（比率 22%、自クレート含む同一手法での cargo tree -e normal 集計）
[PASS] B: unsafe 0件/根拠明記: 対象コアクレート（crates/core crates/http crates/routes）の src/ に unsafe 0 件
[WARN] B補足: workspace lint: ルート Cargo.toml で unsafe_code="warn" を設定済み。CI の clippy -D warnings と組み合わせ実質 deny として機能（.claude/rules/security.md）
[PASS] B補足: cargo geiger（二重検証）: 対象コアクレート（fandhe-backend-core fandhe-backend-http fandhe-backend-routes）の used unsafe（functions/exprs/item_impls/item_traits/methods 合算）が全て 0
[PASS] C: cargo audit 既知脆弱性 0件: Loaded 1226 security advisories / Scanning Cargo.lock for vulnerabilities (356 crate dependencies)
[PASS] C: cargo deny check: deny.toml による全項目チェックで違反 0 件
[FAIL] D: コア実質コード行数 <=5000: 実質コード行数（空行・// コメント行除外）: 8819 行（対象: crates/core crates/http crates/routes） / tokei 参考値: "code":15433
[PASS] E: 3拡張点 trait 定義: Middleware / UpgradeHandler / RequestGate すべて crates/core/src に定義あり
[FAIL] E: コアループの feature 非分岐: コアループ関数を crates/core/src/server.rs から検出できず計測不能（スクリプトの検出パターンの陳腐化。下記参照）
[FAIL] F: プラグイン非依存（http/routes）: 検出: crates/http/Cargo.toml に plugin- 依存あり（コメント行への誤検知。下記参照）
```

### 初回実行（2026-07-18）からの差分

| 基準 | 2026-07-18（v0.1.0 前） | 2026-08-26（v0.4.0 系） | 判定 |
| --- | --- | --- | --- |
| A: コア推移的依存 | 9 種類 / axum-ref 50 種類（18%） | **11 種類** / axum-ref 50 種類（**22%**） | PASS（基準 ≤ 50% 継続） |
| B: unsafe（grep + geiger） | 0 件 | 0 件（geiger used unsafe 全 0） | PASS |
| C: cargo audit / deny | 0 件（advisory 1,166・340 deps） | 0 件（advisory 1,226・**356 deps**） | PASS |
| D: コア実質行数 | 2,478 行 | **8,819 行**（tokei code 15,433） | **FAIL（基準 ≤ 5,000 行を超過）** |

- 基準 A の 11 種類は自クレート 3 個（`fandhe-backend-core` / `fandhe-backend-http` / `fandhe-backend-routes`）を含む（初回と同一手法・axum-ref 側も同じ扱い）。外部クレートは bytes / libc / memchr / mio / pin-project-lite / rustc-hash / socket2 / tokio の 8 種類
- 基準 D の増加（2,478 → 8,819 行）は v0.2.0〜v0.4.0 の機能追加（`Interceptor`、`GateContext::peer_addr`、DoS 既定値、chunked 復号上限等）によるもので、受け入れ基準 ≤ 5,000 行を超過している。基準値の見直しまたは行数削減は別イシューで扱う

### 基準 E / F の FAIL はスクリプトの誤検知

- **E（コアループの feature 非分岐）**: スクリプトの awk 抽出が想定する関数シグネチャと現行実装が一致せず「計測不能」となった。実体は `crates/core/src/server.rs` の接続ループ（`run` / `handle_connection` / `handle_connection_with_peer_addr`）内に `#[cfg(feature = ...)]` が 0 箇所であることを手動 grep で確認済み（`server.rs` 内の `cfg(feature` 42 箇所はすべてプラグイン登録メソッド側）
- **F（プラグイン非依存）**: `crates/http/Cargo.toml` の **コメント行**（memchr 導入の説明文中の「plugin-graphql」等の文字列）に反応した誤検知。`cargo tree -p fandhe-backend-http -e normal` の依存一覧にプラグインクレートは含まれない

スクリプトの検出ロジック修正（E の関数パターン更新・F のコメント行除外）は別イシューで扱う。
