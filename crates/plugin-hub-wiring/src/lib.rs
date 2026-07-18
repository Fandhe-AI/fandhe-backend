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
//! # JWT 検証結果のリクエストスコープキャッシュ（TASK-9.3 / #63）
//!
//! [`gate::TenantGate`] は `GateOutcome`（許可/拒否のみ）しかコアへ返せない
//! 契約上、検証で得た `org_id` 等のクレームを直接コアへ渡せない。従来は
//! ハンドラ側で再度クレームが必要な場合に [`jwt::verify_token`] を再呼び出し
//! するしかなく、1 リクエストにつき RS256 署名検証（RSA-2048）が 2 回
//! （ゲート + ハンドラ）走っていた。
//!
//! [`auth::Authenticator`] はこの重複を解消する、検証成功済みトークンの
//! キャッシュ（トークン文字列の SHA-256 ハッシュをキーとする。生トークンは
//! 保持しない）。[`gate::TenantGateConfig::authenticator`] で取得できる
//! `Authenticator` を、`Server::gate(TenantGate::new(config))` で `config` を
//! 消費する**前に** clone してハンドラ側へ持ち回ることで、ゲート通過時点の
//! 検証でキャッシュが温まり、ハンドラ内の [`auth::Authenticator::authenticate`]
//! 呼び出しは署名検証を再実行しない。
//!
//! キャッシュヒットを許すのは「再検証しても同じ結果になる」場合のみ:
//! 検証時点の鍵集合（`Arc<JwksKeySet>`）が現行 [`jwks::SharedJwks::snapshot`]
//! と一致（`Arc::ptr_eq`）していること（鍵ローテーション後は自動的に無効化）、
//! かつ `exp` がヒット時にも毎回再判定され期限内であること。検証**失敗**は
//! キャッシュしない（失敗の再検証コストは要求元に払わせ、無効トークンの
//! 大量投入によるキャッシュ汚染を防ぐ、.claude/rules/security.md）。
//!
//! # pay-for-what-you-use
//!
//! 本クレートを依存に追加しないサービスには、`ring` / `base64` / `serde` /
//! `serde_json` を含む本クレートの依存・コード・バイナリ増が一切発生しない
//! （`cargo tree -p backend-framework-core` に本クレート・本依存が現れないことで
//! 検証可能、.claude/rules/pay-for-what-you-use.md）。`ring` は
//! `crates/plugin-webrtc`（`webrtc` feature 経由）が既に依存グラフへ引き込んで
//! いる実績依存であり、本クレート追加による新規のライセンス・advisory 面の
//! リスク増はない。[`auth`] モジュールのキャッシュも `ring::digest`（既存の
//! `ring` 依存内）のみを用い、新規依存はゼロ。

pub mod auth;
pub mod gate;
pub mod jwks;
pub mod jwt;

pub use auth::Authenticator;
pub use gate::{TenantGate, TenantGateConfig};
pub use jwks::{JwksError, JwksKeySet, SharedJwks};
pub use jwt::{Claims, TokenError, verify_token};
