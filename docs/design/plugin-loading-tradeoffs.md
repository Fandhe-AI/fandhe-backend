# プラグインロード方式の安全性トレードオフ設計文書（TASK-2.4 / #21）

`docs/spec/04-requirements.md` REQ-2 の受け入れ基準「コンパイル時方式と実行時動的ロード
方式の安全性トレードオフが設計文書として記録されている」に対応する成果物。

## 1. 結論

backend-framework は **コンパイル時 Cargo feature flag + `dep:` 構文によるプラグイン機構**
を採用し、**実行時動的ロード（`dylib` + C ABI、`dlopen` 系）は不採用**とする。
本判断は PoC-3（`docs/spec/03-poc/plugin-mechanism/`）で確立され、
`docs/spec/04-requirements.md` REQ-2 に明記済みの決定を、TASK-2.4 の受け入れ検証
（`crates/plugin-webrtc-proxy` + `crates/plugin-graphql` の 2 プラグイン実証、
`scripts/accept/plugin-mechanism-accept.sh`）を踏まえて設計文書として再記録するもの。

## 2. 比較対象の 2 方式

| 項目 | コンパイル時 feature flag（採用） | 実行時動的ロード（不採用） |
|------|-----------------------------------|---------------------------|
| 機構 | Cargo `[features]` + `optional = true` + `dep:` 構文 | `dylib` クレート型 + `dlopen`/`libloading` 等 + C ABI 境界 |
| プラグイン切り替えタイミング | ビルド時（`cargo build --features <name>`） | 実行時（バイナリ起動後にロード） |
| 型安全性 | Rust の型システム・トレイト境界がコンパイル時に保証 | ABI 境界を越えるため Rust の型システムの保証が及ばない |
| `unsafe` の要否 | プラグイン境界自体に `unsafe` は不要（`dep:` は通常の Cargo 依存解決） | シンボル解決・関数ポインタ呼び出しに構造的な `unsafe` を要する |
| feature 無効時のバイナリ影響 | 依存・コード・`unsafe` が Cargo レベルで完全除外（`cargo tree`/`cargo geiger`/バイナリサイズで機械検証可能。TASK-2.2 / #19） | 動的ライブラリ自体は別ファイルとして存在し続け、除外の保証は運用（デプロイ手順）に依存 |
| 無停止差し替え | 不可（再ビルド + デプロイが必須） | 可能（プロセス再起動なしにプラグイン差し替え） |

## 3. 不採用の 3 根拠（PoC-3 実測・REQ-2 準拠）

`docs/spec/04-requirements.md` REQ-2 詳細に明記された 3 根拠を、本タスクの実装
（`crates/plugin-webrtc-proxy`・`crates/plugin-graphql` の 2 インスタンス）を踏まえて
敷衍する。

### (a) プラグイン境界に構造的な `unsafe` を要し、攻撃表面最小化と相容れない

`dlopen`/`LoadLibrary` によるシンボル解決は、OS レベルで任意のコードをプロセス空間に
ロードする操作であり、Rust の安全性検証の外側にある。ロードするライブラリのパス・
シンボル名が実行時にしか確定しないため、`unsafe` を境界の**構造そのもの**に持たざるを
得ない。本フレームワークの出発点（CLAUDE.md「AI によるセキュリティ脆弱性発見リスクに
備える」）と、`.claude/rules/security.md` の「攻撃表面の最小化とメモリ安全性を最優先」
という方針に正面から反する。

対して `crates/plugin-webrtc-proxy`・`crates/plugin-graphql` はいずれも `unsafe` 0 件
（`scripts/unsafe-triage.sh` で継続的に確認、本タスクの検証結果は
`docs/acceptance/req2-plugin-mechanism.md` を参照）であり、`dep:` 構文によるコンパイル時
解決は `unsafe` を一切要求しない。

### (b) Rust に安定 ABI が存在せず、監査困難な失敗モードを抱える

Rust は C ABI 以外の安定 ABI を提供しない。プラグイン側とコア側を別クレートとして
コンパイルした `dylib` を動的リンクする場合、両者が同一の rustc バージョン・
最適化フラグ・依存バージョンでビルドされていることを実行時に保証する仕組みが
言語・ツールチェーンレベルに存在しない。ずれがあっても**コンパイルもロードも成功し、
実行時の特定の呼び出しパスでのみクラッシュ・未定義動作を起こす**（「コンパイルもロード
も成功するが実行時にのみ破綻する」失敗モード）。この種の不具合は静的解析・型検査・
通常のテストスイートでは検出できず、監査（`.claude/rules/security.md` の
「メモリ安全性」観点）を著しく困難にする。

対してコンパイル時 feature flag 方式では、プラグインはコアと**同一のビルドプロセス・
同一の rustc バージョン・同一の依存解決（`Cargo.lock`）** でコンパイルされる。型不一致・
ABI 不一致はコンパイルエラーとして即座に検出され、実行時まで問題が持ち越されない。

### (c) 本フレームワークのユースケースでは無停止差し替えの利点を要しない

動的ロードの主な利点は「プロセスを止めずにプラグインを差し替えられる」ことだが、
backend-framework が想定するユースケース（`.claude/rules/pay-for-what-you-use.md` の
「多数のマイクロサービスを低リソースで運用する」）は、feature 構成がサービスごとに
デプロイ時点で確定し、実行中に切り替える要求を持たない。コンテナオーケストレーション
前提のデプロイフロー（ロールアウトによる新バージョンへの切り替え）で十分に運用でき、
(a)(b) の安全性コストを払ってまで得るべき利点がない。

## 4. コンパイル時方式の限界（採用に伴うトレードオフ）

- **再ビルド必須**: feature 構成を変える場合は必ず `cargo build` の再実行を要する。
  無停止差し替えができない（3 節 (c) の裏返し）。運用は「新しい feature 構成のバイナリを
  ビルドしてロールアウトする」ことを前提とする。
- **feature の組み合わせ数がビルド行列を増やす**: `webrtc-proxy`・`graphql` 等の feature
  が増えるほど CI が検証すべきビルド構成（無効・単独有効・全有効の組み合わせ）が増える。
  `scripts/pay-for-what-you-use-check.sh`（TASK-2.2、#19）が `cargo metadata` からの
  動的列挙により、feature 追加時のスクリプト変更を不要にすることでこの負担を緩和している。
- **同一バイナリ内に複数プラグインが存在しうる**: 動的ロード方式と異なり、複数 feature を
  同時に有効化したビルドでは各プラグインのコードが同一バイナリに含まれる。
  互いに独立したクレートとして分離する設計（本タスクの `plugin-webrtc-proxy` /
  `plugin-graphql`）により、依存・コードの混入は feature 単位で抑えられるが、
  クレート間の意図しない結合（グローバル状態の共有等）を実装者が持ち込まない規律は
  引き続き必要（`.claude/rules/coding-rust.md` の設計原則）。

## 5. 監査容易性への影響

コンパイル時方式は、プラグインの有効/無効という「攻撃表面の実際の状態」が
**ビルド成果物（Cargo.toml の feature 選択・`cargo tree` の出力）から静的に読み取れる**
という利点を持つ。これは AI ファースト保守性（CLAUDE.md の核となる 2 原則の 1 つ）とも
整合する: レビュー担当（人間・AI いずれも）は実行時の状態を推測する必要なく、
`cargo tree -p backend-framework-core --features <構成>` の出力だけで依存グラフの全体像を
把握できる。動的ロード方式では、この情報がデプロイ時の配置ファイル（ロードするプラグイン
一覧）に分散し、ビルド成果物だけからは判定できない。

## 6. 再評価の条件

`docs/spec/04-requirements.md` 除外事項 3 番「実行時動的プラグインロード」に記載のとおり、
将来 Rust の安定 ABI が整備された場合には (b) の根拠が解消されるため再評価の余地がある。
現時点（2026-07-17）ではそのような安定 ABI は存在しない。

## 参照

- `docs/spec/04-requirements.md` REQ-2（プラグイン機構）
- `docs/spec/03-poc/plugin-mechanism/`（PoC-3）
- `docs/design/plugin-boundary.md`（パスインターセプト型・Upgrade 型の実装パターン）
- `docs/acceptance/req2-plugin-mechanism.md`（本タスクの受け入れ検証結果）
- `.claude/rules/security.md`・`.claude/rules/pay-for-what-you-use.md`
