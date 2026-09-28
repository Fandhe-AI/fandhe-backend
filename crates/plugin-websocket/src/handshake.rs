//! RFC 6455 4.2.1 ハンドシェイク検証・101/400/426 応答の組み立て。
//!
//! `crate::matches` / `crate::handle_upgrade` から呼ばれる純関数群。検証は
//! 許可リスト方式・フェイルクローズとし（`.claude/rules/security.md`）、応答
//! バイト列は固定テンプレート + 導出値（`Sec-WebSocket-Accept`）のみから組み
//! 立てる。外部入力（`Sec-WebSocket-Key` 等）を応答ヘッダへ一切エコーしない
//! ことで、レスポンス分割・ヘッダインジェクション経路を構造的に排除する。

use std::fmt;
use std::net::SocketAddr;

use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

use crate::config::WebSocketConfig;
use crate::error::WsError;
use crate::pattern::PathParams;

/// `head` の request-target が `config` の指すパス（完全一致 or 登録済み
/// パターン）に該当するかを判定し、該当する場合は抽出済みパスパラメータを
/// 返す。
///
/// `matches`（真偽値のみを返す委譲判定）と [`crate::handle_upgrade`]（抽出
/// パラメータを `WsOpenContext` へ渡す、イシュー #676）の両方から呼ばれる
/// 共有ヘルパー。クエリ除去・完全一致/パターン照合の判定ロジックを二重化
/// せず 1 箇所に集約する。
///
/// `config.pattern` が `None`（完全一致パス）の場合、一致しても抽出対象の
/// パラメータは存在しないため空の [`PathParams`] を返す（`is_some()` で
/// 「一致」判定はできるが `len() == 0`）。
#[must_use]
pub(crate) fn match_config_path<'a>(
    head: &'a RequestHead,
    config: &WebSocketConfig,
) -> Option<PathParams<'a>> {
    // `RequestHead::target` はクエリ文字列を含む完全な request-target
    // （例: `/ws?token=...`）。`config.path` はクエリを含まないパス成分の
    // みを表すため、比較前に `?` 以降を切り落として path 成分だけを見る。
    let target = head.target();
    let path = target.split('?').next().unwrap_or(target);
    match &config.pattern {
        Some(pattern) => pattern.match_path(path),
        None => (path == config.path).then(PathParams::default),
    }
}

/// リクエストが `config` の指すアップグレード対象（パス + メソッド +
/// `Upgrade: websocket`）に該当するかを判定する。
///
/// コア側 `UpgradeHandler` アダプタ（`crates/core/src/server.rs`）の
/// `matches` 実装から呼ばれる（`UpgradeHandler::matches` は「委譲判定のみ」の
/// 契約であり、詳細なハンドシェイク検証は行わない。詳細検証は委譲確定後の
/// [`validate`] が担う）。パス判定自体は [`match_config_path`] へ委譲する
/// （ロジックの二重化を避ける。抽出したパラメータは本関数では破棄し
/// `bool` のみを返す契約は不変、`UpgradeHandler::matches` が同期 bool API
/// のため）。
///
/// `config` に [`WebSocketConfig::with_path_pattern`] で登録済みのパターンが
/// あれば、完全一致判定より優先してパターン照合を使う（イシュー #675）。
#[must_use]
pub fn matches(head: &RequestHead, config: &WebSocketConfig) -> bool {
    match_config_path(head, config).is_some()
        && head.method() == "GET"
        && head
            .header("upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// 検証済みハンドシェイク（`Sec-WebSocket-Accept` 導出済み）。
pub(crate) struct ValidatedHandshake {
    pub(crate) accept_key: String,
}

/// RFC 6455 4.2.1 の要件を検証し、`Sec-WebSocket-Accept` を導出する。
///
/// 検証項目（許可リスト方式・フェイルクローズ）:
/// - `GET` + `HTTP/1.1`
/// - `Upgrade: websocket`（大小無視）
/// - `Connection` ヘッダのカンマ区切りトークンに `upgrade` を含む（大小無視）
/// - `Sec-WebSocket-Version: 13`（違反時は [`WsError::UnsupportedVersion`]。
///   呼び出し元が `426 Upgrade Required` を返す）
/// - `Sec-WebSocket-Key` が存在し、base64 文字集合・24 文字であること
///
/// 上記いずれかに違反した場合は [`WsError::InvalidHandshake`] /
/// [`WsError::UnsupportedVersion`] を返し、呼び出し元が接続を閉じる。
pub(crate) fn validate(head: &RequestHead) -> Result<ValidatedHandshake, WsError> {
    if head.method() != "GET" {
        return Err(WsError::InvalidHandshake("method must be GET"));
    }
    if head.version != fandhe_backend_http::request::HttpVersion::Http11 {
        return Err(WsError::InvalidHandshake("version must be HTTP/1.1"));
    }
    if !head
        .header("upgrade")
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
    {
        return Err(WsError::InvalidHandshake("missing Upgrade: websocket"));
    }
    if !connection_contains_upgrade(head) {
        return Err(WsError::InvalidHandshake(
            "Connection header must contain 'upgrade'",
        ));
    }

    // バージョン不一致は 426（426 応答は Sec-WebSocket-Version: 13 を
    // 付与する契約、呼び出し元 crate::handle_upgrade を参照）で、他の
    // 検証違反（400）とは異なる応答種別のため個別に判定する。
    match head.header("sec-websocket-version") {
        Some("13") => {}
        _ => return Err(WsError::UnsupportedVersion),
    }

    let key = head
        .header("sec-websocket-key")
        .ok_or(WsError::InvalidHandshake("missing Sec-WebSocket-Key"))?;
    if !is_valid_base64_key(key) {
        return Err(WsError::InvalidHandshake("invalid Sec-WebSocket-Key"));
    }

    let accept_key = derive_accept_key(key.as_bytes());
    Ok(ValidatedHandshake { accept_key })
}

/// `Connection` ヘッダのカンマ区切りトークンに `upgrade`（大小無視）が
/// 含まれるかを判定する（例: `Connection: keep-alive, Upgrade`）。
///
/// `Connection` ヘッダは複数出現しうる（例: `keep-alive` と `Upgrade` が別々の
/// ヘッダ行に分かれる正当なハンドシェイクが存在する）ため、`RequestHead::header`
/// （最初の 1 件のみ返す）ではなく [`RequestHead::headers`] で全件を走査する。
/// `fandhe_backend_http::connection::should_keep_alive` と同じ理由・同じ走査方針。
fn connection_contains_upgrade(head: &RequestHead) -> bool {
    head.headers()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
}

/// `Sec-WebSocket-Key` が RFC 6455 の想定形（base64 エンコードされた 16
/// バイト、24 文字）に妥当かを検証する。
///
/// 完全な base64 デコードは行わず、文字集合・長さのみを検証する軽量
/// チェックにとどめる（`derive_accept_key` は入力バイト列をそのまま SHA-1 に
/// 通すだけで、デコードの成否に依存しないため）。長さを 24 文字ちょうどに
/// 限定するのは、RFC 6455 4.1 が 16 バイトのランダム値を要求しており、
/// 通常のクライアント実装は必ずこの長さで送るため
/// （長さ検証によりリソース枯渇的な極端に長い値も併せて排除する）。
fn is_valid_base64_key(key: &str) -> bool {
    key.len() == 24
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

/// `101 Switching Protocols` 応答をシリアライズする。
///
/// 固定テンプレート + `accept_key`（`derive_accept_key` の base64 出力）のみ
/// から組み立てる。`accept_key` は SHA-1 + base64 の出力であり base64 文字集合
/// （`[A-Za-z0-9+/=]`）以外の文字を含み得ないため、CRLF・ヘッダインジェクション
/// の混入余地はない。
#[must_use]
pub(crate) fn serialize_101(accept_key: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept_key}\r\n\
         \r\n"
    )
    .into_bytes()
}

/// `400 Bad Request` 応答をシリアライズする（ハンドシェイク検証違反）。
#[must_use]
pub(crate) fn serialize_400() -> Vec<u8> {
    b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".to_vec()
}

/// `426 Upgrade Required` 応答をシリアライズする（`Sec-WebSocket-Version`
/// 不一致）。RFC 6455 4.4 に従い `Sec-WebSocket-Version: 13` を明示する。
#[must_use]
pub(crate) fn serialize_426() -> Vec<u8> {
    b"HTTP/1.1 426 Upgrade Required\r\nSec-WebSocket-Version: 13\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".to_vec()
}

/// アプリケーション定義の受理判定フック（イシュー #716）。
///
/// RFC 6455 検証（`validate`）を通過した upgrade 要求について、
/// [`crate::handle_upgrade_with_peer_addr`] が 101 応答を送出する直前に
/// 一度だけ同期で呼ばれる。`{name}` パスパラメータ・リクエストヘッダ・
/// 接続元アドレスを参照して独自の認可判定（例: 存在しない `{id}` への
/// 接続を 404 で拒否する、`Origin`/`Host` を検査して DNS rebinding を
/// 防ぐ）を行いたいユーザー向けの拡張点。
///
/// `RequestGate`（`crates/core` の 3 拡張点の 1 つ）はパスパラメータを
/// 持たないため、本フックはそれを代替する `plugin-websocket` 内蔵の
/// 受理判定手段として設計した（コア拡張点は増やさない。
/// `docs/design/ws-connection-context-and-close.md` 16 節参照）。
///
/// # 契約
///
/// - **同期・非ブロッキング**: `crate::handler::WsMessageHandler` と同様、
///   実装は同期ブロッキング I/O を行わない（`.claude/rules/coding-rust.md`）。
///   非同期 I/O が必要な判定（DB 参照等）は事前にキャッシュしておくこと。
/// - **panic しない**: 評価は core が `tokio::spawn` したタスク内で行われ
///   panic はタスク境界で隔離されるが、契約としては `Err` を返すこと
///   （`.claude/rules/coding-rust.md`「panic はライブラリ境界を越えさせ
///   ない」）。
/// - **拒否時の応答**: `Err(response)` を返すと `handle_upgrade_with_peer_addr`
///   は upgrade を行わず、`normalize_rejection` で正規化した後の
///   `response` を送出して接続を閉じる（101 応答は送出しない）。
pub trait WsHandshakeCheck: Send + Sync + 'static {
    /// `ctx` を検査し、受理する場合は `Ok(())`、拒否する場合はクライアントへ
    /// 返すレスポンスを `Err` で返す。
    ///
    /// # Errors
    ///
    /// 接続を拒否する場合、返す [`Response`] を正規化した上でクライアントへ
    /// 送出する（`normalize_rejection` の doc を参照）。
    fn check(&self, ctx: &WsHandshakeContext<'_>) -> Result<(), Response>;
}

impl<F> WsHandshakeCheck for F
where
    F: Fn(&WsHandshakeContext<'_>) -> Result<(), Response> + Send + Sync + 'static,
{
    fn check(&self, ctx: &WsHandshakeContext<'_>) -> Result<(), Response> {
        self(ctx)
    }
}

/// [`WsHandshakeCheck::check`] へ渡す借用コンテキスト（イシュー #716）。
///
/// `crate::handler::WsOpenContext` と同じく非公開フィールド + アクセサの
/// 構成とし、`#[non_exhaustive]` を付けて将来のフィールド追加を非破壊に
/// する。`head` / `params` は 101 応答送出前・`handle_upgrade_with_peer_addr`
/// のスタックフレーム内でのみ生存するため、`WsOpenContext`（所有 `Vec` へ
/// コピー済み）とは異なり借用のまま渡す（フック呼び出しは同期・1 回限りで
/// 完結するため、コピーの必要がない）。
#[non_exhaustive]
pub struct WsHandshakeContext<'a> {
    head: &'a RequestHead,
    params: &'a PathParams<'a>,
    peer_addr: Option<SocketAddr>,
}

impl<'a> WsHandshakeContext<'a> {
    /// コンテキストを構築する（`pub(crate)`、`crate::handle_upgrade_with_peer_addr`
    /// からのみ呼ばれる）。
    pub(crate) fn new(
        head: &'a RequestHead,
        params: &'a PathParams<'a>,
        peer_addr: Option<SocketAddr>,
    ) -> Self {
        Self {
            head,
            params,
            peer_addr,
        }
    }

    /// アップグレード要求の `RequestHead` を返す。`Host` / `Origin` 等の
    /// ヘッダ検査（DNS rebinding 対策）に使う。
    #[must_use]
    pub fn head(&self) -> &'a RequestHead {
        self.head
    }

    /// `name`（大小無視）のヘッダ値を返す。
    ///
    /// [`RequestHead::header`] とは異なり、同名ヘッダが複数出現する場合は
    /// **`None`**（判定不能・拒否側）を返す（イシュー #716 P1 レビュー指摘の
    /// フェイルクローズ対応）。`RequestHead::header` は先頭 1 件のみを返す
    /// 契約のため、本フックの典型用途である `Origin` / `Host` 等の認可判断に
    /// 使うヘッダが重複指定されたリクエストでは、本フックが見る値と、別の値
    /// を採用する中継先（リバースプロキシ等）の判断が食い違い、認可判定を
    /// 迂回されうる（RFC 9110 5.3 節はヘッダの重複解釈を規定しておらず実装
    /// 依存）。重複は「値を一意に決定できない」ため拒否側（`None`）に倒し、
    /// 呼び出し側の `match` の `_` 分岐（拒否）へフォールバックさせる設計と
    /// する（`.claude/rules/security.md`「認証・認可」）。
    ///
    /// 重複そのものを許容し全出現値を確認したい呼び出し元は、
    /// [`WsHandshakeContext::head`] 経由で [`RequestHead::headers`] を使うこと。
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&'a str> {
        let mut matches = self
            .head
            .headers()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v);
        let first = matches.next()?;
        if matches.next().is_some() {
            // 同名ヘッダが 2 件以上存在する = 値を一意に決定できない。
            // 先頭値を採用すると中継先との判定食い違いによる認可迂回を
            // 招くため、フェイルクローズで「なし」として扱う。
            None
        } else {
            Some(first)
        }
    }

    /// `name` に対応する `{name}` パスパラメータの値を返す（非デコード
    /// 契約、`crate::pattern::PathParams` と同一）。
    /// `WebSocketConfig::with_path_pattern` 未登録（完全一致パス）の場合は
    /// 常に `None`。
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name)
    }

    /// マッチしたパスパラメータ全件を登録順に返す。
    pub fn params(&self) -> impl Iterator<Item = (&str, &str)> {
        self.params.iter()
    }

    /// 接続元の実 peer address を返す（`crate::handler::WsOpenContext::
    /// peer_addr` と同一の由来・フェイルクローズ契約）。
    ///
    /// リバースプロキシ配下ではプロキシ自身のアドレスになる。`None`
    /// （非ソケット経路、または `crate::handle_upgrade`（5 引数の旧 API）
    /// 経由）は IP ベースの認可判定では拒否側に倒すべきことに注意する。
    #[must_use]
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer_addr
    }
}

impl fmt::Debug for WsHandshakeContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `WsOpenContext` と同じ理由でヘッダ値・パスパラメータ・接続元
        // アドレスを出力しない（`.claude/rules/security.md`「ログに PII を
        // 出さない」。ヘッダ・パスパラメータは攻撃者が自由に制御できる
        // 入力でもある）。
        f.debug_struct("WsHandshakeContext").finish_non_exhaustive()
    }
}

/// [`WsHandshakeCheck::check`] が返す拒否レスポンスを、upgrade 未成立の
/// 状態と矛盾しない形へフェイルクローズに正規化する（イシュー #716）。
///
/// - **1xx**（`100..=199`、101 を含む）: クライアントが「upgrade が成功
///   した」と誤認する一方でプラグインは接続を閉じるため、`400 Bad Request`
///   （body なし）に置き換える。
/// - **2xx**（`200..=299`）: 「拒否」の意味に反するため、同じく
///   `400 Bad Request` に置き換える。
/// - **3xx/4xx/5xx**（`300..=599`）: 指定どおりそのまま返す（RFC 6455 4.2.2
///   はリダイレクト応答を許容している）。
/// - **上記いずれにも属さない値**（`0..=99`・`600` 以上。`u16` の型レベルの
///   契約はあるが HTTP ステータスコードとして未定義の範囲、フックの実装
///   ミスや `Response::empty(0)` 等の誤用を想定）: 設計文書（本 doc）が
///   許容する応答以外を送出しないよう、同じく `400 Bad Request` に正規化
///   する（イシュー #716 P2 レビュー指摘）。
///
/// 直列化時の `keep_alive` は常に `false` とする契約は呼び出し元
/// （`crate::handle_upgrade_with_peer_addr`）が担う（拒否後の接続を再利用
/// しない）。
#[must_use]
pub(crate) fn normalize_rejection(response: Response) -> Response {
    if (300..600).contains(&response.status) {
        response
    } else {
        Response::empty(400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    fn head_from(buf: &[u8]) -> RequestHead {
        match parse_request_head(buf).expect("parse should succeed") {
            ParseOutcome::Complete { head, .. } => head,
            ParseOutcome::Incomplete => panic!("expected Complete"),
        }
    }

    fn valid_handshake_head() -> RequestHead {
        head_from(
            b"GET /ws HTTP/1.1\r\n\
              Host: example.com\r\n\
              Upgrade: websocket\r\n\
              Connection: Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Sec-WebSocket-Version: 13\r\n\
              \r\n",
        )
    }

    #[test]
    fn matches_ws_path_and_upgrade_header() {
        let config = WebSocketConfig::default();
        assert!(matches(&valid_handshake_head(), &config));
    }

    #[test]
    fn matches_ignores_query_string_when_comparing_path() {
        // `RequestHead::target` はクエリ文字列を含む完全な request-target。
        // `/ws?token=...` のような正当なアップグレードでも `config.path`
        // （クエリなし）と一致すべき回帰（Bugbot 指摘: Query strings break
        // WebSocket matching）。
        let config = WebSocketConfig::default();
        let head = head_from(b"GET /ws?token=abc HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        assert!(matches(&head, &config));
    }

    #[test]
    fn matches_rejects_other_path_with_query_string() {
        let config = WebSocketConfig::default().with_path("/ws");
        let head = head_from(b"GET /other?token=abc HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        assert!(!matches(&head, &config));
    }

    #[test]
    fn matches_rejects_other_path() {
        let config = WebSocketConfig::default().with_path("/ws");
        let head = head_from(b"GET /other HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        assert!(!matches(&head, &config));
    }

    #[test]
    fn match_config_path_extracts_params_for_registered_pattern() {
        // イシュー #676: `matches` が捨てていた抽出結果を、共有ヘルパー
        // 経由で取得できることを確認する。
        let config = WebSocketConfig::default()
            .with_path_pattern("/devtools/page/{id}")
            .unwrap();
        let head = head_from(b"GET /devtools/page/XYZ HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        let params = match_config_path(&head, &config).expect("pattern should match");
        assert_eq!(params.get("id"), Some("XYZ"));
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn match_config_path_ignores_query_string_when_extracting() {
        let config = WebSocketConfig::default()
            .with_path_pattern("/devtools/page/{id}")
            .unwrap();
        let head =
            head_from(b"GET /devtools/page/XYZ?token=abc HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        let params = match_config_path(&head, &config).expect("pattern should match");
        assert_eq!(params.get("id"), Some("XYZ"));
    }

    #[test]
    fn match_config_path_returns_empty_params_for_exact_match_config() {
        // パターン未登録（完全一致パス）は一致しても抽出対象のパラメータが
        // 存在しないため、空の `PathParams` を返す（受け入れ基準 2）。
        let config = WebSocketConfig::default();
        let head = valid_handshake_head();
        let params = match_config_path(&head, &config).expect("exact path should match");
        assert!(params.is_empty());
    }

    #[test]
    fn match_config_path_returns_none_for_mismatched_pattern() {
        let config = WebSocketConfig::default()
            .with_path_pattern("/devtools/page/{id}")
            .unwrap();
        let head = head_from(b"GET /other HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        assert!(match_config_path(&head, &config).is_none());
    }

    #[test]
    fn matches_rejects_missing_upgrade_header() {
        let config = WebSocketConfig::default();
        let head = head_from(b"GET /ws HTTP/1.1\r\n\r\n");
        assert!(!matches(&head, &config));
    }

    #[test]
    fn matches_routes_devtools_browser_and_page_patterns_independently() {
        // イシュー #675 受け入れ基準 1: 複数パターン登録の `WebSocketConfig`
        // が、それぞれ対応するパスにのみ一致し他方には一致しないこと。
        let browser_config = WebSocketConfig::default()
            .with_path_pattern("/devtools/browser/{id}")
            .unwrap();
        let page_config = WebSocketConfig::default()
            .with_path_pattern("/devtools/page/{id}")
            .unwrap();

        let browser_head =
            head_from(b"GET /devtools/browser/ABC HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        let page_head = head_from(b"GET /devtools/page/XYZ HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");

        assert!(matches(&browser_head, &browser_config));
        assert!(!matches(&browser_head, &page_config));
        assert!(matches(&page_head, &page_config));
        assert!(!matches(&page_head, &browser_config));
    }

    #[test]
    fn matches_respects_registration_order_first_match_wins() {
        // イシュー #675 受け入れ基準 2 の補助的証跡: パターン設定同士が
        // 重複してマッチしうる場合でも、探索順序（`Iterator::position`）は
        // 登録順のまま。実際の「登録順に最初に一致した設定を使う」契約は
        // コア側 `crates/core/src/plugin.rs::try_handle_upgrade` の
        // `.find()`（本 PR では変更しない）が担う。
        let configs = [
            WebSocketConfig::default()
                .with_path_pattern("/devtools/{kind}/{id}")
                .unwrap(),
            WebSocketConfig::default()
                .with_path_pattern("/devtools/page/{id}")
                .unwrap(),
        ];
        let head = head_from(b"GET /devtools/page/XYZ HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        let position = configs.iter().position(|c| matches(&head, c));
        assert_eq!(position, Some(0));
    }

    #[test]
    fn matches_rejects_pattern_with_missing_segment() {
        let config = WebSocketConfig::default()
            .with_path_pattern("/devtools/page/{id}")
            .unwrap();
        let head = head_from(b"GET /devtools/page HTTP/1.1\r\nUpgrade: websocket\r\n\r\n");
        assert!(!matches(&head, &config));
    }

    /// RFC 6455 4.2.2 の既知ベクタで `Sec-WebSocket-Accept` 導出を固定する。
    #[test]
    fn validate_derives_known_accept_key_vector() {
        let handshake = validate(&valid_handshake_head()).expect("valid handshake");
        assert_eq!(handshake.accept_key, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn validate_accepts_connection_header_with_multiple_tokens() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\n\
              Upgrade: websocket\r\n\
              Connection: keep-alive, Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Sec-WebSocket-Version: 13\r\n\
              \r\n",
        );
        assert!(validate(&head).is_ok());
    }

    #[test]
    fn validate_accepts_upgrade_token_split_across_multiple_connection_headers() {
        // `Connection` ヘッダが複数行に分かれ、`upgrade` トークンが最初の行に
        // 含まれない正当なハンドシェイク（`RequestHead::header` は最初の 1 件
        // しか返さないため、全件走査していないと誤って 400 を返す回帰）。
        let head = head_from(
            b"GET /ws HTTP/1.1\r\n\
              Upgrade: websocket\r\n\
              Connection: keep-alive\r\n\
              Connection: Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Sec-WebSocket-Version: 13\r\n\
              \r\n",
        );
        assert!(validate(&head).is_ok());
    }

    #[test]
    fn validate_rejects_missing_connection_upgrade_token() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\n\
              Upgrade: websocket\r\n\
              Connection: keep-alive\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Sec-WebSocket-Version: 13\r\n\
              \r\n",
        );
        assert!(matches!(validate(&head), Err(WsError::InvalidHandshake(_))));
    }

    #[test]
    fn validate_rejects_missing_key() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\r\n",
        );
        assert!(matches!(validate(&head), Err(WsError::InvalidHandshake(_))));
    }

    #[test]
    fn validate_rejects_malformed_key_length() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: short\r\nSec-WebSocket-Version: 13\r\n\r\n",
        );
        assert!(matches!(validate(&head), Err(WsError::InvalidHandshake(_))));
    }

    #[test]
    fn validate_rejects_unsupported_version() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 8\r\n\r\n",
        );
        assert!(matches!(validate(&head), Err(WsError::UnsupportedVersion)));
    }

    #[test]
    fn validate_rejects_missing_upgrade_header() {
        let head = head_from(
            b"GET /ws HTTP/1.1\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        );
        assert!(matches!(validate(&head), Err(WsError::InvalidHandshake(_))));
    }

    #[test]
    fn serialize_101_embeds_accept_key_without_injection_risk() {
        let bytes = serialize_101("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
        assert!(text.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));
        assert!(text.ends_with("\r\n\r\n"));
    }

    #[test]
    fn serialize_400_is_well_formed() {
        let text = String::from_utf8(serialize_400()).unwrap();
        assert!(text.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    }

    #[test]
    fn serialize_426_includes_supported_version_header() {
        let text = String::from_utf8(serialize_426()).unwrap();
        assert!(text.starts_with("HTTP/1.1 426 Upgrade Required\r\n"));
        assert!(text.contains("Sec-WebSocket-Version: 13\r\n"));
    }

    #[test]
    fn normalize_rejection_replaces_1xx_with_400() {
        // 101 を返すフックがあっても、クライアントには upgrade 成功と
        // 誤認させる応答を送出してはならない（イシュー #716 受け入れ基準 1）。
        let normalized = normalize_rejection(Response::empty(101));
        assert_eq!(normalized.status, 400);
    }

    #[test]
    fn normalize_rejection_replaces_2xx_with_400() {
        let normalized = normalize_rejection(Response::empty(200));
        assert_eq!(normalized.status, 400);
    }

    #[test]
    fn normalize_rejection_preserves_3xx_4xx_5xx() {
        for status in [301, 403, 404, 503] {
            let normalized = normalize_rejection(Response::empty(status));
            assert_eq!(normalized.status, status);
        }
    }

    #[test]
    fn normalize_rejection_preserves_body_and_headers_for_non_1xx_2xx() {
        let response = Response::new(404, b"no such page".to_vec())
            .with_header("X-Reason", "not-found")
            .unwrap();
        let normalized = normalize_rejection(response);
        assert_eq!(normalized.status, 404);
        assert_eq!(normalized.body, b"no such page");
    }

    #[test]
    fn normalize_rejection_replaces_out_of_range_status_with_400() {
        // フックの実装ミス（`Response::empty(0)`）や `600` 以上の非標準値は
        // 3xx/4xx/5xx のいずれでもなく設計文書が想定しない応答のため、
        // 無条件通過させず 400 へ正規化する（イシュー #716 P2 レビュー指摘）。
        for status in [0, 1, 99, 600, 999] {
            let normalized = normalize_rejection(Response::empty(status));
            assert_eq!(
                normalized.status, 400,
                "status={status} が正規化されていない"
            );
        }
    }

    fn handshake_check_head() -> RequestHead {
        head_from(
            b"GET /devtools/page/XYZ HTTP/1.1\r\n\
              Host: example.com\r\n\
              Origin: https://example.com\r\n\
              Upgrade: websocket\r\n\
              Connection: Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Sec-WebSocket-Version: 13\r\n\
              \r\n",
        )
    }

    #[test]
    fn handshake_context_exposes_head_header_and_peer_addr() {
        let head = handshake_check_head();
        let params = PathParams::default();
        let addr: std::net::SocketAddr = "127.0.0.1:54321".parse().unwrap();
        let ctx = WsHandshakeContext::new(&head, &params, Some(addr));

        assert_eq!(ctx.header("origin"), Some("https://example.com"));
        assert_eq!(ctx.head().target(), "/devtools/page/XYZ");
        assert_eq!(ctx.peer_addr(), Some(addr));
        assert!(ctx.params().next().is_none());
    }

    #[test]
    fn handshake_context_exposes_path_params() {
        let config = WebSocketConfig::default()
            .with_path_pattern("/devtools/page/{id}")
            .unwrap();
        let head = handshake_check_head();
        let params = match_config_path(&head, &config).expect("pattern should match");
        let ctx = WsHandshakeContext::new(&head, &params, None);

        assert_eq!(ctx.param("id"), Some("XYZ"));
        assert_eq!(ctx.peer_addr(), None);
    }

    #[test]
    fn handshake_context_debug_redacts_head_params_and_peer_addr() {
        let head = handshake_check_head();
        let params = PathParams::default();
        let addr: std::net::SocketAddr = "127.0.0.1:54321".parse().unwrap();
        let ctx = WsHandshakeContext::new(&head, &params, Some(addr));
        let debug = format!("{ctx:?}");
        assert!(!debug.contains("example.com"));
        assert!(!debug.contains("54321"));
    }

    #[test]
    fn handshake_context_header_returns_none_for_duplicate_header() {
        // 重複した `Origin` ヘッダを含むリクエストでは、先頭値を採用すると
        // 別の値を採用する中継先（リバースプロキシ等）と認可判定が食い違い
        // うるため、判定不能として拒否側（`None`）に倒す（イシュー #716 P1
        // レビュー指摘）。
        let head = head_from(
            b"GET /ws HTTP/1.1\r\n\
              Origin: https://allowed.example\r\n\
              Origin: https://evil.example\r\n\
              Upgrade: websocket\r\n\
              Connection: Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
              Sec-WebSocket-Version: 13\r\n\
              \r\n",
        );
        let params = PathParams::default();
        let ctx = WsHandshakeContext::new(&head, &params, None);
        assert_eq!(ctx.header("origin"), None);
    }

    #[test]
    fn handshake_context_header_returns_value_for_single_header() {
        let head = handshake_check_head();
        let params = PathParams::default();
        let ctx = WsHandshakeContext::new(&head, &params, None);
        assert_eq!(ctx.header("origin"), Some("https://example.com"));
    }

    #[test]
    fn closure_implements_handshake_check() {
        // `Fn` への blanket impl 経由でクロージャをそのまま登録できることの
        // 回帰。`WebSocketConfig::with_handshake_check` の受け入れ型が
        // trait オブジェクトへ変換可能であることを固定する。
        let check: std::sync::Arc<dyn WsHandshakeCheck> =
            std::sync::Arc::new(|_ctx: &WsHandshakeContext<'_>| Ok(()));
        let head = handshake_check_head();
        let params = PathParams::default();
        let ctx = WsHandshakeContext::new(&head, &params, None);
        assert!(check.check(&ctx).is_ok());
    }
}
