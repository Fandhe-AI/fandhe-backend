//! body フレーミングの意味解釈（sans-IO）。
//!
//! [`crate::request::parse_request_head`] が構文的に正しいと判定したヘッダ列から、
//! 「body を何バイト読むべきか」を決定する純関数を提供する。ソケット読み取り
//! （[`crate::connection::read_request`]、TASK-1.3-2 / #67）はここで決まった
//! バイト数だけ読み取る責務を持ち、本モジュール自体は I/O を一切行わない。
//!
//! `Transfer-Encoding` は HTTP/1.1 かつ値が単独の `chunked` である場合のみ
//! [`BodyLength::Chunked`] として受理する（イシュー #181）。それ以外
//! （HTTP/1.0 + TE・`gzip` 等の他 coding・複数 TE ヘッダ・`chunked, chunked`
//! のような多重指定）は一律拒否する。`Content-Length` の重複や不正な値、
//! および `Content-Length` と `Transfer-Encoding: chunked` の共存も安全側
//! （拒否）に倒す。これはリクエストスマグリング（前段プロキシとの解釈差異
//! によるインジェクション）対策を兼ねる（.claude/rules/security.md）。
//! 実際の chunked デコードは [`crate::chunked::ChunkedDecoder`]（sans-IO）が
//! 担い、本モジュールは「body をどう読むべきか」の意味決定のみを行う。

use crate::request::{HttpVersion, RequestHead};

/// body として許容する最大バイト数（既定値）。
///
/// リソース枯渇（DoS）対策。`Content-Length` がこの値を超える場合は
/// [`BodyError::BodyTooLarge`] として拒否する。[`body_length`] はこの既定値を
/// 使うが、`Server::max_body_bytes`（イシュー #311）で上限を上書きした場合は
/// [`body_length_with_limit`] が呼び出し元
/// （[`crate::connection::read_request_with_limit`]）から渡された値を使う。
pub const MAX_BODY_BYTES: u64 = 1024 * 1024;

/// ヘッドから決定した body のフレーミング。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyLength {
    /// body を持たない（`Content-Length` 不在、または `Content-Length: 0`）。
    None,
    /// 固定長 body。値は 1 以上 [`MAX_BODY_BYTES`] 以下であることを検証済み。
    Fixed(u64),
    /// chunked transfer-coding の body。
    ///
    /// `Transfer-Encoding` が HTTP/1.1 かつ単独の `chunked` 値であることを
    /// 検証済み。実際の読み取り・デコードは
    /// [`crate::connection::read_request`] が [`crate::chunked::ChunkedDecoder`]
    /// へ委譲する。
    Chunked,
}

/// [`body_length`] が返しうるエラー。
#[derive(Debug, PartialEq, Eq)]
pub enum BodyError {
    /// `Transfer-Encoding` が chunked 以外・不正な形で指定された。
    ///
    /// HTTP/1.0 での指定・`gzip` 等 chunked 以外の coding・複数の
    /// `Transfer-Encoding` ヘッダ・`chunked, chunked` のような多重指定を
    /// 一律拒否する（前段プロキシとの解釈差異を生む余地を残さないため）。
    /// 単独の `chunked` は [`BodyLength::Chunked`] として受理する。
    TransferEncodingUnsupported,
    /// `Content-Length` と `Transfer-Encoding: chunked` が共存した。
    ///
    /// RFC 9112 §6.3 が示すリクエストスマグリング対策として、両者の共存は
    /// 値によらず一律拒否する。
    ContentLengthWithChunked,
    /// `Content-Length` ヘッダが 2 個以上存在した。
    ///
    /// 値が同一であっても、前段プロキシとの解釈差異を生む余地を残さないため
    /// 安全側で一律拒否する。
    DuplicateContentLength,
    /// `Content-Length` の値が ASCII digit のみで構成される非負整数でない
    /// （符号・空白・カンマ区切り・空文字列・オーバーフロー等）。
    InvalidContentLength,
    /// `Content-Length` が上限（既定は [`MAX_BODY_BYTES`]、
    /// `Server::max_body_bytes` で上書き可）を超過した。
    BodyTooLarge,
}

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            BodyError::TransferEncodingUnsupported => "Transfer-Encoding is not supported",
            BodyError::ContentLengthWithChunked => {
                "Content-Length must not be combined with Transfer-Encoding: chunked"
            }
            BodyError::DuplicateContentLength => "duplicate Content-Length header",
            BodyError::InvalidContentLength => "invalid Content-Length value",
            BodyError::BodyTooLarge => "Content-Length exceeds MAX_BODY_BYTES",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for BodyError {}

/// `head` から body フレーミングを決定する。
///
/// `Transfer-Encoding` は HTTP/1.1 かつヘッダが単独 1 行・値が OWS trim 後に
/// ASCII 大小無視で厳密に `chunked` のみである場合に限り
/// [`BodyLength::Chunked`] として受理する。それ以外（HTTP/1.0 での指定・
/// `gzip` 等の他 coding・複数 `Transfer-Encoding` ヘッダ）はすべて
/// [`BodyError::TransferEncodingUnsupported`] として拒否する。`chunked` を
/// 受理する場合、`Content-Length` が同時に存在すれば
/// [`BodyError::ContentLengthWithChunked`] として拒否する
/// （RFC 9112 §6.3 のリクエストスマグリング対策）。
///
/// `Transfer-Encoding` が存在しない場合は `Content-Length` を
/// [`RequestHead::headers`] で全件走査して重複・構文・上限を検証する。
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::body::{body_length, BodyLength};
/// use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
///
/// let buf = b"POST /items HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// assert_eq!(body_length(&head), Ok(BodyLength::Fixed(4)));
/// ```
///
/// `Transfer-Encoding: chunked` 単独指定は [`BodyLength::Chunked`] になる。
///
/// ```
/// use fandhe_backend_http::body::{body_length, BodyLength};
/// use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
///
/// let buf = b"POST /items HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n0\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// assert_eq!(body_length(&head), Ok(BodyLength::Chunked));
/// ```
///
/// 既定の上限は [`MAX_BODY_BYTES`] と一致する（薄い wrapper であることの固定）。
///
/// ```
/// use fandhe_backend_http::body::{body_length, body_length_with_limit, MAX_BODY_BYTES};
/// use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
///
/// let buf = b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// assert_eq!(body_length(&head), body_length_with_limit(&head, MAX_BODY_BYTES));
/// ```
pub fn body_length(head: &RequestHead) -> Result<BodyLength, BodyError> {
    body_length_with_limit(head, MAX_BODY_BYTES)
}

/// `head` から body フレーミングを、`max_body_bytes` を上限として決定する。
///
/// [`body_length`] の一般化版。`Server::max_body_bytes`（イシュー #311）で
/// 利用者が上限を上書きした場合に、コアの `handle_connection_with_permit`
/// （`crates/core/src/server.rs`）経由でこの上限が渡される。判定ロジック自体は
/// [`body_length`] と同一で、`MAX_BODY_BYTES` 参照箇所のみ引数化している。
///
/// # 上限値の扱い
///
/// - `max_body_bytes == 0`: body を持つリクエストを一律拒否する
///   （`Content-Length` が 1 以上ならすべて [`BodyError::BodyTooLarge`]）。
///   `Content-Length: 0` またはヘッダ不在は body なしの正常系
///   （[`BodyLength::None`]）として引き続き受理する。「より厳しい側」への
///   設定でありフェイルクローズ方向のため許容する
/// - 極端な大値（例 `u64::MAX`）: 拒否せずそのまま上限として使う。
///   上限緩和はリソース枯渇（DoS）耐性の後退であり、`Server::max_body_bytes`
///   の呼び出し元（利用者）の責務とする
///
/// # Examples
///
/// 上限 `0` は body を持たないリクエストのみ受理する。
///
/// ```
/// use fandhe_backend_http::body::{body_length_with_limit, BodyError, BodyLength};
/// use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
///
/// fn head_of(buf: &[u8]) -> fandhe_backend_http::request::RequestHead {
///     match parse_request_head(buf).unwrap() {
///         ParseOutcome::Complete { head, .. } => head,
///         ParseOutcome::Incomplete => unreachable!(),
///     }
/// }
///
/// let with_body = head_of(b"POST / HTTP/1.1\r\nContent-Length: 1\r\n\r\nx");
/// assert_eq!(
///     body_length_with_limit(&with_body, 0),
///     Err(BodyError::BodyTooLarge)
/// );
///
/// let without_body = head_of(b"GET / HTTP/1.1\r\n\r\n");
/// assert_eq!(body_length_with_limit(&without_body, 0), Ok(BodyLength::None));
/// ```
pub fn body_length_with_limit(
    head: &RequestHead,
    max_body_bytes: u64,
) -> Result<BodyLength, BodyError> {
    let has_content_length = head
        .headers()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));

    let mut transfer_encodings = head
        .headers()
        .filter(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"))
        .map(|(_, value)| value);
    if let Some(first) = transfer_encodings.next() {
        let is_single_chunked =
            transfer_encodings.next().is_none() && first.trim().eq_ignore_ascii_case("chunked");
        if head.version != HttpVersion::Http11 || !is_single_chunked {
            return Err(BodyError::TransferEncodingUnsupported);
        }
        if has_content_length {
            return Err(BodyError::ContentLengthWithChunked);
        }
        return Ok(BodyLength::Chunked);
    }

    let mut content_length: Option<u64> = None;
    for (name, value) in head.headers() {
        if !name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        if content_length.is_some() {
            return Err(BodyError::DuplicateContentLength);
        }
        content_length = Some(parse_content_length(value)?);
    }

    match content_length {
        None | Some(0) => Ok(BodyLength::None),
        Some(n) if n > max_body_bytes => Err(BodyError::BodyTooLarge),
        Some(n) => Ok(BodyLength::Fixed(n)),
    }
}

/// `Content-Length` の値を厳密な非負整数として解析する。
///
/// ASCII digit（`0`-`9`）のみを許容し、符号・空白・カンマ区切り・空文字列は
/// 拒否する。`u64` の範囲を超える値もオーバーフローとして拒否する。
fn parse_content_length(value: &str) -> Result<u64, BodyError> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(BodyError::InvalidContentLength);
    }
    value
        .parse::<u64>()
        .map_err(|_| BodyError::InvalidContentLength)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{ParseOutcome, parse_request_head};

    fn head_of(buf: &[u8]) -> RequestHead {
        match parse_request_head(buf).expect("parse should succeed") {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("expected Complete"),
        }
    }

    #[test]
    fn no_content_length_means_none() {
        let head = head_of(b"GET / HTTP/1.1\r\n\r\n");
        assert_eq!(body_length(&head), Ok(BodyLength::None));
    }

    #[test]
    fn content_length_zero_means_none() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
        assert_eq!(body_length(&head), Ok(BodyLength::None));
    }

    #[test]
    fn content_length_four() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd");
        assert_eq!(body_length(&head), Ok(BodyLength::Fixed(4)));
    }

    #[test]
    fn duplicate_content_length_same_value_is_rejected() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 4\r\nContent-Length: 4\r\n\r\n");
        assert_eq!(body_length(&head), Err(BodyError::DuplicateContentLength));
    }

    #[test]
    fn duplicate_content_length_different_value_is_rejected() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 4\r\nContent-Length: 5\r\n\r\n");
        assert_eq!(body_length(&head), Err(BodyError::DuplicateContentLength));
    }

    #[test]
    fn non_digit_content_length_is_rejected() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 4a\r\n\r\n");
        assert_eq!(body_length(&head), Err(BodyError::InvalidContentLength));
    }

    #[test]
    fn signed_content_length_is_rejected() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: +4\r\n\r\n");
        assert_eq!(body_length(&head), Err(BodyError::InvalidContentLength));
    }

    #[test]
    fn empty_content_length_is_rejected() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: \r\n\r\n");
        assert_eq!(body_length(&head), Err(BodyError::InvalidContentLength));
    }

    #[test]
    fn comma_separated_content_length_is_rejected() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 4, 4\r\n\r\n");
        assert_eq!(body_length(&head), Err(BodyError::InvalidContentLength));
    }

    #[test]
    fn overflowing_content_length_is_rejected() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 99999999999999999999999\r\n\r\n");
        assert_eq!(body_length(&head), Err(BodyError::InvalidContentLength));
    }

    #[test]
    fn body_too_large_is_rejected() {
        let value = (MAX_BODY_BYTES + 1).to_string();
        let buf = format!("POST / HTTP/1.1\r\nContent-Length: {value}\r\n\r\n");
        let head = head_of(buf.as_bytes());
        assert_eq!(body_length(&head), Err(BodyError::BodyTooLarge));
    }

    #[test]
    fn body_at_exact_limit_is_accepted() {
        let value = MAX_BODY_BYTES.to_string();
        let buf = format!("POST / HTTP/1.1\r\nContent-Length: {value}\r\n\r\n");
        let head = head_of(buf.as_bytes());
        assert_eq!(body_length(&head), Ok(BodyLength::Fixed(MAX_BODY_BYTES)));
    }

    #[test]
    fn transfer_encoding_chunked_is_accepted() {
        // イシュー #181: HTTP/1.1 かつ単独 `chunked` 指定は受理する。
        let head = head_of(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n");
        assert_eq!(body_length(&head), Ok(BodyLength::Chunked));
    }

    #[test]
    fn transfer_encoding_chunked_case_insensitive_is_accepted() {
        let head = head_of(b"POST / HTTP/1.1\r\nTransfer-Encoding: CHUNKED\r\n\r\n");
        assert_eq!(body_length(&head), Ok(BodyLength::Chunked));
    }

    #[test]
    fn transfer_encoding_other_coding_is_rejected() {
        let head = head_of(b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n");
        assert_eq!(
            body_length(&head),
            Err(BodyError::TransferEncodingUnsupported)
        );
    }

    #[test]
    fn transfer_encoding_multiple_codings_in_one_line_is_rejected() {
        let head = head_of(b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n\r\n");
        assert_eq!(
            body_length(&head),
            Err(BodyError::TransferEncodingUnsupported)
        );
    }

    #[test]
    fn transfer_encoding_duplicate_header_lines_is_rejected() {
        let head = head_of(
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n",
        );
        assert_eq!(
            body_length(&head),
            Err(BodyError::TransferEncodingUnsupported)
        );
    }

    #[test]
    fn transfer_encoding_chunked_on_http10_is_rejected() {
        // chunked は HTTP/1.0 では未定義のため拒否する。
        let head = head_of(b"POST / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n");
        assert_eq!(
            body_length(&head),
            Err(BodyError::TransferEncodingUnsupported)
        );
    }

    #[test]
    fn transfer_encoding_chunked_with_content_length_is_rejected() {
        // イシュー #181: 共存はリクエストスマグリング対策として拒否するが、
        // TransferEncodingUnsupported ではなく専用エラーへ意味を明確化する。
        let head =
            head_of(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 4\r\n\r\n");
        assert_eq!(body_length(&head), Err(BodyError::ContentLengthWithChunked));
    }

    #[test]
    fn leading_whitespace_content_length_is_rejected() {
        // OWS trim 後の値のみを検証する request.rs の契約により " 4" のような
        // 生の先頭空白は本モジュールに渡る前に trim 済みだが、値自体に空白が
        // 残るケース（trim では除去されない内部空白）を明示的に固定する。
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 4 4\r\n\r\n");
        assert_eq!(body_length(&head), Err(BodyError::InvalidContentLength));
    }

    #[test]
    fn body_too_large_boundary_plus_one_is_rejected_at_exact_plus_one() {
        // body_at_exact_limit_is_accepted（ちょうど MAX_BODY_BYTES）との対で
        // +1 の境界を固定する。
        let value = (MAX_BODY_BYTES + 1).to_string();
        let buf = format!("POST / HTTP/1.1\r\nContent-Length: {value}\r\n\r\n");
        let head = head_of(buf.as_bytes());
        assert_eq!(body_length(&head), Err(BodyError::BodyTooLarge));
    }

    #[test]
    fn body_error_display_messages_are_stable() {
        // Display 文言の固定（PoC-9 教訓）。RequestError::Body 経由で上位に
        // そのまま連結されるため、文言変化は呼び出し元のエラーメッセージにも
        // 波及する。
        assert_eq!(
            BodyError::TransferEncodingUnsupported.to_string(),
            "Transfer-Encoding is not supported"
        );
        assert_eq!(
            BodyError::ContentLengthWithChunked.to_string(),
            "Content-Length must not be combined with Transfer-Encoding: chunked"
        );
        assert_eq!(
            BodyError::DuplicateContentLength.to_string(),
            "duplicate Content-Length header"
        );
        assert_eq!(
            BodyError::InvalidContentLength.to_string(),
            "invalid Content-Length value"
        );
        assert_eq!(
            BodyError::BodyTooLarge.to_string(),
            "Content-Length exceeds MAX_BODY_BYTES"
        );
    }

    #[test]
    fn body_error_implements_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<BodyError>();
    }

    #[test]
    fn body_length_with_limit_matches_default_wrapper() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd");
        assert_eq!(
            body_length(&head),
            body_length_with_limit(&head, MAX_BODY_BYTES)
        );
    }

    #[test]
    fn body_length_with_limit_custom_limit_accepts_at_boundary() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd");
        assert_eq!(body_length_with_limit(&head, 4), Ok(BodyLength::Fixed(4)));
    }

    #[test]
    fn body_length_with_limit_custom_limit_rejects_over_boundary() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nabcde");
        assert_eq!(
            body_length_with_limit(&head, 4),
            Err(BodyError::BodyTooLarge)
        );
    }

    #[test]
    fn body_length_with_limit_zero_rejects_any_body() {
        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 1\r\n\r\nx");
        assert_eq!(
            body_length_with_limit(&head, 0),
            Err(BodyError::BodyTooLarge)
        );
    }

    #[test]
    fn body_length_with_limit_zero_accepts_no_body() {
        let head = head_of(b"GET / HTTP/1.1\r\n\r\n");
        assert_eq!(body_length_with_limit(&head, 0), Ok(BodyLength::None));

        let head = head_of(b"POST / HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
        assert_eq!(body_length_with_limit(&head, 0), Ok(BodyLength::None));
    }

    #[test]
    fn body_length_with_limit_extreme_large_limit_is_accepted() {
        // 極端な大値は拒否せずそのまま上限として使う（利用者責務、doc 明記済み）。
        // 既定 MAX_BODY_BYTES（1_048_576）を明確に超える値を使い、「引き上げた上限が
        // 実際に効いている」ことを検証する（既定値のままでも通ってしまう値だと
        // max_body_bytes 引数が無視されていても検知できないため）。
        let value = MAX_BODY_BYTES + 1;
        let buf = format!("POST / HTTP/1.1\r\nContent-Length: {value}\r\n\r\n");
        let head = head_of(buf.as_bytes());
        assert_eq!(
            body_length_with_limit(&head, u64::MAX),
            Ok(BodyLength::Fixed(value))
        );
    }
}
