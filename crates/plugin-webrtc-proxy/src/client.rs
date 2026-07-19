//! 上流 WebRTC サービスへの最小 HTTP/1.1 クライアント（[`forward_offer`]）。
//!
//! [`crate::handler::try_handle_rtc_offer`] から呼ばれ、SDP Offer を
//! [`crate::config::ProxyConfig::upstream_addr`] へ POST し、SDP Answer を
//! 受け取って返す。`reqwest`/`hyper` 等の重量 HTTP クライアントは依存最小化
//! （.claude/rules/pay-for-what-you-use.md）のため使わず、`tokio::net::TcpStream`
//! 上に最小限のリクエスト送信・レスポンス受信を自前実装する。
//!
//! クライアント（上位のフレームワーク利用者）由来のヘッダは一切転送しない
//! （自前で組み立てた固定ヘッダのみを送る）。これはヘッダインジェクション・
//! request smuggling を防ぐための意図的な制約（.claude/rules/security.md）。

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::config::ProxyConfig;
use crate::error::ProxyError;

/// 1 回の read で取得するチャンクサイズ。
const READ_CHUNK_BYTES: usize = 4 * 1024;

/// 上流応答のヘッド（ステータスライン + ヘッダ）として許容するバイト数上限。
///
/// `fandhe_backend_http::request::MAX_HEADER_BYTES` と同じ考え方で、ヘッダ終端
/// （`\r\n\r\n`）に到達しないまま無制限にバッファが成長するのを防ぐ
/// （リソース枯渇対策、.claude/rules/security.md）。
const RESPONSE_HEAD_LIMIT: usize = 8 * 1024;

/// SDP Offer を上流 WebRTC サービスへ転送し、SDP Answer の body を返す。
///
/// `config.connect_timeout()` で接続確立、`config.request_timeout()` で
/// リクエスト送信〜応答受信全体をそれぞれタイムアウトさせる
/// （スロー上流対策、.claude/rules/security.md）。上流アドレスは `config` の
/// 静的値のみを使い、`offer_body` の内容から転送先を導出しない（SSRF 防止）。
///
/// 上流が 2xx 以外を返す・`Content-Length` を欠く・応答が
/// `config.max_answer_bytes()` を超過する場合はいずれもエラーで返し、
/// クライアントへは呼び出し元（[`crate::handler`]）が定型のフェイルクローズ
/// 応答へ丸める契約とする（上流内部情報を漏らさない）。
///
/// # Examples
///
/// 上流が未リッスンのループバックアドレスへ接続を試み、フェイルクローズで
/// `Err` になることを示す（外部ネットワークに依存しない決定的な例）。
///
/// ```
/// use fandhe_backend_plugin_webrtc_proxy::ProxyConfig;
/// use fandhe_backend_plugin_webrtc_proxy::client::forward_offer;
///
/// let config = ProxyConfig::new("127.0.0.1:1")
///     .with_connect_timeout(std::time::Duration::from_millis(200));
///
/// let runtime = tokio::runtime::Runtime::new().unwrap();
/// let result = runtime.block_on(forward_offer(&config, b"offer"));
/// assert!(result.is_err());
/// ```
pub async fn forward_offer(config: &ProxyConfig, offer_body: &[u8]) -> Result<Vec<u8>, ProxyError> {
    let mut stream = tokio::time::timeout(
        config.connect_timeout(),
        TcpStream::connect(config.upstream_addr()),
    )
    .await
    .map_err(|_| ProxyError::UpstreamTimeout)?
    .map_err(|_| ProxyError::UpstreamConnect)?;

    let exchange = async {
        write_request(&mut stream, config, offer_body).await?;
        read_response_body(&mut stream, config.max_answer_bytes()).await
    };

    tokio::time::timeout(config.request_timeout(), exchange)
        .await
        .map_err(|_| ProxyError::UpstreamTimeout)?
}

/// 固定ヘッダのみからなる POST リクエストを組み立てて送信する。
///
/// `Host`/`Content-Type`/`Content-Length`/`Connection` のみを自前で設定し、
/// クライアント由来のヘッダを一切転送しない（ヘッダインジェクション対策）。
async fn write_request(
    stream: &mut TcpStream,
    config: &ProxyConfig,
    body: &[u8],
) -> Result<(), ProxyError> {
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/sdp\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        path = config.upstream_path(),
        host = config.upstream_addr(),
        len = body.len(),
    )
    .into_bytes();
    request.extend_from_slice(body);

    stream.write_all(&request).await.map_err(|_| ProxyError::Io)
}

/// 上流からの HTTP/1.1 応答を読み取り、body（SDP Answer）を返す。
///
/// ステータスラインが 2xx でない、`Content-Length` を欠く／重複する、応答が
/// `max_answer_bytes` を超過する場合はいずれも [`ProxyError`] として拒否する
/// （フェイルクローズ）。
async fn read_response_body(
    stream: &mut TcpStream,
    max_answer_bytes: usize,
) -> Result<Vec<u8>, ProxyError> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; READ_CHUNK_BYTES];

    let header_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() >= RESPONSE_HEAD_LIMIT {
            return Err(ProxyError::UpstreamProtocol);
        }
        let n = stream.read(&mut chunk).await.map_err(|_| ProxyError::Io)?;
        if n == 0 {
            return Err(ProxyError::UpstreamProtocol);
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let (status, content_length) = parse_response_head(&buf[..header_end])?;
    if !(200..300).contains(&status) {
        return Err(ProxyError::UpstreamStatus);
    }
    if content_length > max_answer_bytes {
        return Err(ProxyError::AnswerTooLarge);
    }

    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        if body.len() >= max_answer_bytes {
            return Err(ProxyError::AnswerTooLarge);
        }
        let n = stream.read(&mut chunk).await.map_err(|_| ProxyError::Io)?;
        if n == 0 {
            return Err(ProxyError::UpstreamProtocol);
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    Ok(body)
}

/// ステータスライン + ヘッダから `(status, content_length)` を取り出す。
///
/// `Content-Length` が存在しない、重複する、数値として不正な場合はいずれも
/// [`ProxyError::UpstreamProtocol`] として拒否する。重複禁止は
/// `fandhe_backend_http::request` の意味検証方針（重複 `Content-Length` 拒否）と同じ考え方
/// で request smuggling 類似の曖昧さを排除するため。
fn parse_response_head(head: &[u8]) -> Result<(u16, usize), ProxyError> {
    let text = std::str::from_utf8(head).map_err(|_| ProxyError::UpstreamProtocol)?;
    let mut lines = text.split("\r\n");

    let status_line = lines.next().ok_or(ProxyError::UpstreamProtocol)?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().ok_or(ProxyError::UpstreamProtocol)?;
    if !version.starts_with("HTTP/1.") {
        return Err(ProxyError::UpstreamProtocol);
    }
    let status: u16 = parts
        .next()
        .ok_or(ProxyError::UpstreamProtocol)?
        .parse()
        .map_err(|_| ProxyError::UpstreamProtocol)?;

    let mut content_length: Option<usize> = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or(ProxyError::UpstreamProtocol)?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(ProxyError::UpstreamProtocol);
            }
            let parsed: usize = value
                .trim()
                .parse()
                .map_err(|_| ProxyError::UpstreamProtocol)?;
            content_length = Some(parsed);
        }
    }

    let content_length = content_length.ok_or(ProxyError::UpstreamProtocol)?;
    Ok((status, content_length))
}

/// `haystack` 中で `needle` が最初に現れる位置を返す。
///
/// `fandhe_backend_http::request` の同名関数は `pub(crate)` で外部から使えないため、
/// 病的入力による計算量爆発を避ける単純な線形走査として本クレート内に
/// 同じ実装方針で複製する。
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// SDP Answer を固定応答するモック上流サーバを起動し、`(addr, JoinHandle)` を返す。
    async fn spawn_mock_upstream(answer: &'static [u8], status_line: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            // リクエスト全体を読み切るまで待つ（テスト用の簡易実装）。
            let _ = socket.read(&mut buf).await;
            let response = format!(
                "{status_line}\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n",
                answer.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.write_all(answer).await;
        });
        addr
    }

    #[tokio::test]
    async fn forward_offer_returns_answer_body() {
        let addr = spawn_mock_upstream(b"v=0\r\no=- answer", "HTTP/1.1 200 OK").await;
        let config = ProxyConfig::new(addr);

        let answer = forward_offer(&config, b"v=0\r\no=- offer").await.unwrap();
        assert_eq!(answer, b"v=0\r\no=- answer");
    }

    #[tokio::test]
    async fn forward_offer_rejects_non_success_status() {
        let addr = spawn_mock_upstream(b"err", "HTTP/1.1 500 Internal Server Error").await;
        let config = ProxyConfig::new(addr);

        let result = forward_offer(&config, b"offer").await;
        assert!(matches!(result, Err(ProxyError::UpstreamStatus)));
    }

    #[tokio::test]
    async fn forward_offer_rejects_connect_failure() {
        // 127.0.0.1:1 は特権ポートかつ通常未リッスンのため接続拒否が期待できる。
        let config = ProxyConfig::new("127.0.0.1:1");
        let result = forward_offer(&config, b"offer").await;
        assert!(matches!(
            result,
            Err(ProxyError::UpstreamConnect) | Err(ProxyError::UpstreamTimeout)
        ));
    }

    #[tokio::test]
    async fn forward_offer_times_out_on_slow_upstream() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            // 応答を送らずソケットを保持し続け、上流ハング（スロー上流）を模す。
            let _keep_alive = socket;
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        });

        let config =
            ProxyConfig::new(addr).with_request_timeout(std::time::Duration::from_millis(50));
        let result = forward_offer(&config, b"offer").await;
        assert!(matches!(result, Err(ProxyError::UpstreamTimeout)));
    }

    #[tokio::test]
    async fn forward_offer_rejects_answer_over_max_bytes() {
        let addr = spawn_mock_upstream(b"0123456789", "HTTP/1.1 200 OK").await;
        let config = ProxyConfig::new(addr).with_max_answer_bytes(4);

        let result = forward_offer(&config, b"offer").await;
        assert!(matches!(result, Err(ProxyError::AnswerTooLarge)));
    }

    #[test]
    fn parse_response_head_rejects_duplicate_content_length() {
        let head = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nContent-Length: 5\r\n\r\n";
        assert!(matches!(
            parse_response_head(head),
            Err(ProxyError::UpstreamProtocol)
        ));
    }

    #[test]
    fn parse_response_head_rejects_missing_content_length() {
        let head = b"HTTP/1.1 200 OK\r\n\r\n";
        assert!(matches!(
            parse_response_head(head),
            Err(ProxyError::UpstreamProtocol)
        ));
    }

    #[test]
    fn parse_response_head_accepts_well_formed_response() {
        let head = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n";
        assert_eq!(parse_response_head(head).unwrap(), (200, 3));
    }

    #[test]
    fn parse_response_head_rejects_non_http_version_prefix() {
        // ステータスラインが `HTTP/1.` で始まらない応答は上流のプロトコル違反
        // として拒否する（.claude/rules/security.md の入力検証）。
        let head = b"FOO/1.1 200 OK\r\nContent-Length: 3\r\n\r\n";
        assert!(matches!(
            parse_response_head(head),
            Err(ProxyError::UpstreamProtocol)
        ));
    }

    #[test]
    fn parse_response_head_rejects_non_numeric_status() {
        let head = b"HTTP/1.1 OK OK\r\nContent-Length: 3\r\n\r\n";
        assert!(matches!(
            parse_response_head(head),
            Err(ProxyError::UpstreamProtocol)
        ));
    }

    #[test]
    fn parse_response_head_rejects_non_numeric_content_length() {
        let head = b"HTTP/1.1 200 OK\r\nContent-Length: abc\r\n\r\n";
        assert!(matches!(
            parse_response_head(head),
            Err(ProxyError::UpstreamProtocol)
        ));
    }

    #[tokio::test]
    async fn forward_offer_rejects_immediate_eof_before_headers_complete() {
        // 上流が接続確立直後に何も送らず切断した場合、ヘッド終端
        // （`\r\n\r\n`）に到達しないまま EOF となり `UpstreamProtocol` として
        // 拒否されることを固定する（フェイルクローズ）。
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            drop(socket);
        });

        let config = ProxyConfig::new(addr);
        let result = forward_offer(&config, b"offer").await;
        // 接続直後の切断はタイミングによって「書き込み側で ECONNRESET/EPIPE
        // を検出（Io）」と「書き込みは成功しヘッド未完了のまま読み取り側が
        // EOF を検出（UpstreamProtocol）」のいずれもありうる。両者とも
        // フェイルクローズ（クライアントへ内部情報を漏らさず 502 系に丸められる）
        // という契約は共通のため、いずれのエラーでも合格とする。
        assert!(matches!(
            result,
            Err(ProxyError::UpstreamProtocol) | Err(ProxyError::Io)
        ));
    }
}
