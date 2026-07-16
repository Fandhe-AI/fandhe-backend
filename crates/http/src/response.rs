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
//! 任意ヘッダの追加が必要になった場合は、検証（CRLF 禁止等）付きの API を
//! 別途設計すること。

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
}

impl Response {
    /// `status` と `body` から [`Response`] を組み立てる。
    ///
    /// ```
    /// use bf_http::response::Response;
    ///
    /// let res = Response::new(200, b"ok".to_vec());
    /// assert_eq!(res.status, 200);
    /// ```
    #[must_use]
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self { status, body }
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

    /// HTTP/1.1 ワイヤフォーマットへ直列化する。
    ///
    /// `keep_alive` が `false` の場合のみ `Connection: close` を付与する
    /// （keep-alive が既定の HTTP/1.1 では省略するのが一般的であり、明示が
    /// 必要なのはクローズ時のみという方針。呼び出し元はコアループの
    /// `should_keep_alive` 判定結果をそのまま渡す契約）。
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
/// テーブルはコアループ（`crates/core/src/server.rs`）が実際に払い出す
/// ステータスコードに合わせて選定している。
fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
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
    fn serialize_ends_headers_with_blank_line_before_body() {
        let res = Response::new(201, b"body".to_vec());
        let bytes = res.serialize(true);
        let text = String::from_utf8(bytes).unwrap();
        let header_body_split = text.split_once("\r\n\r\n").expect("blank line separator");
        assert_eq!(header_body_split.1, "body");
    }
}
