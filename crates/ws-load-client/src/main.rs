//! 10,000 同時 WebSocket 接続の負荷生成クライアント（TASK-4.3 / #24）。
//!
//! `docs/spec/03-poc/high-concurrency-scale/load-client`（PoC-7、submodule。
//! 参照のみ・改変不可）を移植・改良した計測専用バイナリ。`benches/bench-ws-load.sh`
//! が `crates/core/examples/ws_echo.rs`（フルスクラッチ）・`crates/axum-ref`
//! （`ws` feature 有効）の双方へ同一の負荷を掛けるために起動する。
//!
//! # workspace 内での位置づけ
//!
//! `crates/axum-ref` と同じく依存方向グラフの**外側**にある独立計測バイナリで、
//! workspace 内 path 依存を一切持たない（持ってはならない）。
//! workspace 全体の依存方向規約（依存方向: server → routes → http::* 、
//! `docs/spec/04-requirements.md` REQ-1 / `docs/spec/05-tasks.md` TASK-11.1）との関係は
//! `crates/axum-ref/src/main.rs` の doc と同一（本クレートもこのグラフの外側）。
//! 依存方向の機械検証は `scripts/dep-direction-check.sh` が担う。
//!
//! # PoC-7 からの変更点
//!
//! - 確立成功率・保持完了数の集計を [`ConnectSummary`] としてクライアント側でも
//!   構造化し、`RESULT_JSON`（環境変数でパス指定時のみ）へ機械可読出力する
//!   （`benches/lib/common.sh` の `write_result_json` と同じ「未指定時は no-op」契約）。
//!   接続あたり RSS 増分の判定自体は `benches/bench-ws-load.sh` 側（サーバプロセスの
//!   `ps -o rss=` サンプリング）が担い、本クライアントは接続数・成功率・心拍
//!   レイテンシの計測に徹する（PoC-7 と同じ責務分界）。
//! - 集計関数（成功率・percentile・env パース）を単体テスト可能な関数へ切り出し、
//!   `feature-flow-check.sh`（実装変更にはテスト追加を伴わせる規約）を満たす。
//!
//! # セキュリティ考慮
//!
//! 対象ホストは `TARGET_URL` env で指定する（既定 `ws://127.0.0.1:3000/ws`、
//! ループバック限定）。外部ホストへ向けたい場合は呼び出し側の責任で明示的に
//! 指定すること（誤って外部サービスへ負荷を向けない、`.claude/rules/security.md`）。
//! 本バイナリは制御された自己 DoS（負荷試験）を行うものであり、通常運用では
//! ループバック対向でのみ使う。
//!
//! 環境変数:
//! - `TARGET_URL`: 接続先（例: `ws://127.0.0.1:3000/ws`）
//! - `CONNECTIONS`: 目標接続数（既定 1000）
//! - `RAMP_BATCH`: 1 バッチあたりの同時接続試行数（既定 200）
//! - `RAMP_DELAY_MS`: バッチ間の待機（既定 50ms）
//! - `HOLD_SECS`: 接続確立後の維持時間・秒（既定 20）
//! - `HEARTBEAT_MS`: 心拍間隔・ミリ秒（既定 2000）
//! - `RESULT_JSON`: 指定時のみ、集計結果を JSON でこのパスへ書き出す

use futures_util::{SinkExt, StreamExt};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

/// 文字列（env 値相当）を `usize` としてパースし、欠落・パース失敗時は
/// `default` を返す純関数。
///
/// [`env_usize`] から呼ばれる。プロセス環境変数の読み取り自体と純粋なパース
/// ロジックを分離することで、edition 2024 で `unsafe` 化された
/// `std::env::set_var`/`remove_var`（プロセスグローバル状態の変更、他スレッドの
/// 環境変数読み取りとのデータ競合の可能性）をテストに使わずに済ませる
/// （このモジュール内では `unsafe` を一切使わない、`.claude/rules/coding-rust.md`）。
fn parse_usize_or(raw: Option<&str>, default: usize) -> usize {
    raw.and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// [`parse_usize_or`] の `u64` 版。
fn parse_u64_or(raw: Option<&str>, default: u64) -> u64 {
    raw.and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// `usize` 環境変数を読み、パース失敗・未設定時は `default` を返す。
///
/// 負荷試験パラメータの env 入力はすべてこの関数（および [`env_u64`]）経由で
/// パースし、想定外の文字列でパニックしない安全側処理にする
/// （`.claude/rules/security.md` の入力検証）。
fn env_usize(name: &str, default: usize) -> usize {
    parse_usize_or(std::env::var(name).ok().as_deref(), default)
}

/// `u64` 環境変数を読み、パース失敗・未設定時は `default` を返す（[`env_usize`] 参照）。
fn env_u64(name: &str, default: u64) -> u64 {
    parse_u64_or(std::env::var(name).ok().as_deref(), default)
}

/// 確立成功率（%）を算出する。`requested` が 0 の場合は 0.0 を返し、
/// ゼロ除算による NaN・panic を避ける（呼び出し元が誤って `CONNECTIONS=0` を
/// 渡した場合の安全側処理）。
fn success_rate_percent(connected: u64, requested: u64) -> f64 {
    if requested == 0 {
        return 0.0;
    }
    (connected as f64 / requested as f64) * 100.0
}

/// ソート済みレイテンシ列（マイクロ秒）から百分位数を求める。
///
/// `p` は 0.0〜1.0（例: p95 なら 0.95）。`sorted` が空の場合は 0 を返す。
/// `sorted` は呼び出し前に昇順ソート済みであることを前提とする（このモジュール内
/// では [`main`] が `sort_unstable` 済みの `Vec` のみを渡す）。
fn percentile_us(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// JSON 文字列値として安全に埋め込めるようダブルクォート・バックスラッシュ・
/// 制御文字をエスケープする。`TARGET_URL` はループバック URL を想定するが、
/// 呼び出し側が任意の文字列を env 経由で渡せるため、`RESULT_JSON` 出力時の
/// JSON 構造破壊・インジェクションを防ぐために手作業でエスケープする
/// （このクレートは serde_json 等を追加依存として持ち込まない、
/// `.claude/rules/pay-for-what-you-use.md`）。
fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// 1 回の負荷試験実行の集計結果。`RESULT_JSON` の機械可読出力・標準出力の
/// 人間可読表示の両方の元データとして使う共通構造体。
struct ConnectSummary {
    target: String,
    requested_connections: u64,
    connected: u64,
    failed: u64,
    success_rate_percent: f64,
    heartbeat_samples: usize,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    max_us: u64,
}

impl ConnectSummary {
    /// `RESULT_JSON`（指定時のみ）へ書き出す JSON 文字列を組み立てる。
    ///
    /// `benches/lib/common.sh` の `write_result_json`（未指定時 no-op）契約に
    /// 合わせ、本関数自体は文字列を返すのみで、書き出しの有無判定は
    /// [`main`] 側（`RESULT_JSON` env の有無）が担う。
    fn to_json(&self) -> String {
        format!(
            "{{\"target\":\"{}\",\"requested_connections\":{},\"connected\":{},\"failed\":{},\
             \"success_rate_percent\":{:.2},\"heartbeat_samples\":{},\"heartbeat_rtt_us\":{{\
             \"p50\":{},\"p95\":{},\"p99\":{},\"max\":{}}}}}",
            json_escape(&self.target),
            self.requested_connections,
            self.connected,
            self.failed,
            self.success_rate_percent,
            self.heartbeat_samples,
            self.p50_us,
            self.p95_us,
            self.p99_us,
            self.max_us,
        )
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let target =
        std::env::var("TARGET_URL").unwrap_or_else(|_| "ws://127.0.0.1:3000/ws".to_string());
    let connections = env_usize("CONNECTIONS", 1000);
    let ramp_batch = env_usize("RAMP_BATCH", 200).max(1);
    let ramp_delay_ms = env_u64("RAMP_DELAY_MS", 50);
    let hold_secs = env_u64("HOLD_SECS", 20);
    let heartbeat_ms = env_u64("HEARTBEAT_MS", 2000);

    let connected = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let active = Arc::new(AtomicU64::new(0));
    let latencies_us: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));

    let ramp_start = Instant::now();
    let mut handles = Vec::with_capacity(connections);

    for i in 0..connections {
        let target = target.clone();
        let connected = connected.clone();
        let failed = failed.clone();
        let active = active.clone();
        let latencies_us = latencies_us.clone();

        let handle = tokio::spawn(async move {
            let connect_result = tokio::time::timeout(
                Duration::from_secs(10),
                tokio_tungstenite::connect_async(&target),
            )
            .await;
            let ws = match connect_result {
                Ok(Ok((ws, _resp))) => ws,
                _ => {
                    failed.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            };
            connected.fetch_add(1, Ordering::Relaxed);
            active.fetch_add(1, Ordering::Relaxed);

            let (mut write, mut read) = ws.split();
            let hold_deadline = Instant::now() + Duration::from_secs(hold_secs);

            loop {
                let now = Instant::now();
                if now >= hold_deadline {
                    break;
                }
                let sleep_for = Duration::from_millis(heartbeat_ms).min(hold_deadline - now);
                tokio::time::sleep(sleep_for).await;
                if Instant::now() >= hold_deadline {
                    break;
                }

                let sent_at = Instant::now();
                if write
                    .send(Message::Text(format!("hb-{i}").into()))
                    .await
                    .is_err()
                {
                    break;
                }
                match tokio::time::timeout(Duration::from_secs(5), read.next()).await {
                    Ok(Some(Ok(Message::Text(_) | Message::Binary(_)))) => {
                        let rtt = sent_at.elapsed().as_micros() as u64;
                        latencies_us.lock().await.push(rtt);
                    }
                    _ => break,
                }
            }

            let _ = write.send(Message::Close(None)).await;
            active.fetch_sub(1, Ordering::Relaxed);
        });
        handles.push(handle);

        if (i + 1) % ramp_batch == 0 {
            tokio::time::sleep(Duration::from_millis(ramp_delay_ms)).await;
        }
    }

    let ramp_elapsed = ramp_start.elapsed();
    eprintln!(
        "[ramp] requested={connections} elapsed={:.2}s",
        ramp_elapsed.as_secs_f64()
    );

    for h in handles {
        let _ = h.await;
    }

    let connected_n = connected.load(Ordering::Relaxed);
    let failed_n = failed.load(Ordering::Relaxed);
    let mut lats = latencies_us.lock().await.clone();
    lats.sort_unstable();

    let summary = ConnectSummary {
        target: target.clone(),
        requested_connections: connections as u64,
        connected: connected_n,
        failed: failed_n,
        success_rate_percent: success_rate_percent(connected_n, connections as u64),
        heartbeat_samples: lats.len(),
        p50_us: percentile_us(&lats, 0.50),
        p95_us: percentile_us(&lats, 0.95),
        p99_us: percentile_us(&lats, 0.99),
        max_us: lats.last().copied().unwrap_or(0),
    };

    println!("=== ws-load-client result ===");
    println!("target={}", summary.target);
    println!("requested_connections={}", summary.requested_connections);
    println!("connected={}", summary.connected);
    println!("failed={}", summary.failed);
    println!("success_rate={:.2}%", summary.success_rate_percent);
    println!("heartbeat_samples={}", summary.heartbeat_samples);
    if summary.heartbeat_samples > 0 {
        println!("heartbeat_rtt_us_p50={}", summary.p50_us);
        println!("heartbeat_rtt_us_p95={}", summary.p95_us);
        println!("heartbeat_rtt_us_p99={}", summary.p99_us);
        println!("heartbeat_rtt_us_max={}", summary.max_us);
    }

    // `RESULT_JSON`（指定時のみ）: `benches/lib/common.sh` の `write_result_json` と
    // 同じ「未指定時は no-op」契約。書き込み失敗はスクリプト側の後続処理
    // （bench-ws-load.sh の閾値判定）を静かに欺かないよう、明示的にエラー終了する。
    if let Ok(path) = std::env::var("RESULT_JSON") {
        match std::fs::File::create(&path) {
            Ok(mut file) => {
                if let Err(err) = file.write_all(summary.to_json().as_bytes()) {
                    eprintln!("エラー: RESULT_JSON（{path}）への書き込みに失敗しました: {err}");
                    std::process::exit(1);
                }
            }
            Err(err) => {
                eprintln!("エラー: RESULT_JSON（{path}）を作成できませんでした: {err}");
                std::process::exit(1);
            }
        }
    }
}

// workspace 全体の依存方向規約（依存方向: server → routes → http::* 、
// `crates/axum-ref/src/main.rs` の doc と同一）。本クレートはこのグラフの
// 外側にある独立計測バイナリであり、workspace 内 path 依存を持たない。
// `scripts/dep-direction-check.sh` のエントリポイント宣言検査（チェック 2）が
// 本コメントの存在を機械検証する。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_rate_percent_computes_ratio() {
        assert!((success_rate_percent(9999, 10000) - 99.99).abs() < 0.001);
        assert!((success_rate_percent(10000, 10000) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn success_rate_percent_zero_requested_is_zero() {
        // ゼロ除算による NaN・panic を避ける安全側処理（誤って CONNECTIONS=0 を
        // 渡した場合の防御）。
        assert_eq!(success_rate_percent(0, 0), 0.0);
    }

    #[test]
    fn percentile_us_empty_is_zero() {
        assert_eq!(percentile_us(&[], 0.95), 0);
    }

    #[test]
    fn percentile_us_matches_known_distribution() {
        // index = round((len-1)*p) の最近接丸め（四捨五入）で決まる。
        // 値 1..=100（index 0 が値 1）に対し p50 は index 50（値 51）を指す。
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile_us(&sorted, 0.50), 51);
        assert_eq!(percentile_us(&sorted, 0.99), 99);
        assert_eq!(percentile_us(&sorted, 1.0), 100);
    }

    #[test]
    fn parse_usize_or_falls_back_to_default_on_missing_or_invalid() {
        assert_eq!(parse_usize_or(None, 42), 42);
        assert_eq!(parse_usize_or(Some("not-a-number"), 42), 42);
        assert_eq!(parse_usize_or(Some("7"), 42), 7);
    }

    #[test]
    fn parse_u64_or_parses_valid_value() {
        assert_eq!(parse_u64_or(Some("1234"), 0), 1234);
        assert_eq!(parse_u64_or(None, 99), 99);
    }

    #[test]
    fn json_escape_escapes_quotes_and_backslashes() {
        assert_eq!(json_escape(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    #[test]
    fn connect_summary_to_json_is_well_formed_minimal() {
        let summary = ConnectSummary {
            target: "ws://127.0.0.1:3000/ws".to_string(),
            requested_connections: 10,
            connected: 10,
            failed: 0,
            success_rate_percent: 100.0,
            heartbeat_samples: 0,
            p50_us: 0,
            p95_us: 0,
            p99_us: 0,
            max_us: 0,
        };
        let json = summary.to_json();
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert!(json.contains("\"connected\":10"));
        assert!(json.contains("\"success_rate_percent\":100.00"));
    }
}
