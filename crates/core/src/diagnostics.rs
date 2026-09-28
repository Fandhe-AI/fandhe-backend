//! ライブラリ内部の実行時診断を利用側で差し替え可能にする診断シンク
//! （イシュー #720）。
//!
//! # 背景
//!
//! `crates/core` は accept 失敗・TCP_NODELAY 設定失敗・graceful shutdown /
//! rebind の grace 超過強制クローズの 4 箇所で、従来 `eprintln!` により
//! 固定の日本語文言を直接 stderr へ出力していた。`fandhe-backend-core` は
//! ライブラリであり、利用側（CLI・ログ集約基盤等）が出力先・書式・抑止を
//! 制御できないのは pay-for-what-you-use・可観測性双方の観点で望ましくない
//! （`.claude/rules/security.md` の可観測性節）。
//!
//! # 採用方式（設計比較は `docs/design/diagnostics-sink.md` を参照）
//!
//! `tracing` / `log` 等の外部クレートへコアから直接依存する案ではなく、
//! 利用側が実装する [`Diagnostics`] trait の登録口（[`crate::server::Server::diagnostics`]）
//! を追加した。新規依存はゼロで、feature ゲートも不要
//! （[`crate::interceptor::Interceptor`] と同じ「外部依存ゼロの純コア機能」の
//! 位置づけ）。利用側が `tracing::warn!` 等へ転送する実装を書けば、事実上
//! `tracing` 連携も実現できる。
//!
//! # 契約
//!
//! [`Diagnostics::report`] は同期 API（dyn 互換のため、`crate::extension` の
//! 3 拡張点と同じ設計判断）。accept ループ・rebind の背景 drain タスク上で
//! 直接呼ばれるため、[`Server::diagnostics`][crate::server::Server::diagnostics]
//! で**利用者が登録するシンク実装**は以下を必ず守る:
//!
//! - **ブロッキング I/O を行わない**（`crate::extension::Middleware` と同じ
//!   規約。実装が I/O を必要とする場合は非同期チャネルへの送信に留め、実際の
//!   I/O は別タスクで行う）
//! - **panic しない**。コア側は [`std::panic::catch_unwind`] で境界を守るが
//!   （`emit` の doc を参照）、`panic = "abort"` ビルドでは捕捉できないため
//!   契約として明記する
//!
//! ## 既定シンクは上記の非ブロッキング契約の対象外（意図的な例外）
//!
//! 既定シンク [`StderrDiagnostics`] は現行の `eprintln!` 出力と完全互換
//! （文言・接頭辞・出力先が一致する）であることを最優先し、**同期 `eprintln!`
//! をそのまま使う**。上記「ブロッキング I/O を行わない」はカスタムシンクへの
//! 要求であり、既定シンクはこの契約の対象外という意図的な例外である
//! （`crates/plugin-tracing` のように毎リクエスト発火する
//! `Middleware`（PoC-3・PoC-10 実測で同期 I/O が RPS を著しく劣化させることが
//! 判明、`AGENTS.md`「規約: ミドルウェア非同期 I/O 必須化」参照）とは異なり、
//! [`DiagnosticEvent`] の 4 種はいずれも accept 失敗・grace 超過等の
//! **低頻度なエラー・シャットダウン経路限定のイベント**であり、per-request の
//! ホットパスではないため PoC-3/10 の性能劣化根拠はそのまま適用されない）。
//!
//! - **前提とする配置**: stderr が端末・ファイル、または受信側が生きている
//!   パイプであること（一般的な運用環境）
//! - **前提が崩れた場合の影響**: stderr が詰まる（受信側が読まない・壊れた
//!   パイプ等）と `eprintln!` がブロックしうる。影響範囲はイベントごとに
//!   異なる: `AcceptFailed`（`crates/core/src/server.rs` の主 accept
//!   ループ）はバックオフ前に呼ばれるため次回 accept 再試行が遅延する。
//!   `TcpNodelayFailed` は該当 1 接続の処理が遅延する（フェイルオープン方針は
//!   不変）。`ShutdownGraceExceeded` / `RebindDrainGraceExceeded` は**強制
//!   クローズの完了を確定させた後**に通知する順序（`docs/design/
//!   diagnostics-sink.md` 6 節）のため、詰まっても強制クローズ自体の完了は
//!   妨げられない。ただし通知そのものは届かないことがある
//!   （`ShutdownGraceExceeded` は通知の完了を最大 200ms だけ待って
//!   `run_until` から返るため、その後すぐプロセスが終了すると失われうる。
//!   `docs/design/diagnostics-sink.md` 9 節）
//! - **緩和策**: 上記の影響を許容できない場合は
//!   [`crate::server::Server::diagnostics`]
//!   でチャネル経由の非ブロッキングシンク（例: 有界チャネルへ `try_send` し、
//!   別スレッド/タスクが実際の書き込みを行う）を明示的に登録する。
//!   `tracing-appender` の non-blocking writer へ転送する実装も同様に有効
//! - **再検討トリガ**: 将来 `DiagnosticEvent` に per-request 相当の高頻度
//!   イベントが追加される場合、または実運用で stderr 詰まりによる停止が
//!   観測された場合は、既定シンクの非ブロッキング化（`docs/design/
//!   diagnostics-sink.md` 8 節の将来案）を再検討する
//!
//! [`DiagnosticEvent`] の [`Display`][fmt::Display] 実装が返す本文には接頭辞を
//! 含めない（接頭辞は既定シンクのみが付与する）。

use std::fmt;
use std::io;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

/// ライブラリ内部の実行時診断（accept 失敗・grace 超過強制クローズ等）を
/// 受け取るシンク。
///
/// [`crate::server::Server::diagnostics`] で登録する。未登録時の既定は
/// [`StderrDiagnostics`]（現行の `eprintln!` 出力と完全互換）。
///
/// # 契約（モジュール doc も参照）
///
/// - `report` はブロッキング I/O を行ってはならない（accept ループ・rebind
///   drain タスク上で同期的に呼ばれるため）。**この契約は
///   [`Server::diagnostics`][crate::server::Server::diagnostics] で利用者が
///   登録するシンク実装に対するもので、既定シンク [`StderrDiagnostics`] は
///   後方互換のため意図的に対象外**（モジュール doc「既定シンクは上記の
///   非ブロッキング契約の対象外」節を参照）
/// - `report` は panic してはならない（コア側は `catch_unwind` で境界を
///   守るが、フェイルクローズの保証にはしない）
///
/// # Examples
///
/// クロージャで登録し、有界チャネルへ非ブロッキングに転送する（PR #748
/// レビュー指摘 P2 対応。`report` 内で同期 `eprintln!` 等のブロッキング
/// I/O を直接行う例は上記「ブロッキング I/O を行わない」契約に反するため
/// 使わない。実際の I/O は受信側を持つ別スレッド/タスクへ委ねる）:
///
/// ```
/// use fandhe_backend_core::{Diagnostics, DiagnosticEvent};
/// use fandhe_backend_core::server::Server;
/// use std::sync::mpsc;
///
/// // 実運用では `_rx` を別スレッド/タスクで受信し、そこで初めて実際の
/// // I/O（ログ出力・`tracing::warn!` への転送等）を行う。
/// let (tx, _rx) = mpsc::sync_channel::<String>(1024);
/// let server = Server::new().diagnostics(move |event: &DiagnosticEvent<'_>| {
///     // `try_send` は満杯時に待機せず即座に失敗を返す（非ブロッキング）。
///     // 診断イベントは低頻度だが、取りこぼしを許容する前提で扱う。
///     let _ = tx.try_send(event.to_string());
/// });
/// let _ = server;
/// ```
///
/// 出力を抑止したい場合は no-op クロージャを登録する:
///
/// ```
/// use fandhe_backend_core::DiagnosticEvent;
/// use fandhe_backend_core::server::Server;
///
/// let server = Server::new().diagnostics(|_event: &DiagnosticEvent<'_>| {});
/// let _ = server;
/// ```
pub trait Diagnostics: Send + Sync + 'static {
    /// 1 件の診断イベントを受け取る。契約はトレイト doc を参照。
    fn report(&self, event: &DiagnosticEvent<'_>);
}

/// [`Diagnostics::report`] が受け取る診断イベント。
///
/// `crates/core/src/server.rs` の実行時診断 4 箇所に 1 対 1 で対応する。
/// 将来イベントを追加しても breaking change にしないため `#[non_exhaustive]`
/// とする。
///
/// # 機密情報の非混入（`.claude/rules/security.md`）
///
/// 運搬する値は [`io::Error`] と [`Duration`] のみに限定し、peer address・
/// リクエスト内容・ヘッダ等は含めない（現在も将来も含めない方針）。
#[derive(Debug)]
#[non_exhaustive]
pub enum DiagnosticEvent<'a> {
    /// `listener.accept()` が失敗した（`BoundServer::run_until` の主 accept
    /// ループ）。バックオフ後に再試行する。
    AcceptFailed {
        /// accept が返したエラー。
        error: &'a io::Error,
    },
    /// accept 直後のソケットへの TCP_NODELAY 設定が失敗した
    /// （`configure_accepted_stream`）。フェイルオープンで接続は継続する。
    TcpNodelayFailed {
        /// `configure_stream` が返したエラー。
        error: &'a io::Error,
    },
    /// 最終 graceful shutdown（`BoundServer::run_until`）で in-flight 完了待ちが
    /// `grace` を超過し、残存接続を強制クローズする。
    ShutdownGraceExceeded {
        /// `Server::shutdown_grace_period` で設定された猶予期間。
        grace: Duration,
    },
    /// rebind（`RebindHandle::rebind`）による旧世代接続の drain が `grace` を
    /// 超過し、残存接続を強制クローズする。
    RebindDrainGraceExceeded {
        /// `Server::shutdown_grace_period` で設定された猶予期間。
        grace: Duration,
    },
}

impl fmt::Display for DiagnosticEvent<'_> {
    /// 現行 `eprintln!` 引数から接頭辞（`fandhe_backend_core::server: `）を
    /// 除いた本文をそのまま出力する。既定シンク [`StderrDiagnostics`] のみが
    /// この接頭辞を付け足す（モジュール doc を参照）。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiagnosticEvent::AcceptFailed { error } => {
                write!(f, "accept に失敗しました: {error}")
            }
            DiagnosticEvent::TcpNodelayFailed { error } => {
                write!(
                    f,
                    "TCP_NODELAY の設定に失敗しました（接続は継続します）: {error}"
                )
            }
            DiagnosticEvent::ShutdownGraceExceeded { grace } => {
                write!(
                    f,
                    "graceful shutdown の猶予期間（{grace:?}）を超過したため残存接続を強制クローズします"
                )
            }
            DiagnosticEvent::RebindDrainGraceExceeded { grace } => {
                write!(
                    f,
                    "rebind による旧世代接続の drain が猶予期間（{grace:?}）を超過したため強制クローズします"
                )
            }
        }
    }
}

/// 既定の診断シンク。現行の `eprintln!` 出力と完全互換（文言・接頭辞・
/// 出力先が一致する）。
///
/// [`crate::server::Server::diagnostics`] を一度も呼ばない場合、`Server` は
/// 本シンクを使う。
///
/// **`report` は同期 `eprintln!` を実行し、[`Diagnostics`] trait の
/// 「ブロッキング I/O を行わない」契約の対象外**（モジュール doc「既定
/// シンクは上記の非ブロッキング契約の対象外」節に前提・影響・緩和策を記載）。
/// stderr の詰まりが許容できない環境では、独自シンク（有界チャネル +
/// 別タスクでの書き込み等）を [`crate::server::Server::diagnostics`] で
/// 登録すること。
///
/// # Examples
///
/// ```
/// use fandhe_backend_core::StderrDiagnostics;
/// use fandhe_backend_core::server::Server;
///
/// let server = Server::new().diagnostics(StderrDiagnostics);
/// let _ = server;
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct StderrDiagnostics;

impl StderrDiagnostics {
    /// 実際に stderr へ書き出す行を組み立てる（接頭辞付き）。単体テストで
    /// 現行文言との一致を検証しやすいよう整形処理のみを切り出している。
    fn format_line(event: &DiagnosticEvent<'_>) -> String {
        format!("fandhe_backend_core::server: {event}")
    }
}

impl Diagnostics for StderrDiagnostics {
    fn report(&self, event: &DiagnosticEvent<'_>) {
        eprintln!("{}", Self::format_line(event));
    }
}

// クロージャをそのまま `Diagnostics` として登録できるようにする便利 impl。
// `Server::new().diagnostics(|_event: &DiagnosticEvent<'_>| {})` で出力を
// 抑止できる（利用者は独自 struct を書かずに済む）。引数の型注釈は必須
// （`|_| {}` のみでは HRTB が絡み型推論に失敗しコンパイルが通らない）。
impl<F> Diagnostics for F
where
    F: Fn(&DiagnosticEvent<'_>) + Send + Sync + 'static,
{
    fn report(&self, event: &DiagnosticEvent<'_>) {
        self(event);
    }
}

/// `sink.report(event)` を panic 境界の内側で呼ぶ非公開ヘルパ。
///
/// 利用者が登録した [`Diagnostics`] 実装が panic しても、accept ループ
/// （`BoundServer::run_until`）や rebind の背景 drain タスク
/// （`spawn_generation_drain`）へ伝播させない（`.claude/rules/coding-rust.md`
/// 「panic はライブラリ境界を越えさせない」）。`panic = "abort"` ビルドでは
/// `catch_unwind` が捕捉できないため、この保護は `unwind` パニック戦略限定の
/// 多層防御であり、[`Diagnostics::report`] の「panic しない」契約を代替
/// しない。
pub(crate) fn emit(sink: &dyn Diagnostics, event: DiagnosticEvent<'_>) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| sink.report(&event)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn accept_failed_display_matches_legacy_wording() {
        let err = io::Error::other("boom");
        let event = DiagnosticEvent::AcceptFailed { error: &err };
        assert_eq!(event.to_string(), "accept に失敗しました: boom");
    }

    #[test]
    fn tcp_nodelay_failed_display_matches_legacy_wording() {
        let err = io::Error::other("nodelay boom");
        let event = DiagnosticEvent::TcpNodelayFailed { error: &err };
        assert_eq!(
            event.to_string(),
            "TCP_NODELAY の設定に失敗しました（接続は継続します）: nodelay boom"
        );
    }

    #[test]
    fn shutdown_grace_exceeded_display_matches_legacy_wording() {
        let event = DiagnosticEvent::ShutdownGraceExceeded {
            grace: Duration::from_millis(100),
        };
        assert_eq!(
            event.to_string(),
            "graceful shutdown の猶予期間（100ms）を超過したため残存接続を強制クローズします"
        );
    }

    #[test]
    fn rebind_drain_grace_exceeded_display_matches_legacy_wording() {
        let event = DiagnosticEvent::RebindDrainGraceExceeded {
            grace: Duration::from_millis(250),
        };
        assert_eq!(
            event.to_string(),
            "rebind による旧世代接続の drain が猶予期間（250ms）を超過したため強制クローズします"
        );
    }

    #[test]
    fn stderr_diagnostics_format_line_has_fixed_prefix() {
        let event = DiagnosticEvent::ShutdownGraceExceeded {
            grace: Duration::from_secs(1),
        };
        assert_eq!(
            StderrDiagnostics::format_line(&event),
            "fandhe_backend_core::server: graceful shutdown の猶予期間（1s）を超過したため残存接続を強制クローズします"
        );
    }

    #[test]
    fn closure_sink_receives_events() {
        let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let received_for_closure = Arc::clone(&received);
        let sink: Box<dyn Diagnostics> = Box::new(move |event: &DiagnosticEvent<'_>| {
            received_for_closure.lock().unwrap().push(event.to_string());
        });

        let err = io::Error::other("x");
        emit(&*sink, DiagnosticEvent::AcceptFailed { error: &err });

        assert_eq!(received.lock().unwrap().len(), 1);
        assert_eq!(received.lock().unwrap()[0], "accept に失敗しました: x");
    }

    #[test]
    fn emit_does_not_propagate_panicking_sink() {
        struct PanicSink;
        impl Diagnostics for PanicSink {
            fn report(&self, _event: &DiagnosticEvent<'_>) {
                panic!("sink panicked");
            }
        }

        let err = io::Error::other("y");
        // panic がここまで伝播しなければ成功（`catch_unwind` が境界を守っている）。
        emit(&PanicSink, DiagnosticEvent::AcceptFailed { error: &err });
    }
}
