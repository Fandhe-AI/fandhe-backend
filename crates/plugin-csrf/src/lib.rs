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
//! # 他のゲートとの併用（登録順）
//!
//! Cookie 認証の画面では、**CSRF ゲートを先に、認証ゲート（`JwtGate`・Cookie セッションを
//! 検証する利用者独自のゲート等）をその後に登録する**ことを推奨する。
//!
//! ```
//! use fandhe_backend_core::Server;
//! use fandhe_backend_core::extension::{GateContext, GateOutcome, RequestGate};
//! use fandhe_backend_http::request::RequestHead;
//! use fandhe_backend_plugin_csrf::CsrfGate;
//!
//! // 認証ゲートの代役（実際は `JwtGate` や利用者独自のゲート）。
//! struct Auth;
//! impl RequestGate for Auth {
//!     fn name(&self) -> &'static str {
//!         "auth"
//!     }
//!     fn check(&self, _head: &RequestHead, _ctx: &GateContext, _extensions: &mut fandhe_backend_core::extension::RequestExtensions) -> GateOutcome {
//!         GateOutcome::Allow
//!     }
//! }
//!
//! let csrf = CsrfGate::builder().allow_origin("https://app.example.com").build().unwrap();
//! let _server = Server::new()
//!     .gate(csrf) // 1. 送信元の検査（安価・副作用なし）
//!     .gate(Auth); // 2. 資格情報の検査
//! ```
//!
//! 根拠（挙動は `tests/gate.rs` の登録順テストで固定）:
//!
//! 1. **許可/拒否の結論は登録順に依らない**。複数ゲートは AND 結合（全ゲートが `Allow` の
//!    ときだけ通る）で、順序で変わるのは「どの拒否応答が返るか」と「後ろのゲートが評価
//!    されるか」だけ。順序の誤りで CSRF 防御が抜けることはない。
//! 2. **最初の拒否で短絡し、後ろのゲートの `check` は呼ばれない**。Cookie 認証では偽造
//!    された要求にも Cookie が自動で付く。CSRF を先にすれば、送信元の検査に落ちた要求が
//!    資格情報を評価するゲート（キャッシュ書き込み・通知・監査等の副作用を持ちうる）に
//!    届かない。
//! 3. **安価な検査を先に置く**。本ゲートはヘッダの 1 回走査のみ（ロック・確保・暗号処理なし）。
//!    状態を変えるメソッドでは、送信元の検査に落ちた要求に署名検証等のコストを払わせない。
//!    `GET`/`HEAD`/`OPTIONS` は本ゲートを素通りするためこの利点は及ばない。
//! 4. **偽造要求への応答が一様になる**。CSRF を先にすると、認証の状態に関係なく同じ
//!    「403・本文なし・理由なし」を返し、`WWW-Authenticate` 等を見せない。
//!
//! 注意:
//!
//! - ゲートはサーバ全体に効き、ルートごとに付け外しできない。
//! - ブラウザ以外のクライアント（OIDC の token endpoint、Bearer で呼ぶ API 等）は
//!   `Sec-Fetch-Site` も `Origin` も送らないため 403 になる。該当パスは
//!   [`CsrfGateBuilder::exempt_path`]（完全一致）で除外する。除外で飛ばされるのは CSRF の
//!   検査だけで、後ろの認証ゲートは評価される。
//! - `JwtGate` は `Authorization: Bearer` のみを見て Cookie は見ない。Cookie の画面と
//!   Bearer の API を 1 サーバに混在させて `JwtGate` を全体登録すると、Cookie の画面の要求も
//!   401 になる。パス単位の使い分けは現状コアにもゲートにも無い。
//!
//! 設計の詳細は `docs/design/plugin-boundary.md` 5.6.6 節を参照。

#![forbid(unsafe_code)]

mod config;
mod gate;

pub use config::{
    CsrfConfigError, CsrfGateBuilder, MAX_ALLOWED_ORIGINS, MAX_ENTRY_LEN, MAX_EXEMPT_PATHS,
};
pub use gate::CsrfGate;
