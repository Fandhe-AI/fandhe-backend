//! クエリ文字列 key-value パーサ（sans-IO、イシュー #306）。
//!
//! [`crate::request::RequestHead::query`] は `?` より後の生文字列を返すのみで、
//! `&`/`=` への分解は呼び出し元の責務（イシュー #258 の契約）。本モジュールは
//! その分解処理を [`crate::http`] クレート内へ集約し、`crates/core` の
//! 各サンプル・`crates/plugin-*` で個別実装されがちな同型コード（`&`/`=` の
//! 手動 split）の重複を解消する。
//!
//! # 呼び出し契約
//!
//! [`parse_query`] は [`crate::request::RequestHead::query`] が返す `Some(&str)`
//! （`?` 以降・生文字列）を入力に取る sans-IO 純関数。呼び出し元
//! （`crates/routes` のハンドラ・`crates/plugin-openapi` 等）が
//! `RequestHead::query()` の戻り値をそのまま渡す想定。
//!
//! # 非デコード契約（[`crate::request::RequestHead::path`] / `query` と同一方針）
//!
//! percent-decode・`application/x-www-form-urlencoded` の `+` → 半角スペース
//! デコードは一切行わない。デコードを本パーサ内で行うと、ルーティング層
//! （`Router::dispatch`）とクエリパーサとでデコード有無が食い違い、OWASP A01
//! （アクセス制御の不備）の温床となる正規化バイパスを生みかねない
//! （`.claude/rules/security.md`）。デコードが必要な呼び出し元は本関数が返す
//! borrow に対して明示的に別途デコード処理を適用すること。
//!
//! # DoS 耐性（`.claude/rules/security.md` リソース枯渇対策）
//!
//! [`chunked`](crate::chunked) モジュールと同じ「バッファ確保前に上限を検査し
//! fail-closed で拒否する」方針を踏襲する。[`parse_query`] は key-value 分解
//! そのものに入る前に全長・組数の 2 上限を 1 パスで検証し、超過時は
//! [`QueryPairs`] を一切生成しない（部分結果を返さない）。

/// クエリ文字列として許容する最大バイト数。
///
/// [`crate::request::MAX_HEADER_BYTES`]（16 KiB）の半分。request-target 全体が
/// ヘッダ部の上限内に収まる前提と整合しつつ、クエリ単体の独立上限として
/// 明示する。
pub const MAX_QUERY_BYTES: usize = 8 * 1024;

/// クエリ文字列として許容する key-value 組数の上限。
///
/// `a&a&a&...` のような 1 文字 pair の連打による、下流ハンドラでの線形探索
/// 増幅・[`QueryPairs`] 消費コストの増幅を抑止する。
pub const MAX_QUERY_PAIRS: usize = 256;

/// [`parse_query`] が返しうるエラー。
#[derive(Debug, PartialEq, Eq)]
pub enum QueryError {
    /// クエリ文字列全体が [`MAX_QUERY_BYTES`] を超過した。
    QueryTooLong,
    /// key-value 組数が [`MAX_QUERY_PAIRS`] を超過した。
    TooManyPairs,
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            QueryError::QueryTooLong => "query string exceeds MAX_QUERY_BYTES",
            QueryError::TooManyPairs => "query pair count exceeds MAX_QUERY_PAIRS",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for QueryError {}

/// クエリ文字列を `&` 区切りの key-value 組へ分解する sans-IO 純関数。
///
/// [`crate::request::RequestHead::query`] が返す `?` 以降の生文字列
/// （percent-decode 前）を受け取り、`&` で区切ったセグメントをさらに最初の
/// `=` で `(key, value)` に分割する。割り当てを行わず、返す各要素は入力
/// `query` からの borrow（ゼロコピー）。
///
/// 上限超過時は [`QueryPairs`] を一切生成せず `Err` を返す
/// （fail-closed。「本モジュール」節の DoS 耐性を参照）。
///
/// # 分解セマンティクス
///
/// - 重複キー（`a=1&a=2`）はすべて出現順に返す。除重・上書きは呼び出し側の責務
/// - `=` を含まないセグメント（`a`）は値を空文字列として扱う（`("a", "")`）
/// - `=` の右辺が空（`a=`）も同様に値は空文字列
/// - キーが空（`=v`）でも `("", "v")` として返す（握りつぶさない）
/// - 空セグメント（`&&`・先頭/末尾の `&`）はスキップする
/// - 2 個目以降の `=` は値の一部として扱う（`a=b=c` → `("a", "b=c")`）
/// - percent-decode・`+` → 半角スペースのデコードは行わない（「非デコード契約」節）
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::query::parse_query;
///
/// let pairs: Vec<(&str, &str)> = parse_query("a=1&a=2").unwrap().collect();
/// assert_eq!(pairs, vec![("a", "1"), ("a", "2")]);
/// ```
///
/// `=` なしキーは値を空文字列として扱う:
///
/// ```
/// use fandhe_backend_http::query::parse_query;
///
/// let pairs: Vec<(&str, &str)> = parse_query("a").unwrap().collect();
/// assert_eq!(pairs, vec![("a", "")]);
/// ```
///
/// 空セグメントはスキップし、空キー・空値は保持する:
///
/// ```
/// use fandhe_backend_http::query::parse_query;
///
/// let pairs: Vec<(&str, &str)> = parse_query("a&&=v&b=").unwrap().collect();
/// assert_eq!(pairs, vec![("a", ""), ("", "v"), ("b", "")]);
/// ```
///
/// 上限超過は fail-closed で `Err` を返す:
///
/// ```
/// use fandhe_backend_http::query::{parse_query, QueryError, MAX_QUERY_PAIRS};
///
/// let query = vec!["a=1"; MAX_QUERY_PAIRS + 1].join("&");
/// assert_eq!(parse_query(&query).unwrap_err(), QueryError::TooManyPairs);
/// ```
pub fn parse_query(query: &str) -> Result<QueryPairs<'_>, QueryError> {
    if query.len() > MAX_QUERY_BYTES {
        return Err(QueryError::QueryTooLong);
    }
    let pair_count = query.split('&').filter(|seg| !seg.is_empty()).count();
    if pair_count > MAX_QUERY_PAIRS {
        return Err(QueryError::TooManyPairs);
    }
    Ok(QueryPairs { remainder: query })
}

/// [`parse_query`] が返すゼロコピーイテレータ。
///
/// `Iterator<Item = (&'a str, &'a str)>` を実装し、内部では入力文字列の
/// 未走査部分（`remainder`）を保持するのみで追加の割り当ては行わない。
#[derive(Debug, Clone)]
pub struct QueryPairs<'a> {
    remainder: &'a str,
}

impl<'a> Iterator for QueryPairs<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.remainder.is_empty() {
                return None;
            }
            let (segment, rest) = match self.remainder.split_once('&') {
                Some((seg, rest)) => (seg, rest),
                None => (self.remainder, ""),
            };
            self.remainder = rest;
            if segment.is_empty() {
                // `&&`・先頭/末尾の `&` による空セグメントはスキップし、
                // 次のセグメントへ読み進める（[`parse_query`] のセマンティクス）。
                continue;
            }
            return Some(match segment.split_once('=') {
                Some((key, value)) => (key, value),
                None => (segment, ""),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(query: &str) -> Vec<(&str, &str)> {
        parse_query(query).unwrap().collect()
    }

    #[test]
    fn empty_query_yields_no_pairs() {
        assert_eq!(collect(""), Vec::<(&str, &str)>::new());
    }

    #[test]
    fn duplicate_keys_are_all_returned_in_order() {
        assert_eq!(collect("a=1&a=2"), vec![("a", "1"), ("a", "2")]);
    }

    #[test]
    fn key_without_equals_has_empty_value() {
        assert_eq!(collect("a"), vec![("a", "")]);
    }

    #[test]
    fn key_with_trailing_equals_has_empty_value() {
        assert_eq!(collect("a="), vec![("a", "")]);
    }

    #[test]
    fn empty_key_is_preserved() {
        assert_eq!(collect("=v"), vec![("", "v")]);
    }

    #[test]
    fn empty_segments_are_skipped() {
        assert_eq!(collect("a&&b"), vec![("a", ""), ("b", "")]);
    }

    #[test]
    fn leading_and_trailing_ampersands_are_skipped() {
        assert_eq!(collect("&a=1&"), vec![("a", "1")]);
    }

    #[test]
    fn only_first_equals_splits_key_and_value() {
        assert_eq!(collect("a=b=c"), vec![("a", "b=c")]);
    }

    #[test]
    fn percent_encoded_sequences_are_not_decoded() {
        // 非デコード契約の固定: `%20` はそのまま値へ残る（デコードは呼び出し側の責務）。
        assert_eq!(collect("q=a%20b"), vec![("q", "a%20b")]);
    }

    #[test]
    fn plus_is_not_decoded_to_space() {
        // 非デコード契約の固定: `+` もそのまま値へ残る。
        assert_eq!(collect("q=a+b"), vec![("q", "a+b")]);
    }

    #[test]
    fn query_exactly_at_max_bytes_is_accepted() {
        let query = "a".repeat(MAX_QUERY_BYTES);
        assert!(parse_query(&query).is_ok());
    }

    #[test]
    fn query_exceeding_max_bytes_is_rejected() {
        let query = "a".repeat(MAX_QUERY_BYTES + 1);
        assert_eq!(parse_query(&query).unwrap_err(), QueryError::QueryTooLong);
    }

    #[test]
    fn pair_count_exactly_at_max_is_accepted() {
        let query = vec!["a=1"; MAX_QUERY_PAIRS].join("&");
        assert!(parse_query(&query).is_ok());
    }

    #[test]
    fn pair_count_exceeding_max_is_rejected() {
        let query = vec!["a=1"; MAX_QUERY_PAIRS + 1].join("&");
        assert_eq!(parse_query(&query).unwrap_err(), QueryError::TooManyPairs);
    }

    #[test]
    fn fail_closed_rejection_yields_no_partial_pairs() {
        // 上限超過時は QueryPairs を一切生成しない（部分結果を返さない）ことの固定。
        let query = vec!["a=1"; MAX_QUERY_PAIRS + 1].join("&");
        assert!(parse_query(&query).is_err());
    }

    #[test]
    fn query_error_display_messages_are_stable() {
        assert_eq!(
            QueryError::QueryTooLong.to_string(),
            "query string exceeds MAX_QUERY_BYTES"
        );
        assert_eq!(
            QueryError::TooManyPairs.to_string(),
            "query pair count exceeds MAX_QUERY_PAIRS"
        );
    }

    #[test]
    fn query_error_implements_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<QueryError>();
    }
}
