//! Outbox（追記専用イベントログ）ストレージ抽象（TASK-9.4 / #64 §4.1、実装は #243）。
//!
//! `docs/design/outbox-consent-integration.md` §4.1 が確定した trait 境界を
//! そのまま実装する。コアは本 trait を一切知らない。
//! `fandhe-backend-plugin-hub-wiring` を依存に加えた利用側サービスが、この
//! trait の実装（インメモリ or PostgreSQL）を選択して注入する
//! （依存方向: `PostgreSQL 実装（利用側）→ OutboxStore（本クレート）←
//! インメモリ実装（本モジュール、テスト用）`）。
//!
//! # `RequestGate::check` との関係
//!
//! [`crate::gate::TenantGate`]（`RequestGate` 拡張点）の判定通過**後**に、
//! ハンドラ層（利用側サービスのエンドポイント実装）から呼ばれる想定であり、
//! `RequestGate::check` の同期 API 制約（`crates/core/src/extension.rs` doc）
//! の対象外（同ドキュメント §4.2）。本トレイト自体は同期 `fn` として設計
//! されている（§4.1 のシグネチャに合わせたもの。PostgreSQL 実装で非同期 DB
//! クライアントを用いる場合はシグネチャを `async fn` へ変更する必要がある
//! 旨が §4.2 に記載されているが、本クレートに実装するインメモリ版は
//! I/O を伴わないため、本タスクでは §4.1 の同期シグネチャをそのまま採用する）。
//!
//! # トランザクション境界
//!
//! [`OutboxStore::enqueue`] の PostgreSQL 実装は、呼び出し元の業務
//! トランザクション内で実行する契約とする（§5.3）。本クレートはトランザク
//! ション管理そのものを担わない。
//!
//! # pay-for-what-you-use
//!
//! 本モジュールは `sqlx`/`tokio-postgres` 等の DB クライアントに一切依存
//! しない。インメモリ実装（[`InMemoryOutboxStore`]）は既存依存（`std`
//! のみ）で完結し、新規依存を追加しない。

use std::sync::Mutex;

/// Outbox に追記する 1 イベント。
///
/// フィールドは `id` / `org_id` / `event_type` / `payload` の 4 つのみ
/// （§4.1・§5.1）。配送状態列（`delivered_at`・`retry_count` 等）は Outbox
/// Relay（`micro-service-hub` 側）の責務として本型の契約外に置く。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEvent {
    /// イベント識別子。一意性の強制は実装（呼び出し元）の責務とする。
    pub id: String,
    /// テナント識別子。[`OutboxStore::list_for_org`] のスコープ判定に使う。
    pub org_id: String,
    /// イベント種別（例 `"consent_revoked"`）。
    pub event_type: String,
    /// イベント本体（JSON 文字列等）。本型は内容を解釈せず不透明に扱う。
    pub payload: String,
}

/// [`OutboxStore`] 操作の失敗理由。
///
/// 現時点ではテナントコンテキスト欠落のみを区別する。PostgreSQL 実装が
/// 追加する接続エラー等は、利用側サービスが独自のエラー型でラップして
/// 扱う想定（本 enum に他クレートから variant を追加することはできない
/// ため。本クレートは実装を確定しない、§4・§9「対象外」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxError {
    /// `org_id` が指定されない、または空文字列（テナントコンテキスト欠落）。
    /// [`OutboxStore::list_for_org`] はこの場合エラーではなく空集合
    /// （`Ok(vec![])`）を返す契約のため、本 variant は
    /// [`InMemoryOutboxStore`] では未使用（将来の PostgreSQL 実装が
    /// 接続前検証で使う余地を残すために定義のみ行う）。
    MissingOrgId,
}

/// Outbox（追記専用イベントログ）ストレージ抽象。
///
/// `docs/design/outbox-consent-integration.md` §4.1 のシグネチャそのもの。
/// 実装はテナントコンテキスト欠落時に常に空集合を返すフェイルクローズを
/// 守ること（§7、`list_for_org` の doc 参照）。
pub trait OutboxStore: Send + Sync {
    /// テナントスコープ済みイベントを追記する。
    /// 実装は「業務書き込みと同一トランザクション」で呼ばれることを想定する
    /// （§5.3）。
    fn enqueue(&self, event: OutboxEvent) -> Result<(), OutboxError>;

    /// 指定テナントのイベントのみを返す。
    ///
    /// `org_id` が `None` の場合は常に空集合を返す（テナントコンテキスト
    /// 欠落時のフェイルクローズ、§7「クエリ規約」。PostgreSQL 実装は
    /// クエリを発行する前に空集合を返すこと）。
    fn list_for_org(&self, org_id: Option<&str>) -> Result<Vec<OutboxEvent>, OutboxError>;
}

/// テスト・スパイク用のインメモリ [`OutboxStore`] 実装（PoC-6 相当、§4.1）。
///
/// `Mutex<Vec<OutboxEvent>>` に追記するのみで、プロセスをまたいだ永続化・
/// 配送は行わない。PostgreSQL 実装は利用側サービスまたは独立アダプタ
/// クレートが提供する（本クレートに sqlx 等を追加しない、
/// .claude/rules/pay-for-what-you-use.md）。
///
/// **実運用での利用は禁止**（無制限に成長し続けるため、
/// .claude/rules/security.md リソース枯渇対策。[`crate::audit::MemoryAuditSink`]
/// と同型の注意事項）。
#[derive(Debug, Default)]
pub struct InMemoryOutboxStore {
    events: Mutex<Vec<OutboxEvent>>,
}

impl InMemoryOutboxStore {
    /// 空の `InMemoryOutboxStore` を作る。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::outbox::{InMemoryOutboxStore, OutboxStore};
    ///
    /// let store = InMemoryOutboxStore::new();
    /// assert_eq!(store.list_for_org(Some("org-a")).unwrap().len(), 0);
    /// ```
    pub fn new() -> Self {
        Self::default()
    }
}

impl OutboxStore for InMemoryOutboxStore {
    /// # Panics
    ///
    /// 内部 `Mutex` が poison 状態（他スレッドでの panic 伝播）の場合に
    /// panic する。テスト・スパイク専用の型であり、本番運用での使用を
    /// 想定しない（上記 doc の禁止事項）ため、poison recovery は行わない
    /// （[`crate::audit::MemoryAuditSink`] と同方針）。
    fn enqueue(&self, event: OutboxEvent) -> Result<(), OutboxError> {
        self.events
            .lock()
            .expect("InMemoryOutboxStore mutex poisoned")
            .push(event);
        Ok(())
    }

    /// `org_id` が `None` または空文字列の場合は、`Vec` を走査する前に
    /// 空集合を返す（§7「クエリ規約」のフェイルクローズをインメモリ実装
    /// でも踏襲する）。
    ///
    /// # Panics
    ///
    /// [`Self::enqueue`] と同様、内部 `Mutex` が poison 状態の場合に panic
    /// する。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::outbox::{
    ///     InMemoryOutboxStore, OutboxEvent, OutboxStore,
    /// };
    ///
    /// let store = InMemoryOutboxStore::new();
    /// store
    ///     .enqueue(OutboxEvent {
    ///         id: "evt-1".to_string(),
    ///         org_id: "org-a".to_string(),
    ///         event_type: "consent_revoked".to_string(),
    ///         payload: "{}".to_string(),
    ///     })
    ///     .unwrap();
    ///
    /// // テナントコンテキスト欠落（`None`）は常に空集合。
    /// assert_eq!(store.list_for_org(None).unwrap().len(), 0);
    /// // 正当な org_id は自テナント分のみ返る。
    /// assert_eq!(store.list_for_org(Some("org-a")).unwrap().len(), 1);
    /// ```
    fn list_for_org(&self, org_id: Option<&str>) -> Result<Vec<OutboxEvent>, OutboxError> {
        let Some(org_id) = org_id.filter(|id| !id.is_empty()) else {
            return Ok(Vec::new());
        };
        let events = self
            .events
            .lock()
            .expect("InMemoryOutboxStore mutex poisoned");
        Ok(events
            .iter()
            .filter(|event| event.org_id == org_id)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, org_id: &str) -> OutboxEvent {
        OutboxEvent {
            id: id.to_string(),
            org_id: org_id.to_string(),
            event_type: "consent_revoked".to_string(),
            payload: "{}".to_string(),
        }
    }

    #[test]
    fn list_for_org_none_returns_empty_even_with_events_present() {
        let store = InMemoryOutboxStore::new();
        store.enqueue(event("evt-1", "org-a")).unwrap();
        assert_eq!(store.list_for_org(None).unwrap(), Vec::new());
    }

    #[test]
    fn list_for_org_empty_string_returns_empty() {
        let store = InMemoryOutboxStore::new();
        store.enqueue(event("evt-1", "org-a")).unwrap();
        assert_eq!(store.list_for_org(Some("")).unwrap(), Vec::new());
    }

    #[test]
    fn list_for_org_does_not_leak_other_tenant_events() {
        // テナント境界強制の中心テスト: org-a / org-b それぞれのイベントを
        // 混在させ、越境取得が 0 件であることを確認する。
        let store = InMemoryOutboxStore::new();
        store.enqueue(event("evt-a1", "org-a")).unwrap();
        store.enqueue(event("evt-a2", "org-a")).unwrap();
        store.enqueue(event("evt-b1", "org-b")).unwrap();

        let org_a_events = store.list_for_org(Some("org-a")).unwrap();
        assert_eq!(org_a_events.len(), 2);
        assert!(org_a_events.iter().all(|e| e.org_id == "org-a"));

        let org_b_events = store.list_for_org(Some("org-b")).unwrap();
        assert_eq!(org_b_events.len(), 1);
        assert_eq!(org_b_events[0].id, "evt-b1");

        // org-c は 1 件も enqueue していないため 0 件（存在しないテナント）。
        assert_eq!(store.list_for_org(Some("org-c")).unwrap().len(), 0);
    }
}
