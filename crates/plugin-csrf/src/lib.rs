//! `fandhe-backend-plugin-csrf`: CSRF 検査ゲートプラグイン（REQ-17、イシュー #825）。
//!
//! 拡張点対応: RequestGate
//!
//! Cookie 認証の画面（素の `<form method="post">` を受けるハンドラ）を CSRF から守るため、
//! 状態を変える要求の送信元をハンドラ到達前に一律検査する [`CsrfGate`] を提供する。
//!
//! # 依存方向（依存逆転型プラグイン）
//!
//! workspace 全体の依存方向は次の一方向を維持する（依存方向: server → routes → http::*）。
//! 本クレートはプラグイン → コアの一方向依存を取り、コア側に feature や再エクスポートを
//! 追加しない。利用側が `Server::gate(...)` に登録した場合のみコストが発生し、
//! 登録しない構成の依存・コード・バイナリ増はゼロ（pay-for-what-you-use）。
//! 外部依存は無く `#![forbid(unsafe_code)]`。判定はヘッダの 1 回走査のみで、
//! I/O・ロック・`.await`・ヒープ確保を行わない（NFR-7）。
//!
//! # 判定規則
//!
//! 上から順に評価する。
//!
//! 1. `GET`・`HEAD`・`OPTIONS`（大文字小文字区別の完全一致）は無条件に許可。
//!    `get` や拡張メソッドは検査対象（安全側）。
//! 2. パスが除外パスと完全一致すれば許可（[`CsrfGateBuilder::exempt_path`]）。
//! 3. `Sec-Fetch-Site` が 2 件以上なら拒否。1 件なら値が `same-origin` / `none`
//!    （大文字小文字区別）のときのみ許可し、他（`same-site`・`cross-site`・空値・未知値）は
//!    拒否する。この場合 `Origin` は参照しない。
//! 4. `Sec-Fetch-Site` が無ければ、`Origin` が 1 件でバイト完全一致で許可リストに
//!    含まれるときのみ許可する。`Origin: null`・重複・不一致は拒否。
//! 5. どちらのヘッダも無ければ拒否（フェイルクローズ）。
//!
//! 拒否は常に同一の `403`・本文なし・ヘッダ値を反映しない応答で、理由を返さない。
//!
//! ## 設計判断
//!
//! - **`Sec-Fetch-*` の優先**: forbidden header name でブラウザ上のスクリプトから
//!   偽造できない。ブラウザ以外のクライアントは自由に付けられるが、被害者の Cookie を
//!   持たないため CSRF の脅威モデル外。`same-site` を拒否するのはサブドメイン乗っ取りで
//!   防御が崩れるのを避けるため。
//! - **除外パスは完全一致のみ**: 照合キーは `RequestHead::path()`（最初の `?` より前、
//!   % デコードも末尾スラッシュ正規化もしない）。接頭辞一致は、正規化しない契約の下で
//!   `/token` が `/tokenX` や `/token/../admin` に及ぶ等の範囲拡大を招きやすいため採らない。
//!   完全一致は後から非破壊に拡張できるが逆は破壊的変更になる。帰結として `/token?x=1` は
//!   除外されるが、`/token/`・`/tok%65n`・絶対形式の target は除外されず検査を受ける。
//! - **重複ヘッダは拒否**: 曖昧さはフェイルクローズに倒す。
//! - **Origin の比較**: `plugin-cors` と同じ正規化なしのバイト完全一致。構築時に
//!   ブラウザがシリアライズする形（小文字 scheme・小文字ホスト・既定ポート省略・
//!   パスなし）であることを検証し、決して一致しない設定ミスを構築エラーにする。
//!
//! # 使い方
//!
//! ```
//! use fandhe_backend_plugin_csrf::CsrfGate;
//!
//! let gate = CsrfGate::builder()
//!     .allow_origin("https://app.example.com")
//!     .exempt_path("/token")
//!     .build()
//!     .unwrap();
//! // `Server::new(...).gate(gate)` のように登録する。
//! # let _ = gate;
//! ```
//!
//! `Server::gate` で JWT ゲート等と併用する際の登録順は #826 で文書化する。

#![forbid(unsafe_code)]

mod config;
mod gate;

pub use config::{
    CsrfConfigError, CsrfGateBuilder, MAX_ALLOWED_ORIGINS, MAX_ENTRY_LEN, MAX_EXEMPT_PATHS,
};
pub use gate::CsrfGate;
