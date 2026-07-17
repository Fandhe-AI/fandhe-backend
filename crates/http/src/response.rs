//! HTTP/1.1 レスポンス直列化（TASK-1.4-2 / #70）。
//!
//! コアの接続ループ（`crates/core/src/server.rs`）が `RequestGate::Reject`・
//! ハンドラ結果・エラー応答を HTTP/1.1 ワイヤフォーマットへ直列化する際に使う
//! 唯一の経路。`backend_framework_core::extension::GateOutcome` の doc に
//! 明記されているとおり、ステータス行の組み立て（reason phrase 付与等）は
//! このモジュールの責務であり、コアループ自身は文字列組み立てを行わない。
//!
//! # セキュリティ設計（レスポンス分割対策）
//!
//! [`Response`] は任意のヘッダ名・値を外部から受け取る API を**意図的に持たない**。
//! ステータスコードは `u16`、reason phrase は本モジュールの固定テーブルから引き、
//! body は生バイト列として `Content-Length` 付きで送出する。これにより CRLF を
//! 含む文字列がヘッダとして書き出される経路が構造的に存在せず、レスポンス分割・
//! ヘッダインジェクションを型レベルで排除する（`.claude/rules/security.md`）。
//!
//! 唯一の例外が [`Response::with_content_type`] であり、値を `&'static str` に
//! 限定することで「呼び出し元（このクレート・上位クレートのソースコード）が
//! 静的に書いた文字列以外は絶対に渡せない」という型レベルの制約を維持したまま
//! `Content-Type` ヘッダの付与を可能にする（TASK-2.1 / #18、
//! `crates/plugin-webrtc-proxy` のようにレスポンス種別ごとに固定の
//! `Content-Type` を返すプラグインの配線で必要になった）。外部入力（リクエスト
//! ヘッダ・body 等）に由来する動的な値をヘッダとして送出する API は今後も
//! 追加しない方針を維持する。

/// 直列化対象の 1 レスポンス。
///
/// `status` は HTTP ステータスコード、`body` はレスポンスボディの生バイト列。
/// ヘッダは `Content-Length`（常時）と `Connection`（`serialize` の
/// `keep_alive` 引数に応じて）のみを自動付与し、それ以外のヘッダを持たない
/// 最小構成とする（本モジュールの doc を参照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP ステータスコード。
    pub status: u16,
    /// レスポンスボディの生バイト列。
    pub body: Vec<u8>,
    /// `Content-Type` ヘッダ値。`None` の場合はヘッダ自体を出力しない
    /// （TASK-1.4-2 / #70 時点の既定挙動を保つ）。[`Response::with_content_type`]
    /// の doc を参照。
    content_type: Option<&'static str>,
}

impl Response {
    /// `status` と `body` から [`Response`] を組み立てる。`Content-Type` は
    /// 未設定（ヘッダを出力しない）。
    ///
    /// ```
    /// use bf_http::response::Response;
    ///
    /// let res = Response::new(200, b"ok".to_vec());
    /// assert_eq!(res.status, 200);
    /// ```
    #[must_use]
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
            content_type: None,
        }
    }

    /// body なしの `status` レスポンスを組み立てる。
    ///
    /// ```
    /// use bf_http::response::Response;
    ///
    /// let res = Response::empty(404);
    /// assert!(res.body.is_empty());
    /// ```
    #[must_use]
    pub fn empty(status: u16) -> Self {
        Self::new(status, Vec::new())
    }

    /// `Content-Type` ヘッダ値を設定する。
    ///
    /// 値を `&'static str` に限定することで、外部入力（リクエストヘッダ・body
    /// 等）に由来する動的な文字列を渡す経路を型レベルで排除する（本モジュール
    /// 冒頭の doc・`.claude/rules/security.md` のレスポンス分割対策を参照）。
    /// 呼び出し元はソースコード上の文字列リテラルのみを渡せるため、値は常に
    /// このクレート・上位クレートの開発者が静的に書いたものに限られる。
    ///
    /// それでも CRLF を含む値が渡された場合（開発者の誤り）は、レスポンス
    /// 分割を未然に防ぐため `debug_assert!` でパニックさせ、デバッグビルドで
    /// 早期に検知する（リリースビルドでは呼び出し元が `&'static str` リテラル
    /// のみを渡す契約を信頼し、コストのかかる実行時チェックを省く）。
    ///
    /// ```
    /// use bf_http::response::Response;
    ///
    /// let res = Response::new(200, b"{}".to_vec()).with_content_type("application/json");
    /// let text = String::from_utf8(res.serialize(true)).unwrap();
    /// assert!(text.contains("Content-Type: application/json\r\n"));
    /// ```
    #[must_use]
    pub fn with_content_type(mut self, content_type: &'static str) -> Self {
        debug_assert!(
            !content_type.contains(['\r', '\n']),
            "Content-Type に CRLF を含む値を渡そうとした（レスポンス分割の危険、呼び出し元の実装ミス）"
        );
        self.content_type = Some(content_type);
        self
    }

    /// HTTP/1.1 ワイヤフォーマットへ直列化する。
    ///
    /// `keep_alive` が `false` の場合のみ `Connection: close` を付与する
    /// （keep-alive が既定の HTTP/1.1 では省略するのが一般的であり、明示が
    /// 必要なのはクローズ時のみという方針。呼び出し元はコアループの
    /// `should_keep_alive` 判定結果をそのまま渡す契約）。
    ///
    /// ステータスに関わらず常に `Content-Length` と body を出力する
    /// （ルーティング未実装の #70 時点では影響しない）。将来 `HEAD` メソッド
    /// 対応（`crates/routes`、TASK-1.5 以降）を追加する際は、`HEAD` 応答で
    /// body を省略しつつ `Content-Length` は `GET` 相当の値を保つ必要がある
    /// ため、本メソッドにメソッド情報を渡すか呼び出し元で body 省略を
    /// 制御する拡張が必要になる点に注意する。
    ///
    /// ```
    /// use bf_http::response::Response;
    ///
    /// let res = Response::new(200, b"hi".to_vec());
    /// let bytes = res.serialize(true);
    /// let text = String::from_utf8(bytes).unwrap();
    /// assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
    /// assert!(text.contains("Content-Length: 2\r\n"));
    /// assert!(text.ends_with("\r\n\r\nhi"));
    /// assert!(!text.contains("Connection: close"));
    /// ```
    ///
    /// ```
    /// use bf_http::response::Response;
    ///
    /// let res = Response::empty(400);
    /// let bytes = res.serialize(false);
    /// let text = String::from_utf8(bytes).unwrap();
    /// assert!(text.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    /// assert!(text.contains("Connection: close\r\n"));
    /// ```
    #[must_use]
    pub fn serialize(&self, keep_alive: bool) -> Vec<u8> {
        let reason = reason_phrase(self.status);
        let mut out = Vec::with_capacity(64 + self.body.len());
        out.extend_from_slice(b"HTTP/1.1 ");
        out.extend_from_slice(self.status.to_string().as_bytes());
        out.push(b' ');
        out.extend_from_slice(reason.as_bytes());
        out.extend_from_slice(b"\r\n");
        if let Some(content_type) = self.content_type {
            out.extend_from_slice(b"Content-Type: ");
            out.extend_from_slice(content_type.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"Content-Length: ");
        out.extend_from_slice(self.body.len().to_string().as_bytes());
        out.extend_from_slice(b"\r\n");
        if !keep_alive {
            out.extend_from_slice(b"Connection: close\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

/// 既知ステータスコードの reason phrase を返す固定テーブル。
///
/// 未知のコードは空文字列を返す（`HTTP/1.1 <code> \r\n` のように reason
/// phrase 省略として出力される。RFC 7230 上 reason phrase は省略可能）。
/// テーブルはコアループ（`crates/core/src/server.rs`）・`crates/routes`
/// （`bf_routes::Router::dispatch`、TASK-1.5 / #14 でメソッド不一致時に 405 を
/// 払い出す）・`crates/plugin-webrtc-proxy`（TASK-2.1 / #18 の配線経由で
/// 502/504 を払い出す。上流中継失敗時のフォールバックステータス）・
/// `crates/plugin-webrtc`（TASK-8.1 / #26 の `try_handle_rtc_offer` が同時接続数
/// 上限到達時に 503 を払い出す）が実際に払い出すステータスコードに合わせて
/// 選定している。
fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_includes_status_and_reason() {
        let res = Response::empty(200);
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[test]
    fn serialize_unknown_status_has_empty_reason() {
        let res = Response::empty(999);
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.starts_with("HTTP/1.1 999 \r\n"));
    }

    #[test]
    fn serialize_content_length_matches_body() {
        let res = Response::new(200, b"hello".to_vec());
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains("Content-Length: 5\r\n"));
        assert!(text.ends_with("hello"));
    }

    #[test]
    fn serialize_close_adds_connection_close_header() {
        let res = Response::empty(200);
        let text = String::from_utf8(res.serialize(false)).unwrap();
        assert!(text.contains("Connection: close\r\n"));
    }

    #[test]
    fn serialize_keep_alive_omits_connection_header() {
        let res = Response::empty(200);
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(!text.contains("Connection:"));
    }

    #[test]
    fn serialize_omits_content_type_by_default() {
        let res = Response::new(200, b"hi".to_vec());
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(!text.contains("Content-Type:"));
    }

    #[test]
    fn serialize_includes_content_type_when_set() {
        let res = Response::new(200, b"{}".to_vec()).with_content_type("application/json");
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains("Content-Type: application/json\r\n"));
    }

    #[test]
    fn serialize_bad_gateway_and_gateway_timeout_have_reason_phrase() {
        // TASK-2.1 / #18: crates/plugin-webrtc-proxy が上流中継失敗時に払い出す
        // 502/504 が空 reason phrase に劣化しないことを確認する（PoC-9 教訓）。
        let bad_gateway = String::from_utf8(Response::empty(502).serialize(false)).unwrap();
        assert!(bad_gateway.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));

        let gateway_timeout = String::from_utf8(Response::empty(504).serialize(false)).unwrap();
        assert!(gateway_timeout.starts_with("HTTP/1.1 504 Gateway Timeout\r\n"));
    }

    #[test]
    fn serialize_service_unavailable_has_reason_phrase() {
        // TASK-8.1 / #26: crates/plugin-webrtc が同時接続数上限到達時に払い出す
        // 503 が空 reason phrase に劣化しないことを確認する（PR #138 Bugbot 指摘）。
        let service_unavailable = String::from_utf8(Response::empty(503).serialize(false)).unwrap();
        assert!(service_unavailable.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
    }

    #[test]
    fn serialize_ends_headers_with_blank_line_before_body() {
        let res = Response::new(201, b"body".to_vec());
        let bytes = res.serialize(true);
        let text = String::from_utf8(bytes).unwrap();
        let header_body_split = text.split_once("\r\n\r\n").expect("blank line separator");
        assert_eq!(header_body_split.1, "body");
    }
}
