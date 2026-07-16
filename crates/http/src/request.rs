//! HTTP/1.1 リクエストライン・ヘッダの sans-IO パーサ。
//!
//! [`parse_request_head`] はソケット I/O を一切持たない純関数（`&[u8]` → 構造体）
//! として実装する。理由は 3 つ:
//! 1. ソケット読み取りループ（TASK-1.3-2 / #67）と責務を分離できる
//! 2. I/O なしでそのまま fuzz（TASK-15.3 / #51）に供せる
//! 3. doc test・単体テストが書きやすく AI ファースト保守性に適う
//!
//! body の読み取り・keep-alive 判定・`Content-Length` / `Transfer-Encoding` の
//! 意味解釈は本モジュールの責務外（TASK-1.3-2 / #67 が担う）。本モジュールは
//! `headers()` で同名ヘッダ全件を出現順で取得できる API を提供し、#67 が
//! 重複 `Content-Length` 等の意味検証をできる構造にとどめる。

/// リクエストヘッド（リクエストライン + ヘッダ + 空行）として許容するバイト数上限。
///
/// この上限はリソース枯渇（DoS）対策であり、`\r\n\r\n`（ヘッダ終端）に到達しない
/// まま累積バイト数がこの値を超えた場合も [`ParseError::HeaderSectionTooLarge`]
/// を返し、無限バッファ成長を防ぐ。
pub const MAX_HEADER_BYTES: usize = 16 * 1024;

/// 1 リクエストで許容するヘッダの最大本数。
///
/// ヘッダ本数を無制限に許すとパース処理・後段のヘッダ格納がリソース枯渇の
/// 攻撃対象になるため、超過時は [`ParseError::TooManyHeaders`] を返す。
pub const MAX_HEADER_COUNT: usize = 100;

/// リクエストラインで受理する HTTP バージョン。
///
/// HTTP/1.0・HTTP/1.1 以外（HTTP/0.9・HTTP/2 等）は
/// [`ParseError::UnsupportedVersion`] として拒否する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersion {
    /// `HTTP/1.0`
    Http10,
    /// `HTTP/1.1`
    Http11,
}

/// パース済みリクエストヘッド（body は含まない）。
///
/// body の読み取りは呼び出し元（ソケット I/O 層、TASK-1.3-2 / #67）の責務。
/// ヘッダは出現順を保持したまま保持し、[`RequestHead::header`] /
/// [`RequestHead::headers`] 経由でのみアクセスさせる（フィールドは非公開）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    /// リクエストメソッド（RFC 9110 tchar のみで構成される token として検証済み）。
    pub method: String,
    /// request-target（SP・制御文字を含まないことを検証済み）。
    pub target: String,
    /// リクエストバージョン（HTTP/1.0 または HTTP/1.1 のみ）。
    pub version: HttpVersion,
    /// 出現順を保持したヘッダ列（名前は大小文字を区別せず比較する契約）。
    headers: Vec<(String, String)>,
}

impl RequestHead {
    /// 大文字小文字を無視して先頭一致するヘッダ値を取得する。
    ///
    /// 同名ヘッダが複数存在する場合は最初の 1 件のみを返す。全件が必要な
    /// 呼び出し元（重複 `Content-Length` 検査等、#67 が担う）は
    /// [`RequestHead::headers`] を使うこと。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_http::request::parse_request_head;
    ///
    /// let buf = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
    /// let outcome = parse_request_head(buf).unwrap();
    /// let head = match outcome {
    ///     bf_http::request::ParseOutcome::Complete { head, .. } => head,
    ///     _ => unreachable!(),
    /// };
    /// assert_eq!(head.header("host"), Some("example.com"));
    /// assert_eq!(head.header("HOST"), Some("example.com"));
    /// assert_eq!(head.header("missing"), None);
    /// ```
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// 全ヘッダを出現順に走査するイテレータを返す。
    ///
    /// 同名ヘッダの重複検査（例: `Content-Length` の重複拒否）は呼び出し元
    /// （#67）が本イテレータを使って行う契約とする。
    ///
    /// # Examples
    ///
    /// ```
    /// use bf_http::request::parse_request_head;
    ///
    /// let buf = b"GET / HTTP/1.1\r\nX-A: 1\r\nX-B: 2\r\nX-A: 3\r\n\r\n";
    /// let outcome = parse_request_head(buf).unwrap();
    /// let head = match outcome {
    ///     bf_http::request::ParseOutcome::Complete { head, .. } => head,
    ///     _ => unreachable!(),
    /// };
    /// let all: Vec<_> = head.headers().collect();
    /// assert_eq!(all, vec![("X-A", "1"), ("X-B", "2"), ("X-A", "3")]);
    /// ```
    pub fn headers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

/// [`parse_request_head`] の成功時の結果。
#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome {
    /// ヘッド解析が完了した。
    ///
    /// `consumed` は `buf` 先頭からヘッダ終端の空行（`\r\n\r\n`）までの
    /// 消費バイト数。呼び出し元は `buf[..consumed]` を読み捨て、残り
    /// （パイプライン済みの次リクエストや body 先頭）を保持する契約。
    Complete {
        /// 解析済みリクエストヘッド。
        head: RequestHead,
        /// ヘッダ終端までの消費バイト数。
        consumed: usize,
    },
    /// ヘッダ終端（`\r\n\r\n`）に未到達。呼び出し元は追加のバイト列を
    /// 読み取ってから再試行する（sans-IO 設計につき本関数はリトライしない）。
    Incomplete,
}

/// [`parse_request_head`] が返しうるエラー。
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// ヘッダ部が [`MAX_HEADER_BYTES`] を超過した（リソース枯渇対策）。
    HeaderSectionTooLarge,
    /// ヘッダ本数が [`MAX_HEADER_COUNT`] を超過した（リソース枯渇対策）。
    TooManyHeaders,
    /// リクエストラインが `method SP target SP version` の形式に違反する。
    InvalidRequestLine,
    /// HTTP/1.0・HTTP/1.1 以外のバージョンが指定された。
    UnsupportedVersion,
    /// ヘッダ名・ヘッダ値が構文違反（tchar 違反・制御文字混入・obs-fold 等）。
    InvalidHeader,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            ParseError::HeaderSectionTooLarge => "header section exceeds MAX_HEADER_BYTES",
            ParseError::TooManyHeaders => "header count exceeds MAX_HEADER_COUNT",
            ParseError::InvalidRequestLine => "invalid request line",
            ParseError::UnsupportedVersion => "unsupported HTTP version",
            ParseError::InvalidHeader => "invalid header",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for ParseError {}

/// `buf` 先頭から 1 リクエスト分のヘッド（リクエストライン + ヘッダ）を解析する。
///
/// sans-IO な純関数であり、ソケット読み取りは行わない。呼び出し元（#67）が
/// ソケットから読み取ったバイト列をそのまま渡し、[`ParseOutcome::Incomplete`]
/// が返れば追加読み取り後に再試行する契約。
///
/// # 受理する構文
///
/// - 行終端は `\r\n` のみ（bare LF・bare CR は拒否）
/// - リクエストラインは `method SP target SP version` の 3 要素固定
/// - メソッドは RFC 9110 tchar のみ、request-target は SP・制御文字を含まない
/// - ヘッダ名は tchar のみ（コロン前の空白は tchar 違反として拒否）、値は
///   前後の OWS を trim し、trim 後の値に HTAB 以外の制御文字を含まない
/// - 継続行（obs-fold）は拒否する
///
/// # Examples
///
/// ```
/// use bf_http::request::{parse_request_head, HttpVersion, ParseOutcome};
///
/// let buf = b"POST /items HTTP/1.1\r\nHost: example.com\r\nContent-Length: 4\r\n\r\nbody";
/// match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, consumed } => {
///         assert_eq!(head.method, "POST");
///         assert_eq!(head.target, "/items");
///         assert_eq!(head.version, HttpVersion::Http11);
///         // "body" はヘッダ終端より後ろにあるため consumed に含まれない。
///         assert_eq!(&buf[consumed..], b"body");
///     }
///     ParseOutcome::Incomplete => unreachable!(),
/// }
/// ```
pub fn parse_request_head(buf: &[u8]) -> Result<ParseOutcome, ParseError> {
    const TERMINATOR: &[u8] = b"\r\n\r\n";

    let terminator_pos = find_subslice(buf, TERMINATOR);

    let header_end = match terminator_pos {
        Some(pos) => {
            let consumed = pos + TERMINATOR.len();
            if consumed > MAX_HEADER_BYTES {
                return Err(ParseError::HeaderSectionTooLarge);
            }
            pos
        }
        None => {
            if buf.len() >= MAX_HEADER_BYTES {
                return Err(ParseError::HeaderSectionTooLarge);
            }
            return Ok(ParseOutcome::Incomplete);
        }
    };

    let header_section = &buf[..header_end];
    let mut lines = split_by_crlf(header_section);

    let request_line = lines.next().ok_or(ParseError::InvalidRequestLine)?;
    let (method, target, version) = parse_request_line(request_line)?;

    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if headers.len() >= MAX_HEADER_COUNT {
            return Err(ParseError::TooManyHeaders);
        }
        headers.push(parse_header_line(line)?);
    }

    let head = RequestHead {
        method,
        target,
        version,
        headers,
    };
    Ok(ParseOutcome::Complete {
        head,
        consumed: header_end + TERMINATOR.len(),
    })
}

/// `haystack` 中で `needle` が最初に現れる位置を返す（見つからなければ `None`）。
///
/// 正規表現やバックトラックを伴わない単純な線形走査であり、病的入力による
/// 計算量爆発（ReDoS 相当）を起こさない。
pub(crate) fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// ヘッダ部（末尾の空行を含まない）を `\r\n` 区切りで分割するイテレータを返す。
///
/// 分割はリテラル `\r\n` の出現位置でのみ行う。bare LF・bare CR はどの区切り
/// にも一致しないため区切られずセグメント内に残り、後続の tchar / 制御文字
/// 検証（[`parse_request_line`] / [`parse_header_line`]）で拒否される。これに
/// より obs-fold（継続行）も、先頭が SP/HTAB で始まる非 token セグメントとして
/// 自然に拒否される。
fn split_by_crlf(section: &[u8]) -> impl Iterator<Item = &[u8]> {
    CrlfSplit {
        rest: Some(section),
    }
}

/// [`split_by_crlf`] の内部イテレータ実装。
struct CrlfSplit<'a> {
    rest: Option<&'a [u8]>,
}

impl<'a> Iterator for CrlfSplit<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.rest?;
        match find_subslice(rest, b"\r\n") {
            Some(pos) => {
                let (line, tail) = (&rest[..pos], &rest[pos + 2..]);
                self.rest = Some(tail);
                Some(line)
            }
            None => {
                self.rest = None;
                Some(rest)
            }
        }
    }
}

/// RFC 9110 の tchar（token 構成文字）判定。
///
/// `!#$%&'*+-.^_`|~` と DIGIT・ALPHA のみを許容する。ヘッダ名・メソッドの
/// token 検証に使う。
fn is_tchar(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
    ) || b.is_ascii_alphanumeric()
}

/// 制御文字（DoS・インジェクション対策で拒否する対象）判定。
///
/// `allow_htab` が `true` の場合のみ HTAB（0x09）を許容する
/// （ヘッダ値の内部 HTAB を許すため）。
fn is_forbidden_ctl(b: u8, allow_htab: bool) -> bool {
    if allow_htab && b == 0x09 {
        return false;
    }
    b < 0x20 || b == 0x7F
}

/// リクエストライン 1 行を `(method, target, version)` に分解する。
fn parse_request_line(line: &[u8]) -> Result<(String, String, HttpVersion), ParseError> {
    if line.iter().any(|&b| is_forbidden_ctl(b, false)) {
        return Err(ParseError::InvalidRequestLine);
    }

    let mut parts = line.split(|&b| b == b' ');
    let method = parts.next().filter(|s| !s.is_empty());
    let target = parts.next().filter(|s| !s.is_empty());
    let version = parts.next().filter(|s| !s.is_empty());
    // 4 要素目が存在する（= SP が 3 個以上ある）場合は 3 要素固定に違反する。
    if parts.next().is_some() {
        return Err(ParseError::InvalidRequestLine);
    }
    let (method, target, version) = match (method, target, version) {
        (Some(m), Some(t), Some(v)) => (m, t, v),
        _ => return Err(ParseError::InvalidRequestLine),
    };

    if !method.iter().all(|&b| is_tchar(b)) {
        return Err(ParseError::InvalidRequestLine);
    }
    // request-target: SP は split で既に除外済み。制御文字は上で行全体を検査済み。
    if target.is_empty() {
        return Err(ParseError::InvalidRequestLine);
    }

    let http_version = parse_version(version)?;

    let method = String::from_utf8(method.to_vec()).map_err(|_| ParseError::InvalidRequestLine)?;
    let target = String::from_utf8(target.to_vec()).map_err(|_| ParseError::InvalidRequestLine)?;

    Ok((method, target, http_version))
}

/// バージョン token（例: `HTTP/1.1`）を [`HttpVersion`] に変換する。
///
/// `HTTP/` prefix を持たない場合は構文違反として [`ParseError::InvalidRequestLine`]、
/// prefix はあるが 1.0/1.1 以外の場合は [`ParseError::UnsupportedVersion`] を返す。
fn parse_version(version: &[u8]) -> Result<HttpVersion, ParseError> {
    const PREFIX: &[u8] = b"HTTP/";
    if !version.starts_with(PREFIX) {
        return Err(ParseError::InvalidRequestLine);
    }
    match &version[PREFIX.len()..] {
        b"1.1" => Ok(HttpVersion::Http11),
        b"1.0" => Ok(HttpVersion::Http10),
        _ => Err(ParseError::UnsupportedVersion),
    }
}

/// ヘッダ行 1 行を `(name, value)` に分解する。
///
/// - ヘッダ名は tchar のみ（コロン直前の空白は tchar 違反として自然に拒否される）
/// - 値は前後の OWS（SP/HTAB）を trim し、trim 後に HTAB 以外の制御文字が
///   残っていれば拒否する
/// - obs-fold（SP/HTAB で始まる継続行）はコロン欠如または名前の tchar 違反として拒否される
/// - 制御文字チェックは RFC 9110 の `obs-text`（0x80–0xFF）を通過させるが、これは
///   UTF-8 マルチバイト列の後続バイトを許容するためであり、生の obs-text を
///   オペークなバイト列として保持する意図ではない。値は本関数の戻り値が
///   `String`（= 有効な UTF-8 保証）であることに従い、最終的に厳密な UTF-8 として
///   検証する。マルチバイト列として不正な生の obs-text（例: 単独の `0xE9`）は
///   [`ParseError::InvalidHeader`] として拒否する
fn parse_header_line(line: &[u8]) -> Result<(String, String), ParseError> {
    let colon_pos = line
        .iter()
        .position(|&b| b == b':')
        .ok_or(ParseError::InvalidHeader)?;
    let (name, value_raw) = (&line[..colon_pos], &line[colon_pos + 1..]);

    if name.is_empty() || !name.iter().all(|&b| is_tchar(b)) {
        return Err(ParseError::InvalidHeader);
    }

    let value = trim_ows(value_raw);
    if value.iter().any(|&b| is_forbidden_ctl(b, true)) {
        return Err(ParseError::InvalidHeader);
    }

    let name = String::from_utf8(name.to_vec()).map_err(|_| ParseError::InvalidHeader)?;
    let value = String::from_utf8(value.to_vec()).map_err(|_| ParseError::InvalidHeader)?;

    Ok((name, value))
}

/// 前後の OWS（SP `0x20` / HTAB `0x09`）を取り除く。
fn trim_ows(bytes: &[u8]) -> &[u8] {
    let is_ows = |b: &u8| *b == b' ' || *b == b'\t';
    let start = bytes.iter().position(|b| !is_ows(b)).unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !is_ows(b))
        .map_or(start, |p| p + 1);
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(buf: &[u8]) -> (RequestHead, usize) {
        match parse_request_head(buf).expect("parse should succeed") {
            ParseOutcome::Complete { head, consumed } => (head, consumed),
            ParseOutcome::Incomplete => panic!("expected Complete, got Incomplete"),
        }
    }

    #[test]
    fn get_without_headers() {
        let buf = b"GET / HTTP/1.1\r\n\r\n";
        let (head, consumed) = complete(buf);
        assert_eq!(head.method, "GET");
        assert_eq!(head.target, "/");
        assert_eq!(head.version, HttpVersion::Http11);
        assert_eq!(head.headers().count(), 0);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn get_with_multiple_headers() {
        let buf = b"GET /path HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\n\r\n";
        let (head, consumed) = complete(buf);
        assert_eq!(head.method, "GET");
        assert_eq!(head.target, "/path");
        assert_eq!(head.header("host"), Some("example.com"));
        assert_eq!(head.header("Accept"), Some("*/*"));
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn post_with_body_is_not_consumed() {
        let buf = b"POST /items HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd";
        let (head, consumed) = complete(buf);
        assert_eq!(head.method, "POST");
        assert_eq!(head.header("content-length"), Some("4"));
        assert_eq!(&buf[consumed..], b"abcd");
    }

    #[test]
    fn http_10_is_accepted() {
        let buf = b"GET / HTTP/1.0\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.version, HttpVersion::Http10);
    }

    #[test]
    fn consumed_excludes_pipelined_next_request() {
        let buf = b"GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\n\r\n";
        let (head, consumed) = complete(buf);
        assert_eq!(head.target, "/a");
        assert_eq!(&buf[consumed..], b"GET /b HTTP/1.1\r\n\r\n");
    }

    #[test]
    fn header_lookup_is_case_insensitive_and_preserves_order() {
        let buf = b"GET / HTTP/1.1\r\nX-A: 1\r\nX-B: 2\r\nX-A: 3\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.header("x-a"), Some("1"));
        let all: Vec<_> = head.headers().collect();
        assert_eq!(all, vec![("X-A", "1"), ("X-B", "2"), ("X-A", "3")]);
    }

    #[test]
    fn incomplete_on_empty_input() {
        assert_eq!(parse_request_head(b""), Ok(ParseOutcome::Incomplete));
    }

    #[test]
    fn incomplete_mid_request_line() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/1."),
            Ok(ParseOutcome::Incomplete)
        );
    }

    #[test]
    fn incomplete_just_before_terminator() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/1.1\r\n\r"),
            Ok(ParseOutcome::Incomplete)
        );
    }

    #[test]
    fn header_section_too_large_when_terminator_missing() {
        let mut buf = vec![b'a'; MAX_HEADER_BYTES];
        assert_eq!(
            parse_request_head(&buf),
            Err(ParseError::HeaderSectionTooLarge)
        );
        buf.truncate(MAX_HEADER_BYTES - 1);
        assert_eq!(parse_request_head(&buf), Ok(ParseOutcome::Incomplete));
    }

    #[test]
    fn header_section_too_large_when_terminator_found_over_budget() {
        let filler = vec![b'a'; MAX_HEADER_BYTES];
        let mut buf = b"GET / HTTP/1.1\r\nX-Big: ".to_vec();
        buf.extend_from_slice(&filler);
        buf.extend_from_slice(b"\r\n\r\n");
        assert_eq!(
            parse_request_head(&buf),
            Err(ParseError::HeaderSectionTooLarge)
        );
    }

    #[test]
    fn too_many_headers() {
        let mut buf = b"GET / HTTP/1.1\r\n".to_vec();
        for i in 0..=MAX_HEADER_COUNT {
            buf.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
        }
        buf.extend_from_slice(b"\r\n");
        assert_eq!(parse_request_head(&buf), Err(ParseError::TooManyHeaders));
    }

    #[test]
    fn invalid_method_character() {
        let buf = b"G@T / HTTP/1.1\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidRequestLine));
    }

    #[test]
    fn request_line_missing_element() {
        assert_eq!(
            parse_request_head(b"GET /\r\n\r\n"),
            Err(ParseError::InvalidRequestLine)
        );
    }

    #[test]
    fn request_line_extra_element() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/1.1 extra\r\n\r\n"),
            Err(ParseError::InvalidRequestLine)
        );
    }

    #[test]
    fn unsupported_version_http_2() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/2.0\r\n\r\n"),
            Err(ParseError::UnsupportedVersion)
        );
    }

    #[test]
    fn unsupported_version_http_0_9() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/0.9\r\n\r\n"),
            Err(ParseError::UnsupportedVersion)
        );
    }

    #[test]
    fn malformed_version_prefix() {
        assert_eq!(
            parse_request_head(b"GET / FOO/1.1\r\n\r\n"),
            Err(ParseError::InvalidRequestLine)
        );
    }

    #[test]
    fn bare_lf_in_request_line_is_rejected() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/1.1\nHost: x\r\n\r\n"),
            Err(ParseError::InvalidRequestLine)
        );
    }

    #[test]
    fn colon_before_space_in_header_name_is_rejected() {
        let buf = b"GET / HTTP/1.1\r\nHost : example.com\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidHeader));
    }

    #[test]
    fn obs_fold_is_rejected() {
        let buf = b"GET / HTTP/1.1\r\nHost: example.com\r\n value\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidHeader));
    }

    #[test]
    fn nul_byte_in_target_is_rejected() {
        let buf = b"GET /\0 HTTP/1.1\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidRequestLine));
    }

    #[test]
    fn control_char_in_header_value_is_rejected() {
        let buf = b"GET / HTTP/1.1\r\nX-Bad: a\0b\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidHeader));
    }

    #[test]
    fn header_missing_colon_is_rejected() {
        let buf = b"GET / HTTP/1.1\r\nHostexample.com\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidHeader));
    }

    #[test]
    fn valid_utf8_multibyte_header_value_is_accepted() {
        // "café" の "é" は UTF-8 で 0xC3 0xA9（obs-text 範囲だが正当なマルチバイト列）。
        let buf = b"GET / HTTP/1.1\r\nX-Name: caf\xC3\xA9\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(
            head.headers().find(|(n, _)| *n == "X-Name").map(|(_, v)| v),
            Some("café")
        );
    }

    #[test]
    fn lone_obs_text_byte_in_header_value_is_rejected() {
        // 単独の 0xE9 は有効な UTF-8 マルチバイト列を構成しないため拒否する。
        let buf = b"GET / HTTP/1.1\r\nX-Bad: a\xE9b\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidHeader));
    }
}
