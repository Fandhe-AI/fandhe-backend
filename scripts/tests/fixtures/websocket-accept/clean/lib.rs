//! run-websocket-accept-tests.sh 用フィクスチャ: unsafe を含まない擬似ソース。
//! websocket-accept.sh の基準 A'（自コード unsafe 0 件）の grep パイプラインが
//! false positive を出さないことを検証する対象。

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
