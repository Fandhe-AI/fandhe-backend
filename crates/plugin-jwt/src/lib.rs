//! `fandhe-backend-plugin-jwt`: 汎用 JWT 検証プラグイン（REQ-16、イシュー #812 系）。
//!
//! 拡張点対応: RequestGate
//!
//! # 依存方向（依存逆転型プラグイン）
//!
//! workspace 全体の依存方向は次の一方向を維持する（依存方向: server → routes → http::*）。
//! 本クレートは Gate 型パターン（`docs/design/plugin-boundary.md`）に従い
//! **プラグイン → コアの一方向依存**を取り、コア側に feature や再エクスポートを
//! 追加しない。利用側サービスが本クレートを依存に加えた場合のみコストが発生し、
//! 加えない構成では依存・コード・バイナリの増加はゼロ（pay-for-what-you-use）。
//!
//! # 実装状況
//!
//! 現状は検証ポリシー型 [`JwtPolicy`]、JWKS 鍵集合 [`KeySet`]
//! （RS256・ES256、[`SharedKeySet`] による差し替え）、トークン検証関数 [`verify()`]
//! （署名・`typ`・`iss`・`aud`・`exp`・`nbf`）、署名検証結果をキャッシュする
//! [`JwtAuthenticator`]、`RequestGate` 実装 [`JwtGate`]、未知 `kid` の通知口
//! [`UnknownKeyNotifier`]。設計は
//! `docs/design/jwt-verify-plugin.md` を参照。
//!
//! `jwks-refresh` feature（既定では無効）を有効にすると、ゲートの外で [`SharedKeySet`] を
//! 最新に保つ取り直しタスク（定期取得・未知 `kid` 契機・失敗時の現行鍵保持）が使える。
//! HTTP クライアントも TLS も持たず、取得関数（`JwksFetcher`）をアプリが渡す。
//! 取得関数の書き方は `docs/guide/jwks-refresh.md` を参照。
//!
//! `dpop` feature（既定では無効）を有効にすると、DPoP proof（RFC 9449）の解析と検査
//! （`jwk`・サムプリント・`ath`・`htm`・`htu`・`iat`）の純関数 `dpop::verify_proof` が使える。
//! 状態（`jti` の保持）もゲート配線も含まない sans-IO の部品で、新規依存を足さない。
//!
//! `iss`・`aud`・`typ`・許可 `alg` の欠けた構成は型の段階で作れない。`none`・HS 系は
//! [`Algorithm`] に変種がなく、型として表現できない（RFC 8725 3.1・3.8・3.9・3.11・3.12、
//! RFC 9068 4 節）。
//!
//! # Cargo feature
//!
//! - `dpop`: 新規依存なし。DPoP proof の純関数（`dpop` モジュール）だけを有効にする。
//! - `jwks-refresh`: optional の `tokio`（`time`・`sync`）のみを足す。無効時は依存も
//!   コードもバイナリ増もゼロ。タスクはアプリが `tokio::spawn` する（本クレートは spawn しない）。

// workspace lint の `unsafe_code = "warn"` と併記してよい。ソース側の forbid が
// lint レベルを引き上げ、unsafe を構造的に排除する。
#![forbid(unsafe_code)]

pub mod auth;
#[cfg(feature = "dpop")]
pub mod dpop;
pub mod gate;
pub mod jwks;
pub mod notify;
pub mod policy;
#[cfg(feature = "jwks-refresh")]
pub mod refresh;
pub mod verify;

pub use auth::{CACHE_CAPACITY, JwtAuthenticator, JwtAuthenticatorBuilder, bearer_token};
pub use gate::JwtGate;
pub use notify::{DEFAULT_UNKNOWN_KEY_MIN_INTERVAL, UnknownKeyEvent, UnknownKeyNotifier};

pub use jwks::{
    Jwk, JwksError, KeySet, MAX_JWKS_DOC_LEN, MAX_KEYS, MAX_KEYS_PER_KID, MAX_KID_LEN, SharedKeySet,
};

#[cfg(feature = "jwks-refresh")]
pub use refresh::{
    DEFAULT_FETCH_TIMEOUT, DEFAULT_REFRESH_MIN_INTERVAL, FetchError, FetchFuture, JwksFetcher,
    JwksRefresher, JwksResponse, MAX_FETCH_TIMEOUT, MAX_REFRESH_MIN_INTERVAL, MAX_REFRESH_PERIOD,
    MAX_STALE_LIMIT, RefreshConfig, RefreshConfigBuilder, RefreshConfigError, RefreshEvent,
    RefreshFailure, RefreshHandle, RefreshObserver, RefreshStatus, RefreshTask, RefreshTrigger,
};

pub use policy::{
    Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtPolicy, MAX_AUDIENCES, MAX_CLAIM_VALUE_LEN,
    MAX_CLOCK_SKEW, PolicyError, TokenType,
};

pub use verify::{Claims, MAX_TOKEN_AUDIENCES, MAX_TOKEN_LEN, NoExtraClaims, VerifyError, verify};
