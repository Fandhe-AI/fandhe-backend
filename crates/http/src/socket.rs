//! 接続受理直後の TCP ソケットオプション設定（TASK-1.3-3 / #68）。
//!
//! feature `net`（既定 off）が有効なときのみコンパイル対象になる。コアの
//! 接続受理ループ（TASK-1.4 / #13、`crates/core`）が `accept` 直後に
//! [`configure_stream`] を呼ぶ契約とし、本クレートは `tokio::net` を必要
//! 最小限（`TcpStream` の 1 メソッド呼び出し）に限定して利用する
//! （pay-for-what-you-use、`crates/http/Cargo.toml` の feature `net` 定義参照）。

/// 接続を `TCP_NODELAY`（Nagle アルゴリズム無効化）に設定する。
///
/// 小さいリクエスト／レスポンスを都度フラッシュする HTTP/1.1 の応答性を
/// 優先し、ACK 待ちによる遅延蓄積を避けるため既定で有効化する契約とする。
///
/// 呼び出し元（コアの接続受理ループ）は `accept` 直後にこの関数を呼ぶ。
/// 失敗時はソケットオプション設定の失敗を握りつぶさず `io::Error` として
/// 伝播するため、呼び出し元は該当接続のみをクローズし、accept ループ全体を
/// 継続する判断ができる（.claude/rules/security.md フェイルセーフ）。
///
/// # Examples
///
/// ```
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> std::io::Result<()> {
/// use fandhe_backend_http::socket::configure_stream;
/// use tokio::net::{TcpListener, TcpStream};
///
/// let listener = TcpListener::bind("127.0.0.1:0").await?;
/// let addr = listener.local_addr()?;
///
/// let accept_task = tokio::spawn(async move {
///     let (stream, _) = listener.accept().await.unwrap();
///     configure_stream(&stream).unwrap();
///     assert!(stream.nodelay().unwrap());
/// });
///
/// let _client: TcpStream = TcpStream::connect(addr).await?;
/// accept_task.await.unwrap();
/// # Ok(())
/// # }
/// ```
pub fn configure_stream(stream: &tokio::net::TcpStream) -> std::io::Result<()> {
    stream.set_nodelay(true)
}

#[cfg(test)]
mod tests {
    use super::configure_stream;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn configure_stream_enables_nodelay_on_accepted_socket() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind should succeed on loopback");
        let addr = listener.local_addr().expect("local_addr should succeed");

        let accept_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept should succeed");
            configure_stream(&stream).expect("set_nodelay should succeed");
            assert!(
                stream.nodelay().expect("nodelay() should succeed"),
                "TCP_NODELAY should be enabled after configure_stream"
            );
        });

        let _client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect should succeed");
        accept_task.await.expect("accept task should not panic");
    }
}
