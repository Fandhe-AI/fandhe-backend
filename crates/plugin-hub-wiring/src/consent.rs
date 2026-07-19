//! 同意ゲート（`(org_id, service, info_type)` 単位のオプトイン状態）
//! ストレージ抽象（TASK-9.4 / #64 §4.1、実装は #243）。
//!
//! `docs/design/outbox-consent-integration.md` §4.1 が確定した trait 境界を
//! そのまま実装する。コアは本 trait を一切知らない。
//! `fandhe-backend-plugin-hub-wiring` を依存に加えた利用側サービスが、この
//! trait の実装（インメモリ or PostgreSQL）を選択して注入する。
//!
//! # オプトイン原則（デフォルト非共有、フェイルクローズ）
//!
//! [`ConsentStore::is_granted`] は判定不能（未登録テナント・未登録
//! `(service, info_type)` の組）の場合もオプトイン原則により拒否側
//! （`false`）に倒す（§8「オプトイン原則の維持」）。同意管理サービスへの
//! 参照が失敗する PostgreSQL 実装であっても、フェイルオープン（誤って
//! 共有）ではなくフェイルクローズ（同意なしとして扱う）を既定とすること
//! （§8）。
//!
//! # `RequestGate::check` との関係
//!
//! [`crate::gate::TenantGate`]（`RequestGate` 拡張点）の判定通過**後**に、
//! ハンドラ層から呼ばれる想定であり、`RequestGate::check` の同期 API 制約
//! （`crates/core/src/extension.rs` doc）の対象外（§4.2）。本トレイトは
//! §4.1 のシグネチャに合わせ同期 `fn` として設計する（インメモリ実装は
//! I/O を伴わないため。PostgreSQL 実装で非同期 DB クライアントを用いる
//! 場合はシグネチャ変更が必要になる旨は §4.2 参照）。
//!
//! # `OutboxStore` との関係（同意取り消しの伝播）
//!
//! `ConsentStore::revoke` 呼び出し時、同意取り消しを他サービスへ伝播する
//! 必要がある場合は [`crate::outbox::OutboxStore::enqueue`] で
//! `consent_revoked` イベントを発火する運用とする（§8「同意取り消し時の
//! Outbox イベント発火」）。この連携は実装（呼び出し元）の責務であり、
//! [`ConsentStore`] trait 自体は [`crate::outbox::OutboxStore`] に依存しない
//! （trait 境界の独立性を保つ）。
//!
//! # pay-for-what-you-use
//!
//! 本モジュールは同意管理サービスの実 API/スキーマ（`micro-service-hub`
//! 側）に一切依存しない。インメモリ実装（[`InMemoryConsentStore`]）は
//! 既存依存（`std` のみ）で完結し、新規依存を追加しない。

use std::collections::HashMap;
use std::sync::Mutex;

/// `(org_id, service, info_type)` の組（同意状態のキー）。
type ConsentKey = (String, String, String);

/// [`ConsentStore`] 操作の失敗理由。
///
/// 現時点では区別するエラーがないため空 enum とする。PostgreSQL 実装が
/// 接続エラー等を追加する場合は、利用側サービスが独自のエラー型でラップ
/// して扱う想定（本 enum に他クレートから variant を追加することはできない
/// ため。本クレートは実装を確定しない、§4・§9「対象外」）。`is_granted`
/// 自体は判定不能時もエラーではなく
/// `Ok(false)`（オプトイン原則によるフェイルクローズ）を返す契約のため、
/// 本 enum は現状使用箇所を持たない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentError {}

/// 同意ゲート（`(org_id, service, info_type)` 単位のオプトイン状態）
/// ストレージ抽象。
///
/// `docs/design/outbox-consent-integration.md` §4.1 のシグネチャそのもの。
/// 実装はオプトイン原則（デフォルト非共有）をフェイルクローズとして守る
/// こと（§8）。
pub trait ConsentStore: Send + Sync {
    /// `(org_id, service, info_type)` の同意状態を判定する。
    ///
    /// 判定不能（未登録テナント等）はオプトイン原則により拒否側
    /// （`false`）に倒す。`org_id` は必須パラメータであり、暗黙のデフォルト
    /// テナントを持たせない（§6.2）。空文字列 `org_id` は
    /// [`OutboxStore::list_for_org`](crate::outbox::OutboxStore::list_for_org)
    /// の読み出し側ガードと対称に、実装が明示的にガードし登録状態に
    /// 関わらず常に `false`（フェイルクローズ）を返すこと。
    fn is_granted(
        &self,
        org_id: &str,
        service: &str,
        info_type: &str,
    ) -> Result<bool, ConsentError>;

    /// 同意取り消し時に呼び出す。実装は Outbox への `consent_revoked`
    /// イベント発火を責務に含めてよい（§8）。
    fn revoke(&self, org_id: &str, service: &str, info_type: &str) -> Result<(), ConsentError>;
}

/// テスト・スパイク用のインメモリ [`ConsentStore`] 実装（PoC-6 相当、§4.1）。
///
/// `Mutex<HashMap<(org_id, service, info_type), bool>>` で同意状態を保持
/// する。未登録の組は `is_granted` が `false`（オプトイン原則の既定値）を
/// 返す。PostgreSQL 実装（同意管理サービスの `consent_grants` テーブル参照）
/// は利用側サービスが提供する（本クレートに sqlx 等を追加しない、
/// .claude/rules/pay-for-what-you-use.md）。
///
/// **実運用での利用は禁止**（プロセス内メモリのみで永続化されず、
/// プロセス再起動で同意状態が失われるため。[`crate::audit::MemoryAuditSink`]
/// と同型の注意事項）。
#[derive(Debug, Default)]
pub struct InMemoryConsentStore {
    grants: Mutex<HashMap<ConsentKey, bool>>,
}

impl InMemoryConsentStore {
    /// 空の `InMemoryConsentStore` を作る（同意皆無 = 全拒否の初期状態）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::consent::{ConsentStore, InMemoryConsentStore};
    ///
    /// let store = InMemoryConsentStore::new();
    /// assert!(!store.is_granted("org-a", "crm", "email").unwrap());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// テスト・初期データ投入用: `(org_id, service, info_type)` を同意済みに
    /// する。
    ///
    /// 空文字列 `org_id` で登録しても、[`Self::is_granted`] は読み出し側の
    /// 明示ガードにより常に `false` を返す（フェイルクローズ、§7・§8）。
    ///
    /// # Panics
    ///
    /// 内部 `Mutex` が poison 状態の場合に panic する（テスト・スパイク
    /// 専用の型であり本番運用を想定しない、[`Self`] doc の禁止事項）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::consent::{ConsentStore, InMemoryConsentStore};
    ///
    /// let store = InMemoryConsentStore::new();
    /// store.grant("org-a", "crm", "email");
    /// assert!(store.is_granted("org-a", "crm", "email").unwrap());
    /// ```
    pub fn grant(&self, org_id: &str, service: &str, info_type: &str) {
        self.grants
            .lock()
            .expect("InMemoryConsentStore mutex poisoned")
            .insert(
                (
                    org_id.to_string(),
                    service.to_string(),
                    info_type.to_string(),
                ),
                true,
            );
    }
}

impl ConsentStore for InMemoryConsentStore {
    /// 登録済みかつ `true` の場合のみ `Ok(true)` を返す。未登録の組・
    /// [`Self::revoke`] 済みの組は `Ok(false)`（オプトイン原則のフェイル
    /// クローズ）。空文字列 `org_id` は
    /// [`OutboxStore::list_for_org`](crate::outbox::OutboxStore::list_for_org)
    /// と対称の明示ガードにより、[`Self::grant`] で登録済みであっても常に
    /// `Ok(false)` を返す（暗黙のデフォルトテナントへのフォールバックを
    /// 防ぐ、§6.2・§7・§8）。
    ///
    /// # Panics
    ///
    /// 内部 `Mutex` が poison 状態の場合に panic する。
    fn is_granted(
        &self,
        org_id: &str,
        service: &str,
        info_type: &str,
    ) -> Result<bool, ConsentError> {
        // フェイルクローズ: 空文字列 org_id は暗黙のデフォルトテナントを
        // 意味しないため、登録状態に関わらず常に拒否する
        // （`OutboxStore::list_for_org` の読み出し側ガードと対称、§7・§8）。
        if org_id.is_empty() {
            return Ok(false);
        }

        let key = (
            org_id.to_string(),
            service.to_string(),
            info_type.to_string(),
        );
        Ok(*self
            .grants
            .lock()
            .expect("InMemoryConsentStore mutex poisoned")
            .get(&key)
            .unwrap_or(&false))
    }

    /// 同意を取り消す（未登録の組に対する呼び出しも含め、常に `false` を
    /// 明示的に書き込む）。取り消し後は [`Self::is_granted`] が `false` を
    /// 返す（フェイルクローズ、§8）。
    ///
    /// 本インメモリ実装は [`crate::outbox::OutboxStore`] への
    /// `consent_revoked` イベント発火を行わない（trait 境界の独立性を保つ
    /// ための最小実装。伝播が必要な利用側サービスは `revoke` 呼び出しの
    /// 前後で `OutboxStore::enqueue` を明示的に呼ぶこと、モジュール doc
    /// 「`OutboxStore` との関係」参照）。
    ///
    /// # Panics
    ///
    /// 内部 `Mutex` が poison 状態の場合に panic する。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_hub_wiring::consent::{ConsentStore, InMemoryConsentStore};
    ///
    /// let store = InMemoryConsentStore::new();
    /// store.grant("org-a", "crm", "email");
    /// assert!(store.is_granted("org-a", "crm", "email").unwrap());
    ///
    /// store.revoke("org-a", "crm", "email").unwrap();
    /// assert!(!store.is_granted("org-a", "crm", "email").unwrap());
    /// ```
    fn revoke(&self, org_id: &str, service: &str, info_type: &str) -> Result<(), ConsentError> {
        self.grants
            .lock()
            .expect("InMemoryConsentStore mutex poisoned")
            .insert(
                (
                    org_id.to_string(),
                    service.to_string(),
                    info_type.to_string(),
                ),
                false,
            );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_granted_default_is_false_for_unregistered_pair() {
        // オプトイン原則: 何も登録していないテナントは常に拒否。
        let store = InMemoryConsentStore::new();
        assert!(!store.is_granted("org-a", "crm", "email").unwrap());
    }

    #[test]
    fn is_granted_is_false_for_empty_org_id() {
        // フェイルクローズ: 空文字列 org_id は明示ガードにより、grant で
        // 登録済みであっても常に false を返す（暗黙のデフォルトテナントに
        // フォールバックしない。OutboxStore::list_for_org の読み出し側
        // ガードと対称）。
        let store = InMemoryConsentStore::new();
        assert!(!store.is_granted("", "svc", "email").unwrap());

        store.grant("", "svc", "email");
        assert!(!store.is_granted("", "svc", "email").unwrap());
    }

    #[test]
    fn is_granted_true_after_grant() {
        let store = InMemoryConsentStore::new();
        store.grant("org-a", "crm", "email");
        assert!(store.is_granted("org-a", "crm", "email").unwrap());
    }

    #[test]
    fn is_granted_does_not_leak_across_org_boundary() {
        // テナント境界強制: org-a に同意付与しても org-b からは越境取得
        // できない（別テナントは常に false）。
        let store = InMemoryConsentStore::new();
        store.grant("org-a", "crm", "email");

        assert!(store.is_granted("org-a", "crm", "email").unwrap());
        assert!(!store.is_granted("org-b", "crm", "email").unwrap());
    }

    #[test]
    fn is_granted_does_not_leak_across_service_or_info_type_boundary() {
        // (org_id, service, info_type) の 3 つ組すべてが一致しないと
        // 同意済みとみなされないことを確認する。
        let store = InMemoryConsentStore::new();
        store.grant("org-a", "crm", "email");

        assert!(!store.is_granted("org-a", "billing", "email").unwrap());
        assert!(!store.is_granted("org-a", "crm", "phone").unwrap());
    }

    #[test]
    fn revoke_after_grant_causes_is_granted_to_return_false() {
        // フェイルクローズ: revoke 後は is_granted が必ず false になる。
        let store = InMemoryConsentStore::new();
        store.grant("org-a", "crm", "email");
        assert!(store.is_granted("org-a", "crm", "email").unwrap());

        store.revoke("org-a", "crm", "email").unwrap();
        assert!(!store.is_granted("org-a", "crm", "email").unwrap());
    }

    #[test]
    fn revoke_on_unregistered_pair_keeps_is_granted_false() {
        // 未登録の組に対する revoke はエラーにならず、is_granted は false
        // のまま（フェイルクローズの既定値と整合）。
        let store = InMemoryConsentStore::new();
        store.revoke("org-a", "crm", "email").unwrap();
        assert!(!store.is_granted("org-a", "crm", "email").unwrap());
    }

    #[test]
    fn revoke_for_one_org_does_not_affect_other_org() {
        // テナント境界強制: org-a の revoke は org-b の同意状態に影響しない。
        let store = InMemoryConsentStore::new();
        store.grant("org-a", "crm", "email");
        store.grant("org-b", "crm", "email");

        store.revoke("org-a", "crm", "email").unwrap();

        assert!(!store.is_granted("org-a", "crm", "email").unwrap());
        assert!(store.is_granted("org-b", "crm", "email").unwrap());
    }
}
