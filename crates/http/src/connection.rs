//! keep-alive 判定・ソケット読み取りループ（TASK-1.3-2 / #67）。
//!
//! [`should_keep_alive`] は `Connection` ヘッダと HTTP バージョンからの意味
//! 判定を行う sans-IO 純関数。[`read_request`] は [`crate::request::parse_request_head`]
//! （構文解析）と [`crate::body::body_length`]（body フレーミング解釈）を組み合わせ、
//! 1 リクエスト分（ヘッド + body）をソケットから読み取る非同期関数であり、本クレート
//! で唯一 tokio（`io-util`）に依存する箇所。
//!
//! 呼び出し元はコアの接続受理ループ（TASK-1.4 / #13）であり、1 コネクションにつき
//! `buf: Vec<u8>` を 1 つ保持して繰り返し [`read_request`] を呼ぶ契約とする。
//! パイプライン済みの次リクエスト先頭バイトは `buf` に残したまま返すため、
//! バッファの接続単位再利用（TASK-1.3-3 / #68）へそのまま接続できる。
//!
//! 読み取り・アイドルタイムアウト（スロークライアント対策）は接続ループ全体の
//! 設計と一体であるため本モジュールの責務外とし、TASK-1.4 側で扱う
//! （.claude/rules/security.md のリソース枯渇対策）。

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::body::{BodyError, BodyLength, body_length};
use crate::request::{HttpVersion, ParseError, ParseOutcome, RequestHead, parse_request_head};

/// 一括読み取りするチャンクサイズ。
///
/// 大きすぎると小さいリクエストでも無駄なメモリ確保が増え、小さすぎると
/// システムコール回数が増える。8 KiB は一般的な HTTP リクエストヘッドの
/// サイズ感（[`crate::request::MAX_HEADER_BYTES`] = 16 KiB）に対して妥当な
/// 折衷値として選んだ暫定値であり、性能チューニングは #68 のスコープ。
const READ_CHUNK_BYTES: usize = 8 * 1024;

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

/// `Connection` ヘッダと HTTP バージョンから keep-alive の可否を判定する。
///
/// `Connection` ヘッダは複数出現しうるため [`RequestHead::headers`] で全件を
/// カンマ区切り token に分解し、各 token を前後の OWS を trim した上で ASCII
/// 大小文字を無視して比較する。
///
/// - HTTP/1.1: `close` token が含まれない限り keep-alive（既定 true）
/// - HTTP/1.0: `keep-alive` token が含まれる場合のみ keep-alive（既定 false）
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

    match head.version {
        HttpVersion::Http11 => !tokens().any(|token| token.eq_ignore_ascii_case("close")),
        HttpVersion::Http10 => tokens().any(|token| token.eq_ignore_ascii_case("keep-alive")),
    }
}

/// `reader` から 1 リクエスト分（ヘッド + body）を読み取る。
///
/// `buf` は呼び出し元（コネクション単位）が保持する読み取りバッファ。本関数は
/// `buf` 先頭からヘッドをパースし、消費済みバイト列を drain した上でパイプ
/// ライン済みの残余（次リクエスト先頭）を `buf` に残す。呼び出し元は次の
/// リクエストを読むために同じ `buf` をそのまま次の呼び出しへ渡す契約。
///
/// - `buf` が空の状態でヘッド読み取り前に EOF に達した場合は、正常なコネクション
///   終了として `Ok(None)` を返す
/// - ヘッド途中・body 途中で EOF に達した場合は [`RequestError::UnexpectedEof`]
///
/// ```
/// use bf_http::connection::read_request;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let mut socket: &[u8] = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
/// let mut buf = Vec::new();
/// let req = read_request(&mut socket, &mut buf).await.unwrap().unwrap();
/// assert_eq!(req.head.method, "GET");
/// assert!(req.body.is_empty());
/// # }
/// ```
pub async fn read_request<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> Result<Option<Request>, RequestError> {
    let (head, consumed) = match read_head(reader, buf).await? {
        Some(head_and_consumed) => head_and_consumed,
        None => return Ok(None),
    };
    buf.drain(..consumed);

    let body = match body_length(&head)? {
        BodyLength::None => Vec::new(),
        BodyLength::Fixed(n) => read_body(reader, buf, n).await?,
    };

    Ok(Some(Request { head, body }))
}

/// ヘッド部分を読み取り、`(head, consumed)` を返す。
///
/// リクエスト先頭（`buf` が空かつこれから 1 バイト目を読む状態）で EOF に
/// 達した場合のみ `Ok(None)` を返し、それ以外の途中 EOF は
/// [`RequestError::UnexpectedEof`] とする。
async fn read_head<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> Result<Option<(RequestHead, usize)>, RequestError> {
    loop {
        match parse_request_head(buf)? {
            ParseOutcome::Complete { head, consumed } => return Ok(Some((head, consumed))),
            ParseOutcome::Incomplete => {
                let started_empty = buf.is_empty();
                let n = read_chunk(reader, buf).await?;
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
async fn read_body<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    n: u64,
) -> Result<Vec<u8>, RequestError> {
    let n = usize::try_from(n).unwrap_or(usize::MAX);

    while buf.len() < n {
        let read = read_chunk(reader, buf).await?;
        if read == 0 {
            return Err(RequestError::UnexpectedEof);
        }
    }

    let body = buf[..n].to_vec();
    buf.drain(..n);
    Ok(body)
}

/// `reader` から最大 [`READ_CHUNK_BYTES`] バイトを読み取り `buf` 末尾に追記する。
///
/// 戻り値は読み取ったバイト数（0 は EOF を意味する）。
async fn read_chunk<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> Result<usize, RequestError> {
    let start = buf.len();
    buf.resize(start + READ_CHUNK_BYTES, 0);
    let read = reader
        .read(&mut buf[start..])
        .await
        .map_err(RequestError::Io)?;
    buf.truncate(start + read);
    Ok(read)
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
        let mut buf = Vec::new();
        let req = read_request(&mut socket, &mut buf)
            .await
            .unwrap()
            .expect("request should be present");
        assert_eq!(req.head.method, "GET");
        assert!(req.body.is_empty());
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn reads_request_with_body() {
        let mut socket: &[u8] = b"POST /items HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd";
        let mut buf = Vec::new();
        let req = read_request(&mut socket, &mut buf)
            .await
            .unwrap()
            .expect("request should be present");
        assert_eq!(req.head.method, "POST");
        assert_eq!(req.body, b"abcd");
        assert!(buf.is_empty());
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

        let mut buf = Vec::new();
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
        let mut buf = Vec::new();

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
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn immediate_eof_returns_none() {
        let mut socket: &[u8] = b"";
        let mut buf = Vec::new();
        let req = read_request(&mut socket, &mut buf).await.unwrap();
        assert!(req.is_none());
    }

    #[tokio::test]
    async fn eof_mid_head_is_unexpected_eof() {
        let mut socket: &[u8] = b"GET / HTTP/1.1\r\n";
        let mut buf = Vec::new();
        let err = read_request(&mut socket, &mut buf).await.unwrap_err();
        assert!(matches!(err, RequestError::UnexpectedEof));
    }

    #[tokio::test]
    async fn eof_mid_body_is_unexpected_eof() {
        let mut socket: &[u8] = b"POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc";
        let mut buf = Vec::new();
        let err = read_request(&mut socket, &mut buf).await.unwrap_err();
        assert!(matches!(err, RequestError::UnexpectedEof));
    }

    #[tokio::test]
    async fn parse_error_is_propagated() {
        let mut socket: &[u8] = b"G@T / HTTP/1.1\r\n\r\n";
        let mut buf = Vec::new();
        let err = read_request(&mut socket, &mut buf).await.unwrap_err();
        assert!(matches!(err, RequestError::Parse(_)));
    }

    #[tokio::test]
    async fn body_error_is_propagated() {
        let mut socket: &[u8] = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        let mut buf = Vec::new();
        let err = read_request(&mut socket, &mut buf).await.unwrap_err();
        assert!(matches!(err, RequestError::Body(_)));
    }
}
