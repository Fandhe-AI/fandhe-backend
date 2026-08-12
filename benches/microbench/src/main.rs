//! 決定的マイクロベンチ本体（イシュー #615）。
//!
//! `parse_request_head`（`fandhe-backend-http`）→ `Router::dispatch`
//! （`fandhe-backend-routes`）→ `Response::serialize` の同期・決定的な
//! per-request パスに対して、ヒープアロケーション回数・バイト数を計測する。
//! 対象は実時間ベンチ（`benches/bench-http.sh`）と同一の 4 シナリオ
//! （`GET /health`・`GET /hello/{name}`・`GET /users/{id}`・`POST /echo`）。
//!
//! **既知の限界**: `crates/core` の接続受理・tokio ランタイム経由の非同期処理は
//! スケジューリング起因で alloc 数が非決定になりうるため対象外（実時間退行クラスは
//! `benches/bench-accept.sh` 系の守備範囲。`docs/design/deterministic-microbench.md`
//! 参照）。
//!
//! CLI:
//! - 引数なし: 計測を実行し、結果を stdout へ JSON（[`Report`]）で出力する
//! - `--check <baseline.json>`: 計測結果をベースラインと厳密比較し、
//!   いずれかの指標が増加していれば非 0 終了する（ラチェット、フェイルクローズ）
//! - `--update-baseline <baseline.json>`: 計測結果でベースラインファイルを上書きする
//!
//! `benches/microbench.sh` はビルド・実行・引数選択のみを担う薄いラッパーで、
//! 比較・判定ロジックは本ファイル 1 箇所（[`compare_with_baseline`]）に集約する。
#![forbid(unsafe_code)]

use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
use fandhe_backend_http::response::Response;
use fandhe_backend_routes::{PathParams, Router};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::ExitCode;
use std::task::{Context, Poll, Waker};

/// カウンティング `#[global_allocator]`。`GlobalAlloc` トレイト実装は
/// `stats_alloc`（dev 相当の計測専用依存）内に閉じ、本クレートは
/// `#![forbid(unsafe_code)]` を維持できる（`crates/http/tests/alloc_count.rs`、
/// PR #602 レビュー指摘 P0 対応と同一パターン）。
#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// 決定性の自己検証で 1 シナリオあたり繰り返す反復回数。全反復で計数が完全
/// 一致しなければベンチ自体を fail-closed に非 0 終了させる（測定ノイズの
/// 混入・非決定なコードパスの混入を即座に検知する前提条件チェック）。
const REPEAT: usize = 10;

/// 1 回の poll で完了しないハンドラ future を検知するための上限反復回数。
/// 本ベンチが対象とするのは同期登録ハンドラ（初回 poll で必ず完了する契約、
/// `Router::route` / `Router::route_param` の doc 参照）のみのため、これを
/// 超えたらベンチの前提が崩れているとみなし fail-closed に終了する。
const MAX_POLLS: usize = 16;

/// 1 シナリオの計測結果（1 リクエストあたりの alloc 回数・バイト数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AllocStats {
    /// `alloc` / `alloc_zeroed` / `realloc` の呼び出し回数（`dealloc` は非計上）。
    /// `stats_alloc` は `realloc` 呼び出しを `Stats::allocations` に含めず
    /// `Stats::reallocations` として別カウントするため、[`measure`] で両者を
    /// 合算する（イシュー #619 Bugbot 指摘 Medium 対応。旧実装は
    /// `Stats::allocations` のみを読んでいたため `Vec`/`String` の容量拡張
    /// （`realloc` 経由）による呼び出し回数の増加がラチェットを一切動かさな
    /// かった）。
    allocations: usize,
    /// 上記呼び出しで確保された合計バイト数。`stats_alloc` の `realloc`
    /// 実装（`stats_alloc` 0.1.10 `src/lib.rs`）は growth 分の差分バイトを
    /// `Stats::bytes_allocated` へ加算済みのため（shrink 分は
    /// `Stats::bytes_deallocated` 側）、本フィールドは `change.bytes_allocated`
    /// のみで `realloc` による増加分を含めて正しく計上できる
    /// （`Stats::bytes_reallocated` は正負が混在する net 差分のため、
    /// gross allocated bytes を計上する本フィールドの用途には使わず二重計上
    /// を避ける）。
    bytes: usize,
}

/// ベンチ対象の 1 シナリオ（実時間ベンチ `benches/bench-http.sh` の 4
/// エンドポイントに対応、イシュー #615 実装計画 2.4 節）。
struct Scenario {
    /// 結果 JSON・ベースラインのキー（安定 ID）。
    name: &'static str,
    /// 生の HTTP/1.1 リクエスト（ヘッド + ボディ）バイト列。
    request: Vec<u8>,
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "get_health",
            request: b"GET /health HTTP/1.1\r\nHost: bench\r\n\r\n".to_vec(),
        },
        Scenario {
            name: "get_hello_name",
            request: b"GET /hello/world HTTP/1.1\r\nHost: bench\r\n\r\n".to_vec(),
        },
        Scenario {
            name: "get_users_id",
            request: b"GET /users/42 HTTP/1.1\r\nHost: bench\r\n\r\n".to_vec(),
        },
        Scenario {
            name: "post_echo",
            request: {
                let body = br#"{"message":"bench"}"#;
                let mut buf = format!(
                    "POST /echo HTTP/1.1\r\nHost: bench\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                buf.extend_from_slice(body);
                buf
            },
        },
    ]
}

/// 実時間ベンチ（`benches/bench-http.sh`）と同一の 4 エンドポイントを持つ
/// `Router` を構築する（`crates/axum-ref` の `health`/`hello`/`get_user`/`echo`
/// ハンドラと応答内容を揃える必要はない。alloc プロファイル計測が目的のため
/// 応答は最小構成とする）。
fn build_router() -> Router {
    Router::new()
        .route("GET", "/health", |_head, _body| {
            Response::new(200, b"ok".to_vec())
        })
        .route_param(
            "GET",
            "/hello/{name}",
            |_head, params: &PathParams<'_>, _body| {
                let name = params.get("name").unwrap_or("world");
                Response::new(200, format!("hello, {name}").into_bytes())
            },
        )
        .expect("route_param pattern /hello/{name} must be valid")
        .route_param(
            "GET",
            "/users/{id}",
            |_head, params: &PathParams<'_>, _body| {
                let id = params.get("id").unwrap_or("0");
                Response::new(200, format!("{{\"id\":{id}}}").into_bytes())
            },
        )
        .expect("route_param pattern /users/{id} must be valid")
        .route("POST", "/echo", |_head, body| {
            Response::new(200, body.to_vec())
        })
}

/// `Router::dispatch` が返す `HandlerFuture`（`Pin<Box<dyn Future<Output =
/// Response> + Send>>`）を tokio ランタイムなしで駆動する。対象は同期登録
/// ハンドラ（`Router::route` / `Router::route_param`）に限定され、これらは
/// 内部で `std::future::ready` に包まれているため初回 poll で必ず完了する
/// 契約（`crates/routes/src/lib.rs` の doc 参照）。tokio 依存を増やさず
/// std のみで駆動するため `Waker::noop()`（1.85 で安定化、本リポジトリの
/// edition 2024 が要求する rustc バージョンで利用可能）を使う。
fn poll_to_completion(mut fut: Pin<Box<dyn Future<Output = Response> + Send>>) -> Response {
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    for _ in 0..MAX_POLLS {
        if let Poll::Ready(res) = fut.as_mut().poll(&mut cx) {
            return res;
        }
    }
    // 同期ハンドラが前提のベンチでこれに到達するのは設計外の入力・実装変更を
    // 意味する。計測の無音破損（誤った 0 alloc 等）を避けるため即座に落とす。
    panic!("handler future did not complete within {MAX_POLLS} polls (async handler mixed in?)");
}

/// リクエストバイト列に対して「パース → ルーティング → シリアライズ」の
/// 1 リクエスト分のパスを 1 回実行する。戻り値は捨てず `std::hint::black_box`
/// へ渡し、最適化によるパス自体の消去（dead code elimination）を防ぐ。
fn run_once(router: &Router, request: &[u8]) {
    let (head, consumed) = match parse_request_head(request).expect("parse should succeed") {
        ParseOutcome::Complete { head, consumed } => (head, consumed),
        ParseOutcome::Incomplete => panic!("scenario request must be a complete HTTP head"),
    };
    let body = &request[consumed..];
    let fut = router.dispatch(&head, body);
    let response = poll_to_completion(fut);
    let serialized = response.serialize(true);
    std::hint::black_box(&serialized);
}

/// `stats_alloc::Stats` から [`AllocStats`] へ変換する純関数（テストで
/// `#[global_allocator]` の実アロケーション抜きに検証できるよう [`measure`]
/// から切り出す）。`allocations` は `change.allocations`（`alloc`/
/// `alloc_zeroed`）と `change.reallocations`（`realloc`）の合算値
/// （[`AllocStats::allocations`] の doc 参照）。
fn alloc_stats_from_change(change: stats_alloc::Stats) -> AllocStats {
    AllocStats {
        allocations: change.allocations + change.reallocations,
        bytes: change.bytes_allocated,
    }
}

/// `f` の実行前後の alloc 回数・バイト数の差分を返す。
fn measure<F: FnOnce()>(f: F) -> AllocStats {
    let region = Region::new(ALLOCATOR);
    f();
    alloc_stats_from_change(region.change())
}

/// 各シナリオを [`REPEAT`] 回計測し、全反復で一致することを検証したうえで
/// 代表値（1 件目）を返す。不一致は非決定な alloc パスが紛れ込んだことを
/// 意味するため、`Err` で理由を返し呼び出し元が非 0 終了する（fail-closed）。
fn measure_scenario(router: &Router, scenario: &Scenario) -> Result<AllocStats, String> {
    // ウォームアップ: 初回のみ発生しうる遅延初期化コスト（アロケータ内部の
    // スレッドローカルキャッシュ構築等）を計測対象から除外する
    // （`crates/http/tests/alloc_count.rs` と同じ理由）。
    run_once(router, &scenario.request);

    let mut results = Vec::with_capacity(REPEAT);
    for _ in 0..REPEAT {
        let stats = measure(|| run_once(router, &scenario.request));
        results.push(stats);
    }

    let first = results[0];
    for (i, stats) in results.iter().enumerate().skip(1) {
        if *stats != first {
            return Err(format!(
                "scenario '{}' is non-deterministic: iteration 0 = {:?}, iteration {i} = {:?}",
                scenario.name, first, stats
            ));
        }
    }
    Ok(first)
}

/// 計測レポート（JSON 出力・ベースライン比較双方の内部表現）。
#[derive(Debug)]
struct Report {
    /// `rustc --version` の出力（toolchain 差異によるベースライン不一致の
    /// 切り分け用メタデータ、比較そのものには使わない）。
    rustc_version: String,
    /// シナリオ名 → 計測値。`BTreeMap` でキー順を安定させ、JSON 出力・
    /// ベースラインファイルの diff を決定的にする。
    scenarios: BTreeMap<&'static str, AllocStats>,
}

fn run_all(router: &Router) -> Result<Report, String> {
    let mut scenario_results = BTreeMap::new();
    for scenario in scenarios() {
        let stats = measure_scenario(router, &scenario)?;
        scenario_results.insert(scenario.name, stats);
    }
    Ok(Report {
        rustc_version: rustc_version(),
        scenarios: scenario_results,
    })
}

/// `rustc --version` を実行して取得する。取得できない場合でもベンチ本体の
/// 目的（alloc 回帰検知）は継続できるため、メタデータ欠落として `"unknown"`
/// を返すのみに留める（fail-open。alloc 指標そのものの判定には影響しない）。
fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

impl Report {
    fn to_json(&self) -> serde_json::Value {
        let mut scenarios = serde_json::Map::new();
        for (name, stats) in &self.scenarios {
            scenarios.insert(
                (*name).to_string(),
                serde_json::json!({
                    "allocations": stats.allocations,
                    "bytes": stats.bytes,
                }),
            );
        }
        serde_json::json!({
            "rustc_version": self.rustc_version,
            "scenarios": scenarios,
        })
    }

    /// ベースライン JSON をパースする。**JSON の `scenarios` オブジェクトに含まれる
    /// キーを [`scenarios`] の既知シナリオ名一覧と突き合わせ、未知キー（＝現在の
    /// コードに存在しないシナリオ）は即座に `Err` にする**（イシュー #619
    /// codex-review 指摘 P1 対応）。旧実装は現在の `scenarios()` に存在するキーだけを
    /// 選んで読み込んでいたため、コードからシナリオを削除するとベースライン側の
    /// 余分なキーが読み込み時点で無音に消え、[`compare_with_baseline`] のキー集合
    /// 比較が常に一致してしまい「シナリオ名の集合が一致しない場合は Err」という
    /// 契約が骨抜きになっていた（fail-closed 違反、計測対象の無音縮小）。本実装は
    /// 読み込みの時点で未知キーを検知するため、シナリオ削除は
    /// `compare_with_baseline` まで待たず `from_json` の時点で確実に検知できる。
    fn from_json(value: &serde_json::Value) -> Result<Report, String> {
        let rustc_version = value
            .get("rustc_version")
            .and_then(|v| v.as_str())
            .ok_or("baseline: missing string field \"rustc_version\"")?
            .to_string();
        let scenarios_obj = value
            .get("scenarios")
            .and_then(|v| v.as_object())
            .ok_or("baseline: missing object field \"scenarios\"")?;
        // 現在の scenarios() のキーのみを許容名として静的解決し、JSON 側の
        // 任意文字列キーを &'static str へ安全に対応付ける（未知キーは既知の
        // シナリオ名と一致しないため後段で明示エラーにする）。
        let known_names: BTreeMap<&str, &'static str> =
            scenarios().into_iter().map(|s| (s.name, s.name)).collect();
        let mut parsed_scenarios = BTreeMap::new();
        for (key, entry) in scenarios_obj {
            let name = *known_names.get(key.as_str()).ok_or_else(|| {
                format!(
                    "baseline: unknown scenario \"{key}\" (not present in current scenarios(); \
                     if this scenario was intentionally removed, regenerate the baseline with \
                     --update-baseline)"
                )
            })?;
            let allocations = entry
                .get("allocations")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| format!("baseline: scenario \"{name}\" missing allocations"))?
                as usize;
            let bytes = entry
                .get("bytes")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| format!("baseline: scenario \"{name}\" missing bytes"))?
                as usize;
            parsed_scenarios.insert(name, AllocStats { allocations, bytes });
        }
        Ok(Report {
            rustc_version,
            scenarios: parsed_scenarios,
        })
    }
}

/// 現在の計測値とベースラインを比較する（ラチェット方式、
/// `scripts/unsafe-triage.sh` と同型）。
///
/// - いずれかのシナリオで `allocations` または `bytes` が増加していれば
///   `Err`（退行、fail-closed で非 0 終了させる）
/// - 減少があれば標準エラーへ「ベースライン縮小を検討」の情報メッセージを出す
///   （exit 0 のまま、`--update-baseline` での明示更新を促すのみ）
/// - シナリオ名の集合が一致しない場合も `Err`（ベースラインの陳腐化を検知）
fn compare_with_baseline(current: &Report, baseline: &Report) -> Result<(), String> {
    if current.scenarios.keys().collect::<Vec<_>>() != baseline.scenarios.keys().collect::<Vec<_>>()
    {
        return Err(format!(
            "scenario set mismatch: current={:?}, baseline={:?}",
            current.scenarios.keys().collect::<Vec<_>>(),
            baseline.scenarios.keys().collect::<Vec<_>>()
        ));
    }

    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    for (name, cur) in &current.scenarios {
        let base = baseline.scenarios[name];
        if cur.allocations > base.allocations || cur.bytes > base.bytes {
            regressions.push(format!(
                "{name}: allocations {} -> {} (baseline -> current), bytes {} -> {}",
                base.allocations, cur.allocations, base.bytes, cur.bytes
            ));
        } else if cur.allocations < base.allocations || cur.bytes < base.bytes {
            improvements.push(format!(
                "{name}: allocations {} -> {}, bytes {} -> {} (baseline is now loose)",
                base.allocations, cur.allocations, base.bytes, cur.bytes
            ));
        }
    }

    if !improvements.is_empty() {
        eprintln!("情報: 以下のシナリオでベースラインより指標が改善しています。");
        eprintln!("      `--update-baseline` でベースラインを縮小できます（レビュー承認前提）:");
        for line in &improvements {
            eprintln!("  - {line}");
        }
    }

    if !regressions.is_empty() {
        // `rustc_version` が異なる場合はここで併記する（`docs/design/
        // deterministic-microbench.md` 6 節の運用手順「まず差分がコード変更
        // 起因か toolchain 起因か rustc_version の変化で切り分ける」を CI ログ
        // だけで実行できるようにする。切り分け材料の提示に留め、判定自体は
        // toolchain 差異でも `Err` のまま返す＝fail-closed は維持する）。
        let toolchain_note = if current.rustc_version != baseline.rustc_version {
            format!(
                "\n注記: rustc バージョンが baseline と異なります（baseline=\"{}\", current=\"{}\"）。\
                 toolchain 更新起因の可能性がある場合は docs/design/deterministic-microbench.md \
                 6 節の手順に従って切り分けること。",
                baseline.rustc_version, current.rustc_version
            )
        } else {
            String::new()
        };
        return Err(format!(
            "alloc profile regression detected (ratchet violation):\n{}{}",
            regressions.join("\n"),
            toolchain_note
        ));
    }

    Ok(())
}

fn usage() -> &'static str {
    "使い方: microbench [--check <baseline.json> | --update-baseline <baseline.json>]\n\
     \n\
     引数なし: 計測結果を JSON で stdout へ出力する\n\
     --check <path>: 計測結果を <path> のベースラインと比較し、退行があれば非 0 終了する\n\
     --update-baseline <path>: 計測結果で <path> を上書きする"
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let router = build_router();

    let report = match run_all(&router) {
        Ok(report) => report,
        Err(msg) => {
            eprintln!("エラー: {msg}");
            return ExitCode::FAILURE;
        }
    };

    match args.as_slice() {
        [] => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report.to_json()).expect("serialize report")
            );
            ExitCode::SUCCESS
        }
        [flag, path] if flag == "--update-baseline" => {
            match write_baseline(Path::new(path), &report) {
                Ok(()) => {
                    eprintln!("ベースラインを更新しました: {path}");
                    ExitCode::SUCCESS
                }
                Err(msg) => {
                    eprintln!("エラー: {msg}");
                    ExitCode::FAILURE
                }
            }
        }
        [flag, path] if flag == "--check" => match read_baseline(Path::new(path)) {
            Ok(baseline) => match compare_with_baseline(&report, &baseline) {
                Ok(()) => {
                    eprintln!("OK: 計測値はベースライン以下です");
                    ExitCode::SUCCESS
                }
                Err(msg) => {
                    eprintln!("エラー: {msg}");
                    ExitCode::FAILURE
                }
            },
            Err(msg) => {
                eprintln!("エラー: {msg}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn write_baseline(path: &Path, report: &Report) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&report.to_json()).map_err(|e| e.to_string())?;
    fs::write(path, json + "\n").map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn read_baseline(path: &Path) -> Result<Report, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    Report::from_json(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`alloc_stats_from_change`] が `realloc` 呼び出し（`Stats::reallocations`）
    /// を `AllocStats::allocations` へ合算することを検証する（イシュー #619
    /// Bugbot 指摘 Medium の回帰テスト。実アロケータを動かさず合成
    /// `stats_alloc::Stats` から純粋に検証するため、`cargo test` の並列実行に
    /// よるカウンタ混入（上記コメント参照）の影響を受けない。旧実装は
    /// `change.allocations` のみを読んでいたため、`Vec`/`String` の容量拡張
    /// （`realloc` 経由）による回数増加がラチェットを動かさなかった）。
    #[test]
    fn alloc_stats_from_change_includes_reallocations_in_count() {
        let change = stats_alloc::Stats {
            allocations: 2,
            reallocations: 3,
            bytes_allocated: 128,
            ..Default::default()
        };
        let stats = alloc_stats_from_change(change);
        assert_eq!(
            stats.allocations, 5,
            "alloc + realloc の呼び出し回数を合算すること"
        );
        assert_eq!(stats.bytes, 128);
    }

    /// 4 シナリオそれぞれのレスポンス（status・body）が期待どおりであることを
    /// 検証する。alloc 回数の検証はここに含めない — `cargo test` はテストを
    /// 並列スレッド実行するため、プロセス全体で共有される
    /// `#[global_allocator]` カウンタへ他テストの alloc が混入し計数が
    /// フレーキーになる（`crates/http/tests/alloc_count.rs` が「本ファイル
    /// 1 個 = 1 テストに限定する」とする理由と同じ）。alloc 回数・決定性の
    /// 検証は本バイナリの通常実行（`cargo run --release` 相当、単一プロセス・
    /// 単一スレッドの `main` が担う）に委ねる。
    #[test]
    fn get_health_returns_200_ok() {
        let router = build_router();
        let request = b"GET /health HTTP/1.1\r\nHost: bench\r\n\r\n";
        let (head, consumed) = match parse_request_head(request).unwrap() {
            ParseOutcome::Complete { head, consumed } => (head, consumed),
            ParseOutcome::Incomplete => unreachable!(),
        };
        let response = poll_to_completion(router.dispatch(&head, &request[consumed..]));
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"ok".to_vec());
    }

    #[test]
    fn get_hello_name_returns_path_param() {
        let router = build_router();
        let request = b"GET /hello/alice HTTP/1.1\r\nHost: bench\r\n\r\n";
        let (head, consumed) = match parse_request_head(request).unwrap() {
            ParseOutcome::Complete { head, consumed } => (head, consumed),
            ParseOutcome::Incomplete => unreachable!(),
        };
        let response = poll_to_completion(router.dispatch(&head, &request[consumed..]));
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"hello, alice".to_vec());
    }

    #[test]
    fn get_users_id_returns_path_param() {
        let router = build_router();
        let request = b"GET /users/7 HTTP/1.1\r\nHost: bench\r\n\r\n";
        let (head, consumed) = match parse_request_head(request).unwrap() {
            ParseOutcome::Complete { head, consumed } => (head, consumed),
            ParseOutcome::Incomplete => unreachable!(),
        };
        let response = poll_to_completion(router.dispatch(&head, &request[consumed..]));
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{\"id\":7}".to_vec());
    }

    #[test]
    fn post_echo_returns_request_body() {
        let router = build_router();
        let body = b"{\"message\":\"hi\"}";
        let mut request = format!(
            "POST /echo HTTP/1.1\r\nHost: bench\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);
        let (head, consumed) = match parse_request_head(&request).unwrap() {
            ParseOutcome::Complete { head, consumed } => (head, consumed),
            ParseOutcome::Incomplete => unreachable!(),
        };
        let response = poll_to_completion(router.dispatch(&head, &request[consumed..]));
        assert_eq!(response.status, 200);
        assert_eq!(response.body, body.to_vec());
    }

    /// [`Report::to_json`] / [`Report::from_json`] の往復整合性を検証する
    /// （ベースライン読み書きのフォーマット退行検知）。
    #[test]
    fn report_json_roundtrip() {
        let mut scenarios = BTreeMap::new();
        for scenario in scenarios_for_test() {
            scenarios.insert(
                scenario,
                AllocStats {
                    allocations: 2,
                    bytes: 64,
                },
            );
        }
        let report = Report {
            rustc_version: "rustc 1.99.0 (test)".to_string(),
            scenarios,
        };
        let json = report.to_json();
        let restored = Report::from_json(&json).expect("roundtrip must succeed");
        assert_eq!(restored.rustc_version, report.rustc_version);
        assert_eq!(restored.scenarios, report.scenarios);
    }

    fn scenarios_for_test() -> Vec<&'static str> {
        scenarios().into_iter().map(|s| s.name).collect()
    }

    /// [`compare_with_baseline`] が alloc 回数増加を退行として検知することを
    /// 検証する（ラチェットの中核契約）。
    #[test]
    fn compare_detects_allocation_increase_as_regression() {
        let baseline = single_scenario_report(2, 64);
        let current = single_scenario_report(3, 64);
        let err = compare_with_baseline(&current, &baseline).expect_err("must be a regression");
        assert!(err.contains("regression"));
    }

    /// バイト数のみの増加も退行として検知することを検証する。
    #[test]
    fn compare_detects_byte_increase_as_regression() {
        let baseline = single_scenario_report(2, 64);
        let current = single_scenario_report(2, 96);
        let err = compare_with_baseline(&current, &baseline).expect_err("must be a regression");
        assert!(err.contains("regression"));
    }

    /// 指標が同一・改善（減少）のいずれであっても `Ok` を返すことを検証する
    /// （増加のみを退行として扱う片側ラチェット）。
    #[test]
    fn compare_allows_equal_and_improved_metrics() {
        let baseline = single_scenario_report(2, 64);
        assert!(compare_with_baseline(&baseline, &baseline).is_ok());

        let improved = single_scenario_report(1, 32);
        assert!(compare_with_baseline(&improved, &baseline).is_ok());
    }

    fn single_scenario_report(allocations: usize, bytes: usize) -> Report {
        let mut scenarios = BTreeMap::new();
        scenarios.insert("get_health", AllocStats { allocations, bytes });
        Report {
            rustc_version: "rustc 1.99.0 (test)".to_string(),
            scenarios,
        }
    }

    /// [`Report::from_json`] が、現在の [`scenarios`] に存在しないシナリオ名
    /// （＝コードから削除されたシナリオ）を含むベースライン JSON を無音に
    /// 読み飛ばさず `Err` にすることを検証する（イシュー #619 codex-review 指摘
    /// P1 対応の回帰テスト。旧実装は現在シナリオへ絞り込んで読み込むため、この
    /// ケースが `Ok` になった上で `compare_with_baseline` のキー集合比較も
    /// 誤って一致してしまい、シナリオ削除による計測対象の無音縮小を検知
    /// できなかった）。
    #[test]
    fn from_json_rejects_baseline_scenario_removed_from_code() {
        let json = serde_json::json!({
            "rustc_version": "rustc 1.99.0 (test)",
            "scenarios": {
                "get_health": { "allocations": 2, "bytes": 64 },
                "this_scenario_was_deleted": { "allocations": 1, "bytes": 8 },
            },
        });
        let err = Report::from_json(&json).expect_err("unknown baseline scenario must be Err");
        assert!(err.contains("this_scenario_was_deleted"));
    }
}
