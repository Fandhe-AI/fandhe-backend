# Outbox・同意ゲート 実データモデル統合設計

TASK-9.4（#64、REQ-9）対応。`fandhe-backend-plugin-hub-wiring` が概念実証した Outbox パターン・
同意ゲートを、`micro-service-hub` の Outbox Relay（ポーリング配送）・同意管理サービス
（PostgreSQL の実データモデル）と統合するための設計を記録する。

## 1. 目的と位置付け

### 1.1 対応関係

- **タスク**: `docs/spec/05-tasks.md` TASK-9.4「Outbox・同意ゲートの micro-service-hub
  実データモデルとの統合設計」
- **Issue**: [#64](https://github.com/Fandhe-AI/backend-framework/issues/64)
- **要件**: `docs/spec/04-requirements.md` REQ-9（hub 共通配線ミドルウェア、Should）。
  特に L225〜227（Outbox・同意ゲートを PoC-6 のスパイクに留め、MVP では実データモデル
  との統合設計を TASK-9.4 で行うと定めた記述）
- **前提タスク**: TASK-9.1（[#61](https://github.com/Fandhe-AI/backend-framework/issues/61)、
  PR #152 で main にマージ済み）。`crates/plugin-hub-wiring` に依存逆転型プラグインとして
  `TenantGate`（`RequestGate` 拡張点）が存在する

### 1.2 成果物の範囲（設計のみ）

本ドキュメントは TASK-9.4 の成果物である**統合設計ドキュメント**である。次は本ドキュメントの
スコープに**含まない**（9 節「関連タスクとの境界」参照）。

- `OutboxStore` / `ConsentStore` trait の実コード実装（方針のみを本書で確定する）
- `micro-service-hub` 側の Outbox Relay・同意管理サービスとの実 API/スキーマ結線
- 実データモデルを用いた E2E 統合検証

### 1.3 完了判定の 2 段階（MS-6 完了基準への対応）

`docs/spec/06-roadmap.md` MS-6 節は、本マイルストーンの実施時期（2026-09-16〜09-24）が
`micro-service-hub` の Outbox Relay 完了見込み（2026-09-30、`micro-service-hub` MS-5）より
**前**になるため、TASK-9.4 の完了判定を「設計完了」と「E2E 統合検証完了」の 2 段階に分けると
明記している。本ドキュメントはこの分割に従う。

| 段階 | 内容 | 完了条件 | 状態 |
|------|------|---------|------|
| 設計完了 | 本ドキュメントによる統合設計の確定 | PR がレビュー通過・main へマージされること | 本 Issue（#64）のスコープ |
| E2E 統合検証完了 | 実 PostgreSQL・実 `micro-service-hub` Outbox Relay/同意管理サービスとの結線検証 | 越境 0 行・同意フィルタ・Relay 配送の実測確認 | [#97](https://github.com/Fandhe-AI/backend-framework/issues/97)（`micro-service-hub` Outbox Relay 完了、目標 2026-09-30 以降に実施） |

MS-6 完了基準「Outbox・同意ゲートの実データモデルとの統合設計が TASK-9.4 で完了し、
`micro-service-hub` 側の E2E 検証タイミングが明記されている」は、本ドキュメントの提出
（設計完了）と本節の記述（E2E 検証タイミング = #97、2026-09-30 以降）の両方をもって満たす。

## 2. 現状（PoC-6 スパイク）の整理

`docs/spec/03-poc/hub-wiring-middleware/`（[PoC-6 README](../spec/03-poc/hub-wiring-middleware/README.md)）
が概念実証したインメモリ実装の契約を要約する。実コードは
[`core/src/plugins/hub_tenant.rs`](../spec/03-poc/hub-wiring-middleware/core/src/plugins/hub_tenant.rs)
（PoC 用スパイク実装、TASK-9.1 でマージ済みの `crates/plugin-hub-wiring` とは別物）。

### 2.1 `Outbox`（追記専用イベントログ）

- `enqueue(event: OutboxEvent)`: イベントを末尾に追記する（`Mutex<Vec<OutboxEvent>>`）
- `list_for_org(org_id: Option<&str>) -> Vec<OutboxEvent>`: 指定テナントのイベントのみを
  返す。**`org_id` が `None` の場合は常に空集合を返す**（テナントコンテキスト欠落時の
  フェイルクローズ、PoC-6 README「hub 仕様の読み取り結果」表 3 行目）
- `OutboxEvent` は `id` / `org_id` / `event_type` / `payload` の 4 フィールドを持つ

### 2.2 `ConsentGate`（同意ゲート）

- `(org_id, service, info_type)` 単位でオプトイン状態（真偽値）を保持する
- `filter_fields(org_id, service, fields) -> Vec<Field>`: 同意済みフィールドのみを抽出する
  （オプトイン原則、デフォルト非共有）。同意皆無のテナントは全フィールド除外

### 2.3 継承する不変条件

実データモデル統合後も以下は変更しない（4・7 節で PostgreSQL 実装への引き継ぎ方法を記述）。

1. テナントコンテキスト欠落時は Outbox・同意ゲートともに常に空集合/全拒否を返す
2. 同意ゲートはオプトイン原則（デフォルト非共有）を既定動作とする
3. コアは Outbox・同意ゲート固有のシンボルへ一切依存しない（依存方向は
   `fandhe-backend-plugin-hub-wiring` → コアの一方向、[`plugin-boundary.md` 5.6 節](./plugin-boundary.md)）

## 3. 統合先（`micro-service-hub`）の前提

`docs/spec/06-roadmap.md` MS-6 節の外部依存記述に基づく。実 API/スキーマは本ドキュメント
執筆時点で未確定であり、**着手前に `micro-service-hub` 側の進捗を確認する運用**
（roadmap MS-6 節「着手前に `micro-service-hub` 側の進捗を必ず確認し」）を維持する。
以下は roadmap に明記された確定情報のみを記載し、それ以外（テーブル DDL・カラム名・
API エンドポイント URL 等の詳細）は 11 節「未決事項」に切り出し、確定情報として扱わない。

| 統合先 | 役割 | `micro-service-hub` 側の対応 REQ/マイルストーン | 目標完了時期 |
|--------|------|---------------------------------------------|------------|
| Outbox Relay | Outbox テーブルをポーリングし配送する常駐プロセス | REQ-4 / MS-5 | 2026-09-30 |
| 同意管理サービス | `(org_id, service, info_type)` 同意状態を PostgreSQL で永続化するサービス | REQ-2 / MS-3 | 2026-08-31 |

`fandhe-backend-plugin-hub-wiring` はいずれのサービスとも直接通信しない（4 節の trait 境界により、
実データモデルとの結線は利用側サービス・別クレートが担う）。

## 4. trait 境界設計（依存逆転の維持）

TASK-9.1 で確立した依存逆転型パターン（[`plugin-boundary.md` 5.6 節](./plugin-boundary.md)）を
維持したまま実データモデルに対応するため、`fandhe-backend-plugin-hub-wiring` 側にストレージ抽象を
定義する方針とする。

### 4.1 方針

```rust
// crates/plugin-hub-wiring/src/outbox.rs（設計方針、未実装）
//
// コアは本 trait を一切知らない。`fandhe-backend-plugin-hub-wiring` を依存に加えた
// 利用側サービスが、この trait の実装（インメモリ or PostgreSQL）を
// 選択して注入する。
pub trait OutboxStore: Send + Sync {
    /// テナントスコープ済みイベントを追記する。
    /// 実装は「業務書き込みと同一トランザクション」で呼ばれることを想定する（5.3 節）。
    fn enqueue(&self, event: OutboxEvent) -> Result<(), OutboxError>;

    /// `org_id` が `None` の場合は常に空集合を返す（フェイルクローズ、7 節）。
    fn list_for_org(&self, org_id: Option<&str>) -> Result<Vec<OutboxEvent>, OutboxError>;
}

// crates/plugin-hub-wiring/src/consent.rs（設計方針、未実装）
pub trait ConsentStore: Send + Sync {
    /// `(org_id, service, info_type)` の同意状態を判定する。
    /// 判定不能（未登録テナント等）はオプトイン原則により拒否側に倒す。
    fn is_granted(&self, org_id: &str, service: &str, info_type: &str) -> Result<bool, ConsentError>;

    /// 同意取り消し時に呼び出す。実装は Outbox への `consent_revoked` イベント
    /// 発火を責務に含めてよい（8 節）。
    fn revoke(&self, org_id: &str, service: &str, info_type: &str) -> Result<(), ConsentError>;
}
```

- インメモリ実装（PoC-6 相当、`Mutex<Vec<...>>` / `Mutex<HashMap<...>>`）はテスト・スパイク
  用として `fandhe-backend-plugin-hub-wiring` 内に残す
- PostgreSQL 実装は、利用側サービス（`micro-service-hub` の hub 基幹）または独立した
  アダプタクレートが `OutboxStore` / `ConsentStore` を実装して提供する。これにより
  `fandhe-backend-plugin-hub-wiring` は `sqlx`/`tokio-postgres` 等の DB クライアントに依存しない
  （pay-for-what-you-use、[`pay-for-what-you-use.md`](../../.claude/rules/pay-for-what-you-use.md)）
- コア（`crates/core`）は本 trait 群を一切知らない。依存方向は
  `PostgreSQL 実装（利用側） → OutboxStore/ConsentStore（fandhe-backend-plugin-hub-wiring） ← インメモリ実装（テスト用）`
  であり、`fandhe-backend-plugin-hub-wiring` → コアの既存の依存逆転（5.6 節）とは独立した、
  プラグイン内部の trait 境界として設計する

### 4.2 同期 API 制約との関係

`RequestGate::check`（`TenantGate` が実装する既存拡張点）は同期 API 制約を持つ
（[`plugin-boundary.md` 5.6.1 節](./plugin-boundary.md)）。`OutboxStore::enqueue` /
`ConsentStore::is_granted` は `RequestGate::check` から直接呼ばれる想定ではなく、
`TenantGate` によるゲート判定通過**後**にハンドラ層（利用側サービスのエンドポイント
実装）から呼ばれる想定のため、この制約の対象外である。ただし、実装フェーズで
非同期 DB クライアント（`sqlx` 等）を用いる場合、trait のシグネチャを `async fn`
（`async-trait` または native async trait）にする必要があり、これは `RequestGate`
とは別の trait であるため既存拡張点の同期制約に抵触しない。

## 5. データモデルマッピング

### 5.1 Outbox

| PoC-6 インメモリ構造 | PostgreSQL 実データモデル（想定、要 `micro-service-hub` 側確認） | 対応方針 |
|---|---|---|
| `OutboxEvent { id, org_id, event_type, payload }` | `outbox` テーブル + 配送状態列（例: `delivered_at`, `retry_count`） | `id`/`org_id`/`event_type`/`payload` は PoC-6 の型をそのまま踏襲し、配送状態列は Relay 側の責務として `OutboxStore` の契約外に置く |
| `Mutex<Vec<OutboxEvent>>`（プロセス内） | 永続化テーブル（プロセスをまたいで共有） | `OutboxStore` trait を介した差し替えにより吸収 |
| `list_for_org(Option<&str>)` | `SELECT ... WHERE org_id = $1`（`org_id` 必須のパラメータクエリ） | `None` 時はクエリを発行せず空集合を即返す（7 節、文字列連結 SQL は禁止） |

### 5.2 同意ゲート

| PoC-6 インメモリ構造 | PostgreSQL 実データモデル（想定、要 `micro-service-hub` 側確認） | 対応方針 |
|---|---|---|
| `(org_id, service, info_type) -> bool`（`HashMap` 相当） | 同意管理サービスの `consent_grants` テーブル（`micro-service-hub` REQ-2 が定義） | `fandhe-backend-plugin-hub-wiring` は同意管理サービスのテーブルへ直接アクセスしない。`ConsentStore` の PostgreSQL 実装（利用側）が同意管理サービスの API またはテーブルを参照する |
| `filter_fields(org_id, service, fields)` | 同意管理サービスへの参照結果を用いたフィールドフィルタ | ロジックは `fandhe-backend-plugin-hub-wiring` 側に残し、`ConsentStore::is_granted` の呼び出し結果のみを利用する（判定ロジックと永続化を分離） |

### 5.3 トランザクション境界と責務分界

- **業務書き込みと Outbox `enqueue` の同一トランザクション化**: `micro-service-hub` の
  Outbox パターンの前提（`docs/spec/03-poc/hub-wiring-middleware/README.md` の
  「hub 仕様の読み取り結果」表、`data-ownership-propagation` PoC-3 由来）に従い、
  `OutboxStore::enqueue` の PostgreSQL 実装は呼び出し元の業務トランザクション内で実行する
  契約とする。`fandhe-backend-plugin-hub-wiring` はトランザクション管理そのものを担わず、呼び出し元
  （利用側サービス）がコネクション/トランザクションコンテキストを `OutboxStore` 実装へ
  渡す設計とする
- **配送責務は Outbox Relay 側**: `fandhe-backend-plugin-hub-wiring`・`OutboxStore` の責務は
  `enqueue`（追記）までであり、ポーリング配送・リトライ・配送状態管理は
  `micro-service-hub` の Outbox Relay の責務である。この境界を越えない

## 6. 拒否表現の 2 層設計（PoC-13 知見の踏襲）

TASK-9.4 の受け入れ基準の中心となる設計。`micro-service-hub` PoC-13
（`app-layer-security-wiring`）の知見（PoC-6 README「hub 仕様の読み取り結果」表 1 行目、
発見事項4）を実データモデル統合設計に引き継ぐ。

### 6.1 2 層の役割分担

| 層 | 実装 | 拒否表現 | 目的 |
|----|------|---------|------|
| アプリ層 | `TenantGate`（`RequestGate::check` → `GateOutcome::Reject`） | 401（トークン欠落・改竄・期限切れ）/ 403（`org_id` クレーム欠落） | クライアント入力の誤り検知用の明示的拒否。エラー原因をクライアントへ返せる |
| データ層 | PostgreSQL RLS（`FORCE ROW LEVEL SECURITY`）+ `SET LOCAL app.current_org_id` | 越境行は 0 行（クエリ結果が空集合）/ 呼び出し元でこれを 404 相当として扱う | 越境アクセスの試行自体を「対象が存在しない」として扱うフェイルクローズ。アプリ層の判定漏れがあってもデータ層で遮断する多層防御 |

PoC-6 は単一レイヤー（インメモリ `MockDb`）で両者を統合していたため 404 に一本化していたが
（PoC-6 README 発見事項4）、実データモデル統合では `micro-service-hub` PoC-13 の設計に従い
**明示的に分離する**。

### 6.2 Outbox・同意ゲートへの適用

- `OutboxStore::list_for_org`: アプリ層では `TenantGate` 通過済み（= `org_id` 確定済み）の
  場合のみ呼び出される。データ層では、実装が PostgreSQL 経由なら RLS により越境行は
  クエリ結果に現れない（0 行）。`org_id: None` を渡すような呼び出し（テナントコンテキスト
  欠落）はアプリ層側で発生させない契約とし、万一発生した場合も trait 実装側で空集合を
  返すフェイルクローズを維持する（7 節）
- `ConsentStore::is_granted`: 越境（他テナントの同意状態を参照しようとする呼び出し）は
  同意管理サービス側の RLS 相当の境界強制で遮断される想定。`fandhe-backend-plugin-hub-wiring` 側は
  `is_granted` の呼び出し引数に `org_id` を必須パラメータとして持たせ、暗黙のデフォルト
  テナントを持たせない

### 6.3 単一層の失敗が越境に直結しない設計（OWASP A01 対応）

アプリ層（`TenantGate`）とデータ層（RLS）を分離することで、片方の実装ミス・設定漏れが
即座に越境アクセスへ直結しない多層防御を構成する。データ層の RLS 適用漏れ検知は
11 節・E2E 検証計画（#97）に引き継ぐ。

## 7. フェイルクローズ規約の実データモデルへの引き継ぎ

PoC-6 の不変条件（テナントコンテキスト欠落時は常に空集合/拒否）を、PostgreSQL 実装で
どう保証するかを規約として定める。

- **RLS ポリシー**: `outbox` テーブル・同意管理サービスの参照テーブルに
  `FORCE ROW LEVEL SECURITY` を適用し、`app.current_org_id`（`SET LOCAL`）未設定時は
  ポリシーが恒常的に偽となるよう設計する（`multitenancy-org-isolation` PoC-4 の設計思想、
  PoC-6 README 表 2 行目を踏襲）
- **クエリ規約**: `org_id: Option<&str>` が `None` の場合、`OutboxStore` 実装は
  **クエリを発行する前に**空集合を返す（PoC-6 の `MockDb::query_account` と同型の設計、
  データベースラウンドトリップ自体を発生させないことで RLS 設定ミスへの依存を減らす
  二重の防御とする）
- **パラメータ化クエリの徹底（OWASP A03 対応）**: `Outbox` `payload`（JSON）・同意テーブル
  参照はプレースホルダ付きクエリ（`sqlx` のコンパイル時検証クエリ等）または型付き ORM 経由
  とし、文字列連結による SQL 構築を禁止する規約を明記する。実装フェーズのレビューで
  `format!`/`+` による SQL 文字列組み立てがないことを確認する

## 8. 同意ゲートの実装方針

- **オプトイン原則の維持**: デフォルト非共有を PostgreSQL 実装後も既定動作とする。
  同意管理サービスへの参照が失敗（接続エラー・タイムアウト）した場合も、
  フェイルオープン（誤って共有）ではなく**フェイルクローズ（同意なしとして扱う）**を
  既定とする
- **キャッシュと失効の整合**: `filter_fields` 相当の判定を同意管理サービスへの都度参照
  にすると、TASK-9.3（JWT 検証キャッシュ）と同様にレイテンシ・負荷の課題が生じうる。
  キャッシュを導入する場合は、同意管理サービス側が発行する `consent_revoked` イベント
  （`micro-service-hub` REQ-2/PoC-2 の同意管理モデルに由来）との整合を取り、キャッシュ
  失効の遅延がフェイルオープンに転じないよう設計する必要がある（キャッシュ導入の要否・
  具体的な失効方式は 11 節「未決事項」に切り出す）
- **同意取り消し時の Outbox イベント発火**: `ConsentStore::revoke` 呼び出し時、
  同意取り消しを他サービスへ伝播する必要がある場合は `OutboxStore::enqueue` で
  `consent_revoked` イベントを発火する（4.1 節の trait シグネチャに反映済み）。
  これにより、同意取り消しの伝播も既存の Outbox パターンに統一される

## 9. 関連タスクとの境界

| タスク/Issue | 責務 | 本ドキュメントとの関係 |
|---|---|---|
| TASK-9.2（[#62](https://github.com/Fandhe-AI/backend-framework/issues/62)） | JWT 検証を RS256 + JWKS 連携へ差し替える | 参照のみ。本ドキュメントは `TenantGate` の認証方式自体を変更しない |
| TASK-9.3（[#63](https://github.com/Fandhe-AI/backend-framework/issues/63)） | JWT 検証結果のリクエストスコープキャッシュ最適化 | 参照のみ。8 節の同意ゲートキャッシュとは対象が異なる（認証結果 vs 同意状態） |
| TASK-9.5（[#65](https://github.com/Fandhe-AI/backend-framework/issues/65)） | hub 共通配線受け入れテスト | 本ドキュメントが定めた trait 境界・拒否表現設計を前提に、越境遮断率・削減率を検証する |
| TASK-9.6（[#89](https://github.com/Fandhe-AI/backend-framework/issues/89)） | `cross_tenant_attempt` 監査ログ実装（実装済み: `crates/plugin-hub-wiring/src/audit.rs`） | 境界のみ。越境試行の監査ログ記録は本ドキュメントのスコープ外（10 節で規約参照のみ言及）。フィールド詳細は本タスク時点で確認可能な最小スキーマとして定義し、実 micro-service-hub PoC-13 標準との厳密整合の最終確認は E2E 統合検証（#97）で行う |
| E2E 統合検証（[#97](https://github.com/Fandhe-AI/backend-framework/issues/97)） | 実 PostgreSQL・実 `micro-service-hub` サービスとの結線検証 | 本ドキュメントの設計を前提に、`micro-service-hub` Outbox Relay 完了（2026-09-30 以降）後に実施 |

`OutboxStore` / `ConsentStore` trait の実コード実装自体は、本ドキュメントでは方針確定に
留める。実装が必要になる場合は TASK-9.5（#65）または後続タスクで判断する
（out-of-scope-tracking 対象、下記「対象外」参照）。

## 10. セキュリティ考慮（OWASP Top 10）

- **A01 アクセス制御の不備**: 6 節の 2 層設計（アプリ層 401/403、データ層 RLS フェイル
  クローズ 0 行）により、単一層の失敗が越境に直結しない設計とする。テナントコンテキスト
  欠落時は常に空集合（既定拒否）
- **A02 暗号化の失敗 / シークレット管理**: 本ドキュメントに実鍵・接続文字列・実トークンを
  一切記載しない（記載箇所はすべてプレースホルダ）。JWT 方式の詳細は TASK-9.2（#62）に
  委ね、`fandhe-backend-plugin-hub-wiring` の現行 HS256 実装が本番流用不可のスパイクであることは
  `crates/plugin-hub-wiring/src/lib.rs` の doc comment 既述のとおり本ドキュメントでも
  前提として維持する
- **A03 インジェクション**: 7 節「クエリ規約」で、パラメータ化クエリの徹底・文字列連結
  SQL の禁止を明記した
- **A04 安全でない設計**: フェイルクローズ（デフォルト拒否）・オプトイン原則（デフォルト
  非共有）を全レイヤーの既定とする。フェイルオープンになり得る箇所（同意キャッシュの
  失効遅延、Outbox Relay 障害時の挙動）は 11 節「未決事項」に明示した
- **A05 設定ミス**: RLS ポリシー・`SET LOCAL` の適用漏れの検知は、コンパイル時に機械
  確認できない性質のものであるため、E2E 検証計画（#97）の検証項目に引き継ぐ（11 節）
- **A09 ログと監視の不備**: 越境試行の監査ログ（`cross_tenant_attempt`）記録は
  TASK-9.6（#89）の担当である旨を 9 節で境界として明記した。ログに機密（トークン・PII）
  を含めない規約は [`security.md`](../../.claude/rules/security.md) を参照
- **リソース枯渇（DoS）**: Outbox 追記の無制限成長・ポーリング負荷への対策（保持期間・
  上限設定）は本ドキュメントでは確定せず、11 節「未決事項」に記録する

## 11. 未決事項・E2E 検証計画

### 11.1 `micro-service-hub` 側 API/スキーマ確定待ちの項目

- `outbox` テーブルの実カラム定義（配送状態列の具体的な名称・型）
- 同意管理サービスの `consent_grants` 相当テーブルの実スキーマ・アクセス方式（直接
  テーブル参照か API 経由か）
- `consent_revoked` イベントの実際のペイロード形式・配信経路
- 同意ゲートキャッシュの要否・具体的な失効方式（8 節）
- Outbox の保持期間・上限設定方針（リソース枯渇対策、10 節）

これらは `micro-service-hub` 側の該当マイルストーン（同意管理サービス: MS-3、目標
2026-08-31。Outbox Relay: MS-5、目標 2026-09-30）の進捗確認を経て確定する
（roadmap MS-6 節の運用に従う）。

### 11.2 E2E 統合検証計画（[#97](https://github.com/Fandhe-AI/backend-framework/issues/97)）

`micro-service-hub` Outbox Relay 完了（2026-09-30 以降）後に、以下を実 PostgreSQL・実
`micro-service-hub` サービスとの結線で検証する。

| 検証項目 | 検証方法 |
|---|---|
| 越境アクセス時の 0 行（RLS フェイルクローズ） | 実 PostgreSQL に対し、越境クエリを実行し 0 行を確認（PoC-6 と同型のテストケースを実データで再実行） |
| 同意フィルタの実データ整合 | 実同意管理サービスの `consent_grants` に対する `filter_fields` 相当の判定結果を確認 |
| Outbox Relay 配送 | `enqueue` したイベントが Relay によりポーリング配送されることを確認 |
| RLS ポリシー・`SET LOCAL` の適用漏れ検知 | 10 節 A05 で申し送った設定ミス検知を、実運用環境で検証 |

## 対象外（out-of-scope-tracking）

以下は本ドキュメント・対応 PR のスコープに含めない（いずれも既存 Issue で追跡済み）。

- 実データモデルとの E2E 統合検証 → [#97](https://github.com/Fandhe-AI/backend-framework/issues/97)（`micro-service-hub` Outbox Relay 完了待ち）
- RS256 + JWKS への差し替え → [#62](https://github.com/Fandhe-AI/backend-framework/issues/62)（TASK-9.2）
- JWT 検証結果キャッシュ → [#63](https://github.com/Fandhe-AI/backend-framework/issues/63)（TASK-9.3）
- `cross_tenant_attempt` 監査ログ実装 → [#89](https://github.com/Fandhe-AI/backend-framework/issues/89)（TASK-9.6）
- `OutboxStore` / `ConsentStore` trait の実コード実装（本書は方針のみを確定。実装は
  TASK-9.5（#65）または後続タスクで判断する）
