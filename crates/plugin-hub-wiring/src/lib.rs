//! `bf-plugin-hub-wiring`: hub 共通配線プラグイン（TASK-9.1 / #61）。
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
//! 同期 API の既存拡張点であり、TenantGate の判定はヘッダ検査 + HMAC 検証
//! のみで非同期処理を要しないため、既存プラグインが依存逆転を採れなかった
//! 理由（拡張点の同期 API 制約）に該当しない。`crates/core` の
//! `Cargo.toml`・`server.rs`・`plugin.rs` は本クレートのために一切変更しない
//! （`docs/design/plugin-boundary.md` 「Gate 型パターン」節）。
//!
//! 利用側サービスは本クレートを依存に加え、
//! `Server::gate(TenantGate::new(TenantGateConfig::from_jwks_json(jwks_json)?))`
//! （`crates/core/src/server.rs`）で登録する。
//!
//! # RS256 + JWKS（TASK-9.2 / #62）
//!
//! [`jwt`] モジュールの JWT 検証は TASK-9.1（#61）の HS256（HMAC-SHA256）
//! 共有秘密鍵スパイクから、RS256（非対称鍵）+ JWKS（[`jwks`] モジュール、
//! [RFC 7517]）連携へ差し替え済み。HMAC 実装は本番実装に流用せず削除した
//! （`docs/spec/05-tasks.md` TASK-9.2）。
//!
//! JWKS の**取得**（HTTP フェッチ・自動リフレッシュ）は本クレートの責務外
//! （`RequestGate::check` は同期・I/O なしの契約、`crates/core/src/extension.rs`
//! doc）であり、利用側サービスが取得した JWKS JSON ドキュメントを注入する。
//! 再起動なしの鍵ローテーションは [`jwks::SharedJwks::set`] が担う。JWKS
//! 自動リフレッシュヘルパー（HTTP クライアント連携）・実 hub エンドポイントとの
//! E2E 結線は本タスクのスコープ外（追加の HTTP クライアント依存を要するため。
//! micro-service-hub 側の JWKS エンドポイントは roadmap 上 MS-1 完了目標
//! 2026-07-30 で本タスク時点では未提供）。
//!
//! [RFC 7517]: https://www.rfc-editor.org/rfc/rfc7517
//!
//! # pay-for-what-you-use
//!
//! 本クレートを依存に追加しないサービスには、`ring` / `base64` / `serde` /
//! `serde_json` を含む本クレートの依存・コード・バイナリ増が一切発生しない
//! （`cargo tree -p backend-framework-core` に本クレート・本依存が現れないことで
//! 検証可能、.claude/rules/pay-for-what-you-use.md）。`ring` は
//! `crates/plugin-webrtc`（`webrtc` feature 経由）が既に依存グラフへ引き込んで
//! いる実績依存であり、本クレート追加による新規のライセンス・advisory 面の
//! リスク増はない。
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
//! [#97]: https://github.com/Fandhe-AI/backend-framework/issues/97

pub mod audit;
pub mod gate;
pub mod jwks;
pub mod jwt;

pub use audit::{
    AuditCategory, AuditContext, AuditEvent, AuditSink, MemoryAuditSink, TenantLookupOutcome,
};
pub use gate::{TenantGate, TenantGateConfig};
pub use jwks::{JwksError, JwksKeySet, SharedJwks};
pub use jwt::{Claims, TokenError, verify_token};
