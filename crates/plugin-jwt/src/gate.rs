//! `RequestGate` 実装 [`JwtGate`]（イシュー #816）と、検証結果の受け渡し型 [`GateVerified`]（#904）。
//!
//! コアの `Server::gate` に登録する。`check` は同期・I/O なし・`.await` なし
//! （`crates/core/src/extension.rs` の契約）。使う同期プリミティブは短時間保持の
//! `RwLock`（署名検証中は保持しない）・`AtomicU64`・`SystemTime::now()` だけ。
//! 検証は [`JwtAuthenticator`] に委ね、拒否は 401 の `GateOutcome::Reject` で返す
//! （応答の組み立ては [`JwtAuthenticator::unauthorized_response`]）。
//!
//! # 検証結果のハンドラへの受け渡し
//!
//! `check` は検証（DPoP を設定した認証器では proof の検証と `jti` の登録を含む）に成功すると、
//! 検証済みの `Claims` を [`GateVerified`] に包んで `RequestExtensions` へ入れる。入れた値は
//! 同じリクエストの `RequestHead` だけに載り、ハンドラは [`JwtGate::verified`] で読む。
//! ハンドラは検証を繰り返さない（DPoP では `jti` の 2 回目の登録が `Replayed` になるため
//! 繰り返せない）。[`GateVerified`] は外部から構築できないので、値が存在することが
//! 「このゲートがこのリクエストを受理した」ことの証明になる。ゲートを通っていない要求
//! （ゲート未登録・別経路）では値がなく、[`NotAdmitted`] で拒否される（フェイルクローズ）。
//! 共有状態（`jti` の履歴など）はゲート通過の証明に使わない。

use std::fmt;

use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestExtensions, RequestGate};
use fandhe_backend_http::error::error_response;
use fandhe_backend_http::request::RequestHead;
use serde::de::DeserializeOwned;

use crate::auth::JwtAuthenticator;
use crate::verify::{Claims, NoExtraClaims};

/// [`JwtGate`] が受理したリクエストに付く、検証済みクレームの入れ物。
///
/// 外部から構築できない（フィールドもコンストラクタも非公開）。`RequestExtensions` へ入れられるのは
/// [`JwtGate::check`](RequestGate::check) だけで、読み取りは [`JwtGate::verified`] を使う。
/// `Debug` はクレームの値を出さない。
pub struct GateVerified<E = NoExtraClaims> {
    claims: Claims<E>,
}

impl<E> GateVerified<E> {
    /// 検証済みのクレーム。
    pub fn claims(&self) -> &Claims<E> {
        &self.claims
    }
}

impl<E> fmt::Debug for GateVerified<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GateVerified").finish_non_exhaustive()
    }
}

/// このリクエストは [`JwtGate`] を通っていない（`RequestHead` に [`GateVerified`] がない）。
///
/// ゲートが登録されていない、またはゲートを通らない経路に届いた要求。ハンドラは認証失敗として
/// 扱い、クレームを推測や再検証で補わないこと（フェイルクローズ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotAdmitted;

impl fmt::Display for NotAdmitted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("この要求は JwtGate を通っていません")
    }
}

impl std::error::Error for NotAdmitted {}

/// `Authorization: Bearer` の JWT を検証する `RequestGate`。
///
/// DPoP（`dpop` feature）を設定した認証器では `Authorization: DPoP` も受ける。受理した要求の
/// クレームは [`GateVerified`] として `RequestHead` に載り、ハンドラは [`JwtGate::verified`] で読む。
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
/// let gate = JwtGate::new(auth);
/// assert_eq!(gate.name(), "jwt-gate");
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

    /// ゲートと共有する認証器（clone）を返す。
    pub fn authenticator(&self) -> JwtAuthenticator<E> {
        self.auth.clone()
    }

    /// ハンドラ用: このリクエストを受理したゲートが入れた検証済みクレームを返す。
    ///
    /// 検証は繰り返さない。ゲートを通っていない要求は [`NotAdmitted`]（フェイルクローズ）。
    /// `E` は登録したゲートの追加クレーム型と同じにすること（違うと値が見つからず `NotAdmitted`）。
    pub fn verified(head: &RequestHead) -> Result<&Claims<E>, NotAdmitted> {
        head.extensions()
            .get::<GateVerified<E>>()
            .map(GateVerified::claims)
            .ok_or(NotAdmitted)
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
        extensions: &mut RequestExtensions,
    ) -> GateOutcome {
        match self.auth.authenticate(head) {
            // 同じ `E` のゲートが二重に登録されているなど、既に値がある場合は上書きせず拒否する。
            Ok(claims) => match extensions.insert(GateVerified { claims }) {
                Ok(()) => GateOutcome::Allow,
                Err(_) => GateOutcome::Reject {
                    response: error_response(500, "internal_error"),
                },
            },
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
