//! body フレーミングの意味解釈（sans-IO）。
//!
//! [`request::parse_request_head`] が構文的に正しいと判定したヘッダ列から、
//! 「body を何バイト読むべきか」を決定する純関数を提供する。ソケット読み取り
//! （[`crate::connection::read_request`]、TASK-1.3-2 / #67）はここで決まった
//! バイト数だけ読み取る責務を持ち、本モジュール自体は I/O を一切行わない。
//!
//! `Transfer-Encoding` は本マイルストーンでは chunked 未対応のため一律拒否する。
//! `Content-Length` の重複や不正な値も安全側（拒否）に倒す。これはリクエスト
//! スマグリング（前段プロキシとの解釈差異によるインジェクション）対策を兼ねる
//! （.claude/rules/security.md）。

use crate::request::RequestHead;

/// body として許容する最大バイト数（暫定上限）。
///
/// リソース枯渇（DoS）対策。`Content-Length` がこの値を超える場合は
/// [`BodyError::BodyTooLarge`] として拒否する。この上限を設定可能にすることは
/// サーバビルダー設計（TASK-1.4 以降）のスコープであり、本モジュールでは
/// 固定値として扱う。
pub const MAX_BODY_BYTES: u64 = 1024 * 1024;

/// ヘッドから決定した body のフレーミング。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyLength {
    /// body を持たない（`Content-Length` 不在、または `Content-Length: 0`）。
    None,
    /// 固定長 body。値は 1 以上 [`MAX_BODY_BYTES`] 以下であることを検証済み。
    Fixed(u64),
}

/// [`body_length`] が返しうるエラー。
#[derive(Debug, PartialEq, Eq)]
pub enum BodyError {
    /// `Transfer-Encoding` ヘッダが存在した。
    ///
    /// chunked encoding は本マイルストーンでは未対応であり、値によらず一律
    /// 拒否する。`Content-Length` との共存によるリクエストスマグリングも
    /// 同時に遮断する。
    TransferEncodingUnsupported,
    /// `Content-Length` ヘッダが 2 個以上存在した。
    ///
    /// 値が同一であっても、前段プロキシとの解釈差異を生む余地を残さないため
    /// 安全側で一律拒否する。
    DuplicateContentLength,
    /// `Content-Length` の値が ASCII digit のみで構成される非負整数でない
    /// （符号・空白・カンマ区切り・空文字列・オーバーフロー等）。
    InvalidContentLength,
    /// `Content-Length` が [`MAX_BODY_BYTES`] を超過した。
    BodyTooLarge,
}

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            BodyError::TransferEncodingUnsupported => "Transfer-Encoding is not supported",
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
/// `Transfer-Encoding` が 1 つでも存在すれば拒否し、`Content-Length` は
/// [`RequestHead::headers`] で全件走査して重複・構文・上限を検証する。
///
/// # Examples
///
/// ```
/// use bf_http::body::{body_length, BodyLength};
/// use bf_http::request::{parse_request_head, ParseOutcome};
///
/// let buf = b"POST /items HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// assert_eq!(body_length(&head), Ok(BodyLength::Fixed(4)));
/// ```
pub fn body_length(head: &RequestHead) -> Result<BodyLength, BodyError> {
    if head
        .headers()
        .any(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"))
    {
        return Err(BodyError::TransferEncodingUnsupported);
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
        Some(n) if n > MAX_BODY_BYTES => Err(BodyError::BodyTooLarge),
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
    fn transfer_encoding_is_rejected() {
        let head = head_of(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n");
        assert_eq!(
            body_length(&head),
            Err(BodyError::TransferEncodingUnsupported)
        );
    }

    #[test]
    fn transfer_encoding_with_content_length_is_rejected() {
        let head =
            head_of(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 4\r\n\r\n");
        assert_eq!(
            body_length(&head),
            Err(BodyError::TransferEncodingUnsupported)
        );
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
}
