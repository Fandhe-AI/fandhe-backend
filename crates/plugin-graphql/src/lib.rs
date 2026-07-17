//! `bf-plugin-graphql`: GraphQL プラグイン境界の最小実装（TASK-2.4 / #21）。
//!
//! # 位置づけ
//!
//! `docs/spec/05-tasks.md` TASK-2.4 は「少なくとも 2 種のプラグインを feature
//! flag で着脱でき、両方無効のコア性能が REQ-1 の性能基準を維持すること」を
//! 受け入れ基準とする。依存グラフ上 TASK-2.4 は TASK-5.1（実 GraphQL 実行、
//! `async-graphql` 統合）より前に位置し、実プロトコル実装は本タスクのスコープ
//! 外である（`docs/spec/06-roadmap.md` の依存グラフ: `TASK-2.1 → TASK-5.1`
//! は `TASK-2.1 → TASK-2.4` とは独立した系列）。本クレートは
//! `crates/plugin-webrtc-proxy`（TASK-2.1 / #18 で確立したパスインターセプト
//! 型プラグイン境界パターンの第 1 号）に続く**第 2 のプラグイン境界インスタンス**
//! として、`POST /graphql` への固定 JSON 応答のみを提供する。
//!
//! TASK-5.1（#38）が本クレートを実 GraphQL 実行エンジンへ拡張する際は、
//! [`try_handle_graphql`] のシグネチャ（パスインターセプト型、`Option` で
//! フォールスルーを表現）を維持したまま内部実装のみを差し替える想定
//! （`docs/design/plugin-boundary.md` 4 節のパターンを踏襲）。
//!
//! # なぜ WebSocket 側の第 2 インスタンスを作らないか
//!
//! `UpgradeHandler` シームを使う実 WebSocket プラグイン（`crates/plugin-websocket`、
//! TASK-4.1 / #22）は本タスク着手時点で別 PR（#137）として並行実装中であり、
//! 同名クレート・同一の `crates/core` 配線箇所（`server.rs`・`Cargo.toml`）を
//! 対象とするため、ここでスタブを重複実装すると衝突・二重実装になる
//! （`.claude/rules/out-of-scope-tracking.md`）。よって TASK-2.4 の「2 種の
//! プラグイン」は本クレート（GraphQL・パスインターセプト型）と既存の
//! `webrtc-proxy` feature（同じくパスインターセプト型、TASK-2.1 で配線済み）
//! の組み合わせで実証する。`docs/design/plugin-loading-tradeoffs.md` に
//! この判断の詳細根拠を記録する。
//!
//! # コアループへの配線について
//!
//! 本クレート単体では HTTP サーバのリスンループを持たない。`graphql` feature
//! （`optional = true` + `dep:` 構文、`.claude/rules/pay-for-what-you-use.md`）
//! 有効時のみ `backend_framework_core::plugin::try_intercept` から
//! [`try_handle_graphql`] が呼ばれる（`crates/core/src/plugin.rs` を参照）。
//! feature 無効時（既定）は本クレート自体が `backend-framework-core` の
//! 依存グラフから除外される。
//!
//! # workspace 内での依存方向
//!
//! `docs/spec/04-requirements.md` REQ-1 / `docs/spec/05-tasks.md` TASK-11.1 の
//! 方針に従い、依存方向は `server → routes → http::*` の一方向を維持する。
//! 本クレートはプラグイン層（`bf-plugin-*`）に位置し、workspace 内 path 依存は
//! `bf-http`（下位層の sans-IO パーサ）のみ。依存方向の機械検証は
//! `scripts/dep-direction-check.sh` が担う。
//!
//! # Examples
//!
//! 対象外パスは `None` を返し、無関係なリクエストへの性能影響がないことを示す。
//!
//! ```
//! use bf_http::request::{parse_request_head, ParseOutcome};
//! use bf_plugin_graphql::try_handle_graphql;
//!
//! let buf = b"GET /health HTTP/1.1\r\n\r\n";
//! let head = match parse_request_head(buf).unwrap() {
//!     ParseOutcome::Complete { head, .. } => head,
//!     ParseOutcome::Incomplete => unreachable!(),
//! };
//!
//! assert!(try_handle_graphql(&head).is_none());
//! ```

use bf_http::request::RequestHead;

/// 本プラグインがパスインターセプトの対象とするリクエストパス。
pub const GRAPHQL_PATH: &str = "/graphql";

/// [`try_handle_graphql`] が返す完結済み HTTP レスポンスの中間表現。
///
/// `crates/plugin-webrtc-proxy::handler::Response` と同型（ステータス・
/// `Content-Type`・body のみを保持する軽量な中間表現）。ソケットへの実書き込みは
/// 呼び出し元（コア接続ループ側）の責務とする。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP ステータスコード。
    pub status: u16,
    /// `Content-Type` ヘッダ値（`&'static str` 限定。
    /// `bf_http::response::Response::with_content_type` の制約に合わせる）。
    pub content_type: &'static str,
    /// レスポンス body。
    pub body: Vec<u8>,
}

/// `POST /graphql` をパスインターセプトし、固定 JSON 応答を返す。
///
/// - メソッド・パスが対象外なら `None` を返す（呼び出し元は次のハンドラへ
///   フォールスルーする契約。`crates/plugin-webrtc-proxy::try_handle_rtc_offer`
///   と同型）
/// - リクエスト body・ヘッダの内容は一切解釈しない。実 GraphQL クエリ実行は
///   TASK-5.1（#38）のスコープであり、本関数は「プラグイン境界が正しく機能する
///   こと」の実証に用途を限定する
/// - 応答 body は `&'static str` から生成した固定 JSON のみであり、リクエスト
///   由来の動的な値を一切埋め込まない（レスポンス分割・JSON インジェクション
///   対策、`.claude/rules/security.md`）
///
/// # Examples
///
/// ```
/// use bf_http::request::{parse_request_head, ParseOutcome};
/// use bf_plugin_graphql::try_handle_graphql;
///
/// let buf = b"POST /graphql HTTP/1.1\r\nContent-Length: 0\r\n\r\n";
/// let head = match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, .. } => head,
///     ParseOutcome::Incomplete => unreachable!(),
/// };
///
/// let response = try_handle_graphql(&head).expect("対象パスなので Some");
/// assert_eq!(response.status, 200);
/// assert_eq!(response.content_type, "application/json");
/// ```
pub fn try_handle_graphql(head: &RequestHead) -> Option<Response> {
    if head.method != "POST" || head.target != GRAPHQL_PATH {
        return None;
    }

    Some(Response {
        status: 200,
        content_type: "application/json",
        body: FIXED_BODY.as_bytes().to_vec(),
    })
}

/// 固定応答 body。実行結果を持たないダミー GraphQL レスポンス
/// （`{"data": null}`、GraphQL over HTTP の最小妥当な形）。
const FIXED_BODY: &str = "{\"data\":null}";

#[cfg(test)]
mod tests {
    use super::*;
    use bf_http::request::{ParseOutcome, parse_request_head};

    fn head(raw: &[u8]) -> RequestHead {
        match parse_request_head(raw).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => unreachable!(),
        }
    }

    #[test]
    fn matches_post_graphql_path() {
        let head = head(b"POST /graphql HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
        let response = try_handle_graphql(&head).expect("対象パスなので Some");
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        assert_eq!(response.body, FIXED_BODY.as_bytes());
    }

    #[test]
    fn falls_through_on_unrelated_path() {
        let head = head(b"GET /health HTTP/1.1\r\n\r\n");
        assert!(try_handle_graphql(&head).is_none());
    }

    #[test]
    fn falls_through_on_wrong_method() {
        // GET /graphql（GraphQL over HTTP の GET クエリ形式）は本スタブの
        // 対象外とする。実装は TASK-5.1（#38）で GET 対応を検討する。
        let head = head(b"GET /graphql HTTP/1.1\r\n\r\n");
        assert!(try_handle_graphql(&head).is_none());
    }
}
