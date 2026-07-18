//! run-graphql-accept-tests.sh 用フィクスチャ: unsafe を含む擬似ソース。
//! graphql-accept.sh の基準 A' の grep パイプラインが実コードの unsafe を
//! 見逃さないことを検証する対象（見逃しは受け入れ判定の偽 PASS に直結するため
//! 最も重大な回帰）。

pub fn deref_raw(ptr: *const i32) -> i32 {
    // SAFETY: このフィクスチャは検出ロジックのテスト専用であり、実際に
    // 呼び出されることはない。
    unsafe { *ptr }
}
