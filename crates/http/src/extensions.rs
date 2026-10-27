//! リクエスト単位の型付き拡張領域 [`RequestExtensions`]（イシュー #924）。
//!
//! コアの `RequestGate`（`crates/core`）が得た検証結果（例: 検証済み JWT の
//! Claims）を、**そのゲートを通過した同じリクエストのハンドラだけ**へ渡す
//! ための器。`RequestHead` が保持するためリクエストと同じ寿命になり、
//! keep-alive の次リクエストや別接続へ値が漏れる経路を構造的に持たない。
//!
//! 書き込みは本番経路では `RequestGate::check` が受け取る `&mut` 経由のみで、
//! ハンドラ等の後段は `&RequestHead` しか持たないため読み取り専用になる。
//! 値は型（`TypeId`）で引くため文字列キーによる衝突・なりすましはなく、
//! クライアントが送るバイト列から値が入る経路もない（パーサは常に空で構築する）。
//!
//! 設計は `docs/design/gate-request-extensions.md` を参照。外部依存はなく、
//! `unsafe` も使わない（pay-for-what-you-use）。

use std::any::{Any, TypeId};
use std::fmt;
use std::sync::Arc;

/// リクエスト単位の型付き値の入れ物。
///
/// 値は `Arc` で保持するため [`Clone`] は参照カウントの複製だけで、`T: Clone` は
/// 要求しない（`RequestHead: Clone` が WebSocket 委譲経路で実際に使われるため）。
/// 空の間はヒープを確保しない。件数はゲート数程度の少数を前提に線形探索する。
///
/// 読み出し側からの `remove` / `get_mut` は意図的に提供しない。
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::extensions::RequestExtensions;
///
/// struct Principal(String);
///
/// let mut ext = RequestExtensions::new();
/// assert!(ext.insert(Principal("alice".to_string())).is_ok());
/// assert_eq!(ext.get::<Principal>().map(|p| p.0.as_str()), Some("alice"));
///
/// // 同じ型の 2 回目の insert は上書きせず値を返す（fail-closed）。
/// assert!(ext.insert(Principal("mallory".to_string())).is_err());
/// assert_eq!(ext.get::<Principal>().map(|p| p.0.as_str()), Some("alice"));
/// ```
#[derive(Clone, Default)]
pub struct RequestExtensions {
    entries: Vec<(TypeId, Arc<dyn Any + Send + Sync>)>,
}

impl RequestExtensions {
    /// 空の拡張領域を作る（ヒープ確保なし）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 型 `T` の値を登録する。
    ///
    /// 同じ型が既にある場合は**上書きせず** `Err(value)` で返す。後から評価される
    /// ゲートが先のゲートの検証結果を黙って差し替えることを防ぐため、呼び出し側
    /// （ゲート）は `Err` を拒否として扱うこと（フェイルクローズ）。
    ///
    /// # Errors
    ///
    /// 同じ型が登録済みのとき、渡した値をそのまま `Err` で返す。
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> Result<(), T> {
        let id = TypeId::of::<T>();
        if self.entries.iter().any(|(t, _)| *t == id) {
            return Err(value);
        }
        self.entries.push((id, Arc::new(value)));
        Ok(())
    }

    /// 型 `T` の値への参照を返す。未登録なら `None`。
    #[must_use]
    pub fn get<T: 'static>(&self) -> Option<&T> {
        let id = TypeId::of::<T>();
        self.entries
            .iter()
            .find(|(t, _)| *t == id)
            .and_then(|(_, v)| v.as_ref().downcast_ref::<T>())
    }

    /// 型 `T` の値を `Arc` で返す。`'static` な future を返す非同期ハンドラへ、
    /// 値を複製せずに move するために使う。
    #[must_use]
    pub fn get_shared<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let id = TypeId::of::<T>();
        self.entries
            .iter()
            .find(|(t, _)| *t == id)
            .and_then(|(_, v)| Arc::clone(v).downcast::<T>().ok())
    }

    /// 型 `T` の値が登録済みかを返す。
    #[must_use]
    pub fn contains<T: 'static>(&self) -> bool {
        let id = TypeId::of::<T>();
        self.entries.iter().any(|(t, _)| *t == id)
    }

    /// 登録件数を返す。
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 1 件も登録されていなければ `true`。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 件数のみを出力する。値は Claims 等の機密を含みうるため `Debug` に出さない。
impl fmt::Debug for RequestExtensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestExtensions")
            .field("len", &self.entries.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct A(u32);
    struct B(&'static str);

    #[test]
    fn insert_and_get_roundtrip() {
        let mut e = RequestExtensions::new();
        assert!(e.is_empty());
        assert!(e.insert(A(7)).is_ok());
        assert!(e.insert(B("x")).is_ok());
        assert_eq!(e.len(), 2);
        assert_eq!(e.get::<A>().map(|a| a.0), Some(7));
        assert_eq!(e.get::<B>().map(|b| b.0), Some("x"));
        assert!(e.get::<String>().is_none());
        assert!(e.contains::<A>());
        assert!(!e.contains::<String>());
    }

    #[test]
    fn duplicate_insert_is_rejected_and_keeps_original() {
        let mut e = RequestExtensions::new();
        e.insert(A(1)).unwrap();
        let back = e.insert(A(2)).unwrap_err();
        assert_eq!(back.0, 2);
        assert_eq!(e.get::<A>().map(|a| a.0), Some(1));
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn get_shared_and_clone_share_value() {
        let mut e = RequestExtensions::new();
        e.insert(A(5)).unwrap();
        let shared = e.get_shared::<A>().unwrap();
        let cloned = e.clone();
        assert!(Arc::ptr_eq(&shared, &cloned.get_shared::<A>().unwrap()));
        assert!(e.get_shared::<B>().is_none());
    }

    #[test]
    fn debug_does_not_leak_values() {
        let mut e = RequestExtensions::new();
        e.insert(String::from("super-secret-claims")).unwrap();
        let s = format!("{e:?}");
        assert!(!s.contains("super-secret"));
        assert!(s.contains("len"));
    }

    #[test]
    fn is_send_sync_clone() {
        fn assert_traits<T: Send + Sync + Clone>() {}
        assert_traits::<RequestExtensions>();
    }
}
