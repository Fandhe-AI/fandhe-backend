//! `{name}` パスパラメータのパターンパース・セグメント照合（TASK-176、#176）。
//!
//! `crates/routes::Router` の `route_param` / `dispatch` から呼ばれる内部実装を
//! 切り出したモジュール。公開 API（[`PathParams`] / [`RoutePatternError`] /
//! [`Segment`] / [`ParamRoute`]）は `lib.rs` から re-export される。
//! 本モジュール単体では `fandhe-backend-http` の型に依存しない（`ParamRoute::handler` の
//! シグネチャのみ `lib.rs` 側の `ParamRouteHandler` 型を利用する）。

use std::error::Error;
use std::fmt;

/// パス上のパラメータ名 → 値の抽出結果。
///
/// 値は % デコードしない生の文字列スライスで、`RequestHead::target`（借用元）と
/// 同じライフタイムを持つ（値側は追加アロケーションなしのゼロコピー）。
/// パラメータ名はパターン文字列（`Router` が起動時に保持し続けるパース済み
/// [`Segment`]）から取得するため、リクエスト単位のライフタイム `'a` とは
/// 独立して存在できるよう `String` として複製する（マッチ成立時のみ・
/// パラメータ数ぶんの小さな複製のみで、DoS 耐性に影響する規模ではない）。
/// デコードが必要な場合は呼び出し側（ハンドラ）の責務で行い、デコード後の値を
/// 再検証すること（正規化バイパス対策、モジュール doc「マッチング方針」節）。
#[derive(Debug, Default)]
pub struct PathParams<'a> {
    params: Vec<(String, &'a str)>,
}

impl<'a> PathParams<'a> {
    /// `name` に対応する値を返す。未束縛または存在しないパラメータ名なら `None`。
    ///
    /// ```
    /// use fandhe_backend_routes::Router;
    /// use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
    ///
    /// let router = Router::new()
    ///     .route_param("GET", "/users/{id}", |_head, params, _body| {
    ///         assert_eq!(params.get("id"), Some("42"));
    ///         assert_eq!(params.get("missing"), None);
    ///         fandhe_backend_http::response::Response::empty(200)
    ///     })
    ///     .unwrap();
    /// let head = match parse_request_head(b"GET /users/42 HTTP/1.1\r\n\r\n").unwrap() {
    ///     ParseOutcome::Complete { head, .. } => head,
    ///     ParseOutcome::Incomplete => unreachable!(),
    /// };
    /// assert_eq!(router.dispatch(&head, &[]).status, 200);
    /// ```
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }

    /// 束縛済みの `(name, value)` を登録順（パターン上の出現順）に返す。
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.params.iter().map(|(k, v)| (k.as_str(), *v))
    }

    /// 束縛済みパラメータ数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.params.len()
    }

    /// 束縛済みパラメータが 1 つもないか。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }
}

/// パターン登録時（[`super::Router::route_param`]）の検証エラー。
///
/// `fandhe-backend-routes` は外部依存ゼロを維持する契約（`crates/routes/Cargo.toml` 参照）の
/// ため、`thiserror` を使わず `std::error::Error` を手実装する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutePatternError {
    /// パターンが `/` で始まっていない。
    MissingLeadingSlash,
    /// `{}`（パラメータ名が空）のセグメントがあった。
    EmptyParamName,
    /// パラメータ名に `[A-Za-z0-9_]` 以外の文字が含まれていた。
    InvalidParamName(String),
    /// `a{b}` のように 1 セグメント内でリテラルと `{name}` が混在していた。
    MixedSegment(String),
    /// 同一パターン内で同じパラメータ名が複数回使われていた。
    DuplicateParamName(String),
    /// `//`（連続スラッシュ）や末尾 `/` により空セグメントが生じた
    /// （`/hello//{name}`・`/hello/{name}/` 等）。空セグメントを `Literal("")`
    /// として受理すると、`route()` の静的パスがそのような URL を要求しない
    /// 通常のリクエスト（`/hello/alice` 等）を誤って 404 にしてしまうため、
    /// 登録時点で fail-closed に拒否する（セグメント数不変条件、モジュール doc
    /// 「マッチング方針」節）。
    EmptySegment,
    /// `{name}` セグメントを 1 つも含まないパターンが渡された
    /// （完全一致ルートは [`super::Router::route`] を使う責務分界のため）。
    NoParamSegment,
}

impl fmt::Display for RoutePatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingLeadingSlash => write!(f, "パターンは '/' で始まる必要があります"),
            Self::EmptyParamName => {
                write!(f, "パラメータ名が空のセグメント '{{}}' は許可されません")
            }
            Self::InvalidParamName(name) => {
                write!(
                    f,
                    "パラメータ名 '{name}' に使用できない文字が含まれています（[A-Za-z0-9_] のみ許可）"
                )
            }
            Self::MixedSegment(seg) => {
                write!(
                    f,
                    "セグメント '{seg}' はリテラルと {{name}} の混在が許可されません"
                )
            }
            Self::DuplicateParamName(name) => {
                write!(f, "パラメータ名 '{name}' が同一パターン内で重複しています")
            }
            Self::EmptySegment => {
                write!(
                    f,
                    "連続スラッシュ・末尾スラッシュによる空セグメントは許可されません"
                )
            }
            Self::NoParamSegment => {
                write!(
                    f,
                    "{{name}} セグメントを含まないパターンは route() を使ってください"
                )
            }
        }
    }
}

impl Error for RoutePatternError {}

/// パース済みパターンの 1 セグメント。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// リテラル文字列との完全一致を要求するセグメント。
    Literal(String),
    /// `{name}` — 非空の 1 セグメントにマッチしパラメータとして束縛する。
    Param(String),
}

/// 登録済みのパラメータ付きルート 1 件（method + パース済みパターン + ハンドラ）。
pub struct ParamRoute {
    pub(crate) method: String,
    pub(crate) segments: Vec<Segment>,
    pub(crate) handler: super::ParamRouteHandler,
}

/// `pattern`（先頭 `/` を持つことが呼び出し元 [`parse_pattern`] で保証済み）を
/// `/` 区切りでセグメント分割する。
///
/// 先頭の `/` は空セグメントを生まないよう取り除く（`"/a/b"` → `["a", "b"]`）。
/// `fandhe-backend-http` は target の % デコード・正規化を行わないため、ここでの分割も
/// バイト列（UTF-8 として妥当な範囲の `str`）をそのまま `/` で割るのみに留める。
///
/// 本関数はパターン文字列専用であり、リクエストの `target` 分割には使わない
/// （先頭 `/` の有無を検証しないため、origin-form 以外の `target`（`*`・
/// authority-form・absolute-URI form 等）を無検証で受理してしまう）。
/// `target` 分割には [`request_target_segments`] を使うこと。
pub(crate) fn split_segments(pattern: &str) -> Vec<&str> {
    let trimmed = pattern.strip_prefix('/').unwrap_or(pattern);
    if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').collect()
    }
}

/// リクエストの `target` を、パラメータルート照合用にセグメント分割する。
///
/// `target` が先頭 `/` で始まる origin-form の場合のみ `Some` を返す。
/// `fandhe-backend-http` の `RequestHead::target` は request-target の形式（origin-form /
/// absolute-form / authority-form / asterisk-form、RFC 9112 3.2 節）を区別せず
/// 生文字列のまま保持するため、ここで origin-form を明示的に要求しないと
/// `*`（asterisk-form）や `hello/alice`（先頭 `/` を欠く不正形式）のような
/// 文字列がセグメント数の偶然の一致だけでパラメータパターン
/// （例: `/{name}`・`/hello/{name}`）に一致してしまう
/// （fail-closed・無正規化契約からの逸脱、モジュール doc「マッチング方針」節）。
/// origin-form でない `target` は `None` を返し、呼び出し元はパラメータルート
/// 照合を行わない（静的ルートの完全一致判定のみに委ねる）。
pub(crate) fn request_target_segments(target: &str) -> Option<Vec<&str>> {
    let stripped = target.strip_prefix('/')?;
    Some(if stripped.is_empty() {
        Vec::new()
    } else {
        stripped.split('/').collect()
    })
}

/// `pattern` をパースし [`Segment`] 列に変換する。登録時検証（fail-closed）。
pub(crate) fn parse_pattern(pattern: &str) -> Result<Vec<Segment>, RoutePatternError> {
    if !pattern.starts_with('/') {
        return Err(RoutePatternError::MissingLeadingSlash);
    }

    let raw_segments = split_segments(pattern);
    let mut segments = Vec::with_capacity(raw_segments.len());
    let mut seen_names: Vec<&str> = Vec::new();
    let mut has_param = false;

    for raw in raw_segments {
        if raw.is_empty() {
            return Err(RoutePatternError::EmptySegment);
        }
        let starts = raw.starts_with('{');
        let ends = raw.ends_with('}');
        if starts && ends && raw.len() >= 2 {
            let name = &raw[1..raw.len() - 1];
            if name.is_empty() {
                return Err(RoutePatternError::EmptyParamName);
            }
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(RoutePatternError::InvalidParamName(name.to_string()));
            }
            if seen_names.contains(&name) {
                return Err(RoutePatternError::DuplicateParamName(name.to_string()));
            }
            seen_names.push(name);
            has_param = true;
            segments.push(Segment::Param(name.to_string()));
        } else if raw.contains('{') || raw.contains('}') {
            // 混在（`a{b}` 等）・片方だけの `{` / `}` はすべて不正パターンとして
            // 拒否する（暗黙のリテラル解釈によるサイレントな挙動変化を避ける）。
            return Err(RoutePatternError::MixedSegment(raw.to_string()));
        } else {
            segments.push(Segment::Literal(raw.to_string()));
        }
    }

    if !has_param {
        return Err(RoutePatternError::NoParamSegment);
    }

    Ok(segments)
}

/// パース済み `segments` と実際の `target_segments` を照合する。
///
/// セグメント数が一致しない場合は不一致（`None`）。各セグメントは
/// [`Segment::Literal`] ならバイト完全一致、[`Segment::Param`] なら次の条件を
/// 満たす非空セグメントにのみマッチする（フェイルクローズの入力検証、
/// モジュール doc「マッチング方針」節）:
///
/// - `.` / `..` と一致しない（パス走査対策）
/// - `?` / `#` を含まない（クエリ・フラグメントの過剰キャプチャ防止）
pub(crate) fn match_segments<'a>(
    segments: &[Segment],
    target_segments: &[&'a str],
) -> Option<PathParams<'a>> {
    if segments.len() != target_segments.len() {
        return None;
    }

    let mut params = Vec::new();
    for (segment, value) in segments.iter().zip(target_segments.iter()) {
        match segment {
            Segment::Literal(lit) => {
                if lit != value {
                    return None;
                }
            }
            Segment::Param(name) => {
                if value.is_empty()
                    || *value == "."
                    || *value == ".."
                    || value.contains('?')
                    || value.contains('#')
                {
                    return None;
                }
                params.push((name.clone(), *value));
            }
        }
    }

    Some(PathParams { params })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pattern_rejects_missing_leading_slash() {
        assert_eq!(
            parse_pattern("hello/{name}"),
            Err(RoutePatternError::MissingLeadingSlash)
        );
    }

    #[test]
    fn parse_pattern_rejects_empty_param_name() {
        assert_eq!(
            parse_pattern("/hello/{}"),
            Err(RoutePatternError::EmptyParamName)
        );
    }

    #[test]
    fn parse_pattern_rejects_invalid_param_name_chars() {
        assert_eq!(
            parse_pattern("/hello/{na-me}"),
            Err(RoutePatternError::InvalidParamName("na-me".to_string()))
        );
    }

    #[test]
    fn parse_pattern_rejects_mixed_segment() {
        assert_eq!(
            parse_pattern("/hello/a{name}"),
            Err(RoutePatternError::MixedSegment("a{name}".to_string()))
        );
    }

    #[test]
    fn parse_pattern_rejects_duplicate_param_name() {
        assert_eq!(
            parse_pattern("/a/{id}/b/{id}"),
            Err(RoutePatternError::DuplicateParamName("id".to_string()))
        );
    }

    #[test]
    fn parse_pattern_rejects_consecutive_slash_empty_segment() {
        // '/hello//{name}' は '/' split で中間に空セグメントを生む
        // （PR #191 Bugbot 指摘、comment id 3608815178）。
        assert_eq!(
            parse_pattern("/hello//{name}"),
            Err(RoutePatternError::EmptySegment)
        );
    }

    #[test]
    fn parse_pattern_rejects_trailing_slash_empty_segment() {
        // '/hello/{name}/' は末尾 '/' により末尾に空セグメントを生む
        // （PR #191 Bugbot 指摘、comment id 3608815178）。
        assert_eq!(
            parse_pattern("/hello/{name}/"),
            Err(RoutePatternError::EmptySegment)
        );
    }

    #[test]
    fn parse_pattern_rejects_pattern_without_param() {
        assert_eq!(
            parse_pattern("/hello/world"),
            Err(RoutePatternError::NoParamSegment)
        );
    }

    #[test]
    fn parse_pattern_accepts_multiple_params() {
        let segments = parse_pattern("/users/{id}/posts/{post_id}").unwrap();
        assert_eq!(
            segments,
            vec![
                Segment::Literal("users".to_string()),
                Segment::Param("id".to_string()),
                Segment::Literal("posts".to_string()),
                Segment::Param("post_id".to_string()),
            ]
        );
    }

    #[test]
    fn match_segments_rejects_segment_count_mismatch() {
        let segments = parse_pattern("/hello/{name}").unwrap();
        assert!(match_segments(&segments, &["hello", "a", "b"]).is_none());
        assert!(match_segments(&segments, &["hello"]).is_none());
    }

    #[test]
    fn match_segments_rejects_empty_segment() {
        let segments = parse_pattern("/hello/{name}").unwrap();
        assert!(match_segments(&segments, &["hello", ""]).is_none());
    }

    #[test]
    fn match_segments_rejects_dot_and_dotdot_path_traversal() {
        let segments = parse_pattern("/hello/{name}").unwrap();
        assert!(match_segments(&segments, &["hello", "."]).is_none());
        assert!(match_segments(&segments, &["hello", ".."]).is_none());
    }

    #[test]
    fn match_segments_rejects_query_and_fragment_chars() {
        let segments = parse_pattern("/hello/{name}").unwrap();
        assert!(match_segments(&segments, &["hello", "a?b"]).is_none());
        assert!(match_segments(&segments, &["hello", "a#b"]).is_none());
    }

    #[test]
    fn match_segments_binds_param_value() {
        let segments = parse_pattern("/hello/{name}").unwrap();
        let params = match_segments(&segments, &["hello", "alice"]).unwrap();
        assert_eq!(params.get("name"), Some("alice"));
        assert_eq!(params.len(), 1);
        assert!(!params.is_empty());
    }

    #[test]
    fn match_segments_does_not_decode_percent_encoding() {
        // 非デコード契約（モジュール doc「マッチング方針」節）を固定化するテスト。
        // `%2e%2e` は literal な文字列としてそのまま束縛され、`..` として
        // 解釈・拒否されない（デコードは呼び出し側責務）。
        let segments = parse_pattern("/hello/{name}").unwrap();
        let params = match_segments(&segments, &["hello", "%2e%2e"]).unwrap();
        assert_eq!(params.get("name"), Some("%2e%2e"));
    }

    #[test]
    fn split_segments_handles_root_and_leading_slash() {
        assert_eq!(split_segments("/"), Vec::<&str>::new());
        assert_eq!(split_segments("/a/b"), vec!["a", "b"]);
    }
}
