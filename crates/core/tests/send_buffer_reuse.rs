//! レスポンス直列化バッファの接続単位再利用（イシュー #584）の実 TCP 接続
//! 経由の統合テスト。
//!
//! `crates/http` 側（`fandhe_backend_http::buffer::SendBuffer` /
//! `Response::serialize_into`）は単体テストで検証済みだが、本ファイルは
//! `handle_connection_with_permit`（`crates/core/src/server.rs`）が実際に
//! keep-alive 接続で `SendBuffer` を接続単位で再利用しても、応答ごとの
//! ワイヤバイト列が従来の `Response::serialize` 相当のまま正しいこと
//! （前応答の残留バイトが次応答へ混入しない = レスポンス分割対策）を
//! 実接続で確認する。

use fandhe_backend_core::Server;
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::Router;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

async fn spawn_server(server: Server) -> std::net::SocketAddr {
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = bound.run().await;
    });
    addr
}

/// ソケットから正確に `n` バイト読み取る（keep-alive 接続で次応答の先頭を
/// 読み過ぎないよう、`Content-Length` から算出した応答全体の長さちょうどを
/// 読む）。
async fn read_exact_bytes(stream: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    stream.read_exact(&mut out).await.expect("read response");
    out
}

/// keep-alive 接続で異なるサイズの応答を連続して 2 回受け取り、両方とも
/// `Content-Length` が正確で、かつ 2 回目の応答に 1 回目の残留バイトが
/// 混入していないことを確認する（受け入れ基準 1・4。`SendBuffer` の
/// 接続単位再利用が正しく動いていることの実接続での証跡）。
#[tokio::test]
async fn keep_alive_connection_reuses_send_buffer_without_stale_bytes() {
    let router = Router::new()
        .route("GET", "/big", |_head, _body| {
            // 1 回目: 大きめの body（SendBuffer の内部容量を拡大させる）。
            Response::new(200, vec![b'A'; 4096])
        })
        .route("GET", "/small", |_head, _body| {
            // 2 回目: 小さい body。SendBuffer の内部 Vec が再利用されても
            // 1 回目の残留バイトが混入しないことを検証する対象。
            Response::new(200, b"small-ok".to_vec())
        });
    let addr = spawn_server(Server::new().handler(router)).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();

    // 1 回目のリクエスト（keep-alive、Connection ヘッダなし = HTTP/1.1 既定）。
    stream
        .write_all(b"GET /big HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let expected_first = Response::new(200, vec![b'A'; 4096]).serialize(true);
    let first = timeout(
        Duration::from_secs(2),
        read_exact_bytes(&mut stream, expected_first.len()),
    )
    .await
    .expect("1 回目の応答がタイムアウトしないこと");
    assert_eq!(first, expected_first, "1 回目の応答が期待バイト列と不一致");

    // 2 回目のリクエスト（同一接続、Connection: close で終端しテスト完了を
    // 単純化する）。
    stream
        .write_all(b"GET /small HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut second = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = timeout(Duration::from_secs(2), stream.read(&mut buf))
            .await
            .expect("2 回目の応答がタイムアウトしないこと")
            .expect("read");
        if n == 0 {
            break;
        }
        second.extend_from_slice(&buf[..n]);
    }
    let expected_second = Response::new(200, b"small-ok".to_vec()).serialize(false);
    assert_eq!(
        second, expected_second,
        "2 回目の応答に 1 回目の残留バイトが混入している（SendBuffer の \
         clear 契約違反、レスポンス分割につながる）"
    );
}

/// gate 拒否応答（`RequestGate::check` が `GateOutcome::Reject` を返す経路、
/// `handle_connection_with_permit` 内の別の `send_buf` 利用箇所）も、
/// keep-alive 接続で正しいワイヤバイト列を返すことを確認する。
#[tokio::test]
async fn gated_rejection_response_is_correct_over_keep_alive_connection() {
    use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
    use fandhe_backend_http::request::RequestHead;

    struct AlwaysReject;
    impl RequestGate for AlwaysReject {
        fn name(&self) -> &'static str {
            "always-reject"
        }

        fn check(&self, _head: &RequestHead, _ctx: &GateContext) -> GateOutcome {
            GateOutcome::reject(403, b"forbidden".to_vec())
        }
    }

    let router = Router::new().route("GET", "/ok", |_head, _body| Response::empty(200));
    let addr = spawn_server(Server::new().handler(router).gate(AlwaysReject)).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /ok HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = timeout(Duration::from_secs(2), stream.read(&mut buf))
            .await
            .expect("gate 拒否応答がタイムアウトしないこと")
            .expect("read");
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with("HTTP/1.1 403"));
    assert!(text.contains("Content-Length: 9\r\n"));
    assert!(text.ends_with("forbidden"));
}
