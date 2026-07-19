# フレームワーク正式名称の決定記録

> **ステータス: 正式名称 `fandhe-backend` に確定。**
> ユーザー決定（2026-07-19、親イシュー #200）により、fandhe ブランドで複数
> フレームワーク（backend / frontend / AI）を展開する方針のもと、org 名 `fandhe`
> をプレフィックスとする統一命名 **`fandhe-backend`** を正式名称として確定した。
> 旧候補 `wrenframe`（イシュー #92 で AI エージェントが選定・提案し、
> レビューゲート確定待ちとしていたもの）は、この方針転換により**経緯**として
> 2 節以降にそのまま保持する（削除しない）。決定の根拠・可用性証跡・確定版
> 新旧マッピング表は「決定（確定版）」節を参照。

対応: #200（`fandhe-backend` への改名ツリー・親イシュー）・#201（本イシュー、
決定記録の改訂）、#92（`chore(global): フレームワーク正式名称の確定`、旧候補
`wrenframe` の選定記録）、`docs/spec/01-brainstorm.md` 「未解消（残る確認事項）」の
「フレームワーク名の確定（Phase 2 以降）」（該当行は仕様書 submodule 側の
履歴としてそのまま残し、書き換えない）。

## 決定（確定版）

**決定名称: `fandhe-backend`**（ユーザー決定 2026-07-19、親イシュー #200）。

### 根拠

- fandhe ブランドで複数フレームワーク（backend / frontend / AI）を展開する方針が
  新たに定まり、各フレームワークを org 名 `fandhe` をプレフィックスとする統一命名
  体系（`fandhe-backend` / `fandhe-web` / `fandhe-ai` 等）に揃えることで、ブランド
  としての一貫性・検索性・今後の関連プロジェクトとの整合を優先した
- 旧候補 `wrenframe`（2〜5 節、単独ブランド案）は 3〜4 節の一次スクリーニングでは
  重大な衝突が見つからず有効な候補だったが、本方針転換（統一プレフィックス採用）
  により経緯（過去の選定記録）として扱う。`wrenframe` 自体の可用性評価が誤って
  いたわけではない
- `fandhe-backend` は「fandhe 傘下の backend フレームワーク」であることが名称
  から直接読み取れ、`fandhe-web` / `fandhe-ai` 等の将来追加フレームワークとも
  命名規則上の対称性を持つ

### 可用性確認の証跡（2026-07-19、実装時に再確認済み）

親イシュー #200 記載の確認結果（crates.io `fandhe-backend`/`fandhe`/`fandhe-web`/
`fandhe-ai` 未登録、npm `@fandhe/backend`・`fandhe-backend` 未登録、GitHub
`Fandhe-AI/fandhe-backend` 未使用）を出典としつつ、本イシュー実装時（2026-07-19）
に同一 API で再確認した実測結果は次のとおり（3 節と同形式。200=使用中/404=未使用）。

| 対象 | 確認方法 | 結果 |
|------|---------|------|
| crates.io `fandhe-backend` | `https://crates.io/api/v1/crates/fandhe-backend` | 未使用 (404) |
| crates.io `fandhe` | `https://crates.io/api/v1/crates/fandhe` | 未使用 (404) |
| crates.io `fandhe-web` | `https://crates.io/api/v1/crates/fandhe-web` | 未使用 (404) |
| crates.io `fandhe-ai` | `https://crates.io/api/v1/crates/fandhe-ai` | 未使用 (404) |
| npm `fandhe-backend` | `https://registry.npmjs.org/fandhe-backend` | 未使用 (404) |
| npm `@fandhe/backend` | `https://registry.npmjs.org/@fandhe%2Fbackend` | 未使用 (404) |
| GitHub `Fandhe-AI/fandhe-backend` | `https://api.github.com/repos/Fandhe-AI/fandhe-backend` | 未使用 (404) |

3 節と同様、crates.io の API 応答はブラウザ相当の `User-Agent` ヘッダ付与が
必要（既定 User-Agent では Cloudflare 由来の 403 を返す環境がある）。本確認も
crates.io/npm registry・GitHub API による一次スクリーニングであり、商標登録
データベースの調査や法的なクリアランスではない（6 節参照。フェイルクローズ:
到達不能・想定外応答の場合は「未使用」と断定せず #200 の記載を出典として明記する
方針だが、今回は両出典が一致した）。

### 確定版 新旧マッピング表（#200 本文を正とし転記。#202〜#205 の実装が参照する）

| 種別 | 旧 | 新 |
|------|-----|-----|
| Cargo package（コア） | `backend-framework-core` | `fandhe-backend-core` |
| Cargo package | `bf-http` / `bf-routes` / `bf-http-fuzz` | `fandhe-backend-http` / `fandhe-backend-routes` / `fandhe-backend-http-fuzz` |
| Cargo package | `bf-plugin-<x>`（8 種） | `fandhe-backend-plugin-<x>` |
| Rust import | `bf_http::` / `bf_routes::` / `bf_plugin_<x>::` / `backend_framework_core::` | `fandhe_backend_http::` 等 |
| 環境変数 | `BF_*`（`BF_HUB_GATE`・`BF_TRACING_PROBE_*` 等） | `FANDHE_BACKEND_*` |
| ts パッケージ | `backend-framework-openapi-ts` | `@fandhe/backend-openapi-ts` |
| GitHub リポジトリ | `Fandhe-AI/backend-framework` | `Fandhe-AI/fandhe-backend` |
| 文書表記 | `backend-framework` / `wrenframe` | `fandhe-backend` |

対象外（#200 と同一方針）: `axum-ref`・`ws-load-client`・`gen-openapi` 等の
中立な補助バイナリ名、`docs/spec/`（別リポジトリ `backend-framework-spec`。
本ツリーでは参照表記のみ更新し、リポジトリ自体の改名は別途判断）。

## 1. 背景

- 現状フレームワークは `backend-framework`（仮称）。OSS 公開方針
  （`docs/spec/01-brainstorm.md` 5 項）が決定済みであり、公開に先立ち正式名称・
  命名根拠・反映方針の確定が必要（#92 受け入れ条件 1・2）。
- 既存の名称露出箇所（crate 名 `backend-framework-core` / `bf-http` /
  `bf-routes` / `bf-plugin-*`、`ts/package.json` の `backend-framework-openapi-ts`、
  README.md・CLAUDE.md の仮称注記）は本イシュー（#201）では変更しない。反映は
  7 節の段階的移行計画に従う。

## 経緯（旧候補 `wrenframe` の選定記録、#92）

> この節（2〜5 節）は `fandhe-backend` 確定前（#92 時点）の選定記録であり、
> 現在の正式名称の決定は「決定（確定版）」節を参照。以下は改変せず保持する
> （受け入れ条件 3、#201）。

## 2. 命名基準（評価軸）

1. **crates.io 可用性**: 名称本体・想定される派生名（`<name>-core` 等）が
   未使用であること
2. **npm 可用性**: `ts/` パイプライン（TASK-6.1、#54）が生成する npm パッケージ名
   との整合
3. **GitHub 上の深刻な衝突がないこと**: 同名の著名リポジトリ・組織が存在しないこと
4. **既存 OSS・商標との混同リスクの低さ**: なりすまし・typosquat を誘発しにくいこと
   （[[security]] のサプライチェーン観点）
5. **検索性・発音のしやすさ**: 一般名詞すぎず、検索エンジン・crates.io 検索で
   一意に到達できること
6. **フレームワークの性格との整合**: 「軽量・高速・高並行・安全」という性格を
   想起させること
7. **既存 `bf-` プレフィックス資産からの移行コスト**: 短い代替プレフィックスに
   置き換えやすいこと

## 3. 可用性確認の証跡

確認日: 2026-07-18。確認方法・出典を明記する（登記や商標登録の確認ではなく、
一次スクリーニングである点に注意。6 節参照）。

### crates.io（`https://crates.io/api/v1/crates/<name>`、200=使用中/404=未使用）

| 候補 | crates.io | npm registry | GitHub 完全一致リポジトリ名 | 備考 |
|------|-----------|--------------|----------------------------|------|
| `fenrir` | 使用中 (200) | 未確認 | — | 早期に除外 |
| `bolt` | 使用中 (200) | 未確認 | — | 一般名詞すぎる。早期に除外 |
| `vela` | 使用中 (200) | 未確認 | — | 早期に除外 |
| `forgex` | 使用中 (200) | 未確認 | — | 早期に除外 |
| `ferrite` | 使用中 (200) | 未確認 | — | 早期に除外 |
| `axl` | 使用中 (200) | 未確認 | — | 早期に除外 |
| `swiftly` | 未使用 (404) | 未確認 | — | 一般語で検索性低。除外 |
| `nimbly` | 未使用 (404) | 未確認 | — | 一般語で検索性低。除外 |
| `swiftrs` | 未使用 (404) | 未確認 | — | `swift` 言語との混同リスク。除外 |
| `corebolt` | 未使用 (404) | 未確認 | — | 除外（`bolt` 系との混同） |
| `fennec` | 使用中 (200) | 未確認 | — | 早期に除外 |
| `corvus` | 使用中 (200) | 未確認 | — | 早期に除外 |
| `quill` | 使用中 (200) | 未確認 | — | 早期に除外 |
| `hikari` | 使用中 (200) | 未確認 | — | 早期に除外 |
| `wren` | 使用中 (200) | 使用中 (200) | — | Wren スクリプト言語との衝突あり。除外 |
| `corvid` | 未使用 (404) | 使用中 (200) | 一致なし | npm 側衝突で除外 |
| `rapidus` | 未使用 (404) | 使用中 (200) | 一致なし | npm 側衝突で除外 |
| `fenn` | 未使用 (404) | 未使用 (404) | 一致なし | 人名としての一般性が高く検索性に懸念。次点 |
| `zephyrs` | 未使用 (404) | 未使用 (404) | 一致なし | **Zephyr Project（Linux Foundation の RTOS）との商標混同リスクが高いため除外**（axis 4 不充足） |
| `quillweb` | 未使用 (404) | 未使用 (404) | 一致なし | 次点。`quill` 本体が既に使用中で紛らわしい |
| `hikariweb` | 未使用 (404) | 未使用 (404) | 一致なし | 次点。`hikari`（HikariCP 等）との混同リスク |
| `wrenframe` | 未使用 (404) | 未使用 (404) | 一致なし | **決定候補**（4 節） |

GitHub 完全一致確認: `https://api.github.com/search/repositories?q=<name>+in:name`
で取得した結果を `full_name` の末尾セグメントが候補名と完全一致するものに絞り込み、
`corvid` / `rapidus` / `fenn` / `zephyrs` / `wrenframe` のいずれについても完全一致
リポジトリを確認できなかった（2026-07-18 確認）。

Web 検索（一般 Web 衝突確認）: `wrenframe` を Web 検索したところ、ヒットしたのは
額縁・眼鏡フレーム等の無関係な物販サイトと、綴りが異なる別サービス
（`wyreframe.studio`、`wireframe.cc` 等のワイヤーフレーム制作ツール）のみで、
ソフトウェア・フレームワークとしての衝突は確認されなかった（2026-07-18 確認）。

### 派生 crate 名の確認（決定候補 `wrenframe` について）

| 派生名 | crates.io |
|--------|-----------|
| `wrenframe-core` | 未使用 (404) |
| `wrenframe-http` | 未使用 (404) |
| `wrenframe-routes` | 未使用 (404) |

## 4. 候補評価マトリクス

| 候補 | crates.io/npm 可用性 | GitHub/Web 衝突 | 検索性 | 性格整合 | 移行コスト | 総合 |
|------|----------------------|-----------------|--------|----------|------------|------|
| `fenn` | ○（両方未使用） | 低 | △（一般人名で検索埋没しやすい） | △（軽量さを直接連想させない） | ○（短い） | 次点 |
| `zephyrs` | ○（両方未使用） | **× Zephyr Project と混同リスク大** | ○ | ○（軽やかさ＝軽量・高速を連想） | ○ | **除外**（axis 4 不充足） |
| `quillweb` | ○ | 低 | △（`quill` 本体が既存で紛らわしい） | △ | △（やや長い） | 次点 |
| `hikariweb` | ○ | 低〜中（`hikari`/HikariCP 想起） | △ | △ | △ | 次点 |
| `wrenframe` | ○（両方未使用、派生名も未使用） | 低（完全一致なし、Web 検索でも無関係な結果のみ） | ○（複合語で一意に検索可能） | ○（wren＝小型・俊敏な鳥 + frame＝骨組み。「軽量・高速・高並行」の性格と直接整合） | ○（`wrenframe-` を新プレフィックスに採用可能。`bf-` からの移行手順は 5 節で計画） | **推奨** |

## 5. 決定と根拠

**決定候補: `wrenframe`**（1 節で述べたとおり本 PR のレビューゲート通過をもって
正式に確定する）。

根拠:

- 3〜4 節の可用性確認で crates.io・npm・GitHub 完全一致・一般 Web 検索のいずれでも
  重大な衝突が見つからなかった（唯一の直接一致は `wren` 単体で、こちらは Wren
  スクリプト言語との衝突により候補から除外済み。複合語 `wrenframe` は独立した
  検索性を持つ）
- 「wren（鳥）+ frame（骨組み）」という構成が、本フレームワークの性格
  （軽量・高速・高並行・安全な最小コア + プラグイン拡張）を素直に想起させる
- 派生 crate 名（`wrenframe-core` 等）も確認時点で未使用であり、5 節の段階的
  移行計画（`bf-` → `wrenframe-`）と両立する

**留保事項**（人間管理者の実施が必要、6 節参照）:

- 本確認は crates.io/npm registry・GitHub 検索 API・一般 Web 検索による
  一次スクリーニングであり、商標登録データベースの調査や法的なクリアランス
  ではない。正式な商標・法的確認は人間管理者が別途実施する
- crates.io での名称確保（予約公開）の実施可否・タイミングは人間管理者が判断する

## 6. 責務分界

以下は org 管理者権限・法務判断が必要な**人間実施**の作業であり、AI エージェントの
自律実装スコープ外とする。

- リポジトリ名の変更（GitHub リダイレクト・外部リンク・CI シークレット等への
  影響評価を含む）
- crates.io 上での名称確保（予約公開）の要否判断・実施
- 商標・法的なクリアランス確認（3 節の一次スクリーニングを超える正式調査）
- `docs/spec/`（別リポジトリ `Fandhe-AI/backend-framework-spec`）側の名称関連記述の
  更新（submodule のため本リポジトリ側からは書き換えない）

## 7. 反映方針（段階的移行計画）

> 旧計画（#92 時点、下段の各段階名は当時 `wrenframe-*` を用いていた）を、
> `fandhe-backend` 確定後の実イシュー（#200 ツリー配下 #201〜#205）に対応付けて
> 改訂する。各段階は個別 Issue・個別 PR とし、影響箇所リストなど旧計画で有用
> だった内容は確定名（`fandhe-backend`）に読み替えて維持する。

名称確定の影響範囲が広く（`bf-` プレフィックス crate 群・環境変数・
`ts/package.json`・リポジトリ名・ドキュメント全体）、1 回の変更にまとめると
レビュー困難・ロールバック困難になるため、次の段階に分けて実施する。

### 第 1 段階: 決定記録の改訂（本イシュー #201 で実施）

- 本決定記録（`docs/design/framework-naming.md`）の `fandhe-backend` 確定への
  改訂・確定版新旧マッピング表の追加
- `docs/design/README.md` インデックス説明の更新
- `README.md` / `CLAUDE.md` の仮称注記を「正式名称 `fandhe-backend`（確定）」の
  記述へ更新（実装フェーズの詳細は本ドキュメントへ誘導）

### 第 2 段階: crate・import 一括改名（#202、実施済み）

- crate 名リネーム: `backend-framework-core` → `fandhe-backend-core`、
  `bf-http` → `fandhe-backend-http`、`bf-routes` → `fandhe-backend-routes`、
  `bf-http-fuzz` → `fandhe-backend-http-fuzz`、`bf-plugin-*`（8 種）→
  `fandhe-backend-plugin-*`、Rust import（`bf_http::` 等）→
  `fandhe_backend_http::` 等
- 影響箇所（リネーム時に追随が必要な範囲、実装時に全数確認すること）:
  - workspace 内の相互参照（各 `Cargo.toml` の `[dependencies]` セクション、
    feature 経由の `dep:` 配線、`crates/core/Cargo.toml` の feature 定義）
  - CI（`.github/workflows/ci.yml` のジョブ・キャッシュキー等がクレート名に
    依存していないか確認）
  - `benches/**`・`scripts/**`・`scripts/accept/**`（クレート名を直接参照する
    シェルスクリプト・レポート）
  - `docs/dep-impact/records.md`・`docs/acceptance/*.md`（記録済みレポート内の
    クレート名表記。過去レポートは実測値の記録として書き換えず、注記を追加する
    方針を推奨）
  - `AGENTS.md`（クレート名を含む説明箇所）

### 第 3 段階: 環境変数改名（#203、実施済み）

- `BF_*`（`BF_HUB_GATE`・`BF_TRACING_PROBE_*` 等）→ `FANDHE_BACKEND_*`
- 参照箇所（コード・CI・スクリプト・ドキュメントの環境変数参照）の全数確認

**breaking change の記録**（#209 の crate・import 改名と同種の判断）:

- 後方互換シムは設けない。旧環境変数名 `BF_*` を指定しても新コードは無視し、
  `FANDHE_BACKEND_*` を認識しない旧コードへのフォールバックも行わない
  （pre-1.0・外部利用者が存在しないため、シム維持コストに見合わない）
- 旧名 `BF_HUB_GATE=off` を設定し続けた場合、新コードでは `Server::gate` の
  `TenantGate` 登録判定に影響しない（安全側＝ゲート有効のまま）ため fail-open
  にはならない。`BF_TRACING_PROBE_OUTPUT` 等の必須 env は旧名指定時に
  「未指定」として明示的にエラー停止し、サイレントな誤動作は発生しない
- 影響範囲: `crates/plugin-hub-wiring`・`crates/plugin-tracing` の example、
  `benches/**`・`scripts/**` の呼び出し環境変数。移行は利用箇所を
  `FANDHE_BACKEND_*` へ置換するのみで追加の互換コードは不要

### 第 4 段階: ts パッケージ改名（#204 で実施）

- `ts/package.json` の `name`（`backend-framework-openapi-ts` →
  `@fandhe/backend-openapi-ts`）および `ts/package-lock.json` の追随

### 第 5 段階: ドキュメント・CI・スクリプト表記統一（#205、未実施）

- `docs/design/*.md`・`docs/acceptance/*.md`・`benches/README.md` 等、ドキュメント
  全体での呼称統一（`backend-framework` / `wrenframe` 表記から `fandhe-backend`
  への置換。本決定記録の経緯節・過去レポートの実測値記録は対象外）
- `docs/spec/**` は submodule のため対象外（別リポジトリ側での対応が必要な場合は
  そちらへ別途申し入れる）

### ツリー外（人間管理者実施、6 節）

- リポジトリ名の変更（人間管理者実施、6 節）。GitHub はリポジトリ名変更時に
  旧 URL からのリダイレクトを提供するが、`docs/spec/`（submodule）側の
  `.gitmodules` 参照 URL・`Fandhe-AI/backend-framework-spec` 側からの逆参照
  リンクへの影響を事前に確認する
- crates.io への名称確保・公開（人間管理者実施、6 節）

## 参照

- 決定の親イシュー・改名ツリー: #200（本記録は配下 #201 対応）
- 背景: `docs/spec/01-brainstorm.md`「未解消（残る確認事項）」
- レビューゲート運用: [[review-gate]]（`docs/design/review-gate.md`）
- スコープ外課題の追跡: [[out-of-scope-tracking]]
- セキュリティ規約（サプライチェーン・なりすまし観点）: [[security]]
- 文体: [[japanese-style]]
