//! `RequestGate` 実装 [`JwtGate`]（イシュー #816）。
//!
//! コアの `Server::gate` に登録する。`check` は同期・I/O なし・`.await` なし
//! （`crates/core/src/extension.rs` の契約）。使う同期プリミティブは短時間保持の
//! `RwLock`（署名検証中は保持しない）・`AtomicU64`・`SystemTime::now()` だけ。
//! 検証は [`JwtAuthenticator`] に委ね、拒否は 401 の `GateOutcome::Reject` で返す
//! （応答の組み立ては [`JwtAuthenticator::unauthorized_response`]）。
//! クレームは `GateOutcome` に載せられないため、ハンドラは
//! [`JwtGate::authenticator`] で取り出した共有ハンドルから得る。DPoP（`dpop` feature）を
//! 設定した認証器では、ゲートが `jti` を登録するため、ハンドラは `authenticate` ではなく
//! [`authenticate_after_gate`](JwtAuthenticator::authenticate_after_gate) を呼ぶ。

use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
use fandhe_backend_http::request::RequestHead;
use serde::de::DeserializeOwned;

use crate::auth::JwtAuthenticator;
use crate::verify::NoExtraClaims;

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
            Err(e) => GateOutcome::Reject {
                response: self.auth.unauthorized_response(head, &e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwks::{KeySet, SharedKeySet};
    use crate::policy::{Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtPolicy, TokenType};
    use crate::verify::VerifyError;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    fn auth() -> JwtAuthenticator {
        let policy = JwtPolicy::new(
            Issuer::new("https://issuer.example").unwrap(),
            Audiences::single("api").unwrap(),
            AllowedAlgorithms::only(Algorithm::Es256),
            TokenType::AccessToken,
        );
        JwtAuthenticator::builder(SharedKeySet::new(KeySet::empty()), policy).build()
    }

    fn assert_full(err: &VerifyError, challenge: &str) {
        let ParseOutcome::Complete { head, .. } =
            parse_request_head(b"GET / HTTP/1.1\r\n\r\n").unwrap()
        else {
            panic!()
        };
        let s =
            String::from_utf8(auth().unauthorized_response(&head, err).serialize(true)).unwrap();
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
        assert_full(&VerifyError::MissingToken, "Bearer");
        assert_full(&VerifyError::Expired, "Bearer error=\"invalid_token\"");
    }
}
