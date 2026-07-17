//! `bf-plugin-hub-wiring`: hub 共通配線プラグイン（TASK-9.1 / #61）。
//!
//! 拡張点対応: `RequestGate`（[`gate::TenantGate`]）
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
//! `Server::gate(TenantGate::new(TenantGateConfig::new(secret)))`
//! （`crates/core/src/server.rs`）で登録する。
//!
//! # スパイクである旨（本番流用禁止）
//!
//! [`jwt`] モジュールの JWT 検証は HS256（HMAC-SHA256）の簡略実装であり、
//! `docs/spec/03-poc/hub-wiring-middleware` PoC-6 の再現スパイクである。
//! TASK-9.2 で RS256 + JWKS への差し替えを予定しており、複数サービス間で
//! 共有秘密鍵を配布する必要がある本番構成での長期利用は想定しない。
//!
//! # pay-for-what-you-use
//!
//! 本クレートを依存に追加しないサービスには、`hmac` / `sha2` / `base64` /
//! `serde` / `serde_json` を含む本クレートの依存・コード・バイナリ増が一切
//! 発生しない（`cargo tree -p backend-framework-core` に本クレート・本依存が
//! 現れないことで検証可能、.claude/rules/pay-for-what-you-use.md）。

pub mod gate;
pub mod jwt;

pub use gate::{TenantGate, TenantGateConfig};
pub use jwt::{Claims, TokenError, verify_token};
