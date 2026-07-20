//! エラーレスポンス共通化ヘルパ（イシュー #310）。
//!
//! 各ハンドラ（`crates/routes` の `RouteHandler` クロージャ・`crates/plugin-*`）が
//! 都度自前定義していた `Result<T, E> -> Response` 変換・JSON エラーボディ
//! （`{"error":"..."}` 形式）の定型記述を共通化する最小ヘルパ。
//!
//! # 設計方針（pay-for-what-you-use）
//!
//! [`IntoResponse`] trait は [`crate::response::Response`] を返すだけで直列化
//! 形式を規定しない。JSON への変換は [`error_response`] 関数側に分離し、
//! serde 等の直列化 crate には依存しない手実装エスケープで組み立てる
//! （`crates/http` は tokio の `io-util` 以外の実行時依存を持たない方針、
//! `crate` ルート doc を参照）。
//!
//! # セキュリティ設計（情報漏えい対策）
//!
//! [`HttpError::message`] は `&'static str` に限定する。[`response::Response::
//! with_content_type`] と同じ型レベル制約パターンであり、呼び出し元の
//! ソースコード上に静的に書かれた文字列リテラルしか渡せない。これにより、
//! 実行時エラー（DB エラー詳細・ファイルパス・スタックトレース等）の
//! `Display` / `Debug` 出力をそのままエラーボディへ流し込む経路が構造的に
//! 存在しない。**エラーボディはスタックトレース・内部情報を一切含まない
//! ことを既定とする**（`.claude/rules/security.md` の情報漏えい対策）。
//! 実行時の詳細情報を利用者に返したい場合は、呼び出し元が意図的に
//! `&'static str` の文言へ要約してから渡すこと。

use crate::response::Response;

/// `Response` への変換を表す最小 trait（axum の `IntoResponse` 相当の縮小版）。
///
/// ハンドラの戻り値型（`Response` そのもの・[`HttpError`]・両者の
/// `Result`）を境界で一様に `Response` へ変換するための共通契約。
/// `crates/routes` の `Router::route` 等が要求するハンドラ戻り値型は変更
/// しないため、クロージャ内で `(|| -> Result<Response, HttpError> { .. })()
/// .into_response()` のように境界で 1 回だけ呼び出す使い方を想定する。
///
/// # 例（`?` 演算子を使ったハンドラ、受け入れ条件 3）
///
/// `find_item` が返す `Result<Vec<u8>, HttpError>` を `?` で伝播させ、
/// ハンドラ自身も `Result<Response, HttpError>` を返す。呼び出し元は
/// 境界で 1 回だけ `.into_response()` を呼べば、成功・失敗いずれも
/// 適切な `Response`（エラー時は JSON エラーボディ標準形）になる。
///
/// ```
/// use fandhe_backend_http::error::{HttpError, IntoResponse};
/// use fandhe_backend_http::response::Response;
///
/// fn find_item(id: u64) -> Result<Vec<u8>, HttpError> {
///     if id == 1 {
///         Ok(b"{}".to_vec())
///     } else {
///         Err(HttpError::new(404, "item not found"))
///     }
/// }
///
/// fn handler(id: u64) -> Result<Response, HttpError> {
///     let body = find_item(id)?; // ? でエラーを伝播する
///     Ok(Response::new(200, body).with_content_type("application/json"))
/// }
///
/// // 成功系: ハンドラが組み立てた Response がそのまま出る。
/// let ok = handler(1).into_response();
/// assert_eq!(ok.status, 200);
///
/// // 失敗系: HttpError が境界で JSON エラーボディへ変換される。
/// let err = handler(2).into_response();
/// assert_eq!(err.status, 404);
/// assert_eq!(err.body, br#"{"error":"item not found"}"#);
/// ```
pub trait IntoResponse {
    /// `self` を [`Response`] へ変換する。
    fn into_response(self) -> Response;
}

impl IntoResponse for Response {
    /// 恒等変換。既に `Response` を構築済みのハンドラをそのまま
    /// `IntoResponse` 境界に載せるための実装。
    fn into_response(self) -> Response {
        self
    }
}

/// ステータスコードとユーザー提示メッセージからなる標準形エラー。
///
/// `message` を `&'static str` に限定する理由はモジュール冒頭の doc
/// （セキュリティ設計）を参照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpError {
    status: u16,
    message: &'static str,
}

impl HttpError {
    /// `status` と `message` から [`HttpError`] を組み立てる。
    ///
    /// ```
    /// use fandhe_backend_http::error::HttpError;
    ///
    /// let err = HttpError::new(404, "item not found");
    /// assert_eq!(err.status(), 404);
    /// assert_eq!(err.message(), "item not found");
    /// ```
    #[must_use]
    pub fn new(status: u16, message: &'static str) -> Self {
        Self { status, message }
    }

    /// ステータスコードを返す。
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// ユーザー提示メッセージを返す。
    #[must_use]
    pub fn message(&self) -> &'static str {
        self.message
    }
}

impl IntoResponse for HttpError {
    /// [`error_response`] へ委譲し、JSON エラーボディを持つ `Response` を返す。
    fn into_response(self) -> Response {
        error_response(self.status, self.message)
    }
}

impl std::fmt::Display for HttpError {
    /// `<status> <message>` 形式で表示する。`message` は `&'static str`
    /// 限定（モジュール冒頭 doc のセキュリティ設計）のため、実行時の
    /// スタックトレース・内部情報がここに混入することはない。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.message)
    }
}

impl std::error::Error for HttpError {}

/// `Result<T, E>` を境界で一様に `Response` 化する blanket impl。
///
/// `Ok` / `Err` いずれの型も [`IntoResponse`] を実装していればよく、
/// ハンドラ内部で `?` 演算子によりエラーを伝播させたあと、境界
/// （ハンドラの戻り値を最終的に `Response` として送出する箇所）で
/// 1 回だけ `.into_response()` を呼ぶ使い方を想定する。
impl<T: IntoResponse, E: IntoResponse> IntoResponse for Result<T, E> {
    fn into_response(self) -> Response {
        match self {
            Ok(ok) => ok.into_response(),
            Err(err) => err.into_response(),
        }
    }
}

/// JSON エラーボディ標準形 `{"error":"<message>"}` を持つ [`Response`] を
/// 組み立てるヘルパ関数。
///
/// serde 等の直列化 crate に依存せず、RFC 8259 準拠の手実装エスケープ
/// （`"` `\` と U+0000–U+001F の制御文字を `\uXXXX` 等でエスケープ）で
/// `message` を直列化するため、`message` にどのような文字が含まれていても
/// 常に妥当な JSON を出力する。`Content-Type` は
/// [`crate::response::Response::with_content_type`] を使って設定する
/// （このクレートの唯一の CRLF 混入対策済みヘッダ付与経路、
/// `.claude/rules/security.md`）。
///
/// ```
/// use fandhe_backend_http::error::error_response;
///
/// let res = error_response(404, "item not found");
/// assert_eq!(res.status, 404);
/// assert_eq!(res.body, br#"{"error":"item not found"}"#);
/// let text = String::from_utf8(res.serialize(true)).unwrap();
/// assert!(text.contains("Content-Type: application/json\r\n"));
/// ```
///
/// メッセージに `"` や制御文字が含まれる場合もエスケープされ、妥当な JSON
/// を維持する（JSON インジェクション対策、`.claude/rules/security.md`）。
///
/// ```
/// use fandhe_backend_http::error::error_response;
///
/// let res = error_response(400, "invalid \"field\"\nvalue");
/// assert_eq!(
///     res.body,
///     br#"{"error":"invalid \"field\"\nvalue"}"#
/// );
/// ```
#[must_use]
pub fn error_response(status: u16, message: &'static str) -> Response {
    let mut body = String::with_capacity(message.len() + 16);
    body.push_str(r#"{"error":""#);
    escape_json_string(message, &mut body);
    body.push_str(r#""}"#);
    Response::new(status, body.into_bytes()).with_content_type("application/json")
}

/// `input` を RFC 8259 §7 準拠の JSON 文字列エスケープで `out` へ追記する
/// 私有ヘルパ。`"` `\` は個別エスケープシーケンスへ、その他の U+0000–U+001F
/// 制御文字は `\uXXXX` へ変換する（`\n` `\r` `\t` 等の定義済み短縮形が
/// あるものはそちらを使う）。デリミタ自体（引用符）は呼び出し元
/// （[`error_response`]）が付与する。
fn escape_json_string(input: &str, out: &mut String) {
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_response_has_standard_json_body_and_status() {
        let res = error_response(404, "item not found");
        assert_eq!(res.status, 404);
        assert_eq!(res.body, br#"{"error":"item not found"}"#);
    }

    #[test]
    fn error_response_sets_json_content_type() {
        let res = error_response(500, "internal error");
        let text = String::from_utf8(res.serialize(true)).unwrap();
        assert!(text.contains("Content-Type: application/json\r\n"));
    }

    #[test]
    fn error_response_escapes_quote_and_backslash() {
        let res = error_response(400, r#"bad "field" \ value"#);
        assert_eq!(
            res.body,
            br#"{"error":"bad \"field\" \\ value"}"#.as_slice()
        );
    }

    #[test]
    fn error_response_escapes_newline_cr_tab() {
        // レスポンス分割・JSON 妥当性回帰: メッセージに生の改行/CR/タブが
        // 含まれても、ボディ内では常にエスケープ済みシーケンスとして出力され、
        // 生の CRLF がボディに混入しない。
        let res = error_response(400, "line1\nline2\rline3\ttab");
        assert_eq!(
            res.body,
            br#"{"error":"line1\nline2\rline3\ttab"}"#.as_slice()
        );
        let text = String::from_utf8(res.body.clone()).unwrap();
        assert!(!text.contains('\n'));
        assert!(!text.contains('\r'));
    }

    #[test]
    fn error_response_escapes_other_control_chars_as_unicode_escape() {
        let res = error_response(400, "null:\u{0}bell:\u{7}");
        assert_eq!(
            res.body,
            br#"{"error":"null:\u0000bell:\u0007"}"#.as_slice()
        );
    }

    #[test]
    fn http_error_implements_display_and_error() {
        // Bugbot 指摘対応（PR #332）: `HttpError` は `?` 演算での伝播を前提とした
        // Result エラー型としてドキュメント化されているにもかかわらず
        // `Display` / `std::error::Error` を欠いていた。他の公開エラー型
        // （`BodyError` 等）との一貫性を回帰させないためのテスト。
        let err = HttpError::new(404, "item not found");
        assert_eq!(err.to_string(), "404 item not found");
        let dyn_err: &dyn std::error::Error = &err;
        assert_eq!(dyn_err.to_string(), "404 item not found");
    }

    #[test]
    fn http_error_into_response_matches_error_response() {
        let err = HttpError::new(403, "forbidden");
        let res = err.into_response();
        assert_eq!(res.status, 403);
        assert_eq!(res.body, br#"{"error":"forbidden"}"#);
    }

    #[test]
    fn response_into_response_is_identity() {
        let res = Response::new(200, b"hi".to_vec());
        let converted = res.clone().into_response();
        assert_eq!(res, converted);
    }

    #[test]
    fn result_ok_into_response_uses_ok_conversion() {
        let result: Result<Response, HttpError> = Ok(Response::new(200, b"ok".to_vec()));
        let res = result.into_response();
        assert_eq!(res.status, 200);
        assert_eq!(res.body, b"ok");
    }

    #[test]
    fn result_err_into_response_uses_err_conversion() {
        let result: Result<Response, HttpError> = Err(HttpError::new(404, "not found"));
        let res = result.into_response();
        assert_eq!(res.status, 404);
        assert_eq!(res.body, br#"{"error":"not found"}"#);
    }
}
