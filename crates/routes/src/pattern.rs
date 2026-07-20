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
    /// // doc test は crate の dev-dependencies（tokio、イシュー #315）を利用できる。
    /// let res = tokio::runtime::Builder::new_current_thread().build().unwrap()
    ///     .block_on(router.dispatch(&head, &[]));
    /// assert_eq!(res.status, 200);
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
    /// `{*name}`（ワイルドカードセグメント）がパターンの最終セグメント以外に
    /// 現れた（イシュー #317）。`/a/{*w}/b` のように中間へ配置すると後続
    /// セグメントの意味論が曖昧になる（どこまでワイルドカードが吸収するか
    /// 決定不能）ため、登録時に fail-closed で拒否する。
    WildcardNotLast(String),
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
            Self::WildcardNotLast(seg) => {
                write!(
                    f,
                    "ワイルドカードセグメント '{seg}' はパターンの最終セグメントにのみ配置できます"
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
    /// `{*name}` — パターンの最終セグメントにのみ配置できるワイルドカード
    /// （イシュー #317）。残り target セグメントのうち **1 個以上**の非空
    /// セグメント列に一致し、束縛値は `/` を含む残りパス全体になる
    /// （0 セグメントは不一致。空マッチを許すと `/static` 単体を意図せず
    /// 捕捉してしまうため保守側に倒す、`match_segments` doc comment 参照）。
    Wildcard(String),
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
///
/// `{name}` に加え末尾ワイルドカード `{*name}`（イシュー #317）を受理する。
/// 名前の文字種検証（`[A-Za-z0-9_]`）・空名チェック・重複チェックは `{name}` /
/// `{*name}` で共通の名前空間を使う（`/{x}/{*x}` は [`RoutePatternError::DuplicateParamName`]）。
/// `{*name}` が最終セグメント以外に現れた場合は [`RoutePatternError::WildcardNotLast`]。
pub(crate) fn parse_pattern(pattern: &str) -> Result<Vec<Segment>, RoutePatternError> {
    if !pattern.starts_with('/') {
        return Err(RoutePatternError::MissingLeadingSlash);
    }

    let raw_segments = split_segments(pattern);
    let last_index = raw_segments.len().saturating_sub(1);
    let mut segments = Vec::with_capacity(raw_segments.len());
    let mut seen_names: Vec<&str> = Vec::new();
    let mut has_param = false;

    for (index, raw) in raw_segments.into_iter().enumerate() {
        if raw.is_empty() {
            return Err(RoutePatternError::EmptySegment);
        }
        let starts = raw.starts_with('{');
        let ends = raw.ends_with('}');
        if starts && ends && raw.len() >= 2 {
            let inner = &raw[1..raw.len() - 1];
            // `{*name}` はワイルドカード、それ以外は従来どおり単一セグメント
            // パラメータ（`{name}`）として扱う。
            let (is_wildcard, name) = match inner.strip_prefix('*') {
                Some(rest) => (true, rest),
                None => (false, inner),
            };
            if name.is_empty() {
                return Err(RoutePatternError::EmptyParamName);
            }
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(RoutePatternError::InvalidParamName(name.to_string()));
            }
            if seen_names.contains(&name) {
                return Err(RoutePatternError::DuplicateParamName(name.to_string()));
            }
            if is_wildcard && index != last_index {
                return Err(RoutePatternError::WildcardNotLast(raw.to_string()));
            }
            seen_names.push(name);
            has_param = true;
            segments.push(if is_wildcard {
                Segment::Wildcard(name.to_string())
            } else {
                Segment::Param(name.to_string())
            });
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

/// セグメント値がパラメータ束縛として安全か判定する（フェイルクローズの入力検証、
/// モジュール doc「マッチング方針」節）。[`Segment::Param`] の 1 セグメント値・
/// [`Segment::Wildcard`] が吸収する各セグメント値の両方に同一基準を適用する
/// （イシュー #317: ワイルドカード導入でパス走査対策の対象セグメント数が
/// 増えるため、判定ロジックを共通化して漏れを防ぐ）:
///
/// - 非空であること
/// - `.` / `..` と一致しないこと（パス走査対策）
/// - `?` / `#` を含まないこと（クエリ・フラグメントの過剰キャプチャ防止）
fn is_safe_segment_value(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('?')
        && !value.contains('#')
}

/// パース済み `segments` と実際の `target_segments` を照合する。`path` は
/// `target_segments` の分割元となった文字列そのもの（[`request_target_segments`]
/// に渡したものと同一）で、末尾が [`Segment::Wildcard`] の場合の束縛値
/// （`/` を含む残りパス全体）をゼロコピーで切り出すためのオフセット計算に使う
/// （呼び出し元 [`super::Router::dispatch`] は `RequestHead::path()` を渡す）。
///
/// 末尾が [`Segment::Wildcard`] でない場合はセグメント数が一致しない場合に
/// 不一致（`None`）。末尾が [`Segment::Wildcard`] の場合は
/// `target_segments.len() >= segments.len()`（ワイルドカードが**1 個以上**の
/// セグメントを吸収する）を要求する（0 セグメントは不一致、`Segment::Wildcard`
/// doc comment 参照）。各セグメントは [`Segment::Literal`] ならバイト完全一致、
/// [`Segment::Param`] / [`Segment::Wildcard`] は [`is_safe_segment_value`] を
/// 満たすセグメントにのみマッチする。
pub(crate) fn match_segments<'a>(
    segments: &[Segment],
    target_segments: &[&'a str],
    path: &'a str,
) -> Option<PathParams<'a>> {
    let wildcard_name = match segments.last() {
        Some(Segment::Wildcard(name)) => Some(name),
        _ => None,
    };

    let prefix_len = if wildcard_name.is_some() {
        segments.len() - 1
    } else {
        segments.len()
    };

    if wildcard_name.is_some() {
        if target_segments.len() < segments.len() {
            return None;
        }
    } else if segments.len() != target_segments.len() {
        return None;
    }

    let mut params = Vec::new();
    for (segment, value) in segments[..prefix_len]
        .iter()
        .zip(target_segments[..prefix_len].iter())
    {
        match segment {
            Segment::Literal(lit) => {
                if lit != value {
                    return None;
                }
            }
            Segment::Param(name) => {
                if !is_safe_segment_value(value) {
                    return None;
                }
                params.push((name.clone(), *value));
            }
            // `parse_pattern` が最終セグメント以外の `Wildcard` を
            // `WildcardNotLast` で拒否済みのため、prefix 部分（先頭
            // `prefix_len` 個）に `Wildcard` が現れることはない。防御的に
            // 不一致へ倒す（fail-closed、パニックさせない）。
            Segment::Wildcard(_) => return None,
        }
    }

    if let Some(name) = wildcard_name {
        let tail = &target_segments[prefix_len..];
        for value in tail {
            if !is_safe_segment_value(value) {
                return None;
            }
        }
        // `path` は `prefix_len` 個のプレフィックスセグメントを `/` 区切りで
        // 保持する origin-form 文字列（先頭 `/` 込み）であることが呼び出し元
        // 契約（`request_target_segments` が `Some` を返した = 先頭 `/`）。
        // 先頭 `/` 分（1 バイト）と各プレフィックスセグメントの長さ + 区切り
        // `/`（1 バイト）を積算し、残りパス全体の開始オフセットを求める。
        let mut start = 1;
        for seg in &target_segments[..prefix_len] {
            start += seg.len() + 1;
        }
        params.push((name.clone(), &path[start..]));
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
        assert!(match_segments(&segments, &["hello", "a", "b"], "/hello/a/b").is_none());
        assert!(match_segments(&segments, &["hello"], "/hello").is_none());
    }

    #[test]
    fn match_segments_rejects_empty_segment() {
        let segments = parse_pattern("/hello/{name}").unwrap();
        assert!(match_segments(&segments, &["hello", ""], "/hello/").is_none());
    }

    #[test]
    fn match_segments_rejects_dot_and_dotdot_path_traversal() {
        let segments = parse_pattern("/hello/{name}").unwrap();
        assert!(match_segments(&segments, &["hello", "."], "/hello/.").is_none());
        assert!(match_segments(&segments, &["hello", ".."], "/hello/..").is_none());
    }

    #[test]
    fn match_segments_rejects_query_and_fragment_chars() {
        let segments = parse_pattern("/hello/{name}").unwrap();
        assert!(match_segments(&segments, &["hello", "a?b"], "/hello/a?b").is_none());
        assert!(match_segments(&segments, &["hello", "a#b"], "/hello/a#b").is_none());
    }

    #[test]
    fn match_segments_binds_param_value() {
        let segments = parse_pattern("/hello/{name}").unwrap();
        let params = match_segments(&segments, &["hello", "alice"], "/hello/alice").unwrap();
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
        let params = match_segments(&segments, &["hello", "%2e%2e"], "/hello/%2e%2e").unwrap();
        assert_eq!(params.get("name"), Some("%2e%2e"));
    }

    #[test]
    fn split_segments_handles_root_and_leading_slash() {
        assert_eq!(split_segments("/"), Vec::<&str>::new());
        assert_eq!(split_segments("/a/b"), vec!["a", "b"]);
    }

    // --- ワイルドカードパスパラメータ `{*name}`（イシュー #317） ---

    #[test]
    fn parse_pattern_accepts_trailing_wildcard() {
        let segments = parse_pattern("/static/{*path}").unwrap();
        assert_eq!(
            segments,
            vec![
                Segment::Literal("static".to_string()),
                Segment::Wildcard("path".to_string()),
            ]
        );
    }

    #[test]
    fn parse_pattern_accepts_wildcard_only_pattern() {
        let segments = parse_pattern("/{*rest}").unwrap();
        assert_eq!(segments, vec![Segment::Wildcard("rest".to_string())]);
    }

    #[test]
    fn parse_pattern_accepts_wildcard_after_param_segment() {
        let segments = parse_pattern("/a/{id}/{*rest}").unwrap();
        assert_eq!(
            segments,
            vec![
                Segment::Literal("a".to_string()),
                Segment::Param("id".to_string()),
                Segment::Wildcard("rest".to_string()),
            ]
        );
    }

    #[test]
    fn parse_pattern_rejects_wildcard_not_last() {
        assert_eq!(
            parse_pattern("/a/{*w}/b"),
            Err(RoutePatternError::WildcardNotLast("{*w}".to_string()))
        );
    }

    #[test]
    fn parse_pattern_rejects_empty_wildcard_name() {
        assert_eq!(
            parse_pattern("/{*}"),
            Err(RoutePatternError::EmptyParamName)
        );
    }

    #[test]
    fn parse_pattern_rejects_invalid_wildcard_name_chars() {
        assert_eq!(
            parse_pattern("/{*na-me}"),
            Err(RoutePatternError::InvalidParamName("na-me".to_string()))
        );
    }

    #[test]
    fn parse_pattern_rejects_duplicate_name_between_param_and_wildcard() {
        // `{name}` と `{*name}` は名前空間を共有する。
        assert_eq!(
            parse_pattern("/{x}/{*x}"),
            Err(RoutePatternError::DuplicateParamName("x".to_string()))
        );
    }

    #[test]
    fn parse_pattern_rejects_mixed_wildcard_segment() {
        assert_eq!(
            parse_pattern("/a/x{*w}"),
            Err(RoutePatternError::MixedSegment("x{*w}".to_string()))
        );
    }

    #[test]
    fn match_segments_wildcard_binds_multi_segment_value_with_slashes() {
        let segments = parse_pattern("/static/{*path}").unwrap();
        let target = ["static", "css", "app.css"];
        let params = match_segments(&segments, &target, "/static/css/app.css").unwrap();
        assert_eq!(params.get("path"), Some("css/app.css"));
    }

    #[test]
    fn match_segments_wildcard_binds_single_segment_value() {
        let segments = parse_pattern("/static/{*path}").unwrap();
        let target = ["static", "app.css"];
        let params = match_segments(&segments, &target, "/static/app.css").unwrap();
        assert_eq!(params.get("path"), Some("app.css"));
    }

    #[test]
    fn match_segments_wildcard_rejects_zero_segments() {
        // ワイルドカードは 1 個以上のセグメントを要求する（0 セグメント不一致）。
        let segments = parse_pattern("/static/{*path}").unwrap();
        let target = ["static"];
        assert!(match_segments(&segments, &target, "/static").is_none());
    }

    #[test]
    fn match_segments_wildcard_rejects_trailing_slash_empty_tail_segment() {
        // 末尾スラッシュは空セグメントを生み `is_safe_segment_value` で拒否される。
        let segments = parse_pattern("/static/{*path}").unwrap();
        let target = ["static", ""];
        assert!(match_segments(&segments, &target, "/static/").is_none());
    }

    #[test]
    fn match_segments_wildcard_rejects_dot_dotdot_in_tail_segments() {
        // 吸収する全セグメントにパス走査対策が個別適用される（受け入れ条件 2）。
        let segments = parse_pattern("/static/{*path}").unwrap();
        assert!(match_segments(&segments, &["static", ".."], "/static/..").is_none());
        assert!(match_segments(&segments, &["static", "a", ".."], "/static/a/..").is_none());
    }

    #[test]
    fn match_segments_wildcard_rejects_query_and_fragment_in_tail_segments() {
        let segments = parse_pattern("/static/{*path}").unwrap();
        assert!(match_segments(&segments, &["static", "a?b"], "/static/a?b").is_none());
        assert!(match_segments(&segments, &["static", "a", "b#c"], "/static/a/b#c").is_none());
    }

    #[test]
    fn match_segments_wildcard_does_not_decode_percent_encoding() {
        // 非デコード契約（モジュール doc「マッチング方針」節）はワイルドカードにも
        // 適用される。`%2e%2e` はリテラルのまま束縛され `..` として解釈されない。
        let segments = parse_pattern("/static/{*path}").unwrap();
        let params = match_segments(&segments, &["static", "%2e%2e"], "/static/%2e%2e").unwrap();
        assert_eq!(params.get("path"), Some("%2e%2e"));
    }
}
