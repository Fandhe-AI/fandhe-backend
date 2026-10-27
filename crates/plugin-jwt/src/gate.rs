//! `RequestGate` 実装 [`JwtGate`]（イシュー #816）。
//!
//! コアの `Server::gate` に登録する。`check` は同期・I/O なし・`.await` なし
//! （`crates/core/src/extension.rs` の契約）。使う同期プリミティブは短時間保持の
//! `RwLock`（署名検証中は保持しない）・`AtomicU64`・`SystemTime::now()` だけ。
//! 検証は [`JwtAuthenticator`] に委ね、拒否は 401 の `GateOutcome::Reject` で返す。
//! クレームは `GateOutcome` に載せられないため、ハンドラは
//! [`JwtGate::authenticator`] で取り出した共有ハンドルから得る。

use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use serde::de::DeserializeOwned;

use crate::auth::JwtAuthenticator;
use crate::verify::{NoExtraClaims, VerifyError};

/// 401 の固定本文。トークン・`kid`・失敗理由を反映しない。
const UNAUTHORIZED_BODY: &[u8] = br#"{"error":"invalid_token"}"#;
/// トークンが無い場合（RFC 6750 3.1: 認証情報なしの要求には `error` を付けない）。
const CHALLENGE_NO_TOKEN: &str = "Bearer";
/// トークンはあるが検証に失敗した場合。`error_description` は付けない。
const CHALLENGE_INVALID: &str = "Bearer error=\"invalid_token\"";

/// `Authorization: Bearer` の JWT を検証する `RequestGate`。
///
/// ```
/// use fandhe_backend_core::extension::RequestGate;
/// use fandhe_backend_plugin_jwt::*;
/// let policy = JwtPolicy::new(
///     Issuer::new("https://issuer.example")?,
///     Audiences::single("api")?,
///     AllowedAlgorithms::only(Algorithm::Es256),
///     TokenType::AccessToken,
/// );
/// let auth = JwtAuthenticator::<NoExtraClaims>::builder(SharedKeySet::new(KeySet::empty()), policy)
///     .build();
/// let handler_side = auth.clone(); // ハンドラ用。Server::gate へ渡す前に取り出す
/// let gate = JwtGate::new(auth);
/// assert_eq!(gate.name(), "jwt-gate");
/// # let _ = handler_side;
/// # Ok::<(), PolicyError>(())
/// ```
pub struct JwtGate<E = NoExtraClaims> {
    auth: JwtAuthenticator<E>,
}

impl<E: DeserializeOwned + Clone + Send + Sync + 'static> JwtGate<E> {
    /// 認証器からゲートを作る。
    pub fn new(auth: JwtAuthenticator<E>) -> Self {
        Self { auth }
    }

    /// ハンドラと共有する認証器（clone）を返す。
    pub fn authenticator(&self) -> JwtAuthenticator<E> {
        self.auth.clone()
    }
}

/// 401 拒否応答。ヘッダ付与に失敗した場合もヘッダ無しの 401 に倒し、`Allow` にはしない。
fn unauthorized(challenge: &str) -> GateOutcome {
    let base = Response::new(401, UNAUTHORIZED_BODY.to_vec()).with_content_type("application/json");
    let response = match base.clone().with_header("WWW-Authenticate", challenge) {
        Ok(r) => r,
        Err(_) => base,
    };
    GateOutcome::Reject { response }
}

impl<E: DeserializeOwned + Clone + Send + Sync + 'static> RequestGate for JwtGate<E> {
    fn name(&self) -> &'static str {
        "jwt-gate"
    }

    fn check(
        &self,
        head: &RequestHead,
        _ctx: &GateContext,
        _extensions: &mut fandhe_backend_core::extension::RequestExtensions,
    ) -> GateOutcome {
        match self.auth.authenticate(head) {
            Ok(_) => GateOutcome::Allow,
            Err(VerifyError::MissingToken) => unauthorized(CHALLENGE_NO_TOKEN),
            Err(_) => unauthorized(CHALLENGE_INVALID),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(o: GateOutcome) -> String {
        match o {
            GateOutcome::Reject { response } => {
                String::from_utf8(response.serialize(true)).unwrap()
            }
            _ => panic!("reject expected"),
        }
    }

    fn assert_full(o: GateOutcome, challenge: &str) {
        let s = text(o);
        let (head, body) = s.split_once("\r\n\r\n").unwrap();
        let lines: Vec<&str> = head.split("\r\n").collect();
        assert_eq!(lines[0], "HTTP/1.1 401 Unauthorized", "{s}");
        let auth: Vec<&&str> = lines[1..]
            .iter()
            .filter(|l| l.to_ascii_lowercase().starts_with("www-authenticate:"))
            .collect();
        assert_eq!(auth.len(), 1, "{s}");
        assert_eq!(*auth[0], format!("WWW-Authenticate: {challenge}"), "{s}");
        assert!(lines.contains(&"Content-Type: application/json"), "{s}");
        assert_eq!(body, r#"{"error":"invalid_token"}"#, "{s}");
        let cl = format!("Content-Length: {}", body.len());
        assert!(lines.contains(&cl.as_str()), "{s}");
    }

    #[test]
    fn challenge_headers_are_attached() {
        assert_full(unauthorized(CHALLENGE_NO_TOKEN), "Bearer");
        assert_full(
            unauthorized(CHALLENGE_INVALID),
            "Bearer error=\"invalid_token\"",
        );
    }
}
