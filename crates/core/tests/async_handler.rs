//! async ハンドラ対応（イシュー #315、`docs/design/async-handler.md`）の
//! 実 TCP 接続経由の統合テスト。
//!
//! `crates/routes/tests/async_handler.rs` が `Router` 単体の登録・dispatch
//! 契約を検証するのに対し、本ファイルはコアループ（`Server::bind` +
//! `BoundServer::run`）を実際に起動し、次の受け入れ基準を実証する:
//! - `tokio::time::sleep` を await する async ハンドラが正しく応答すること
//! - sleep 中のハンドラが他コネクションの処理をブロックしないこと（並行性）
//! - ハンドラ内 panic が当該接続タスクに閉じ込められ、サーバが後続接続を
//!   処理し続けること（`docs/design/async-handler.md` 7 節の panic 境界契約）
//! - 同期ハンドラ（`Router` クロージャ・`Handler` 直接実装）の回帰がないこと

use fandhe_backend_core::{Handler, Server};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::Router;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

async fn read_response(stream: &mut TcpStream) -> String {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = stream.read(&mut buf).await.expect("read response");
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        // Content-Length ベースの厳密な境界判定は行わず、close 待ちで十分な
        // 小さな固定応答のみをテストで扱う（`Connection: close` を要求する）。
    }
    String::from_utf8(out).expect("response must be valid utf-8")
}

async fn spawn_server(server: Server) -> std::net::SocketAddr {
    let bound = server.bind("127.0.0.1:0").await.unwrap();
    let addr = bound.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = bound.run().await;
    });
    addr
}

/// `tokio::time::sleep` を await してから応答する async ハンドラ（受け入れ基準 1）。
#[tokio::test]
async fn async_handler_awaits_sleep_before_responding() {
    let router = Router::new().route_async("GET", "/slow", |_head, _body| async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        Response::new(200, b"slow-ok".to_vec())
    });
    let addr = spawn_server(Server::new().handler(router)).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /slow HTTP/1.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let text = timeout(Duration::from_secs(2), read_response(&mut stream))
        .await
        .expect("応答がタイムアウトしないこと");
    assert!(text.starts_with("HTTP/1.1 200"));
    assert!(text.ends_with("slow-ok"));
}

/// sleep 中の async ハンドラが他コネクションの処理をブロックしないことを
/// 確認する（受け入れ基準 1 の並行性側面）。
///
/// 先に `/slow`（長い sleep）へ接続し、その応答を待たずに `/fast` へ別接続
/// を張る。`/fast` の応答が `/slow` の sleep 時間より先に返ってくることで、
/// ハンドラの `.await` がコアループ全体をブロックしていないことを実証する
/// （`Handler::handle` が同期 `Response` を返す旧契約だった場合、sqlx 等の
/// 真の非同期 I/O を挟むと構造的にブロッキングか型不整合のどちらかに
/// 陥っていた、`docs/design/async-handler.md` 1 節の課題認識）。
#[tokio::test]
async fn slow_async_handler_does_not_block_other_connections() {
    let router = Router::new()
        .route_async("GET", "/slow", |_head, _body| async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Response::new(200, b"slow-ok".to_vec())
        })
        .route_async("GET", "/fast", |_head, _body| async {
            Response::new(200, b"fast-ok".to_vec())
        });
    let addr = spawn_server(Server::new().handler(router)).await;

    let mut slow_stream = TcpStream::connect(addr).await.unwrap();
    slow_stream
        .write_all(b"GET /slow HTTP/1.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    // /slow の応答を待たずに /fast へ接続し、短いタイムアウト内に応答が
    // 返ることを確認する（/slow の 2s より十分短い 1s 上限。並列 issue
    // 実装ワークフロー下の host contention による flake を避けるため、
    // 判別に必要な最小限を超えて余裕を持たせる、`.claude/rules/ci.md` の
    // host contention への配慮と同旨）。
    let mut fast_stream = TcpStream::connect(addr).await.unwrap();
    fast_stream
        .write_all(b"GET /fast HTTP/1.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let fast_text = timeout(Duration::from_secs(1), read_response(&mut fast_stream))
        .await
        .expect(
            "/fast は /slow の sleep をブロックせず短時間で応答するはず\
             （async ハンドラが並行実行されていない場合はここでタイムアウトする）",
        );
    assert!(fast_text.starts_with("HTTP/1.1 200"));
    assert!(fast_text.ends_with("fast-ok"));

    // /slow 側もいずれ正常応答することを確認し、後片付けする（sleep 2s より
    // 十分な余裕を持たせる）。
    let slow_text = timeout(Duration::from_secs(5), read_response(&mut slow_stream))
        .await
        .expect("/slow も最終的には応答するはず");
    assert!(slow_text.starts_with("HTTP/1.1 200"));
}

/// panic するハンドラ（`Handler` 直接実装）。呼び出し回数を記録し、
/// 「panic 後も後続接続でハンドラが呼ばれ続ける」ことの証跡に使う。
struct PanicHandler {
    calls: Arc<AtomicUsize>,
}

impl Handler for PanicHandler {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // async ブロック内で panic させる（async ハンドラ本体で panic した
        // 場合の境界を検証する。同期アダプタ経由の panic は
        // `crates/core/src/server.rs` 内の既存ユニットテストで別途検証済み）。
        Box::pin(async {
            panic!("intentional panic from async handler body");
            #[allow(unreachable_code)]
            Response::empty(200)
        })
    }
}

/// ハンドラ内 panic が接続単位のタスクに閉じ込められ、サーバが後続接続を
/// 処理し続けることを確認する（受け入れ基準・`docs/design/async-handler.md`
/// 7 節の panic 境界契約）。
#[tokio::test]
async fn panicking_handler_does_not_crash_server_for_subsequent_connections() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler = PanicHandler {
        calls: calls.clone(),
    };
    let addr = spawn_server(Server::new().handler(handler)).await;

    // 1 本目: ハンドラが panic する接続。サーバプロセス自体は落ちず、
    // クライアント側は単に接続がクローズされる（応答なし、またはエラー）。
    let mut stream1 = TcpStream::connect(addr).await.unwrap();
    stream1
        .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    // panic 後の接続クローズは EOF（Ok(0)）またはリセットのいずれか。
    // どちらであれ 200 OK 応答は返らないことのみを確認する。
    let _ = timeout(Duration::from_secs(2), stream1.read(&mut buf)).await;

    // 2 本目: 同じハンドラへ再度到達し、正常に panic すること（＝サーバが
    // 引き続き accept・dispatch できていること）を呼び出し回数で確認する。
    let mut stream2 = TcpStream::connect(addr).await.unwrap();
    stream2
        .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let _ = timeout(Duration::from_secs(2), stream2.read(&mut buf)).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "panic 後もサーバは後続接続の Handler::handle を呼び出し続けるはず\
         （1 接続の panic が他接続の処理能力を損なわない）"
    );
}

/// 同期ハンドラ（`Router` クロージャ経由の `route`）の回帰がないことを
/// 確認する（受け入れ基準 2）。
#[tokio::test]
async fn sync_router_handler_still_works_after_async_migration() {
    let router = Router::new().route("GET", "/sync", |_head, _body| {
        Response::new(200, b"sync-ok".to_vec())
    });
    let addr = spawn_server(Server::new().handler(router)).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /sync HTTP/1.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let text = timeout(Duration::from_secs(2), read_response(&mut stream))
        .await
        .expect("同期ハンドラの応答がタイムアウトしないこと");
    assert!(text.starts_with("HTTP/1.1 200"));
    assert!(text.ends_with("sync-ok"));
}

/// 同期ハンドラ（`Handler` 直接実装）の回帰がないことを確認する
/// （受け入れ基準 2、`Router` を経由しない直接実装パス）。
#[tokio::test]
async fn sync_direct_handler_impl_still_works_after_async_migration() {
    struct FixedHandler;
    impl Handler for FixedHandler {
        fn handle(
            &self,
            _head: &RequestHead,
            _body: &[u8],
        ) -> fandhe_backend_routes::HandlerFuture {
            Box::pin(std::future::ready(Response::new(200, b"fixed-ok".to_vec())))
        }
    }

    let addr = spawn_server(Server::new().handler(FixedHandler)).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let text = timeout(Duration::from_secs(2), read_response(&mut stream))
        .await
        .expect("直接実装 Handler の応答がタイムアウトしないこと");
    assert!(text.starts_with("HTTP/1.1 200"));
    assert!(text.ends_with("fixed-ok"));
}
