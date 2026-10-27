//! `fandhe-backend-plugin-hub-wiring`: hub 共通配線プラグイン（TASK-9.1 / #61）。
//!
//! 拡張点対応: RequestGate
//!
//! 実装は [`gate::TenantGate`]。
//!
//! # 依存方向（依存逆転型プラグイン）
//!
//! workspace 全体の依存方向は次の一方向を維持する（依存方向: server → routes → http::*）。
//! 既存 4 プラグイン（websocket / graphql / openapi / webrtc 系、コア →
//! プラグインの optional 依存 + feature ゲート）とは異なり、本クレートは
//! **プラグイン → コアの一方向依存**（依存逆転型）を取る。`RequestGate` は
//! 同期 API の既存拡張点であり、TenantGate の判定はヘッダ検査 + 署名検証
//! のみで非同期処理を要しないため、既存プラグインが依存逆転を採れなかった
//! 理由（拡張点の同期 API 制約）に該当しない。`crates/core` の
//! `Cargo.toml`・`server.rs`・`plugin.rs` は本クレートのために一切変更しない
//! （`docs/design/plugin-boundary.md` 「Gate 型パターン」節）。
//!
//! 利用側サービスは本クレートを依存に加え、検証ポリシーを添えて
//! `Server::gate(TenantGate::new(TenantGateConfig::from_jwks_json(jwks_json, policy)?))`
//! （`crates/core/src/server.rs`）で登録する。
//!
//! # JWT 検証は `fandhe-backend-plugin-jwt` へ委譲（イシュー #820）
//!
//! 署名・`iss`・`aud`・`typ`・`exp`・`nbf` の検証、JWKS 鍵集合、検証結果の
//! キャッシュは汎用 JWT 検証クレート `fandhe-backend-plugin-jwt`（REQ-16）が担う。
//! 本クレートはそのクレート全体を [`jwt`] として再エクスポートする
//! （[`jwt::JwtPolicy`]・[`jwt::SharedKeySet`] など。依存は本クレート 1 つで足りる）。
//! 検証ポリシー（発行者・受信者・許可アルゴリズム・トークン種別）は
//! [`gate::TenantGateConfig`] の構築に**必須**で、ポリシーなしの構成は型として作れない。
//!
//! 本クレートに残すのは hub 固有の判定だけである。
//!
//! - `org_id` クレームの取り出しとテナント境界（[`auth::Authenticator`]、欠落・空は 403）
//! - Outbox・同意ゲート・越境アクセス監査（下記各節）
//!
//! JWKS の**取得**（HTTP フェッチ・自動リフレッシュ）は本クレートの責務外
//! （`RequestGate::check` は同期・I/O なしの契約、`crates/core/src/extension.rs`
//! doc）であり、利用側サービスが取得した JWKS JSON ドキュメントを注入する。
//! 再起動なしの鍵ローテーションは [`jwt::SharedKeySet::set`] が担う。
//!
//! # 検証結果のキャッシュ（TASK-9.3 / #63、実装は #816 から plugin-jwt）
//!
//! [`gate::TenantGate`] は `GateOutcome`（許可/拒否のみ）しかコアへ返せない
//! 契約上、検証で得た `org_id` を直接コアへ渡せない。[`auth::Authenticator`] を
//! [`gate::TenantGateConfig::authenticator`] で取り出し、`Server::gate` で `config` を
//! 消費する**前に** clone してハンドラ側へ持ち回ると、ゲート通過時点の検証で
//! キャッシュが温まり、ハンドラ内の [`auth::Authenticator::authenticate`] は署名検証を
//! 再実行しない。鍵ローテーション後は自動的に無効化され、`exp` は毎回再判定され、
//! 検証失敗はキャッシュされない。
//!
//! # Cargo feature
//!
//! - `dpop`（既定は無効、イシュー #905）: `fandhe-backend-plugin-jwt/dpop` を転送するだけで、
//!   新規依存はない。DPoP を設定した `JwtAuthenticator` を
//!   [`gate::TenantGateConfig::from_authenticator`] に渡したときだけ DPoP の検証と
//!   `WWW-Authenticate: DPoP` チャレンジが働き、設定しない構成の応答は変わらない。
//!   DPoP の認証器は `jti` を登録するため 1 要求につきゲートで 1 回だけ検証される。
//!   ハンドラは [`gate::TenantGate::verified`] で結果を読む。`jti` はプロセス内でのみ
//!   保持し、複数インスタンス間では共有しない（`docs/design/jwt-verify-plugin.md` 9.6・9.13 節）。
//!   DPoP を強制したい場合は必ず DPoP を設定すること（未設定の下流は結合トークンも
//!   Bearer として受理する）。設定例はリポジトリの `docs/guide/dpop.md` を参照。
//!
//! # pay-for-what-you-use
//!
//! 本クレートを依存に追加しないサービスには、`fandhe-backend-plugin-jwt`・`ring`・
//! `serde` 等を含む本クレートの依存・コード・バイナリ増が一切発生しない
//! （`cargo tree -p fandhe-backend-core` に本クレート・本依存が現れないことで
//! 検証可能、.claude/rules/pay-for-what-you-use.md）。plugin → plugin の依存は
//! `scripts/dep-direction-check.sh` に明示した唯一の例外である。
//!
//! # 越境アクセス監査ログ標準整合（TASK-9.6 / #89）
//!
//! [`audit`] モジュールは `docs/design/outbox-consent-integration.md` 6 節の
//! 2 層設計（データ層 RLS 相当の越境アクセスは 0 行 → 404 相当としてフェイル
//! クローズ遮断）を前提に、「正当な 404」と「越境 404」を**監査ログのみで**
//! 区別する仕組みを提供する。外部応答（404）はどちらも完全同一のまま変えない
//! （存在秘匿の維持）。`TenantGate`（本モジュール）の 401/403 判定とは独立の
//! 関心事であり、越境検出はデータ層の所有権判定を行う利用側サービスが
//! [`audit::TenantLookupOutcome`] を介して行う。新規依存の追加はない
//! （既存の `serde` / `serde_json` のみを使用）。実 micro-service-hub PoC-13
//! 標準とのフィールド厳密整合の最終確認は [#97] で行う。
//!
//! [#97]: https://github.com/Fandhe-AI/fandhe-backend/issues/97
//!
//! # Outbox・同意ゲート ストレージ抽象（TASK-9.4 / #64 §4.1、実装は #243）
//!
//! [`outbox`]（`OutboxStore`）・[`consent`]（`ConsentStore`）は
//! `docs/design/outbox-consent-integration.md` §4.1 が確定した trait 境界の
//! 実コード実装。コアはこれらの trait を一切知らない。[`gate::TenantGate`]
//! （`RequestGate` 拡張点）の判定通過**後**に、ハンドラ層（利用側サービスの
//! エンドポイント実装）から呼ばれる想定であり、`RequestGate::check` の同期
//! API 制約（`crates/core/src/extension.rs` doc）の対象外（同ドキュメント
//! §4.2）。テスト用インメモリ実装（[`outbox::InMemoryOutboxStore`]・
//! [`consent::InMemoryConsentStore`]）のみを本クレートに含み、PostgreSQL
//! 実装は利用側サービスまたは独立アダプタクレートが提供する
//! （`sqlx`/`tokio-postgres` 等の DB クライアントを本クレートに追加しない、
//! .claude/rules/pay-for-what-you-use.md）。

pub mod audit;
pub mod auth;
pub mod consent;
pub mod gate;
pub mod outbox;

/// 汎用 JWT 検証クレート `fandhe-backend-plugin-jwt` 全体の再エクスポート。
pub use fandhe_backend_plugin_jwt as jwt;

pub use audit::{
    AuditCategory, AuditContext, AuditEvent, AuditSink, MemoryAuditSink, TenantLookupOutcome,
};
pub use auth::{AuthError, Authenticator, HubClaims, TenantClaims};
pub use consent::{ConsentError, ConsentStore, InMemoryConsentStore};
pub use gate::{NotAdmitted, TenantGate, TenantGateConfig, TenantVerified};
pub use outbox::{InMemoryOutboxStore, OutboxError, OutboxEvent, OutboxStore};
