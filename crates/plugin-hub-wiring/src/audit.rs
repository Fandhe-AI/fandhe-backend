//! 越境アクセス監査ログ標準整合（TASK-9.6 / #89）。
//!
//! # 背景
//!
//! `docs/design/outbox-consent-integration.md` 6 節の 2 層設計では、データ層
//! （RLS 相当）の越境アクセスは「0 行 → 404 相当」としてフェイルクローズ遮断
//! される。存在秘匿（対象レコードの存在を漏らさない）としては正しいが、
//! 「正当な 404（リソース不存在）」と「越境 404（存在するが他テナント）」を
//! 監査ログ上で区別できない（同ドキュメント 9 節が本タスクの担当と明記）。
//! 本モジュールは micro-service-hub PoC-13 の監査ログ標準に整合し、越境試行を
//! `cross_tenant_attempt` カテゴリとして明示的に記録する仕組みを提供する。
//! **外部応答は従来どおり 404 のまま**（存在秘匿を維持）とし、内部の監査ログ
//! のみで区別可能にする（[`TenantLookupOutcome::resolve`] が挙動同一性を保証）。
//!
//! フィールド詳細は本タスク時点で確認可能な標準要素（`cross_tenant_attempt`
//! カテゴリでの明示的記録）を満たす最小スキーマとして定義する。実
//! micro-service-hub PoC-13 標準とのフィールド厳密整合の最終確認は実 hub との
//! E2E 統合検証（[#97]）で行う。
//!
//! [#97]: https://github.com/Fandhe-AI/backend-framework/issues/97
//!
//! # `TenantGate`（401/403）との境界
//!
//! [`crate::gate::TenantGate`]（`RequestGate` 拡張点）は認証失敗（トークン
//! 欠落・署名不一致・`org_id` 欠落等）を 401/403 として遮断する。越境
//! （valid JWT での他テナント資源アクセス試行）は authentication ではなく
//! authorization（データ層の所有権判定）でのみ検出できるため、`TenantGate`
//! 本体は変更しない。ゲート拒否（401/403）自体の監査カテゴリ追加は本タスクの
//! スコープ外（越境試行ではないため）。
//!
//! # lossy 経路の使用禁止
//!
//! `docs/design/tracing-integration.md`「non_blocking writer の lossy 特性」
//! 節が明記するとおり、`plugin-tracing` の non-blocking writer は有界チャネル
//! 満杯時にイベントを黙って失う（lossy）。セキュリティ監査イベント
//! （`cross_tenant_attempt` を含む）は欠落を許容できないため、[`AuditSink`]
//! 実装は `plugin-tracing` 経由の記録経路を使ってはならない
//! （利用側サービスが非 lossy な記録先を実装する）。

use serde::Serialize;
use std::sync::Mutex;

/// 監査イベントのカテゴリ。
///
/// 現時点では越境試行（`cross_tenant_attempt`）のみを扱う。将来カテゴリを
/// 追加する場合も、既存 variant の JSON 表現（`#[serde(rename)]`）は破壊的
/// 変更となるため変更しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AuditCategory {
    /// 越境アクセス試行（valid JWT で他テナント所有のリソースへアクセスを
    /// 試みた）。micro-service-hub PoC-13 の監査ログ標準に合わせ、JSON では
    /// `"cross_tenant_attempt"` として記録する。
    #[serde(rename = "cross_tenant_attempt")]
    CrossTenantAttempt,
}

/// 監査イベントの発生元・リクエスト文脈（[`AuditEvent`] 組み立ての入力）。
///
/// `path` はクエリ文字列を含みうる生の request-target を直接保持せず、
/// [`AuditContext::new`] がコンストラクタ内でクエリ文字列（`?` 以降）を
/// 必ず除去する。クエリ文字列にはトークン・API キー等の機密情報が乗ることが
/// 多い（`crates/plugin-tracing/src/layer.rs` の `record_response` と同方針、
/// .claude/rules/security.md A02）。
#[derive(Debug, Clone)]
pub struct AuditContext {
    /// JWT 検証済みのテナント識別子（`org_id` クレーム）。PII ではなく
    /// テナント ID。
    org_id: String,
    /// HTTP メソッド（例 `"GET"`）。
    method: String,
    /// クエリ文字列除去済みのリクエストパス。
    path: String,
    /// 記録元識別子（例 `"hub-tenant-gate"`・利用側サービス名）。呼び出し元が
    /// どのコンポーネントで越境を検出したかを追跡可能にする。
    source: String,
}

impl AuditContext {
    /// リクエスト文脈から `AuditContext` を組み立てる。
    ///
    /// `path` はクエリ文字列（`?` 以降）を無条件に除去する。呼び出し元は
    /// `RequestHead::target` 等の生の request-target をそのまま渡してよい
    /// （除去は本コンストラクタが必ず行うため、呼び出し元での事前除去は不要）。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_hub_wiring::audit::AuditContext;
    ///
    /// let ctx = AuditContext::new("org-a", "GET", "/widgets/42?token=secret", "hub-tenant-gate");
    /// assert_eq!(ctx.path(), "/widgets/42");
    /// ```
    pub fn new(
        org_id: impl Into<String>,
        method: impl Into<String>,
        path: impl AsRef<str>,
        source: impl Into<String>,
    ) -> Self {
        let raw_path = path.as_ref();
        let stripped = raw_path
            .split_once('?')
            .map_or(raw_path, |(path, _query)| path);
        Self {
            org_id: org_id.into(),
            method: method.into(),
            path: stripped.to_string(),
            source: source.into(),
        }
    }

    /// クエリ文字列除去済みのパスを返す（テスト・呼び出し元での確認用）。
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// 監査ログへ記録する 1 イベント（`serde::Serialize` で JSON 化可能）。
///
/// Authorization ヘッダ値・トークン文字列・JWT クレーム全文・リクエスト
/// ボディをフィールドとして一切持たない（型として持てない設計、
/// .claude/rules/security.md A02）。`path` はクエリ文字列除去済み
/// （[`AuditContext::new`] が保証）。
#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    category: AuditCategory,
    /// イベント発生時刻（UNIX epoch 秒）。
    occurred_at_unix: u64,
    org_id: String,
    method: String,
    path: String,
    source: String,
}

impl AuditEvent {
    /// 越境試行イベントを組み立てる。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_hub_wiring::audit::{AuditContext, AuditEvent};
    ///
    /// let ctx = AuditContext::new("org-a", "GET", "/widgets/42", "hub-tenant-gate");
    /// let event = AuditEvent::cross_tenant_attempt(&ctx, 1_700_000_000);
    /// assert!(event.to_json().contains("cross_tenant_attempt"));
    /// ```
    pub fn cross_tenant_attempt(ctx: &AuditContext, occurred_at_unix: u64) -> Self {
        Self {
            category: AuditCategory::CrossTenantAttempt,
            occurred_at_unix,
            org_id: ctx.org_id.clone(),
            method: ctx.method.clone(),
            path: ctx.path.clone(),
            source: ctx.source.clone(),
        }
    }

    /// イベントを JSON 文字列へシリアライズする。
    ///
    /// `serde_json` によるシリアライズのみを行うため、改行・制御文字は JSON
    /// エスケープされる（ログインジェクション防止、.claude/rules/security.md
    /// A03）。シリアライズ失敗（本型は非 UTF-8 を含まないため通常起こらない）
    /// 時は空オブジェクト `"{}"` を返しフェイルクローズする（panic を
    /// ライブラリ境界の外へ出さない、.claude/rules/coding-rust.md）。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// 監査イベントの記録先（呼び出し元への注入点）。
///
/// # 実装契約
///
/// - **非ブロッキング**: [`RequestGate::check`]（`crates/core/src/extension.rs`
///   doc）等の拡張点フック・ホットパスから呼ばれ得るため、`record` の実装は
///   同期ブロッキング I/O を行わない契約とする（Tokio ワーカーを塞がない、
///   `AGENTS.md`「規約: ミドルウェア非同期 I/O 必須化」・
///   .claude/rules/coding-rust.md）。ファイル書き込み等の実 I/O は非同期
///   チャネルへの送信に留め、実際の書き込みは別タスクで行うこと。
/// - **非 lossy**: セキュリティ監査イベントは欠落を許容できないため、
///   `plugin-tracing` の non-blocking writer（lossy、
///   `docs/design/tracing-integration.md`）を経路に使わないこと。
/// - **低頻度前提**: イベントは越境試行時のみ発火する低頻度イベントである
///   （高頻度パスでの同期コスト積み上げを心配する必要はない）。
pub trait AuditSink: Send + Sync {
    /// 監査イベントを記録する。上記契約（非ブロッキング・非 lossy）を守ること。
    fn record(&self, event: AuditEvent);
}

/// テスト・検証用の参照実装（`Mutex<Vec<AuditEvent>>` にイベントを蓄積する）。
///
/// **実運用での利用は禁止**（無制限に成長し続けるため、.claude/rules/security.md
/// リソース枯渇対策）。実運用の書き込み先（ファイル・監査サービス連携等）は
/// 利用側サービスが [`AuditSink`] を実装して提供する。
#[derive(Debug, Default)]
pub struct MemoryAuditSink {
    events: Mutex<Vec<AuditEvent>>,
}

impl MemoryAuditSink {
    /// 空の `MemoryAuditSink` を作る。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_hub_wiring::audit::MemoryAuditSink;
    ///
    /// let sink = MemoryAuditSink::new();
    /// assert_eq!(sink.len(), 0);
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// 記録済みイベント件数を返す。
    ///
    /// # Panics
    ///
    /// 内部 `Mutex` が poison 状態（他スレッドでの panic 伝播）の場合に
    /// panic する。テスト・検証専用の型であり、本番運用での使用を想定
    /// しない（上記 doc の禁止事項）ため、poison recovery は行わない。
    pub fn len(&self) -> usize {
        self.events
            .lock()
            .expect("MemoryAuditSink mutex poisoned")
            .len()
    }

    /// 記録済みイベントが 0 件かどうかを返す。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 記録済みイベントのスナップショットを返す（テストでの内容検証用）。
    pub fn events(&self) -> Vec<AuditEvent> {
        self.events
            .lock()
            .expect("MemoryAuditSink mutex poisoned")
            .clone()
    }
}

impl AuditSink for MemoryAuditSink {
    fn record(&self, event: AuditEvent) {
        self.events
            .lock()
            .expect("MemoryAuditSink mutex poisoned")
            .push(event);
    }
}

/// データ層アクセスの判定結果。
///
/// 越境を判別できる層（RLS 前の所有権判定・インメモリ実装・privileged 監査
/// クエリ等）がこの enum で結果を返し、[`Self::resolve`] が「正当な 404」と
/// 「越境 404」の外部応答を完全同一に保ちながら、監査ログのみで区別する
/// （受け入れ条件 1: `.claude/rules/out-of-scope-tracking.md` 記載元 Issue #89）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantLookupOutcome<T> {
    /// リソースが見つかり、呼び出し元テナントの所有物であることを確認できた。
    Found(T),
    /// リソースが存在しない（正当な 404。越境ではない）。
    NotFound,
    /// リソースは存在するが、呼び出し元とは異なるテナントの所有物である
    /// （越境試行）。
    CrossTenantAttempt,
}

impl<T> TenantLookupOutcome<T> {
    /// 判定結果を外部応答用の `Option<T>` へ変換しつつ、越境試行のみ監査
    /// ログへ記録する。
    ///
    /// - `Found(T)` → `Some(T)`（記録なし）
    /// - `NotFound` → `None`（記録なし）
    /// - `CrossTenantAttempt` → `None`（**`NotFound` とバイト同一の外部挙動**）
    ///   + `sink.record(..)` で `cross_tenant_attempt` イベントを 1 件記録
    ///
    /// `occurred_at_unix` は呼び出し元が計測した UNIX epoch 秒を渡す（本関数は
    /// I/O なし・同期のみのため時刻取得を内部で行わない。呼び出し元が
    /// `RequestGate::check` と同様のフェイルクローズなクロック取得
    /// （`crates/plugin-hub-wiring/src/gate.rs` の `now_unix` 算出と同型）を
    /// 行う想定）。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_plugin_hub_wiring::audit::{AuditContext, AuditSink, MemoryAuditSink, TenantLookupOutcome};
    ///
    /// let sink = MemoryAuditSink::new();
    /// let ctx = AuditContext::new("org-b", "GET", "/widgets/1", "hub-tenant-gate");
    ///
    /// // 正当な 404: 記録されない。
    /// let not_found: TenantLookupOutcome<&str> = TenantLookupOutcome::NotFound;
    /// assert_eq!(not_found.resolve(&sink, &ctx, 0), None);
    /// assert_eq!(sink.len(), 0);
    ///
    /// // 越境 404: 戻り値は同じ None だが監査ログに 1 件記録される。
    /// let cross_tenant: TenantLookupOutcome<&str> = TenantLookupOutcome::CrossTenantAttempt;
    /// assert_eq!(cross_tenant.resolve(&sink, &ctx, 0), None);
    /// assert_eq!(sink.len(), 1);
    /// ```
    pub fn resolve(
        self,
        sink: &dyn AuditSink,
        ctx: &AuditContext,
        occurred_at_unix: u64,
    ) -> Option<T> {
        match self {
            TenantLookupOutcome::Found(value) => Some(value),
            TenantLookupOutcome::NotFound => None,
            TenantLookupOutcome::CrossTenantAttempt => {
                sink.record(AuditEvent::cross_tenant_attempt(ctx, occurred_at_unix));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_json_contains_cross_tenant_attempt_category() {
        let ctx = AuditContext::new("org-a", "GET", "/widgets/1", "hub-tenant-gate");
        let event = AuditEvent::cross_tenant_attempt(&ctx, 42);
        assert!(event.to_json().contains("\"cross_tenant_attempt\""));
    }

    #[test]
    fn to_json_never_contains_token_or_authorization_header_content() {
        // イベント JSON にトークン文字列・Authorization ヘッダ値が現れない
        // ことの否定検証（`gate.rs` の `reject_body_never_reflects_token_content`
        // と同型、.claude/rules/security.md A02）。
        let token = "eyJhbGciOiJSUzI1NiJ9.super-secret-claims-payload.signature-bytes";
        let ctx = AuditContext::new("org-a", "GET", "/widgets/1", "hub-tenant-gate");
        let event = AuditEvent::cross_tenant_attempt(&ctx, 42);
        let json = event.to_json();
        assert!(!json.contains(token));
        assert!(!json.contains("Authorization"));
        assert!(!json.contains("Bearer"));
    }

    #[test]
    fn context_new_strips_query_string_from_path() {
        let ctx = AuditContext::new(
            "org-a",
            "GET",
            "/widgets/42?token=secret",
            "hub-tenant-gate",
        );
        assert_eq!(ctx.path(), "/widgets/42");
    }

    #[test]
    fn context_new_keeps_path_without_query_string_unchanged() {
        let ctx = AuditContext::new("org-a", "GET", "/widgets/42", "hub-tenant-gate");
        assert_eq!(ctx.path(), "/widgets/42");
    }

    #[test]
    fn resolve_not_found_records_nothing_and_returns_none() {
        let sink = MemoryAuditSink::new();
        let ctx = AuditContext::new("org-a", "GET", "/widgets/1", "hub-tenant-gate");
        let outcome: TenantLookupOutcome<&str> = TenantLookupOutcome::NotFound;
        assert_eq!(outcome.resolve(&sink, &ctx, 0), None);
        assert_eq!(sink.len(), 0);
    }

    #[test]
    fn resolve_cross_tenant_attempt_records_exactly_one_event_and_returns_none() {
        let sink = MemoryAuditSink::new();
        let ctx = AuditContext::new("org-b", "GET", "/widgets/1", "hub-tenant-gate");
        let outcome: TenantLookupOutcome<&str> = TenantLookupOutcome::CrossTenantAttempt;
        assert_eq!(outcome.resolve(&sink, &ctx, 1_700_000_000), None);
        assert_eq!(sink.len(), 1);
        let events = sink.events();
        assert_eq!(events[0].org_id, "org-b");
        assert_eq!(events[0].category, AuditCategory::CrossTenantAttempt);
    }

    #[test]
    fn resolve_found_returns_value_and_records_nothing() {
        let sink = MemoryAuditSink::new();
        let ctx = AuditContext::new("org-a", "GET", "/widgets/1", "hub-tenant-gate");
        let outcome = TenantLookupOutcome::Found("widget-payload");
        assert_eq!(outcome.resolve(&sink, &ctx, 0), Some("widget-payload"));
        assert_eq!(sink.len(), 0);
    }

    #[test]
    fn not_found_and_cross_tenant_attempt_yield_identical_external_outcome() {
        // 受け入れ条件 1 の型レベル固定: 外部から見える戻り値（`Option<T>`）は
        // `NotFound` と `CrossTenantAttempt` で完全に同一（両方 `None`）であり、
        // 監査ログの記録件数のみが異なることを直接比較する。
        let sink_not_found = MemoryAuditSink::new();
        let sink_cross_tenant = MemoryAuditSink::new();
        let ctx = AuditContext::new("org-b", "GET", "/widgets/1", "hub-tenant-gate");

        let not_found: TenantLookupOutcome<&str> = TenantLookupOutcome::NotFound;
        let cross_tenant: TenantLookupOutcome<&str> = TenantLookupOutcome::CrossTenantAttempt;

        let not_found_result = not_found.resolve(&sink_not_found, &ctx, 0);
        let cross_tenant_result = cross_tenant.resolve(&sink_cross_tenant, &ctx, 0);

        assert_eq!(not_found_result, cross_tenant_result);
        assert_eq!(sink_not_found.len(), 0);
        assert_eq!(sink_cross_tenant.len(), 1);
    }
}
