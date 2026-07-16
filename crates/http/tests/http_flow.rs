//! `bf-http` の統合テスト（TASK-11.5-1 / #77）。
//!
//! 単体テスト（`request.rs` / `body.rs` / `connection.rs` の `#[cfg(test)]`）は
//! 各モジュール単体の境界・異常系を検証する。本ファイルはヘッドパース →
//! body フレーミング解釈 → keep-alive 判定の一連フローを
//! `tokio::io::duplex` 経由で結合し、実際のソケット読み取り経路
//! （[`bf_http::connection::read_request`]）を通したときに全体として正しく
//! 振る舞うことを検証する。
//!
//! PoC-9（`docs/spec/03-poc/`）の教訓「body 内容のみ検証しステータス行・
//! ヘッダを検証しないテストがバグを見逃した」を踏まえ、各テストは
//! メソッド・ターゲット・バージョン・全ヘッダ・body 全文・keep-alive 判定を
//! 網羅的にアサートする（部分一致で済ませない）。

use bf_http::body::{BodyLength, body_length};
use bf_http::connection::{read_request, should_keep_alive};
use bf_http::request::HttpVersion;

/// 正常系: ヘッダ・body・keep-alive 判定のすべてを網羅的に検証する
/// （PoC-9 教訓の実践）。
#[tokio::test]
async fn full_request_round_trip_asserts_every_field() {
    let raw = b"POST /items?x=1 HTTP/1.1\r\n\
Host: example.com\r\n\
Content-Type: application/json\r\n\
Content-Length: 13\r\n\
Connection: keep-alive\r\n\
\r\n\
{\"a\":1,\"b\":2}";

    let mut socket: &[u8] = raw;
    let mut buf = Vec::new();

    let req = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        read_request(&mut socket, &mut buf),
    )
    .await
    .expect("test-internal timeout: read_request がハングした")
    .expect("I/O エラーなく読み取れること")
    .expect("EOF ではなく 1 リクエストが読み取れること");

    // リクエストライン全要素。
    assert_eq!(req.head.method, "POST");
    assert_eq!(req.head.target, "/items?x=1");
    assert_eq!(req.head.version, HttpVersion::Http11);

    // 全ヘッダを出現順で検証する（`RequestHead::header` の先頭一致だけに頼らない）。
    let headers: Vec<_> = req.head.headers().collect();
    assert_eq!(
        headers,
        vec![
            ("Host", "example.com"),
            ("Content-Type", "application/json"),
            ("Content-Length", "13"),
            ("Connection", "keep-alive"),
        ]
    );

    // body フレーミング解釈と実際に読み取った body の両方を検証する。
    assert_eq!(
        body_length(&req.head),
        Ok(BodyLength::Fixed(13)),
        "body_length() がヘッドから正しく Fixed(13) を導出すること"
    );
    assert_eq!(req.body, b"{\"a\":1,\"b\":2}");
    assert_eq!(req.body.len(), 13);

    // keep-alive 判定。
    assert!(should_keep_alive(&req.head));

    // パイプライン残余がないことも確認する。
    assert!(buf.is_empty());
}

/// `Connection: close` を伴う body なしリクエストで、keep-alive 判定が
/// `false` になり、かつヘッド・body 双方が正しく分離されることを検証する。
#[tokio::test]
async fn request_without_body_and_explicit_close() {
    let raw = b"GET /health HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n";
    let mut socket: &[u8] = raw;
    let mut buf = Vec::new();

    let req = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        read_request(&mut socket, &mut buf),
    )
    .await
    .expect("test-internal timeout")
    .expect("I/O エラーなく読み取れること")
    .expect("1 リクエストが読み取れること");

    assert_eq!(req.head.method, "GET");
    assert_eq!(req.head.target, "/health");
    assert_eq!(req.head.version, HttpVersion::Http11);
    assert_eq!(
        req.head.headers().collect::<Vec<_>>(),
        vec![("Host", "example.com"), ("Connection", "close")]
    );
    assert!(req.body.is_empty());
    assert!(!should_keep_alive(&req.head));
}

/// パイプライン化された 2 リクエストを 1 つの `duplex` ストリームへ分割書き込みし、
/// 各リクエストのフルフィールドが取り違えなく読み取れることを検証する
/// （残余バッファの接続単位再利用シナリオ、TASK-1.3-3 / #68 の前提）。
#[tokio::test]
async fn pipelined_requests_over_split_writes_preserve_field_integrity() {
    let (mut client, mut server) = tokio::io::duplex(4096);
    let first = b"POST /a HTTP/1.1\r\nContent-Length: 3\r\n\r\nfoo";
    let second = b"GET /b HTTP/1.1\r\nConnection: close\r\n\r\n";

    let write_task = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut payload = Vec::new();
        payload.extend_from_slice(first);
        payload.extend_from_slice(second);
        // 5 バイトずつの分割書き込みで、ヘッド途中・body 途中の部分読み取りを誘発する。
        for chunk in payload.chunks(5) {
            client.write_all(chunk).await.unwrap();
            client.flush().await.unwrap();
        }
    });

    let mut buf = Vec::new();

    let req1 = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        read_request(&mut server, &mut buf),
    )
    .await
    .expect("test-internal timeout (1st)")
    .expect("I/O エラーなし")
    .expect("1 件目が読めること");
    assert_eq!(req1.head.method, "POST");
    assert_eq!(req1.head.target, "/a");
    assert_eq!(req1.head.version, HttpVersion::Http11);
    assert_eq!(req1.body, b"foo");
    assert!(should_keep_alive(&req1.head));

    let req2 = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        read_request(&mut server, &mut buf),
    )
    .await
    .expect("test-internal timeout (2nd)")
    .expect("I/O エラーなし")
    .expect("2 件目が読めること");
    assert_eq!(req2.head.method, "GET");
    assert_eq!(req2.head.target, "/b");
    assert_eq!(req2.head.version, HttpVersion::Http11);
    assert!(req2.body.is_empty());
    assert!(!should_keep_alive(&req2.head));

    assert!(buf.is_empty());
    write_task.await.unwrap();
}

/// 不正な `Transfer-Encoding` 指定は body フレーミング解釈で拒否され、
/// `read_request` がヘッドパース成功後に body エラーとして返すことを検証する
/// （request smuggling 対策の統合経路での固定、.claude/rules/security.md）。
#[tokio::test]
async fn transfer_encoding_is_rejected_end_to_end() {
    let raw =
        b"POST /items HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 4\r\n\r\nabcd";
    let mut socket: &[u8] = raw;
    let mut buf = Vec::new();

    let err = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        read_request(&mut socket, &mut buf),
    )
    .await
    .expect("test-internal timeout")
    .expect_err("Transfer-Encoding 指定は拒否されること");

    assert_eq!(
        err.to_string(),
        "request body error: Transfer-Encoding is not supported"
    );
}
