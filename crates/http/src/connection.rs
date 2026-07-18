//! keep-alive 判定・ソケット読み取りループ（TASK-1.3-2 / #67、TASK-1.3-3 / #68）。
//!
//! [`should_keep_alive`] は `Connection` ヘッダと HTTP バージョンからの意味
//! 判定を行う sans-IO 純関数。[`read_request`] は [`crate::request::parse_request_head`]
//! （構文解析）と [`crate::body::body_length`]（body フレーミング解釈）を組み合わせ、
//! 1 リクエスト分（ヘッド + body）をソケットから読み取る非同期関数であり、本クレート
//! で唯一 tokio（`io-util`）に依存する箇所。`body_length` が `Chunked` と
//! 判定した場合は `read_body_chunked` が [`crate::chunked::ChunkedDecoder`]
//! （sans-IO）へ読み取ったバイト列を渡してデコードする（イシュー #181）。
//!
//! 呼び出し元はコアの接続受理ループ（TASK-1.4 / #13）であり、1 コネクションにつき
//! [`crate::buffer::RecvBuffer`] を 1 つ保持して繰り返し [`read_request`] を呼ぶ
//! 契約とする。パイプライン済みの次リクエスト先頭バイトは `RecvBuffer` に残した
//! まま返すため、呼び出し元は同じ `RecvBuffer` をそのまま次の呼び出しへ渡せば
//! よい。バッファの消費・コンパクション・容量有界化は `RecvBuffer` の責務
//! （TASK-1.3-3 / #68）であり、本モジュールはカーソル操作（`consume`）と
//! 読み取り（`read_chunk`）の呼び出しのみを行う。
//!
//! 読み取り・アイドルタイムアウト（スロークライアント対策）は接続ループ全体の
//! 設計と一体であるため本モジュールの責務外とし、TASK-1.4 側で扱う
//! （.claude/rules/security.md のリソース枯渇対策）。

use tokio::io::AsyncRead;

use crate::body::{BodyError, BodyLength, body_length};
use crate::buffer::RecvBuffer;
use crate::chunked::{ChunkedDecoder, ChunkedError, DecodeOutcome};
use crate::request::{HttpVersion, ParseError, ParseOutcome, RequestHead, parse_request_head};

/// 読み取り済みの 1 リクエスト（ヘッド + body）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// パース済みリクエストヘッド。
    pub head: RequestHead,
    /// body（`Content-Length` 分のバイト列。body なしリクエストでは空）。
    pub body: Vec<u8>,
}

/// [`read_request`] が返しうるエラー。
#[derive(Debug)]
pub enum RequestError {
    /// リクエストヘッドの構文エラー（[`crate::request::parse_request_head`] 由来）。
    Parse(ParseError),
    /// body フレーミングの意味エラー（[`crate::body::body_length`] 由来）。
    Body(BodyError),
    /// chunked body デコードエラー（[`crate::chunked::ChunkedDecoder`] 由来、
    /// イシュー #181）。
    Chunked(ChunkedError),
    /// ヘッドまたは body の途中でソケットが EOF に達した。
    ///
    /// リクエストの先頭（1 バイト目）に到達する前の EOF は正常なコネクション
    /// 終了として扱い [`read_request`] は `Ok(None)` を返す。この派生は
    /// あくまで「読み取り途中」の異常系。
    UnexpectedEof,
    /// ソケット読み取り自体の I/O エラー。
    Io(std::io::Error),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::Parse(e) => write!(f, "request parse error: {e}"),
            RequestError::Body(e) => write!(f, "request body error: {e}"),
            RequestError::Chunked(e) => write!(f, "chunked body error: {e}"),
            RequestError::UnexpectedEof => f.write_str("unexpected EOF while reading request"),
            RequestError::Io(e) => write!(f, "I/O error while reading request: {e}"),
        }
    }
}

impl std::error::Error for RequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RequestError::Parse(e) => Some(e),
            RequestError::Body(e) => Some(e),
            RequestError::Chunked(e) => Some(e),
            RequestError::UnexpectedEof => None,
            RequestError::Io(e) => Some(e),
        }
    }
}

impl From<ParseError> for RequestError {
    fn from(e: ParseError) -> Self {
        RequestError::Parse(e)
    }
}

impl From<BodyError> for RequestError {
    fn from(e: BodyError) -> Self {
        RequestError::Body(e)
    }
}

impl From<ChunkedError> for RequestError {
    fn from(e: ChunkedError) -> Self {
        RequestError::Chunked(e)
    }
}

/// `Connection` ヘッダと HTTP バージョンから keep-alive の可否を判定する。
///
/// `Connection` ヘッダは複数出現しうるため [`RequestHead::headers`] で全件を
/// カンマ区切り token に分解し、各 token を前後の OWS を trim した上で ASCII
/// 大小文字を無視して比較する。
///
/// - HTTP/1.1: `close` token が含まれない限り keep-alive（既定 true）
/// - HTTP/1.0: `keep-alive` token が含まれる場合のみ keep-alive（既定 false）
///
/// # Examples
///
/// ```
/// use bf_http::connection::should_keep_alive;
/// use bf_http::request::{parse_request_head, ParseOutcome};
///
/// let buf = b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
/// assert!(!should_keep_alive(&head));
/// ```
pub fn should_keep_alive(head: &RequestHead) -> bool {
    let tokens = || {
        head.headers()
            .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
            .flat_map(|(_, value)| value.split(','))
            .map(|token| token.trim())
    };

    let has_close = || tokens().any(|token| token.eq_ignore_ascii_case("close"));

    match head.version {
        HttpVersion::Http11 => !has_close(),
        // HTTP/1.0 はデフォルトが close なので `keep-alive` トークンが必要だが、
        // `Connection: keep-alive, close` のように両方が指定された場合は
        // `close` を優先して接続を閉じる（明示的な close 指定は他のトークン
        // より優先されるべきという RFC 7230 の精神に合わせる）。
        HttpVersion::Http10 => {
            !has_close() && tokens().any(|token| token.eq_ignore_ascii_case("keep-alive"))
        }
    }
}

/// `reader` から 1 リクエスト分（ヘッド + body）を読み取る。
///
/// `buf` は呼び出し元（コネクション単位）が保持する [`RecvBuffer`]。本関数は
/// `buf` の未読領域先頭からヘッドをパースし、消費済みバイト数だけカーソルを
/// 前進させる（`RecvBuffer::consume`）。パイプライン済みの残余（次リクエスト
/// 先頭）は `buf` に残るため、呼び出し元は次のリクエストを読むために同じ
/// `buf` をそのまま次の呼び出しへ渡す契約（消費・コンパクション・容量有界化は
/// `RecvBuffer` の責務、TASK-1.3-3 / #68）。
///
/// - `buf` の未読領域が空の状態でヘッド読み取り前に EOF に達した場合は、
///   正常なコネクション終了として `Ok(None)` を返す
/// - ヘッド途中・body 途中で EOF に達した場合は [`RequestError::UnexpectedEof`]
///
/// # Examples
///
/// ```
/// use bf_http::buffer::RecvBuffer;
/// use bf_http::connection::read_request;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let mut socket: &[u8] = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
/// let mut buf = RecvBuffer::new();
/// let req = read_request(&mut socket, &mut buf).await.unwrap().unwrap();
/// assert_eq!(req.head.method, "GET");
/// assert!(req.body.is_empty());
/// # }
/// ```
pub async fn read_request<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut RecvBuffer,
) -> Result<Option<Request>, RequestError> {
    let (head, consumed) = match read_head(reader, buf).await? {
        Some(head_and_consumed) => head_and_consumed,
        None => return Ok(None),
    };
    buf.consume(consumed);

    let body = match body_length(&head)? {
        BodyLength::None => Vec::new(),
        BodyLength::Fixed(n) => read_body(reader, buf, n).await?,
        BodyLength::Chunked => read_body_chunked(reader, buf).await?,
    };

    // keep-alive 接続は同じ RecvBuffer を繰り返し使うため、大 body 処理後の
    // 容量を接続単位で有界化する（.claude/rules/security.md リソース枯渇対策）。
    buf.shrink_if_oversized();

    Ok(Some(Request { head, body }))
}

/// ヘッド部分を読み取り、`(head, consumed)` を返す。
///
/// リクエスト先頭（`buf` の未読領域が空かつこれから 1 バイト目を読む状態）で
/// EOF に達した場合のみ `Ok(None)` を返し、それ以外の途中 EOF は
/// [`RequestError::UnexpectedEof`] とする。
async fn read_head<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut RecvBuffer,
) -> Result<Option<(RequestHead, usize)>, RequestError> {
    loop {
        match parse_request_head(buf.unread())? {
            ParseOutcome::Complete { head, consumed } => return Ok(Some((head, consumed))),
            ParseOutcome::Incomplete => {
                let started_empty = buf.unread().is_empty();
                let n = buf.read_chunk(reader).await.map_err(RequestError::Io)?;
                if n == 0 {
                    if started_empty {
                        return Ok(None);
                    }
                    return Err(RequestError::UnexpectedEof);
                }
            }
        }
    }
}

/// `body_length` で確定した `n` バイトちょうどの body を読み取る。
///
/// `n` は事前に [`crate::body::MAX_BODY_BYTES`] 以下であることが
/// [`body_length`] により検証済みのため、無制限のバッファ成長は発生しない。
/// 未読領域が body ちょうど（パイプライン残余なし）の典型ケースでは
/// `RecvBuffer` のコピー回避専用ヘルパーでコピーを回避する。
async fn read_body<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut RecvBuffer,
    n: u64,
) -> Result<Vec<u8>, RequestError> {
    let n = usize::try_from(n).unwrap_or(usize::MAX);

    while buf.unread().len() < n {
        let read = buf.read_chunk(reader).await.map_err(RequestError::Io)?;
        if read == 0 {
            return Err(RequestError::UnexpectedEof);
        }
    }

    if let Some(body) = buf.take_exact(n) {
        return Ok(body);
    }

    // パイプライン済み残余がある部分一致ケースはコピーで対応する。
    let body = buf.unread()[..n].to_vec();
    buf.consume(n);
    Ok(body)
}

/// chunked transfer-coding の body を [`ChunkedDecoder`] へ委譲して読み取る。
///
/// [`body_length`] が `BodyLength::Chunked` を返した場合にのみ呼ばれる。
/// `buf.unread()` を毎回デコーダへ渡し、消費できた分だけ `buf.consume` で
/// カーソルを前進させる。終端（`0` チャンク + 空 trailer + CRLF）まで正確に
/// 消費するため、パイプライン済み次リクエストの先頭バイトは
/// [`crate::buffer::RecvBuffer`] に残ったまま返る（本モジュール冒頭の
/// keep-alive 契約を維持）。復号総量はデコーダ内部で
/// [`crate::body::MAX_BODY_BYTES`] に有界化済みのため、無制限のバッファ
/// 成長は発生しない。
async fn read_body_chunked<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut RecvBuffer,
) -> Result<Vec<u8>, RequestError> {
    let mut decoder = ChunkedDecoder::new();
    let mut body = Vec::new();
    loop {
        let outcome = decoder.decode(buf.unread(), &mut body)?;
        match outcome {
            DecodeOutcome::Complete { consumed } => {
                buf.consume(consumed);
                return Ok(body);
            }
            DecodeOutcome::Incomplete { consumed } => {
                buf.consume(consumed);
                let n = buf.read_chunk(reader).await.map_err(RequestError::Io)?;
                if n == 0 {
                    return Err(RequestError::UnexpectedEof);
                }
            }
        }
    }
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
    fn http11_defaults_to_keep_alive() {
        let head = head_of(b"GET / HTTP/1.1\r\n\r\n");
        assert!(should_keep_alive(&head));
    }

    #[test]
    fn http11_connection_close_disables_keep_alive() {
        let head = head_of(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n");
        assert!(!should_keep_alive(&head));
    }

    #[test]
    fn http11_connection_close_case_insensitive() {
        let head = head_of(b"GET / HTTP/1.1\r\nConnection: CLOSE\r\n\r\n");
        assert!(!should_keep_alive(&head));
    }

    #[test]
    fn http10_defaults_to_close() {
        let head = head_of(b"GET / HTTP/1.0\r\n\r\n");
        assert!(!should_keep_alive(&head));
    }

    #[test]
    fn http10_connection_keep_alive_enables_keep_alive() {
        let head = head_of(b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n");
        assert!(should_keep_alive(&head));
    }

    #[test]
    fn http10_connection_keep_alive_and_close_disables_keep_alive() {
        // `close` トークンが含まれる場合は HTTP/1.0 でも keep-alive を無効化する
        // （Cursor Bugbot 指摘 #67 PR #102: close トークン無視のリグレッション回帰テスト）。
        let head = head_of(b"GET / HTTP/1.0\r\nConnection: keep-alive, close\r\n\r\n");
        assert!(!should_keep_alive(&head));
    }

    #[test]
    fn connection_header_list_is_parsed() {
        let head = head_of(b"GET / HTTP/1.1\r\nConnection: keep-alive, upgrade\r\n\r\n");
        assert!(should_keep_alive(&head));

        let head = head_of(b"GET / HTTP/1.1\r\nConnection: upgrade, close\r\n\r\n");
        assert!(!should_keep_alive(&head));
    }

    #[test]
    fn multiple_connection_headers_are_all_considered() {
        let head = head_of(b"GET / HTTP/1.1\r\nConnection: upgrade\r\nConnection: close\r\n\r\n");
        assert!(!should_keep_alive(&head));
    }

    #[tokio::test]
    async fn reads_request_without_body() {
        let mut socket: &[u8] = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let mut buf = RecvBuffer::new();
        let req = read_request(&mut socket, &mut buf)
            .await
            .unwrap()
            .expect("request should be present");
        assert_eq!(req.head.method, "GET");
        assert!(req.body.is_empty());
        assert!(buf.unread().is_empty());
    }

    #[tokio::test]
    async fn reads_request_with_body() {
        let mut socket: &[u8] = b"POST /items HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd";
        let mut buf = RecvBuffer::new();
        let req = read_request(&mut socket, &mut buf)
            .await
            .unwrap()
            .expect("request should be present");
        assert_eq!(req.head.method, "POST");
        assert_eq!(req.body, b"abcd");
        assert!(buf.unread().is_empty());
    }

    #[tokio::test]
    async fn reads_request_split_across_multiple_chunks() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let payload = b"POST /items HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd";

        let write_task = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            for chunk in payload.chunks(3) {
                client.write_all(chunk).await.unwrap();
                client.flush().await.unwrap();
            }
        });

        let mut buf = RecvBuffer::new();
        let req = read_request(&mut server, &mut buf)
            .await
            .unwrap()
            .expect("request should be present");
        assert_eq!(req.head.method, "POST");
        assert_eq!(req.body, b"abcd");

        write_task.await.unwrap();
    }

    #[tokio::test]
    async fn pipelined_requests_leave_remainder_in_buf() {
        let mut socket: &[u8] = b"GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\n\r\n";
        let mut buf = RecvBuffer::new();

        let first = read_request(&mut socket, &mut buf)
            .await
            .unwrap()
            .expect("first request should be present");
        assert_eq!(first.head.target, "/a");

        let second = read_request(&mut socket, &mut buf)
            .await
            .unwrap()
            .expect("second request should be present");
        assert_eq!(second.head.target, "/b");
        assert!(buf.unread().is_empty());
    }

    #[tokio::test]
    async fn immediate_eof_returns_none() {
        let mut socket: &[u8] = b"";
        let mut buf = RecvBuffer::new();
        let req = read_request(&mut socket, &mut buf).await.unwrap();
        assert!(req.is_none());
    }

    #[tokio::test]
    async fn eof_mid_head_is_unexpected_eof() {
        let mut socket: &[u8] = b"GET / HTTP/1.1\r\n";
        let mut buf = RecvBuffer::new();
        let err = read_request(&mut socket, &mut buf).await.unwrap_err();
        assert!(matches!(err, RequestError::UnexpectedEof));
    }

    #[tokio::test]
    async fn eof_mid_body_is_unexpected_eof() {
        let mut socket: &[u8] = b"POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc";
        let mut buf = RecvBuffer::new();
        let err = read_request(&mut socket, &mut buf).await.unwrap_err();
        assert!(matches!(err, RequestError::UnexpectedEof));
    }

    #[tokio::test]
    async fn parse_error_is_propagated() {
        let mut socket: &[u8] = b"G@T / HTTP/1.1\r\n\r\n";
        let mut buf = RecvBuffer::new();
        let err = read_request(&mut socket, &mut buf).await.unwrap_err();
        assert!(matches!(err, RequestError::Parse(_)));
    }

    #[tokio::test]
    async fn body_error_is_propagated() {
        // `gzip` は chunked 以外の coding のため BodyError::TransferEncodingUnsupported
        // として拒否される（単独 `chunked` はイシュー #181 で受理対象になった
        // ため、本テストの入力から除外した）。
        let mut socket: &[u8] = b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n";
        let mut buf = RecvBuffer::new();
        let err = read_request(&mut socket, &mut buf).await.unwrap_err();
        assert!(matches!(err, RequestError::Body(_)));
    }

    #[tokio::test]
    async fn reads_chunked_request_body() {
        // イシュー #181: RFC 9112 §7.1 の chunked encoding の例（Wikipedia の
        // 記事から）を最小化した end-to-end 検証。
        let mut socket: &[u8] =
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        let mut buf = RecvBuffer::new();
        let req = read_request(&mut socket, &mut buf)
            .await
            .unwrap()
            .expect("request should be present");
        assert_eq!(req.body, b"Wikipedia");
        assert!(buf.unread().is_empty());
    }

    #[tokio::test]
    async fn chunked_pipelined_requests_leave_remainder_in_buf() {
        // chunked body の終端 CRLF までを正確に消費し、パイプライン済み次
        // リクエストが RecvBuffer に残ることを固定する（keep-alive 契約維持）。
        let mut socket: &[u8] = b"POST /a HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n0\r\n\r\nGET /b HTTP/1.1\r\n\r\n";
        let mut buf = RecvBuffer::new();

        let first = read_request(&mut socket, &mut buf)
            .await
            .unwrap()
            .expect("first request should be present");
        assert_eq!(first.head.target, "/a");
        assert_eq!(first.body, b"abcd");

        let second = read_request(&mut socket, &mut buf)
            .await
            .unwrap()
            .expect("second request should be present");
        assert_eq!(second.head.target, "/b");
        assert!(buf.unread().is_empty());
    }

    #[tokio::test]
    async fn chunked_body_split_across_multiple_socket_chunks() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let payload =
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n0\r\n\r\n";

        let write_task = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            for chunk in payload.chunks(5) {
                client.write_all(chunk).await.unwrap();
                client.flush().await.unwrap();
            }
        });

        let mut buf = RecvBuffer::new();
        let req = read_request(&mut server, &mut buf)
            .await
            .unwrap()
            .expect("request should be present");
        assert_eq!(req.body, b"Wiki");

        write_task.await.unwrap();
    }

    #[tokio::test]
    async fn chunked_error_is_propagated() {
        let mut socket: &[u8] = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nZZZ\r\n";
        let mut buf = RecvBuffer::new();
        let err = read_request(&mut socket, &mut buf).await.unwrap_err();
        assert!(matches!(err, RequestError::Chunked(_)));
    }

    #[tokio::test]
    async fn eof_mid_chunked_body_is_unexpected_eof() {
        let mut socket: &[u8] = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWi";
        let mut buf = RecvBuffer::new();
        let err = read_request(&mut socket, &mut buf).await.unwrap_err();
        assert!(matches!(err, RequestError::UnexpectedEof));
    }

    #[test]
    fn request_error_display_wraps_source_messages() {
        // 上位（コアループ・plugin-webrtc-proxy 等）はこの Display 文言を
        // ログ・エラー応答生成に使いうるため、内包エラーの文言が連結される
        // ことを固定する（PoC-9 教訓）。
        let parse_err = RequestError::from(ParseError::InvalidRequestLine);
        assert_eq!(
            parse_err.to_string(),
            "request parse error: invalid request line"
        );

        let body_err = RequestError::from(BodyError::BodyTooLarge);
        assert_eq!(
            body_err.to_string(),
            "request body error: Content-Length exceeds MAX_BODY_BYTES"
        );

        let chunked_err = RequestError::from(ChunkedError::TooManyChunks);
        assert_eq!(
            chunked_err.to_string(),
            "chunked body error: chunk count exceeds MAX_CHUNK_COUNT"
        );

        assert_eq!(
            RequestError::UnexpectedEof.to_string(),
            "unexpected EOF while reading request"
        );

        let io_err = RequestError::Io(std::io::Error::other("boom"));
        assert_eq!(io_err.to_string(), "I/O error while reading request: boom");
    }

    #[test]
    fn request_error_source_exposes_underlying_error() {
        use std::error::Error;

        let parse_err = RequestError::from(ParseError::InvalidHeader);
        assert!(parse_err.source().is_some());

        let body_err = RequestError::from(BodyError::DuplicateContentLength);
        assert!(body_err.source().is_some());

        let chunked_err = RequestError::from(ChunkedError::InvalidChunkSize);
        assert!(chunked_err.source().is_some());

        // UnexpectedEof はこれ自体が終端要因であり、内包エラーを持たない契約。
        assert!(RequestError::UnexpectedEof.source().is_none());

        let io_err = RequestError::Io(std::io::Error::other("boom"));
        assert!(io_err.source().is_some());
    }

    #[tokio::test]
    async fn pipelined_second_request_parse_error_does_not_affect_first() {
        // 1 リクエスト目が正常でも、パイプライン済みの 2 リクエスト目が不正なら
        // 2 回目の read_request 呼び出しでのみエラーになることを固定する。
        let mut socket: &[u8] = b"GET /a HTTP/1.1\r\n\r\nG@T /b HTTP/1.1\r\n\r\n";
        let mut buf = RecvBuffer::new();

        let first = read_request(&mut socket, &mut buf)
            .await
            .unwrap()
            .expect("first request should be present");
        assert_eq!(first.head.target, "/a");

        let err = read_request(&mut socket, &mut buf).await.unwrap_err();
        assert!(matches!(err, RequestError::Parse(_)));
    }
}
