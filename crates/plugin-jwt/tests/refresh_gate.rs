//! `jwks-refresh` の受け入れ基準をゲート経路で確かめる統合テスト（#897、REQ-16）。
//!
//! `tests/refresh_task.rs`（#895・#896）は認証器を直接呼ぶ。本ファイルは実際の利用形
//! （`JwtGate` を `Server::gate` に登録し、`handle_connection` を `tokio::io::duplex`
//! 越しに駆動する経路）で、tokio の仮想時間（`start_paused`）により次を決定的に確かめる。
//!
//! - R1（回数の上限）: JWKS に無い `kid` のトークンを大量に送っても、取得の開始回数が
//!   「T ÷ 下限 の切り上げ + 定期の取得回数」以下に収まり、偽トークンは 1 件も受理されない。
//!   数えるのは設計 8.7 のとおり半開区間 `[a, a + T)`。
//! - R2（事前公開なしの鍵切替への追従）: 切替の時点ではその場で 401、
//!   「下限 + 取得にかかる時間」以内に 200 を返し始める。旧本文の取得の最中に切り替わる
//!   場合は、事前に未知 `kid` で取得を予約して進行中であることを確かめてから切り替える。
//! - R3（通常の回転）: 事前公開 → 署名鍵の切替 → 旧鍵の退役という正しい運用では、
//!   正当なトークンの拒否が 0 件で、未知 `kid` による追加の取得も起きない。
//!
//! 前提と限界: 時刻は tokio の仮想時間で、実時間は計測しない。トークンの `exp` と
//! 利用者通知口の間引き（`NotifyThrottle`）は壁時計（`SystemTime`）に依存し仮想時間では
//! 進まない。「下限 + 取得時間」の保証は `fetch_timeout <= min_interval` の構築時検査と、
//! 合図ハンドルを `jwks_refresh_trigger` に登録する形（通知口経由ではない）が前提。
//! 取得関数は呼ばれた時点の本文を写し取り（発行者が応答を決める時刻）、設定した遅延の後に
//! 返す。鍵は実行時生成の ES256 で、`kid` の値は出力に載せない。
#![cfg(feature = "jwks-refresh")]

use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B;
use fandhe_backend_core::{Handler, Server, handle_connection};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use fandhe_backend_plugin_jwt::*;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, sleep_until};

const ISS: &str = "https://issuer.example";

// ---- 鍵・トークン ----

struct Ec(EcdsaKeyPair);

impl Ec {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let pk8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        Self(
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pk8.as_ref(), &rng).unwrap(),
        )
    }
    fn jwk(&self, kid: &str) -> String {
        let pt = self.0.public_key().as_ref();
        format!(
            r#"{{"kty":"EC","kid":"{kid}","alg":"ES256","crv":"P-256","x":"{}","y":"{}"}}"#,
            B.encode(&pt[1..33]),
            B.encode(&pt[33..65])
        )
    }
    /// `exp` は壁時計基準（`JwtGate` は `SystemTime` を読む）。
    fn token(&self, kid: &str) -> String {
        let (msg, _) = unsigned(kid);
        let sig = self.0.sign(&SystemRandom::new(), msg.as_bytes()).unwrap();
        format!("{msg}.{}", B.encode(sig.as_ref()))
    }
}

fn unsigned(kid: &str) -> (String, u64) {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let msg = format!(
        "{}.{}",
        B.encode(format!(r#"{{"alg":"ES256","kid":"{kid}","typ":"at+jwt"}}"#)),
        B.encode(format!(r#"{{"iss":"{ISS}","aud":"api","exp":{exp}}}"#))
    );
    (msg, exp)
}

/// 構造は正しいが署名が出鱈目なトークン。
fn forged_token(kid: &str) -> String {
    let (msg, _) = unsigned(kid);
    format!("{msg}.{}", B.encode([7u8; 64]))
}

fn jwks(keys: &[(&Ec, &str)]) -> String {
    let items: Vec<String> = keys.iter().map(|(e, k)| e.jwk(k)).collect();
    format!(r#"{{"keys":[{}]}}"#, items.join(","))
}

// ---- 発行者の偽物 ----

const MODE_OK: u8 = 0;
const MODE_TRANSPORT_ERR: u8 = 1;
const MODE_HANG: u8 = 2;

struct FakeIssuer {
    t0: Instant,
    body: Arc<Mutex<String>>,
    mode: Arc<AtomicU8>,
    delay_ms: Arc<AtomicU64>,
    starts: Arc<Mutex<Vec<Duration>>>,
}

impl JwksFetcher for FakeIssuer {
    fn fetch(&self) -> FetchFuture<'_> {
        self.starts.lock().unwrap().push(Instant::now() - self.t0);
        // 呼ばれた時点の本文・状態を写し取る（切替の直後に始まった取得は旧本文を得る）。
        let body = self.body.lock().unwrap().clone();
        let mode = self.mode.load(Ordering::SeqCst);
        let delay = Duration::from_millis(self.delay_ms.load(Ordering::SeqCst));
        Box::pin(async move {
            match mode {
                MODE_TRANSPORT_ERR => Err(FetchError::Transport),
                MODE_HANG => {
                    sleep(Duration::from_secs(100)).await;
                    Err(FetchError::Other)
                }
                _ => {
                    if !delay.is_zero() {
                        sleep(delay).await;
                    }
                    Ok(JwksResponse::new(body, None))
                }
            }
        })
    }
}

// ---- ゲート ----

struct Ok200;

impl Handler for Ok200 {
    fn handle(&self, _head: &RequestHead, _body: &[u8]) -> fandhe_backend_routes::HandlerFuture {
        Box::pin(std::future::ready(Response::new(200, b"ok".to_vec())))
    }
}

fn policy() -> JwtPolicy {
    JwtPolicy::new(
        Issuer::new(ISS).unwrap(),
        Audiences::single("api").unwrap(),
        AllowedAlgorithms::only(Algorithm::Es256),
        TokenType::AccessToken,
    )
}

/// 合図ハンドルを登録した `JwtGate` を `Server::gate` に載せる。
/// `notified` を渡すと利用者の通知口（間引きあり）も登録する。
fn gate_server(
    shared: &SharedKeySet,
    trigger: RefreshTrigger,
    notified: Option<Arc<AtomicUsize>>,
) -> Server {
    let mut b = JwtAuthenticator::<NoExtraClaims>::builder(shared.clone(), policy())
        .jwks_refresh_trigger(trigger);
    if let Some(n) = notified {
        b = b.unknown_key_notifier(move |_: &UnknownKeyEvent<'_>| {
            n.fetch_add(1, Ordering::SeqCst);
        });
    }
    Server::new().gate(JwtGate::new(b.build())).handler(Ok200)
}

/// 往復した応答の全体（ステータス・ヘッダ・ボディ）。
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// 1 リクエストを `handle_connection` 経由で往復し、応答全体を解析する。
async fn reply(server: &Server, token: &str) -> Reply {
    let req = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    let (mut client, server_stream) = tokio::io::duplex(32 * 1024);
    client.write_all(req.as_bytes()).await.unwrap();
    client.shutdown().await.unwrap();
    handle_connection(server, server_stream).await;
    let mut out = Vec::new();
    client.read_to_end(&mut out).await.unwrap();
    let split = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("ヘッダ終端がない");
    let head = String::from_utf8(out[..split].to_vec()).unwrap();
    let body = out[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .map(|l| {
            let (k, v) = l.split_once(':').expect("ヘッダ行の形式");
            (k.to_string(), v.trim().to_string())
        })
        .collect();
    Reply {
        status,
        headers,
        body,
    }
}

/// ステータスを返す。ただし 200 と 401 は、ヘッダ（Content-Type / Content-Length）と
/// ボディも期待どおりであることを毎回確かめる（それ以外のステータスは想定外で失敗）。
async fn status(server: &Server, token: &str) -> u16 {
    let r = reply(server, token).await;
    match r.status {
        200 => {
            assert_eq!(r.body, b"ok", "200 のボディ");
            assert_eq!(r.header("Content-Length"), Some("2"), "{:?}", r.headers);
            // `Ok200` は Content-Type を付けない（ゲートが余計なヘッダを足していない）。
            assert!(r.header("Content-Type").is_none(), "{:?}", r.headers);
            assert_eq!(r.header("Connection"), Some("close"));
            assert!(r.header("WWW-Authenticate").is_none(), "{:?}", r.headers);
        }
        401 => {
            // 拒否理由は固定の JSON（トークンやキーの情報を含まない）。
            let expected = br#"{"error":"invalid_token"}"#;
            assert_eq!(r.body, expected, "401 のボディ");
            let len = expected.len().to_string();
            assert_eq!(r.header("Content-Length"), Some(len.as_str()));
            assert!(
                r.header("Content-Type").is_some_and(|v| v.contains("json")),
                "{:?}",
                r.headers
            );
        }
        other => panic!("想定外のステータス {other}: {:?}", r.headers),
    }
    r.status
}

// ---- 実験台 ----

struct Rig {
    t0: Instant,
    shared: SharedKeySet,
    body: Arc<Mutex<String>>,
    mode: Arc<AtomicU8>,
    starts: Arc<Mutex<Vec<Duration>>>,
    handle: RefreshHandle,
    _join: JoinHandle<()>,
}

impl Rig {
    fn start(cfg: RefreshConfig, body: String, delay_ms: u64) -> Self {
        let t0 = Instant::now();
        let shared = SharedKeySet::new(KeySet::empty());
        let body = Arc::new(Mutex::new(body));
        let mode = Arc::new(AtomicU8::new(MODE_OK));
        let delay_ms = Arc::new(AtomicU64::new(delay_ms));
        let starts: Arc<Mutex<Vec<Duration>>> = Arc::default();
        let fetcher = FakeIssuer {
            t0,
            body: Arc::clone(&body),
            mode: Arc::clone(&mode),
            delay_ms: Arc::clone(&delay_ms),
            starts: Arc::clone(&starts),
        };
        let (task, handle) = JwksRefresher::new(shared.clone(), fetcher, cfg);
        Self {
            t0,
            shared,
            body,
            mode,
            starts,
            handle,
            _join: tokio::spawn(task),
        }
    }

    fn server(&self) -> Server {
        gate_server(&self.shared, self.handle.trigger(), None)
    }

    fn set_body(&self, b: String) {
        *self.body.lock().unwrap() = b;
    }

    /// 起動から `ms` ミリ秒後まで進める（同時刻の競合を避けるため半端な値で呼ぶ）。
    async fn until(&self, ms: u64) {
        sleep_until(self.t0 + Duration::from_millis(ms)).await;
        tokio::task::yield_now().await;
    }

    fn now_ms(&self) -> u64 {
        (Instant::now() - self.t0).as_millis() as u64
    }

    fn starts_ms(&self) -> Vec<u64> {
        self.starts
            .lock()
            .unwrap()
            .iter()
            .map(|d| d.as_millis() as u64)
            .collect()
    }
}

fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn cfg(period: u64) -> RefreshConfig {
    RefreshConfig::new(s(period)).unwrap()
}

fn ceil_div(a: u64, b: u64) -> u64 {
    a.div_ceil(b)
}

/// `[from_s, to_s)` の各秒の 0.5 秒目に、`servers` へ未知 `kid` のトークンを `per_sec`
/// 件ずつ送る（攻撃者の鍵で正しく署名したものと署名が出鱈目なものを交互に）。
/// 受理（401 以外）の件数を返す。
async fn flood(
    rig: &Rig,
    servers: &[&Server],
    evil: &Ec,
    from_s: u64,
    to_s: u64,
    per_sec: u64,
) -> u64 {
    let mut accepted = 0;
    for sec in from_s..to_s {
        rig.until(sec * 1000 + 500).await;
        for n in 0..per_sec {
            let kid = format!("unknown-{sec}-{n}");
            let t = if n % 2 == 0 {
                evil.token(&kid)
            } else {
                forged_token(&kid)
            };
            let srv = servers[(n as usize) % servers.len()];
            if status(srv, &t).await != 401 {
                accepted += 1;
            }
        }
    }
    accepted
}

/// 取得の開始が下限ごとに間引かれていること。
/// 連続する開始の間隔は下限以上で、どの開始時刻を左端とする半開区間 `[a, a + T)` でも
/// 開始回数は `ceil(T / L)` 以下（定期の項を足す前の強い形。設計 8.7）。
fn assert_rate_limited(starts: &[u64], min_interval_ms: u64) {
    for w in starts.windows(2) {
        assert!(
            w[1] - w[0] >= min_interval_ms,
            "開始の間隔が下限未満: {starts:?}"
        );
    }
    let l = min_interval_ms;
    for t in [l * 5 / 2, l * 3, l * 7 / 2, l * 5, l * 11 / 2] {
        for &a in starts {
            let n = starts.iter().filter(|&&x| x >= a && x < a + t).count() as u64;
            assert!(
                n <= ceil_div(t, l),
                "窓 [{a}, {a}+{t}) の開始 {n} 回が上限超過"
            );
        }
    }
}

// ---- R1: 回数の上限 ----

#[tokio::test(start_paused = true)]
async fn r1_unknown_kid_flood_is_bounded_by_min_interval() {
    let k1 = Ec::new();
    let evil = Ec::new();
    let rig = Rig::start(cfg(3600), jwks(&[(&k1, "k1")]), 0);
    let server = rig.server();
    rig.until(100).await;
    let accepted = flood(&rig, &[&server], &evil, 0, 60, 20).await;
    rig.until(59_900).await;
    assert_eq!(accepted, 0, "偽のトークンが受理された");
    let starts = rig.starts_ms();
    // [0, 60 s): 起動時の 1 回 + 10 秒刻みの合図 5 回。式は ceil(60/10) + 定期 1 回 = 7 以下。
    assert_eq!(starts, [0, 10_000, 20_000, 30_000, 40_000, 50_000]);
    assert!(starts.len() as u64 <= ceil_div(60, 10) + 1);
    assert_rate_limited(&starts, 10_000);
}

#[tokio::test(start_paused = true)]
async fn r1_bound_holds_over_every_half_open_window() {
    let k1 = Ec::new();
    let evil = Ec::new();
    let rig = Rig::start(cfg(3600), jwks(&[(&k1, "k1")]), 0);
    let server = rig.server();
    let accepted = flood(&rig, &[&server], &evil, 0, 300, 4).await;
    assert_eq!(accepted, 0);
    let starts = rig.starts_ms();
    // 300 秒の間ずっと飽和しているので、開始は下限ちょうどの刻みになる。
    assert_eq!(starts.len(), 30);
    assert_rate_limited(&starts, 10_000);
}

#[tokio::test(start_paused = true)]
async fn r1_bound_holds_with_failures_and_slow_fetches() {
    for (name, mode, delay_ms) in [
        ("transport_error", MODE_TRANSPORT_ERR, 0),
        ("timeout", MODE_HANG, 0),
        ("slow_but_ok", MODE_OK, 3_000),
    ] {
        let k1 = Ec::new();
        let evil = Ec::new();
        let rig = Rig::start(cfg(3600), jwks(&[(&k1, "k1")]), delay_ms);
        rig.mode.store(mode, Ordering::SeqCst);
        let server = rig.server();
        let accepted = flood(&rig, &[&server], &evil, 0, 100, 4).await;
        assert_eq!(accepted, 0, "{name}");
        let starts = rig.starts_ms();
        assert!(
            starts.len() as u64 <= ceil_div(100, 10) + 1,
            "{name}: 開始 {} 回",
            starts.len()
        );
        assert_rate_limited(&starts, 10_000);
    }
}

#[tokio::test(start_paused = true)]
async fn r1_bound_holds_with_non_default_min_interval() {
    let k1 = Ec::new();
    let evil = Ec::new();
    let cfg = RefreshConfig::builder(s(60))
        .min_interval(s(7))
        .build()
        .unwrap();
    let rig = Rig::start(cfg, jwks(&[(&k1, "k1")]), 0);
    let server = rig.server();
    let accepted = flood(&rig, &[&server], &evil, 0, 130, 4).await;
    assert_eq!(accepted, 0);
    let starts = rig.starts_ms();
    // T=130 s・L=7 s: ceil(130/7)=19 + 定期（起動時 + 60 秒ごと = 3 回）= 22 以下。
    assert!(starts.len() as u64 <= ceil_div(130, 7) + 3, "{starts:?}");
    assert_rate_limited(&starts, 7_000);
}

#[tokio::test(start_paused = true)]
async fn r1_shared_trigger_across_two_gates_keeps_one_bound() {
    let k1 = Ec::new();
    let evil = Ec::new();
    let rig = Rig::start(cfg(3600), jwks(&[(&k1, "k1")]), 0);
    let a = gate_server(&rig.shared, rig.handle.trigger(), None);
    let b = gate_server(&rig.shared, rig.handle.trigger(), None);
    let accepted = flood(&rig, &[&a, &b], &evil, 0, 100, 6).await;
    assert_eq!(accepted, 0);
    let starts = rig.starts_ms();
    assert!(starts.len() as u64 <= ceil_div(100, 10) + 1, "{starts:?}");
    assert_rate_limited(&starts, 10_000);
}

// ---- R2: 事前公開なしの鍵切替への追従 ----

const FETCH_MS: u64 = 3_000;
const LIMIT_MS: u64 = 10_000 + FETCH_MS;

/// 発行者が `switch_ms` に本文を {k2} へ切り替える。k2 のトークンを 100 ms ごとに
/// 送り、切替の時点で 401 になること・最初に 200 になるまでの時間を返す。
/// `prime_at_ms` を渡すと、その時刻に未知 kid のトークンを送って取得を予約し、
/// 切替の時点で「旧本文の取得が進行中」であることを確かめてから切り替える。
async fn follow_switch(switch_ms: u64, prime_at_ms: Option<u64>) -> u64 {
    let k1 = Ec::new();
    let k2 = Ec::new();
    let rig = Rig::start(cfg(3600), jwks(&[(&k1, "k1")]), FETCH_MS);
    let server = rig.server();
    if let Some(p) = prime_at_ms {
        rig.until(p).await;
        assert_eq!(status(&server, &forged_token("prime")).await, 401);
    }
    rig.until(switch_ms).await;
    if prime_at_ms.is_some() {
        // 予約した取得は下限（起動から 10 s）に始まり、切替の時点で進行中（旧本文を取得中）。
        let starts = rig.starts_ms();
        assert_eq!(starts, [0, 10_000], "取得の予約が始まっていない");
        assert!(
            switch_ms > 10_000 && switch_ms < 10_000 + FETCH_MS,
            "切替が取得中でない"
        );
    }
    rig.set_body(jwks(&[(&k2, "k2")]));
    let s = rig.now_ms();
    // 取り直しを待たず、その場で拒否される（フェイルクローズ）。
    assert_eq!(status(&server, &k2.token("k2")).await, 401);
    loop {
        rig.until(rig.now_ms() + 100).await;
        if status(&server, &k2.token("k2")).await == 200 {
            return rig.now_ms() - s;
        }
        assert!(rig.now_ms() - s <= LIMIT_MS + 1_000, "追従が遅すぎる");
    }
}

#[tokio::test(start_paused = true)]
async fn r2_unannounced_rotation_is_followed_within_min_interval_plus_fetch_time() {
    // 起動時の取得（0〜3 秒）の完了後、直前の開始 f=0 から数えて最悪に近い切替。
    let took = follow_switch(1_050, None).await;
    // 合図は f + L = 10 s に取得を始め、3 秒後の 13 s に反映される。
    assert!(took <= LIMIT_MS, "追従に {took} ms（上限 {LIMIT_MS} ms）");
    // 反映前（取得の完了前）は 200 にならない: 13 s の完了より前の追従はあり得ない。
    assert!(took >= 11_000, "取得の完了より早く受理された: {took}");
}

#[tokio::test(start_paused = true)]
async fn r2_follow_bound_for_various_switch_offsets() {
    // 起動時取得の直後・中間・次の許可の直前・予約した取得の開始直前。
    for switch_ms in [3_050, 6_050, 9_950] {
        let took = follow_switch(switch_ms, None).await;
        assert!(
            took <= LIMIT_MS,
            "切替 {switch_ms} ms: 追従に {took} ms（上限 {LIMIT_MS} ms）"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn r2_switch_during_in_flight_fetch_is_followed_by_next_fetch() {
    // 5 s の未知 kid で取得を予約すると、取得は下限の 10 s に始まり旧本文を得る（3 s 間）。
    // 切替がその取得の最中（10.05 s・10.5 s・12.9 s）に起きると、その取得は旧本文のまま
    // 完了するので、追従は取得中に出た合図による次回の取得（下限 20 s、完了 23 s）になる。
    for switch_ms in [10_050, 10_500, 12_900] {
        let took = follow_switch(switch_ms, Some(5_000)).await;
        assert!(
            took <= LIMIT_MS,
            "切替 {switch_ms} ms: 追従に {took} ms（上限 {LIMIT_MS} ms）"
        );
        // 進行中の取得は旧本文なので、その完了（13 s）では受理されず、次回（23 s）以降。
        assert!(
            took >= 23_000 - switch_ms,
            "切替 {switch_ms} ms: 旧本文の取得で受理された: {took}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn r2_attacker_noise_does_not_delay_following() {
    let k1 = Ec::new();
    let k2 = Ec::new();
    let evil = Ec::new();
    let rig = Rig::start(cfg(3600), jwks(&[(&k1, "k1")]), FETCH_MS);
    let notified = Arc::new(AtomicUsize::new(0));
    let server = gate_server(
        &rig.shared,
        rig.handle.trigger(),
        Some(Arc::clone(&notified)),
    );
    rig.until(5_050).await;
    // 切替の直前に攻撃者の未知 kid が通知口の間引きを使い切っていても、合図は独立に働く。
    for n in 0..50 {
        assert_eq!(
            status(&server, &evil.token(&format!("noise-{n}"))).await,
            401
        );
    }
    assert!(
        notified.load(Ordering::SeqCst) < 50,
        "通知口が間引かれていない"
    );
    rig.set_body(jwks(&[(&k2, "k2")]));
    let sw = rig.now_ms();
    assert_eq!(status(&server, &k2.token("k2")).await, 401);
    let t_acc = loop {
        rig.until(rig.now_ms() + 100).await;
        if status(&server, &k2.token("k2")).await == 200 {
            break rig.now_ms();
        }
        assert!(rig.now_ms() - sw <= LIMIT_MS + 1_000, "追従が遅すぎる");
    };
    assert!(t_acc - sw <= LIMIT_MS, "追従に {} ms", t_acc - sw);
}

// ---- R3: 通常の回転 ----

#[tokio::test(start_paused = true)]
async fn r3_announced_rotation_has_no_verification_failures() {
    let k1 = Ec::new();
    let k2 = Ec::new();
    let rig = Rig::start(cfg(60), jwks(&[(&k1, "k1")]), 0);
    let server = rig.server();
    let mut unexpected_401 = 0;

    for sec in 0..420u64 {
        rig.until(sec * 1000 + 500).await;
        // 発行者の運用: 100 秒に事前公開、200 秒に署名鍵を切替、300 秒に旧鍵を退役。
        if sec == 100 {
            rig.set_body(jwks(&[(&k1, "k1"), (&k2, "k2")]));
        }
        if sec == 300 {
            rig.set_body(jwks(&[(&k2, "k2")]));
        }
        let signing_is_k2 = sec >= 200;
        let current = if signing_is_k2 {
            k2.token("k2")
        } else {
            k1.token("k1")
        };
        if status(&server, &current).await != 200 {
            unexpected_401 += 1;
        }
        // 切替前に発行済みの k1 のトークンは退役までは使われうる。
        if sec < 300 && status(&server, &k1.token("k1")).await != 200 {
            unexpected_401 += 1;
        }
    }
    assert_eq!(unexpected_401, 0, "正当なトークンが拒否された");
    // 取得は定期の予定（60 秒刻み）だけで、未知 kid による追加の取得は一度も起きていない。
    let starts = rig.starts_ms();
    assert_eq!(starts, (0..7).map(|i| i * 60_000).collect::<Vec<_>>());
    // 退役が反映された（360 s の取得）後は旧鍵のトークンは拒否される（期待どおり）。
    // この確認は未知 kid の合図を出すため、取得回数の確認の後に行う。
    assert_eq!(status(&server, &k1.token("k1")).await, 401);
    assert_eq!(status(&server, &k2.token("k2")).await, 200);
}
