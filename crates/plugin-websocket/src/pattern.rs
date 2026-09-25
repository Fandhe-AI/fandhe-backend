//! WS パスの `{name}` パラメータ抽出パターンマッチ（イシュー #674、親 #673）。
//!
//! # 背景・親 Issue #673 の「着手前に確認」事項への回答
//!
//! fandhe-browser の CDP 互換サーバーが `/devtools/browser/{id}` /
//! `/devtools/page/{id}` の 2 系統で WS を受けるために必要な、パスパラメータ
//! 付き WebSocket ルーティングの最初の子タスク。親 Issue は「(a)
//! `crates/routes` の `match_segments` を下位クレートへ切り出して共有するか、
//! (b) `plugin-websocket` 内に独立実装するか」を確認事項として挙げているが、
//! 本クレートは `fandhe-backend-core`・`fandhe-backend-routes` のいずれにも
//! 依存できない（`docs/design/plugin-boundary.md` 6.1 節・`lib.rs` の
//! 「workspace 内での依存方向」節。コア → 本クレートの単方向 optional 依存
//! のみが許される設計上の要件）ため、**(b) 独立実装**として本モジュールを
//! 追加する。(a)（新規下位クレートへの切り出し）は crates.io lockstep 公開
//! 対象 13 クレートへの新規クレート追加という公開 API 面の大きな変更を伴い、
//! 本 Issue のスコープを超えるため見送った（将来 (a) へ巻き上げることは
//! 妨げない）。
//!
//! # `fandhe_backend_routes::pattern` との差分
//!
//! 本モジュールは CDP のユースケース（単一セグメントの ID 抽出）専用に
//! 最小化した設計であり、`routes::pattern` とは次の 2 点が異なる:
//!
//! - 末尾ワイルドカードセグメント `{*name}`（イシュー #317）には対応しない
//!   （CDP のパスはいずれも単一セグメントの ID のみを要求するため、
//!   YAGNI でスコープ外とする）
//! - パラメータを 1 つも含まないパターン（`{`/`}` を含まない文字列）は、
//!   検証を一切行わずそのまま完全一致として扱う。
//!   これは既存 `WebSocketConfig::path`（任意の `String`、無検証）の挙動を
//!   壊さないための契約であり、`WebSocketConfig` へのパターン登録は
//!   `WebSocketConfig::with_path_pattern`（イシュー #675）として実装済み
//!
//! # `WebSocketConfig` / `handshake::matches` への配線について（イシュー #675）
//!
//! `PathPattern::parse` / `match_path` は
//! [`crate::config::WebSocketConfig::with_path_pattern`] から呼ばれ、構築時
//! 検証済みの `PathPattern` を `WebSocketConfig` へ保持させる。
//! `crate::handshake::match_config_path`（非公開のためリンク不可）が登録済み
//! パターンの有無で完全一致とパターン照合を切り替え、抽出結果
//! `Option<PathParams<'_>>` を返す共有ヘルパーとして機能する。
//! `crate::handshake::matches`（`UpgradeHandler::matches` が要求する同期 bool
//! API）はこれを `.is_some()` で真偽値に落とすのみで、抽出結果自体は使わない。
//!
//! 抽出した [`PathParams`] をユーザーハンドラへ渡す経路はイシュー #676 で
//! 実装済み。`crate::handle_upgrade` が 101 応答送出成功後、
//! `match_config_path` の結果を所有 `Vec<(String, String)>` へコピーして
//! [`crate::handler::WsOpenContext`] へ渡し、ハンドラは
//! `WsOpenContext::param` / `WsOpenContext::params` で参照できる。

use std::error::Error;
use std::fmt;

/// パターン文字列 1 件が持てるセグメント数の上限。
///
/// パターンはコード内固定値として渡される信頼できる入力だが、将来的な
/// 間接構築経路（設定ファイル等からの動的パターン登録）を見越した防御的
/// 上限として設ける（`crates/http/src/query.rs` の `MAX_QUERY_PAIRS` 等と
/// 同型の設計方針）。
pub const MAX_PATTERN_SEGMENTS: usize = 32;

/// パターン内 1 セグメントが許容する最大バイト数。
///
/// リテラルセグメント・パラメータ名のいずれにも適用する。上記
/// [`MAX_PATTERN_SEGMENTS`] と同じ防御的上限としての位置づけ。
pub const MAX_SEGMENT_BYTES: usize = 256;

/// [`PathPattern::parse`] の構築時検証エラー。
///
/// 本クレートは外部依存を最小限に絞る契約（`Cargo.toml` 参照）のため
/// `thiserror` を使わず、`crates/routes/src/pattern.rs::RoutePatternError` と
/// 同じ手法（`Display` + `std::error::Error` 手実装）で書く。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathPatternError {
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
    /// `//`（連続スラッシュ）や末尾 `/` により空セグメントが生じた。
    EmptySegment,
    /// パターンのセグメント数が [`MAX_PATTERN_SEGMENTS`] を超過した。
    TooManySegments,
    /// いずれかのセグメントのバイト数が [`MAX_SEGMENT_BYTES`] を超過した。
    SegmentTooLong,
}

impl fmt::Display for PathPatternError {
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
            Self::TooManySegments => {
                write!(
                    f,
                    "パターンのセグメント数が上限（{MAX_PATTERN_SEGMENTS}）を超過しています"
                )
            }
            Self::SegmentTooLong => {
                write!(
                    f,
                    "セグメントのバイト数が上限（{MAX_SEGMENT_BYTES}）を超過しています"
                )
            }
        }
    }
}

impl Error for PathPatternError {}

/// パターン内の 1 セグメント（非公開: `PathPattern` の公開メソッド経由のみで
/// 扱わせ、private-in-public を避ける）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// リテラル文字列との完全一致を要求するセグメント。
    Literal(String),
    /// `{name}` — 非空の 1 セグメントにマッチしパラメータとして束縛する。
    Param(String),
}

/// パース済みパターンの内部表現（非公開）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternKind {
    /// `{`/`}` を含まない入力（検証なしの完全一致、モジュール doc
    /// 「`routes::pattern` との差分」節を参照）。
    Exact(String),
    /// `{name}` を 1 つ以上含む入力（セグメント単位の照合）。
    Segments(Vec<Segment>),
}

/// WS パスパターン（構築時検証済み・不透明型）。
///
/// `{name}` 形式の単一セグメントパラメータを含むパスパターンとリクエスト
/// パスを照合し、値を取り出す。`crates/routes/src/pattern.rs` の
/// `RoutePatternError`/`Segment`/`match_segments` に相当するが、本クレートは
/// `fandhe-backend-routes` に依存できないため独立実装する（モジュール doc
/// 「背景」節を参照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPattern {
    kind: PatternKind,
}

impl PathPattern {
    /// `pattern` をパースする。構築時検証（fail-closed）。
    ///
    /// `{`/`}` を 1 つも含まない入力は検証なしでそのまま完全一致パターンと
    /// して受理する（既存 `WebSocketConfig::path` の挙動を壊さないための
    /// 契約、モジュール doc「`routes::pattern` との差分」節）。`{`/`}` を
    /// 含む入力は次を検証する:
    ///
    /// - 先頭が `/` であること（[`PathPatternError::MissingLeadingSlash`]）
    /// - `/` 区切りの各セグメントが空でないこと
    ///   （[`PathPatternError::EmptySegment`]）
    /// - `{name}` セグメントはリテラルとの混在がないこと
    ///   （[`PathPatternError::MixedSegment`]）
    /// - パラメータ名が非空・`[A-Za-z0-9_]` のみで構成されること
    ///   （[`PathPatternError::EmptyParamName`] /
    ///   [`PathPatternError::InvalidParamName`]）
    /// - パラメータ名がパターン内で重複しないこと
    ///   （[`PathPatternError::DuplicateParamName`]）
    /// - セグメント数・各セグメントのバイト数が [`MAX_PATTERN_SEGMENTS`] /
    ///   [`MAX_SEGMENT_BYTES`] 以下であること
    ///   （[`PathPatternError::TooManySegments`] /
    ///   [`PathPatternError::SegmentTooLong`]）
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_websocket::pattern::PathPattern;
    ///
    /// let pattern = PathPattern::parse("/devtools/page/{id}").unwrap();
    /// let params = pattern.match_path("/devtools/page/ABC").unwrap();
    /// assert_eq!(params.get("id"), Some("ABC"));
    /// ```
    pub fn parse(pattern: &str) -> Result<Self, PathPatternError> {
        if !pattern.contains('{') && !pattern.contains('}') {
            // パラメータなしパターンは無検証で完全一致として受理する。
            return Ok(Self {
                kind: PatternKind::Exact(pattern.to_string()),
            });
        }

        if !pattern.starts_with('/') {
            return Err(PathPatternError::MissingLeadingSlash);
        }

        let raw_segments: Vec<&str> = pattern
            .strip_prefix('/')
            .unwrap_or(pattern)
            .split('/')
            .collect();

        if raw_segments.len() > MAX_PATTERN_SEGMENTS {
            return Err(PathPatternError::TooManySegments);
        }

        let mut segments = Vec::with_capacity(raw_segments.len());
        let mut seen_names: Vec<&str> = Vec::new();

        for raw in raw_segments {
            if raw.is_empty() {
                return Err(PathPatternError::EmptySegment);
            }
            if raw.len() > MAX_SEGMENT_BYTES {
                return Err(PathPatternError::SegmentTooLong);
            }

            let starts = raw.starts_with('{');
            let ends = raw.ends_with('}');
            if starts && ends && raw.len() >= 2 {
                let name = &raw[1..raw.len() - 1];
                if name.is_empty() {
                    return Err(PathPatternError::EmptyParamName);
                }
                if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(PathPatternError::InvalidParamName(name.to_string()));
                }
                if seen_names.contains(&name) {
                    return Err(PathPatternError::DuplicateParamName(name.to_string()));
                }
                seen_names.push(name);
                segments.push(Segment::Param(name.to_string()));
            } else if raw.contains('{') || raw.contains('}') {
                // 混在（`a{b}` 等）・片方だけの `{`/`}` はすべて不正パターン
                // として拒否する（暗黙のリテラル解釈によるサイレントな
                // 挙動変化を避ける、`routes::pattern::parse_pattern` と同方針）。
                return Err(PathPatternError::MixedSegment(raw.to_string()));
            } else {
                segments.push(Segment::Literal(raw.to_string()));
            }
        }

        Ok(Self {
            kind: PatternKind::Segments(segments),
        })
    }

    /// `path` と照合し、一致すればパラメータ抽出結果を返す。不一致なら
    /// `None`。
    ///
    /// パラメータなしパターン（完全一致）は `path == pattern` の単純比較
    /// （既存 `handshake::matches` の `path == config.path` と同一挙動）。
    /// パラメータを含むパターンは `path` が `/` で始まらない場合は
    /// 即不一致とし（origin-form 以外を拒否する fail-closed 方針、
    /// `routes::pattern::request_target_segments` と同方針）、以降は
    /// `path.split('/')` のイテレータをパターンのセグメント列と 1 つずつ
    /// 比較する。攻撃者制御の `path` 全体を `Vec` へ `collect()` せず、
    /// パターン側の（構築時に上限を課された固定）セグメント数分しか
    /// 走査しないため、`path` のセグメント数に依存した無制限の確保を
    /// 行わない（`.claude/rules/security.md` リソース枯渇対策）。
    ///
    /// `Segment::Param` に対応する値は、非空・`.`/`..` と不一致・`?`/`#`
    /// 非含有・[`MAX_SEGMENT_BYTES`] 以下であることを満たした場合のみ
    /// パラメータとして束縛する（`routes::pattern::is_safe_segment_value` と
    /// 同等の基準 + 長さ上限）。値は % デコードしない生のスライスをそのまま
    /// 束縛する契約であり、デコードが必要な場合は呼び出し側（将来の
    /// ハンドラ、#676）の責務でデコード後に再検証すること（正規化バイパス
    /// 対策）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_plugin_websocket::pattern::PathPattern;
    ///
    /// let pattern = PathPattern::parse("/ws").unwrap();
    /// assert!(pattern.match_path("/ws").is_some());
    /// assert!(pattern.match_path("/other").is_none());
    /// ```
    #[must_use]
    pub fn match_path<'a>(&self, path: &'a str) -> Option<PathParams<'a>> {
        match &self.kind {
            PatternKind::Exact(exact) => {
                if path == exact {
                    Some(PathParams { params: Vec::new() })
                } else {
                    None
                }
            }
            PatternKind::Segments(segments) => match_segments(segments, path),
        }
    }
}

fn match_segments<'a>(segments: &[Segment], path: &'a str) -> Option<PathParams<'a>> {
    let stripped = path.strip_prefix('/')?;
    let mut target_iter = stripped.split('/');
    let mut params = Vec::new();

    for segment in segments {
        let value = target_iter.next()?;
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
                params.push((name.clone(), value));
            }
        }
    }

    // セグメント数が一致すること（パターンより多くのセグメントが
    // 残っていれば不一致）。
    if target_iter.next().is_some() {
        return None;
    }

    Some(PathParams { params })
}

/// セグメント値がパラメータ束縛として安全か判定する（フェイルクローズの
/// 入力検証、`routes::pattern::is_safe_segment_value` と同等の基準 + 長さ
/// 上限）:
///
/// - 非空であること
/// - `.` / `..` と一致しないこと（パス走査対策）
/// - `?` / `#` を含まないこと（クエリ・フラグメントの過剰キャプチャ防止）
/// - [`MAX_SEGMENT_BYTES`] 以下であること（束縛値の無制限確保対策）
fn is_safe_segment_value(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('?')
        && !value.contains('#')
        && value.len() <= MAX_SEGMENT_BYTES
}

/// パス上のパラメータ名 → 値の抽出結果。
///
/// 値は % デコードしない生の文字列スライスで、照合対象の `path`（借用元）と
/// 同じライフタイムを持つ（値側は追加アロケーションなしのゼロコピー）。
/// パラメータ名はパターン文字列（構築時にパース済み）から取得するため、
/// `String` として複製する（マッチ成立時のみ・パラメータ数ぶんの小さな
/// 複製のみで、DoS 耐性に影響する規模ではない。
/// `crates/routes/src/pattern.rs::PathParams` と同型の設計）。
#[derive(Debug, Default)]
pub struct PathParams<'a> {
    params: Vec<(String, &'a str)>,
}

impl<'a> PathParams<'a> {
    /// `name` に対応する値を返す。未束縛または存在しないパラメータ名なら
    /// `None`。
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- CDP ユースケース（受け入れ基準 1 件目） ---

    #[test]
    fn cdp_devtools_page_pattern_extracts_id() {
        let pattern = PathPattern::parse("/devtools/page/{id}").unwrap();
        let params = pattern.match_path("/devtools/page/ABC").unwrap();
        assert_eq!(params.get("id"), Some("ABC"));
        assert_eq!(params.len(), 1);
        assert!(!params.is_empty());
    }

    // --- セグメント数不一致（受け入れ基準 2 件目） ---

    #[test]
    fn match_path_rejects_too_few_segments() {
        let pattern = PathPattern::parse("/devtools/page/{id}").unwrap();
        assert!(pattern.match_path("/devtools/page").is_none());
    }

    #[test]
    fn match_path_rejects_too_many_segments() {
        let pattern = PathPattern::parse("/devtools/page/{id}").unwrap();
        assert!(pattern.match_path("/devtools/page/ABC/extra").is_none());
    }

    // --- 空セグメント（受け入れ基準 3 件目: panic せず Err） ---

    #[test]
    fn parse_rejects_consecutive_slash_empty_segment() {
        assert_eq!(
            PathPattern::parse("/hello//{name}"),
            Err(PathPatternError::EmptySegment)
        );
    }

    #[test]
    fn parse_rejects_trailing_slash_empty_segment() {
        assert_eq!(
            PathPattern::parse("/hello/{name}/"),
            Err(PathPatternError::EmptySegment)
        );
    }

    // --- 不正なパターン（受け入れ基準 4 件目: panic せず Err） ---

    #[test]
    fn parse_rejects_unclosed_brace() {
        assert_eq!(
            PathPattern::parse("/hello/{name"),
            Err(PathPatternError::MixedSegment("{name".to_string()))
        );
    }

    #[test]
    fn parse_rejects_unopened_brace() {
        assert_eq!(
            PathPattern::parse("/hello/name}"),
            Err(PathPatternError::MixedSegment("name}".to_string()))
        );
    }

    #[test]
    fn parse_rejects_empty_param_name() {
        assert_eq!(
            PathPattern::parse("/hello/{}"),
            Err(PathPatternError::EmptyParamName)
        );
    }

    #[test]
    fn parse_rejects_invalid_param_name_chars() {
        assert_eq!(
            PathPattern::parse("/hello/{na-me}"),
            Err(PathPatternError::InvalidParamName("na-me".to_string()))
        );
    }

    #[test]
    fn parse_rejects_mixed_segment() {
        assert_eq!(
            PathPattern::parse("/hello/a{name}"),
            Err(PathPatternError::MixedSegment("a{name}".to_string()))
        );
    }

    #[test]
    fn parse_rejects_duplicate_param_name() {
        assert_eq!(
            PathPattern::parse("/a/{id}/b/{id}"),
            Err(PathPatternError::DuplicateParamName("id".to_string()))
        );
    }

    #[test]
    fn parse_rejects_missing_leading_slash() {
        assert_eq!(
            PathPattern::parse("hello/{name}"),
            Err(PathPatternError::MissingLeadingSlash)
        );
    }

    // --- パラメータなしパターンの回帰テスト ---

    #[test]
    fn parse_no_param_pattern_behaves_like_equality() {
        let pattern = PathPattern::parse("/ws").unwrap();
        assert!(pattern.match_path("/ws").is_some());
        assert!(pattern.match_path("/ws/").is_none());
        assert!(pattern.match_path("/other").is_none());
    }

    #[test]
    fn parse_no_param_pattern_with_trailing_slash_is_exact() {
        // `{`/`}` を含まないため無検証で受理され、比較も単純な完全一致。
        let pattern = PathPattern::parse("/ws/").unwrap();
        assert!(pattern.match_path("/ws/").is_some());
        assert!(pattern.match_path("/ws").is_none());
    }

    #[test]
    fn parse_empty_pattern_is_exact() {
        let pattern = PathPattern::parse("").unwrap();
        assert!(pattern.match_path("").is_some());
        assert!(pattern.match_path("/").is_none());
    }

    // --- セグメント値の安全性判定 ---

    #[test]
    fn match_path_rejects_dot_and_dotdot_path_traversal() {
        let pattern = PathPattern::parse("/hello/{name}").unwrap();
        assert!(pattern.match_path("/hello/.").is_none());
        assert!(pattern.match_path("/hello/..").is_none());
    }

    #[test]
    fn match_path_rejects_query_and_fragment_chars() {
        let pattern = PathPattern::parse("/hello/{name}").unwrap();
        assert!(pattern.match_path("/hello/a?b").is_none());
        assert!(pattern.match_path("/hello/a#b").is_none());
    }

    #[test]
    fn match_path_rejects_non_origin_form_target() {
        let pattern = PathPattern::parse("/hello/{name}").unwrap();
        assert!(pattern.match_path("hello/alice").is_none());
    }

    #[test]
    fn match_path_does_not_decode_percent_encoding() {
        // 非デコード契約（モジュール doc）を固定化するテスト。`%2e%2e` は
        // literal な文字列としてそのまま束縛され、`..` として解釈・拒否
        // されない（デコードは呼び出し側責務）。
        let pattern = PathPattern::parse("/hello/{name}").unwrap();
        let params = pattern.match_path("/hello/%2e%2e").unwrap();
        assert_eq!(params.get("name"), Some("%2e%2e"));
    }

    #[test]
    fn match_path_accepts_multiple_params() {
        let pattern = PathPattern::parse("/users/{id}/posts/{post_id}").unwrap();
        let params = pattern.match_path("/users/42/posts/7").unwrap();
        assert_eq!(params.get("id"), Some("42"));
        assert_eq!(params.get("post_id"), Some("7"));
        assert_eq!(params.get("missing"), None);
        assert_eq!(params.len(), 2);
        let collected: Vec<_> = params.iter().collect();
        assert_eq!(collected, vec![("id", "42"), ("post_id", "7")]);
    }

    // --- DoS 耐性の上限検証 ---

    #[test]
    fn parse_rejects_too_many_segments() {
        let too_many: String = (0..=MAX_PATTERN_SEGMENTS)
            .map(|i| format!("/{i}"))
            .collect();
        // セグメント数超過を確実に起こすため、最後のセグメントのみ
        // パラメータ化してパターンとして成立させる。
        let pattern = format!("{too_many}/{{id}}");
        assert_eq!(
            PathPattern::parse(&pattern),
            Err(PathPatternError::TooManySegments)
        );
    }

    #[test]
    fn parse_rejects_segment_too_long() {
        let long_literal = "a".repeat(MAX_SEGMENT_BYTES + 1);
        let pattern = format!("/{long_literal}/{{id}}");
        assert_eq!(
            PathPattern::parse(&pattern),
            Err(PathPatternError::SegmentTooLong)
        );
    }

    #[test]
    fn match_path_rejects_param_value_too_long() {
        let pattern = PathPattern::parse("/hello/{name}").unwrap();
        let long_value = "a".repeat(MAX_SEGMENT_BYTES + 1);
        let path = format!("/hello/{long_value}");
        assert!(pattern.match_path(&path).is_none());
    }
}
