//! Cookie 関連のヘルパ（読み取りパーサ・構築時検証済み書き込み型）。
//!
//! 本モジュールは 2 つの独立した責務を持つ。
//!
//! - **読み取り**（[`parse_cookie_header`]、イシュー #309）: [`crate::request::RequestHead::header`] /
//!   [`crate::request::RequestHead::headers`] は生ヘッダ値を返すのみで、
//!   `Cookie: k=v; k2=v2` を key-value 組へ分解する処理は呼び出し元の責務
//!   だった（[`crate::query`]・[`crate::form`] と同型の未整備領域）。本モジュール
//!   はその分解処理を RFC 6265 の cookie-pair 構文に準拠する形で提供し、
//!   セッション・認証実装（今後のプラグイン）が個別に自前 split を実装するのを防ぐ。
//! - **書き込み**（[`SetCookie`]、イシュー #303）: [`crate::response::Response::with_header`]
//!   は CR/LF/NUL・制御文字のみを拒否する汎用ヘッダ検証であり、RFC 6265 の
//!   cookie-name / cookie-value の文法（`;` や `,` を含む値がヘッダ構造を壊す等）
//!   までは検証しない。[`SetCookie`] は axum の `CookieJar` 相当の最小サブセット
//!   として構築時検証済みの専用型を提供し、
//!   [`crate::response::Response::with_set_cookie`] の唯一の入力とすることで、
//!   `with_header` と同一のフェイルクローズ設計思想を cookie 属性に拡張する
//!   （`response.rs` モジュール冒頭 doc の「構築時検証済み専用型」パターンの
//!   第 2 号、[`crate::response::AllowedMethods`] と同型）。
//!
//! # 読み取り側: 呼び出し契約
//!
//! [`parse_cookie_header`] は単一 `Cookie` ヘッダ値（1 本）を入力に取る
//! sans-IO 純関数。複数 `Cookie` ヘッダが届いた場合の結合は
//! [`crate::request::RequestHead::cookies`] が担う（受け入れ条件 2）。
//!
//! # 読み取り側: 構文仕様（RFC 6265 §4.1.1 cookie-pair 準拠）
//!
//! ```text
//! cookie-pair   = cookie-name "=" cookie-value
//! cookie-name   = token（RFC 9110 tchar のみ、1 文字以上）
//! cookie-value  = *cookie-octet / ( DQUOTE *cookie-octet DQUOTE )
//! cookie-octet  = %x21 / %x23-2B / %x2D-3A / %x3C-5B / %x5D-7E
//!                 （CTL・SP・DQUOTE・カンマ・セミコロン・バックスラッシュを除く）
//! ```
//!
//! - `cookie-name` は空を許容しない（空名は構文違反）
//! - `cookie-value` は空を許容する（`k=` は valid）
//! - DQUOTE で囲んだ値は両端の引用符を除去した内側を返す（除去する契約）
//! - pair 前後の OWS（RFC 9110 §5.6.3、SP `0x20` / HTAB `0x09` のみ）は trim する。
//!   `str::trim()` の Unicode 空白全般 trim は使わない（NBSP・BOM 等を誤って除去し
//!   fail-closed 契約を弱めるため、`crate::request::trim_ows_str`（private ヘルパ）を使う）
//!
//! # 読み取り側: 不正組の扱い（fail-closed。受け入れ条件 1）
//!
//! 構文違反の pair（`=` 欠落・空名・tchar 違反・cookie-octet 違反・空 pair）を
//! 検出した場合は **明示スキップではなく [`CookieError::InvalidCookiePair`] を
//! 返す**。他パーサ（[`crate::query`] 等）と異なり Cookie は認証・セッション
//! 情報を運ぶことが多く、構文違反を暗黙に読み飛ばすと「攻撃者が意図的に
//! 壊した pair の陰に正規 pair を隠す」類の解析不一致（smuggling）を招きうる
//! ため、境界検証の穴を作らない fail-closed を採る（`.claude/rules/security.md`）。
//!
//! # 読み取り側: DoS 耐性（`.claude/rules/security.md` リソース枯渇対策）
//!
//! [`crate::query`] と同じ「分解前に上限を検査し fail-closed で拒否する」
//! 方針を踏襲する。[`parse_cookie_header`] は組数・全長の 2 上限を検証し、
//! 超過時は部分結果を返さない。
//!
//! # 読み取り側: 非デコード契約
//!
//! % デコードは行わない（[`crate::query`]・
//! [`crate::request::RequestHead::path`] と同じ「無正規化のまま返す」契約）。
//! デコード・信頼判断・同名 Cookie の採否（first-wins 等）・受信した cookie
//! 名の `__Host-`/`__Secure-` 接頭辞に基づく信頼判断は呼び出し元の責務とする
//! （読み取り側では接頭辞を検証しない。書き込み側は次節のとおり構築時に強制する）。
//!
//! # 書き込み側: cookie 接頭辞の構築時検証（イシュー #810）
//!
//! RFC 6265bis の cookie 接頭辞は、ブラウザがサブドメイン・平文経路からの
//! cookie 上書き（cookie tossing）を拒否する防御機構である。条件を満たさない
//! 接頭辞付き cookie はブラウザに黙って無視され防御が効かないため、
//! [`SetCookie`] は条件を満たさない値を型として作れないようにする（fail-closed）。
//!
//! | 接頭辞 | 条件 | 構築経路 |
//! |--------|------|----------|
//! | `__Host-` | `Secure`・`Path=/`・`Domain` なし | [`SetCookie::host_prefixed`] |
//! | `__Secure-` | `Secure` | [`SetCookie::secure_prefixed`] |
//!
//! - 接頭辞の判定は ASCII 大文字小文字を区別しない（安全側）。名前は与えられた
//!   とおりに保持し正規化しない
//! - [`SetCookie::new`] は接頭辞付き名前を [`CookieError::ReservedPrefix`] で拒否する
//! - `__Host-` cookie の [`SetCookie::path`] は `/` のみ許可する
//! - `Secure` を外す API は存在しないため、専用関数で構築した cookie は常に `Secure`
//! - `Domain` 属性は現状存在せず、`__Host-` の「Domain なし」は型で満たされる。
//!   **将来 `Domain` 属性を追加する場合は、`__Host-` cookie で拒否する不変条件を
//!   同時に実装しなければならない**
//!
//! # 書き込み側: セッション Cookie の安全な既定値（イシュー #827）
//!
//! 認証ハンドラが発行するセッション Cookie は、`HttpOnly`・`Secure`・
//! `SameSite`・`Path` の付け忘れが XSS 窃取・平文送出・CSRF に直結する。
//! [`SetCookie::session`] は `HttpOnly`・`Secure`・`SameSite=Lax`・`Path=/` を
//! 既定で付与する（`Max-Age` は付けずブラウザのセッション寿命に任せる）。
//! `SameSite` は [`SessionSameSite`]（`Lax` / `Strict` のみ）で変更でき、
//! `SameSite=None` は型として表せない。
//!
//! 削除用の [`SetCookie::session_removal`] は発行用と同じ属性に `Max-Age=0` を
//! 加えて出力する（属性が食い違うとブラウザ上の Cookie が消えない）。
//!
//! 接頭辞との組み合わせ: 既定値（`Secure` + `Path=/`、`Domain` なし）は
//! `__Host-` / `__Secure-` の条件を満たすため、`session("__Host-sid", v)` を
//! そのまま受け付ける（`__Host-` の必須化はしない）。接頭辞の条件の強制は
//! 引き続き [`SetCookie::path`] 等の既存検査が担う。
//!
//! # 書き込み側: スコープ外
//!
//! `Domain` / `Expires` / `Partitioned` 属性は最小サブセット方針によりスコープ外
//! （必要になった時点で別イシュー化する、`.claude/rules/out-of-scope-tracking.md`）。

/// 単一 `Cookie` ヘッダから許容する cookie-pair 数の上限。
///
/// [`crate::query::MAX_QUERY_PAIRS`] と同水準。[`crate::request::RequestHead::cookies`]
/// はこの上限を複数 `Cookie` ヘッダに跨る累積値として適用し、ヘッダ分割による
/// 上限迂回を防ぐ。
pub const MAX_COOKIE_COUNT: usize = 100;

/// 単一 `Cookie` ヘッダとして許容する最大バイト数。
///
/// [`crate::request::MAX_HEADER_BYTES`]（16 KiB）の内数として妥当な値。
/// [`crate::request::RequestHead::cookies`] はこの上限を複数 `Cookie` ヘッダに
/// 跨る累積値として適用する。
pub const MAX_COOKIE_STRING_BYTES: usize = 8 * 1024;

/// [`parse_cookie_header`]（読み取り側）・[`SetCookie`]（書き込み側）が
/// 返しうるエラー。
///
/// `#[non_exhaustive]`: 将来 variant が増えうるため、外部からの `match` には
/// `_ =>` アームが必要。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CookieError {
    /// Cookie 文字列（累積を含む）が [`MAX_COOKIE_STRING_BYTES`] を超過した。
    CookieStringTooLarge,
    /// cookie-pair 数（累積を含む）が [`MAX_COOKIE_COUNT`] を超過した。
    TooManyCookies,
    /// cookie-pair が RFC 6265 構文に違反した（`=` 欠落・空名・tchar 違反・
    /// cookie-octet 違反・空 pair 等）。値そのものは含めない（セッション
    /// トークン等の機密が誤ってエラーメッセージへ漏れるのを防ぐ）。
    InvalidCookiePair,
    /// [`SetCookie::new`] / [`SetCookie::path`]（書き込み側）: cookie 名が空、
    /// または RFC 9110 tchar（RFC 6265 の cookie-name = token と同一基準）
    /// 以外の文字を含む。
    InvalidName,
    /// [`SetCookie::new`]（書き込み側）: cookie 値が RFC 6265 cookie-octet の
    /// 範囲外の文字を含む。
    InvalidValue,
    /// [`SetCookie::path`]（書き込み側）: `Path` 属性値が RFC 6265 path-value
    /// の範囲外の文字を含む、または `/` で始まらない（後者は文法上は
    /// path-value として許容されうるが、RFC 6265 5.2.4 のクッキーパス抽出
    /// アルゴリズムによりユーザーエージェント側でデフォルトパスへ
    /// フォールバックし黙って無視される。呼び出し元が cookie スコープを
    /// 絞ったつもりでも実際には絞られない不整合を招くため、構築時点で
    /// フェイルクローズに拒否する）。
    InvalidPath,
    /// [`SetCookie::new`]（書き込み側）: `__Host-` / `__Secure-` 接頭辞
    /// （ASCII 大文字小文字無視）で始まる名前が渡された。条件
    /// （`Secure`・`Path=/`）を構築時に強制する [`SetCookie::host_prefixed`] /
    /// [`SetCookie::secure_prefixed`] を使う必要がある。
    ReservedPrefix,
    /// [`SetCookie::host_prefixed`] / [`SetCookie::secure_prefixed`]（書き込み側）:
    /// 名前が対応する接頭辞で始まらない（`__Host-` 名を `secure_prefixed` に
    /// 渡した場合も該当し、`__Host-` の追加条件を欠く cookie を作れない）。
    PrefixMismatch,
    /// [`SetCookie::path`]（書き込み側）: `__Host-` cookie に `/` 以外の
    /// `Path` を指定した。
    HostPrefixPath,
    /// [`SessionSameSite::try_from`]（書き込み側）: セッション Cookie に
    /// `SameSite=None` が指定された。
    SessionSameSiteNone,
}

impl std::fmt::Display for CookieError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            CookieError::CookieStringTooLarge => "cookie string exceeds MAX_COOKIE_STRING_BYTES",
            CookieError::TooManyCookies => "cookie pair count exceeds MAX_COOKIE_COUNT",
            CookieError::InvalidCookiePair => "cookie pair violates RFC 6265 cookie-pair syntax",
            CookieError::InvalidName => "cookie 名が空、または RFC 9110 tchar 以外の文字を含む",
            CookieError::InvalidValue => "cookie 値が RFC 6265 cookie-octet の範囲外の文字を含む",
            CookieError::InvalidPath => {
                "Path 属性値が RFC 6265 path-value の範囲外の文字を含む、または '/' で始まらない"
            }
            CookieError::ReservedPrefix => {
                "cookie 名が __Host-/__Secure- 接頭辞で始まる（host_prefixed / secure_prefixed を使うこと）"
            }
            CookieError::PrefixMismatch => "cookie 名が専用構築関数に対応する接頭辞で始まらない",
            CookieError::HostPrefixPath => "__Host- cookie の Path 属性は '/' のみ許可される",
            CookieError::SessionSameSiteNone => {
                "セッション cookie では SameSite=None を指定できない"
            }
        };
        f.write_str(msg)
    }
}

impl std::error::Error for CookieError {}

/// RFC 6265 cookie-octet: `%x21 / %x23-2B / %x2D-3A / %x3C-5B / %x5D-7E`。
///
/// 印字可能 US-ASCII（`0x21`–`0x7E`）から CTL 域外の 5 バイト
/// （SP `0x20`・`"` `0x22`・`,` `0x2C`・`;` `0x3B`・`\` `0x5C`）を除いた集合。
/// これらの除外文字は Cookie ヘッダの `; ` 区切り構文・値の引用構文と衝突する
/// ため、混入するとヘッダ構造が壊れる（レスポンス分割ではないが cookie 属性の
/// 意図しない注入経路になりうる）。空値（cookie-value は RFC 6265 上省略可）
/// は呼び出し元の `bytes().all(...)` が空イテレータに対し `true` を返すため
/// 別途許可している。読み取り側（`parse_cookie_pair`）・書き込み側
/// （[`SetCookie::new`]）の両方が共有する。
fn is_cookie_octet(b: u8) -> bool {
    matches!(b, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)
}

/// 単一の cookie-pair（`name=value` の 1 組）を検証・分解する（読み取り側）。
///
/// 前後の OWS は呼び出し元（[`parse_cookie_header`]）が trim 済みの前提。
/// DQUOTE 囲みの値は引用符を除去した内側を返す。
///
/// `pub(crate)`: [`crate::request::RequestHead::cookies`] が複数 `Cookie`
/// ヘッダを跨る累積 DoS 上限をチェックしたうえで、ヘッダごとの `&str` を
/// 所有権コピーなし（zero-copy）で直接検証するために共有する。
pub(crate) fn parse_cookie_pair(pair: &str) -> Result<(&str, &str), CookieError> {
    let (name, raw_value) = pair.split_once('=').ok_or(CookieError::InvalidCookiePair)?;
    if name.is_empty() || !name.bytes().all(crate::request::is_tchar) {
        return Err(CookieError::InvalidCookiePair);
    }
    let value = match raw_value.strip_prefix('"') {
        Some(inner) => inner
            .strip_suffix('"')
            .ok_or(CookieError::InvalidCookiePair)?,
        None => raw_value,
    };
    if !value.bytes().all(is_cookie_octet) {
        return Err(CookieError::InvalidCookiePair);
    }
    Ok((name, value))
}

/// 単一 `Cookie` ヘッダ値を cookie-pair の列へ分解する sans-IO 純関数（読み取り側）。
///
/// `;` 区切りで pair を分割し、各 pair 前後の OWS を trim してから
/// `parse_cookie_pair` で検証する。区切りは `"; "`（RFC 6265 §5.4 の
/// サーバ側寛容化）・`";"` 単独のいずれも受理する。
///
/// 上限超過・構文違反時は `Vec` を一切構築せず `Err` を返す（fail-closed。
/// モジュール doc の「読み取り側: 不正組の扱い」「読み取り側: DoS 耐性」節を参照）。
///
/// 戻り値は入力 `value` への借用（ゼロコピー）。
///
/// # Examples
///
/// 複数 pair を `"; "` 区切りで分解する:
///
/// ```
/// use fandhe_backend_http::cookie::parse_cookie_header;
///
/// let pairs = parse_cookie_header("a=1; b=2").unwrap();
/// assert_eq!(pairs, vec![("a", "1"), ("b", "2")]);
/// ```
///
/// DQUOTE で囲んだ値は引用符を除去して返す（内側は SP を含まない cookie-octet
/// のみで構成する必要がある。SP は DQUOTE 内でも cookie-octet ではない）:
///
/// ```
/// use fandhe_backend_http::cookie::parse_cookie_header;
///
/// let pairs = parse_cookie_header(r#"a="hello-world""#).unwrap();
/// assert_eq!(pairs, vec![("a", "hello-world")]);
/// ```
///
/// 空値は許容する（`k=` は valid）:
///
/// ```
/// use fandhe_backend_http::cookie::parse_cookie_header;
///
/// assert_eq!(parse_cookie_header("k=").unwrap(), vec![("k", "")]);
/// ```
///
/// 不正な pair は明示スキップせずエラーを返す（fail-closed）:
///
/// ```
/// use fandhe_backend_http::cookie::{parse_cookie_header, CookieError};
///
/// // `=` を含まない pair は構文違反。
/// assert_eq!(
///     parse_cookie_header("a=1; broken; b=2").unwrap_err(),
///     CookieError::InvalidCookiePair
/// );
/// ```
///
/// 空文字列（`Cookie:` のみ）も 1 pair 以上が必要なため構文違反として拒否する:
///
/// ```
/// use fandhe_backend_http::cookie::{parse_cookie_header, CookieError};
///
/// assert_eq!(
///     parse_cookie_header("").unwrap_err(),
///     CookieError::InvalidCookiePair
/// );
/// ```
pub fn parse_cookie_header(value: &str) -> Result<Vec<(&str, &str)>, CookieError> {
    if value.len() > MAX_COOKIE_STRING_BYTES {
        return Err(CookieError::CookieStringTooLarge);
    }
    let segments: Vec<&str> = value.split(';').map(crate::request::trim_ows_str).collect();
    if segments.len() > MAX_COOKIE_COUNT {
        return Err(CookieError::TooManyCookies);
    }
    let mut pairs = Vec::with_capacity(segments.len());
    for segment in segments {
        pairs.push(parse_cookie_pair(segment)?);
    }
    Ok(pairs)
}

/// `SameSite` 属性（型付き指定、書き込み側）。
///
/// `None` は cookie 未設定ではなく RFC 6265bis の `SameSite=None`（クロス
/// サイト送出を許可）を表す型 variant であり、`Option<SameSite>` の `None`
/// （属性自体を出力しない）とは別概念であることに注意。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    /// `SameSite=Strict`。同一サイトのナビゲーションでのみ送出する最も厳格な設定。
    Strict,
    /// `SameSite=Lax`（多くのブラウザの既定相当）。トップレベルナビゲーションの
    /// GET では送出するが、大半のクロスサイトリクエストでは送出しない。
    Lax,
    /// `SameSite=None`。クロスサイトでも送出する。[`SetCookie::same_site`] は
    /// この variant 指定時に `Secure` を自動付与する（下記 doc 参照）。
    None,
}

impl SameSite {
    fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "Strict",
            Self::Lax => "Lax",
            Self::None => "None",
        }
    }
}

/// セッション Cookie 用の `SameSite`（`Lax` / `Strict` のみ、イシュー #827）。
///
/// `SameSite=None` を表す variant を持たないため、[`SetCookie::session_with`] の
/// 入口でクロスサイト送出を許可する設定を型として作れない。実行時に得た
/// [`SameSite`] からは `TryFrom` で変換でき、`None` は
/// [`CookieError::SessionSameSiteNone`] で拒否する。
///
/// ```
/// use fandhe_backend_http::cookie::{CookieError, SameSite, SessionSameSite};
///
/// assert_eq!(SessionSameSite::default(), SessionSameSite::Lax);
/// assert_eq!(
///     SessionSameSite::try_from(SameSite::None).unwrap_err(),
///     CookieError::SessionSameSiteNone
/// );
/// assert_eq!(
///     SessionSameSite::try_from(SameSite::Strict).unwrap(),
///     SessionSameSite::Strict
/// );
/// ```
///
/// `None` variant は存在しない:
///
/// ```compile_fail
/// use fandhe_backend_http::cookie::SessionSameSite;
/// let _ = SessionSameSite::None;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionSameSite {
    /// `SameSite=Lax`（既定）。
    #[default]
    Lax,
    /// `SameSite=Strict`。
    Strict,
}

impl From<SessionSameSite> for SameSite {
    fn from(v: SessionSameSite) -> Self {
        match v {
            SessionSameSite::Lax => SameSite::Lax,
            SessionSameSite::Strict => SameSite::Strict,
        }
    }
}

impl TryFrom<SameSite> for SessionSameSite {
    type Error = CookieError;

    fn try_from(v: SameSite) -> Result<Self, Self::Error> {
        match v {
            SameSite::Lax => Ok(Self::Lax),
            SameSite::Strict => Ok(Self::Strict),
            SameSite::None => Err(CookieError::SessionSameSiteNone),
        }
    }
}

/// 構築時検証済みの `Set-Cookie` 1 件（書き込み側、イシュー #303）。
///
/// フィールドは非公開で、唯一の構築経路が [`SetCookie::new`] であることにより
/// 「cookie-name は tchar のみ・cookie-value は cookie-octet のみ・Path は
/// path-value のみ」という不変条件を型レベルで保証する（[`crate::response::AllowedMethods`]
/// と同一パターン）。[`crate::response::Response::with_set_cookie`] へ渡すと、
/// 検証済みであることを前提に infallible にヘッダへ追加される。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetCookie {
    name: String,
    value: String,
    http_only: bool,
    secure: bool,
    same_site: Option<SameSite>,
    path: Option<String>,
    max_age: Option<i64>,
    /// 接頭辞付き名前なら `Some`。`Some` の値は接頭辞の条件を満たす不変条件を持つ。
    prefix: Option<CookiePrefix>,
}

/// cookie 名接頭辞（RFC 6265bis）。`SetCookie` 内部専用で、条件の強制対象を示す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CookiePrefix {
    Host,
    Secure,
}

const HOST_PREFIX: &[u8] = b"__Host-";
const SECURE_PREFIX: &[u8] = b"__Secure-";

/// 名前の接頭辞を ASCII 大文字小文字無視で判定する。バイト列比較のため
/// 文字境界での panic は起きない。
fn detect_prefix(name: &str) -> Option<CookiePrefix> {
    let b = name.as_bytes();
    if b.len() >= HOST_PREFIX.len() && b[..HOST_PREFIX.len()].eq_ignore_ascii_case(HOST_PREFIX) {
        Some(CookiePrefix::Host)
    } else if b.len() >= SECURE_PREFIX.len()
        && b[..SECURE_PREFIX.len()].eq_ignore_ascii_case(SECURE_PREFIX)
    {
        Some(CookiePrefix::Secure)
    } else {
        None
    }
}

/// RFC 6265 path-value: CTL（`0x00`–`0x1F`, `0x7F`）と `;`（`0x3B`）を除く
/// `%x20`–`0x7E`。空パスは許可しない（`Path` 属性を設定する以上、値は
/// 非空であるべきという保守的な制約。RFC 6265 自体は空 path-value を規定
/// していないため、フェイルクローズ側に倒す）。
fn is_path_value(b: u8) -> bool {
    b != b';' && (0x20..=0x7e).contains(&b)
}

impl SetCookie {
    /// `name` と `value` から検証済みの [`SetCookie`] を構築する。
    ///
    /// - `name`: 非空かつ全バイトが RFC 9110 tchar（RFC 6265 の
    ///   cookie-name = token と同一。`crate::request::is_tchar` を共有）。
    ///   違反は [`CookieError::InvalidName`]
    /// - `value`: 全バイトが RFC 6265 cookie-octet
    ///   （`is_cookie_octet` 参照）。空値は許可する（RFC 6265 上
    ///   cookie-value は空でありうる）。違反は [`CookieError::InvalidValue`]
    ///
    /// `__Host-` / `__Secure-` で始まる名前（ASCII 大文字小文字無視）は
    /// [`CookieError::ReservedPrefix`] で拒否する。接頭辞の条件を構築時に強制する
    /// [`SetCookie::host_prefixed`] / [`SetCookie::secure_prefixed`] を使うこと。
    ///
    /// 既定では `HttpOnly` / `Secure` / `SameSite` / `Path` / `Max-Age` の
    /// いずれも設定しない。**セッション ID 等のシークレットを載せる cookie
    /// には [`SetCookie::http_only`] で `HttpOnly` を付けることを強く推奨する**
    /// （XSS 経由の cookie 窃取防止、`.claude/rules/security.md` のシークレット
    /// 管理方針）。合わせて [`SetCookie::secure`]（平文送出防止）・
    /// [`SetCookie::same_site`]（CSRF 緩和）の付与も推奨する。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{CookieError, SetCookie};
    ///
    /// let cookie = SetCookie::new("session", "abc123").unwrap();
    /// assert_eq!(cookie.to_header_value(), "session=abc123");
    ///
    /// // `;` を含む値は cookie-octet 外のため構築段階で拒否される。
    /// assert_eq!(SetCookie::new("session", "a;b").unwrap_err(), CookieError::InvalidValue);
    ///
    /// // CRLF を含む名前も拒否される（tchar 外）。
    /// assert_eq!(
    ///     SetCookie::new("session\r\nX-Evil", "v").unwrap_err(),
    ///     CookieError::InvalidName
    /// );
    ///
    /// // 接頭辞付き名前は専用関数経由でのみ構築できる。
    /// assert_eq!(
    ///     SetCookie::new("__Host-sid", "v").unwrap_err(),
    ///     CookieError::ReservedPrefix
    /// );
    /// ```
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Result<Self, CookieError> {
        let name = name.into();
        let value = value.into();
        Self::validate_name_value(&name, &value)?;
        if detect_prefix(&name).is_some() {
            return Err(CookieError::ReservedPrefix);
        }
        Ok(Self::build(name, value, None))
    }

    /// 名前（tchar）→ 値（cookie-octet）の共通検証。全構築経路で共有し、
    /// 検証順序のずれ・バイパス経路を作らない。
    fn validate_name_value(name: &str, value: &str) -> Result<(), CookieError> {
        if name.is_empty() || !name.bytes().all(crate::request::is_tchar) {
            return Err(CookieError::InvalidName);
        }
        if !value.bytes().all(is_cookie_octet) {
            return Err(CookieError::InvalidValue);
        }
        Ok(())
    }

    fn build(name: String, value: String, prefix: Option<CookiePrefix>) -> Self {
        let (secure, path) = match prefix {
            Some(CookiePrefix::Host) => (true, Some("/".to_string())),
            Some(CookiePrefix::Secure) => (true, None),
            None => (false, None),
        };
        Self {
            name,
            value,
            http_only: false,
            secure,
            same_site: None,
            path,
            max_age: None,
            prefix,
        }
    }

    /// セッション Cookie を安全な既定値で構築する（イシュー #827）。
    ///
    /// `HttpOnly`・`Secure`・`SameSite=Lax`・`Path=/` を付与する。`Max-Age` は
    /// 付けない（ブラウザのセッション寿命。永続化するなら [`SetCookie::max_age`]）。
    /// 名前・値の検証は [`SetCookie::new`] と共通。`__Host-` / `__Secure-` 名も
    /// 受け付ける（既定値が接頭辞の条件を満たす。必須化はしない）。
    /// 発行後に [`SetCookie::path`] で `Path` を変えた場合は、削除用
    /// ([`SetCookie::session_removal`]) にも同じ `path` を付けること。
    /// TLS はリバースプロキシで終端する前提（`Secure` は外せない）。
    /// 既知の限界: 後から汎用の [`SetCookie::same_site`] で `SameSite::None` を
    /// 指定する明示的な opt-out は可能（型による排除は本関数の入口のみ）。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{CookieError, SetCookie};
    ///
    /// assert_eq!(
    ///     SetCookie::session("sid", "abc").unwrap().to_header_value(),
    ///     "sid=abc; Path=/; SameSite=Lax; Secure; HttpOnly"
    /// );
    /// // __Host- 名との組み合わせ（Path は '/' のみ許可）
    /// let c = SetCookie::session("__Host-sid", "abc").unwrap();
    /// assert_eq!(c.to_header_value(), "__Host-sid=abc; Path=/; SameSite=Lax; Secure; HttpOnly");
    /// assert_eq!(c.path("/api").unwrap_err(), CookieError::HostPrefixPath);
    /// assert_eq!(SetCookie::session("", "abc").unwrap_err(), CookieError::InvalidName);
    /// assert_eq!(SetCookie::session("sid", "a;b").unwrap_err(), CookieError::InvalidValue);
    /// ```
    pub fn session(name: impl Into<String>, value: impl Into<String>) -> Result<Self, CookieError> {
        Self::session_with(name, value, SessionSameSite::Lax)
    }

    /// [`SetCookie::session`] の `SameSite` 指定版。`SameSite=None` は型で排除される。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{SessionSameSite, SetCookie};
    ///
    /// assert_eq!(
    ///     SetCookie::session_with("sid", "abc", SessionSameSite::Strict)
    ///         .unwrap()
    ///         .to_header_value(),
    ///     "sid=abc; Path=/; SameSite=Strict; Secure; HttpOnly"
    /// );
    /// ```
    pub fn session_with(
        name: impl Into<String>,
        value: impl Into<String>,
        same_site: SessionSameSite,
    ) -> Result<Self, CookieError> {
        let name = name.into();
        let value = value.into();
        Self::validate_name_value(&name, &value)?;
        let prefix = detect_prefix(&name);
        let mut c = Self::build(name, value, prefix);
        c.http_only = true;
        c.secure = true;
        c.path = Some("/".to_string());
        c.same_site = Some(same_site.into());
        Ok(c)
    }

    /// [`SetCookie::session`] で発行した Cookie の削除用 `Set-Cookie` を構築する。
    ///
    /// 発行用と同じ属性に `Max-Age=0` を加え、値は空にする（RFC 6265 5.2.2:
    /// delta-seconds が 0 以下なら直ちに失効する）。発行用の構築関数に構造的に
    /// 依存するため、既定値が変わっても属性は食い違わない。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::SetCookie;
    ///
    /// assert_eq!(
    ///     SetCookie::session_removal("sid").unwrap().to_header_value(),
    ///     "sid=; Path=/; Max-Age=0; SameSite=Lax; Secure; HttpOnly"
    /// );
    /// ```
    pub fn session_removal(name: impl Into<String>) -> Result<Self, CookieError> {
        Self::session_removal_with(name, SessionSameSite::Lax)
    }

    /// [`SetCookie::session_removal`] の `SameSite` 指定版（発行時と同じ値を渡すこと）。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{SessionSameSite, SetCookie};
    ///
    /// assert_eq!(
    ///     SetCookie::session_removal_with("sid", SessionSameSite::Strict)
    ///         .unwrap()
    ///         .to_header_value(),
    ///     "sid=; Path=/; Max-Age=0; SameSite=Strict; Secure; HttpOnly"
    /// );
    /// ```
    pub fn session_removal_with(
        name: impl Into<String>,
        same_site: SessionSameSite,
    ) -> Result<Self, CookieError> {
        Self::session_with(name, "", same_site).map(|c| c.max_age(0))
    }

    /// `__Host-` 接頭辞付き cookie を構築する（`Secure` と `Path=/` を自動付与）。
    ///
    /// `name` は接頭辞を含む完全な名前（ASCII 大文字小文字無視で `__Host-` 一致、
    /// 名前は与えたまま保持）。名前・値の基本検証は [`SetCookie::new`] と同一で、
    /// 接頭辞が一致しなければ [`CookieError::PrefixMismatch`]。以後の
    /// [`SetCookie::path`] は `/` のみ許可される。`Domain` 属性は存在しないため
    /// 「Domain なし」の条件は型で満たされる。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{CookieError, SetCookie};
    ///
    /// let cookie = SetCookie::host_prefixed("__Host-sid", "abc").unwrap().http_only();
    /// assert_eq!(cookie.to_header_value(), "__Host-sid=abc; Path=/; Secure; HttpOnly");
    ///
    /// assert_eq!(
    ///     SetCookie::host_prefixed("sid", "abc").unwrap_err(),
    ///     CookieError::PrefixMismatch
    /// );
    /// ```
    pub fn host_prefixed(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, CookieError> {
        let name = name.into();
        let value = value.into();
        Self::validate_name_value(&name, &value)?;
        if detect_prefix(&name) != Some(CookiePrefix::Host) {
            return Err(CookieError::PrefixMismatch);
        }
        Ok(Self::build(name, value, Some(CookiePrefix::Host)))
    }

    /// `__Secure-` 接頭辞付き cookie を構築する（`Secure` を自動付与）。
    ///
    /// `name` は接頭辞を含む完全な名前（ASCII 大文字小文字無視で `__Secure-` 一致）。
    /// 一致しなければ [`CookieError::PrefixMismatch`]。`__Host-` 名は一致しないため、
    /// `__Host-` の追加条件（`Path=/`）を欠く cookie はこの関数でも作れない。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{CookieError, SetCookie};
    ///
    /// let cookie = SetCookie::secure_prefixed("__Secure-sid", "abc").unwrap();
    /// assert_eq!(cookie.to_header_value(), "__Secure-sid=abc; Secure");
    ///
    /// assert_eq!(
    ///     SetCookie::secure_prefixed("__Host-sid", "abc").unwrap_err(),
    ///     CookieError::PrefixMismatch
    /// );
    /// ```
    pub fn secure_prefixed(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, CookieError> {
        let name = name.into();
        let value = value.into();
        Self::validate_name_value(&name, &value)?;
        if detect_prefix(&name) != Some(CookiePrefix::Secure) {
            return Err(CookieError::PrefixMismatch);
        }
        Ok(Self::build(name, value, Some(CookiePrefix::Secure)))
    }

    /// `HttpOnly` 属性を付与する（JavaScript からの cookie 読み取りを禁止し、
    /// XSS 経由のセッション窃取を防ぐ。既定 off）。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::SetCookie;
    ///
    /// let cookie = SetCookie::new("session", "abc").unwrap().http_only();
    /// assert_eq!(cookie.to_header_value(), "session=abc; HttpOnly");
    /// ```
    #[must_use]
    pub fn http_only(mut self) -> Self {
        self.http_only = true;
        self
    }

    /// `Secure` 属性を付与する（HTTPS 接続でのみ送出させ平文送出を防ぐ。既定 off）。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::SetCookie;
    ///
    /// let cookie = SetCookie::new("session", "abc").unwrap().secure();
    /// assert_eq!(cookie.to_header_value(), "session=abc; Secure");
    /// ```
    #[must_use]
    pub fn secure(mut self) -> Self {
        self.secure = true;
        self
    }

    /// `SameSite` 属性を付与する（CSRF 緩和。既定は属性自体を出力しない）。
    ///
    /// `SameSite::None` を指定した場合は `Secure` を自動的に有効化する。
    /// RFC 6265bis・主要ブラウザは `SameSite=None` の cookie に `Secure` の
    /// 同時付与を要求する（付与しない cookie はブラウザ側で拒否・無視され
    /// うる）ため、呼び出し元が付け忘れても安全側の組み合わせに倒す決定的
    /// 挙動として実装する。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{SameSite, SetCookie};
    ///
    /// let cookie = SetCookie::new("session", "abc").unwrap().same_site(SameSite::Lax);
    /// assert_eq!(cookie.to_header_value(), "session=abc; SameSite=Lax");
    ///
    /// // SameSite=None は Secure を自動付与する。
    /// let cookie = SetCookie::new("session", "abc").unwrap().same_site(SameSite::None);
    /// assert_eq!(cookie.to_header_value(), "session=abc; SameSite=None; Secure");
    /// ```
    #[must_use]
    pub fn same_site(mut self, same_site: SameSite) -> Self {
        if same_site == SameSite::None {
            self.secure = true;
        }
        self.same_site = Some(same_site);
        self
    }

    /// `Path` 属性を設定する。
    ///
    /// `path` は RFC 6265 path-value（CTL と `;` を除く `%x20`–`%x7E`）で
    /// 検証し、加えて `/` で始まることを要求する。違反は
    /// [`CookieError::InvalidPath`]。
    ///
    /// `/` で始まらない値を文法上は path-value として許容してしまうと、
    /// RFC 6265 5.2.4 のクッキーパス抽出アルゴリズムによりユーザーエージェント
    /// がその `Path` 属性を無視してデフォルトパスへフォールバックする
    /// （ブラウザからは指定した `Path` が効いていないように見える不整合が
    /// 生じる）ため、構築時点でフェイルクローズに拒否する。
    ///
    /// `__Host-` cookie（[`SetCookie::host_prefixed`]）は `/` のみ許可し、
    /// それ以外は [`CookieError::HostPrefixPath`]（検証順は path-value 検証が先）。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{CookieError, SetCookie};
    ///
    /// let err = SetCookie::host_prefixed("__Host-sid", "v").unwrap().path("/api").unwrap_err();
    /// assert_eq!(err, CookieError::HostPrefixPath);
    ///
    /// let cookie = SetCookie::new("session", "abc").unwrap().path("/api").unwrap();
    /// assert_eq!(cookie.to_header_value(), "session=abc; Path=/api");
    ///
    /// // `;` を含む path は構築段階で拒否される。
    /// let err = SetCookie::new("session", "abc").unwrap().path("/a;b").unwrap_err();
    /// assert_eq!(err, CookieError::InvalidPath);
    ///
    /// // `/` で始まらない path はユーザーエージェント側で無視されうるため拒否される。
    /// let err = SetCookie::new("session", "abc").unwrap().path("api").unwrap_err();
    /// assert_eq!(err, CookieError::InvalidPath);
    /// ```
    pub fn path(mut self, path: impl Into<String>) -> Result<Self, CookieError> {
        let path = path.into();
        if path.is_empty() || !path.starts_with('/') || !path.bytes().all(is_path_value) {
            return Err(CookieError::InvalidPath);
        }
        if self.prefix == Some(CookiePrefix::Host) && path != "/" {
            return Err(CookieError::HostPrefixPath);
        }
        self.path = Some(path);
        Ok(self)
    }

    /// `Max-Age` 属性を秒単位で設定する。
    ///
    /// 負値は cookie の即時削除を指示する慣用手法として許可する（呼び出し元
    /// の意図的な用途）。`i64` を `to_string()` で直列化するため、出力に
    /// 数字と `-` 以外の文字が現れる経路は存在しない（インジェクション経路なし）。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::SetCookie;
    ///
    /// let cookie = SetCookie::new("session", "abc").unwrap().max_age(3600);
    /// assert_eq!(cookie.to_header_value(), "session=abc; Max-Age=3600");
    ///
    /// // 負値は削除用途として許可する。
    /// let cookie = SetCookie::new("session", "abc").unwrap().max_age(-1);
    /// assert_eq!(cookie.to_header_value(), "session=abc; Max-Age=-1");
    /// ```
    #[must_use]
    pub fn max_age(mut self, seconds: i64) -> Self {
        self.max_age = Some(seconds);
        self
    }

    /// `Set-Cookie` ヘッダ値として直列化する。
    ///
    /// `name=value` の後に設定済み属性を固定順（`Path` → `Max-Age` →
    /// `SameSite` → `Secure` → `HttpOnly`）で `"; "` 区切り結合する
    /// （決定的出力・テスト安定性のため、[`crate::response::AllowedMethods::to_header_value`]
    /// と同方針）。未設定の属性は出力しない。
    ///
    /// ```
    /// use fandhe_backend_http::cookie::{SameSite, SetCookie};
    ///
    /// let cookie = SetCookie::new("session", "abc")
    ///     .unwrap()
    ///     .path("/")
    ///     .unwrap()
    ///     .max_age(3600)
    ///     .same_site(SameSite::Lax)
    ///     .secure()
    ///     .http_only();
    /// assert_eq!(
    ///     cookie.to_header_value(),
    ///     "session=abc; Path=/; Max-Age=3600; SameSite=Lax; Secure; HttpOnly"
    /// );
    /// ```
    #[must_use]
    pub fn to_header_value(&self) -> String {
        let mut out = format!("{}={}", self.name, self.value);
        if let Some(path) = &self.path {
            out.push_str("; Path=");
            out.push_str(path);
        }
        if let Some(max_age) = self.max_age {
            out.push_str("; Max-Age=");
            out.push_str(&max_age.to_string());
        }
        if let Some(same_site) = self.same_site {
            out.push_str("; SameSite=");
            out.push_str(same_site.as_str());
        }
        if self.secure {
            out.push_str("; Secure");
        }
        if self.http_only {
            out.push_str("; HttpOnly");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_cookie_header（読み取り側） ---

    #[test]
    fn single_pair_is_parsed() {
        assert_eq!(parse_cookie_header("a=1").unwrap(), vec![("a", "1")]);
    }

    #[test]
    fn multiple_pairs_with_standard_separator_are_parsed() {
        assert_eq!(
            parse_cookie_header("a=1; b=2; c=3").unwrap(),
            vec![("a", "1"), ("b", "2"), ("c", "3")]
        );
    }

    #[test]
    fn semicolon_without_following_space_is_accepted() {
        // RFC 6265 §5.4 のサーバ側寛容化: `";"` 単独区切りも受理する。
        assert_eq!(
            parse_cookie_header("a=1;b=2").unwrap(),
            vec![("a", "1"), ("b", "2")]
        );
    }

    #[test]
    fn empty_value_is_valid() {
        assert_eq!(parse_cookie_header("k=").unwrap(), vec![("k", "")]);
    }

    #[test]
    fn quoted_value_has_quotes_stripped() {
        assert_eq!(
            parse_cookie_header(r#"a="hello-world""#).unwrap(),
            vec![("a", "hello-world")]
        );
    }

    #[test]
    fn space_inside_quoted_value_is_rejected() {
        // SP は DQUOTE で囲んでも cookie-octet ではないため構文違反。
        assert_eq!(
            parse_cookie_header(r#"a="hello world""#).unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn empty_quoted_value_is_valid() {
        assert_eq!(parse_cookie_header(r#"a="""#).unwrap(), vec![("a", "")]);
    }

    #[test]
    fn duplicate_names_are_all_returned_in_order() {
        assert_eq!(
            parse_cookie_header("a=1; a=2").unwrap(),
            vec![("a", "1"), ("a", "2")]
        );
    }

    #[test]
    fn percent_encoded_sequences_are_not_decoded() {
        assert_eq!(
            parse_cookie_header("q=a%20b").unwrap(),
            vec![("q", "a%20b")]
        );
    }

    #[test]
    fn missing_equals_is_rejected() {
        assert_eq!(
            parse_cookie_header("noequals").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn empty_name_is_rejected() {
        assert_eq!(
            parse_cookie_header("=v").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn tchar_violation_in_name_is_rejected() {
        assert_eq!(
            parse_cookie_header("a b=1").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn space_in_unquoted_value_is_rejected() {
        assert_eq!(
            parse_cookie_header("a=b c").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn double_quote_in_unquoted_value_is_rejected() {
        assert_eq!(
            parse_cookie_header(r#"a=b"c"#).unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn comma_in_value_is_rejected() {
        assert_eq!(
            parse_cookie_header("a=b,c").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn unicode_whitespace_around_pair_is_not_trimmed() {
        // NBSP（U+00A0）は HTTP OWS（SP/HTAB）ではないため trim してはならない。
        // `str::trim()` は NBSP を除去してしまうが、それは fail-closed 契約を
        // 弱める（本来 tchar 違反で `InvalidCookiePair` になるべき）。
        assert_eq!(
            parse_cookie_header("a=1;\u{a0}b=2").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn ascii_ows_around_pair_is_still_trimmed() {
        // SP/HTAB（HTTP OWS）は引き続き trim される（回帰防止）。
        assert_eq!(
            parse_cookie_header("a=1; \t b=2\t ").unwrap(),
            vec![("a", "1"), ("b", "2")]
        );
    }

    #[test]
    fn semicolon_in_value_is_rejected_as_separator_ambiguity() {
        // `;` は cookie-pair 区切りであり cookie-octet からも除外されるため、
        // `a=b;c` は 2 pair（`a=b`・`c`）に分解された上で後者が構文違反となる。
        assert_eq!(
            parse_cookie_header("a=b;c").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn backslash_in_value_is_rejected() {
        assert_eq!(
            parse_cookie_header(r"a=b\c").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn unterminated_quote_is_rejected() {
        assert_eq!(
            parse_cookie_header(r#"a="unterminated"#).unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn empty_pair_from_stray_semicolon_is_rejected() {
        assert_eq!(
            parse_cookie_header("a=1;;b=2").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn empty_string_is_rejected() {
        assert_eq!(
            parse_cookie_header("").unwrap_err(),
            CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn cookie_string_exactly_at_max_bytes_is_accepted() {
        // "a=" (2 bytes) の cookie-name/value 部を埋めて MAX_COOKIE_STRING_BYTES
        // ちょうどに合わせる。
        let value = "a".repeat(MAX_COOKIE_STRING_BYTES - 2);
        let cookie = format!("a={value}");
        assert_eq!(cookie.len(), MAX_COOKIE_STRING_BYTES);
        assert!(parse_cookie_header(&cookie).is_ok());
    }

    #[test]
    fn cookie_string_exceeding_max_bytes_is_rejected() {
        let value = "a".repeat(MAX_COOKIE_STRING_BYTES - 1);
        let cookie = format!("a={value}");
        assert_eq!(cookie.len(), MAX_COOKIE_STRING_BYTES + 1);
        assert_eq!(
            parse_cookie_header(&cookie).unwrap_err(),
            CookieError::CookieStringTooLarge
        );
    }

    #[test]
    fn pair_count_exactly_at_max_is_accepted() {
        let cookie = (0..MAX_COOKIE_COUNT)
            .map(|i| format!("k{i}=v"))
            .collect::<Vec<_>>()
            .join("; ");
        assert!(parse_cookie_header(&cookie).is_ok());
    }

    #[test]
    fn pair_count_exceeding_max_is_rejected() {
        let cookie = (0..=MAX_COOKIE_COUNT)
            .map(|i| format!("k{i}=v"))
            .collect::<Vec<_>>()
            .join("; ");
        assert_eq!(
            parse_cookie_header(&cookie).unwrap_err(),
            CookieError::TooManyCookies
        );
    }

    #[test]
    fn fail_closed_rejection_yields_no_partial_pairs_on_syntax_error() {
        // 先頭 pair が正常でも、後続 pair が構文違反なら結果全体を捨てる。
        assert!(parse_cookie_header("a=1; broken").is_err());
    }

    #[test]
    fn cookie_error_display_messages_are_stable() {
        assert_eq!(
            CookieError::CookieStringTooLarge.to_string(),
            "cookie string exceeds MAX_COOKIE_STRING_BYTES"
        );
        assert_eq!(
            CookieError::TooManyCookies.to_string(),
            "cookie pair count exceeds MAX_COOKIE_COUNT"
        );
        assert_eq!(
            CookieError::InvalidCookiePair.to_string(),
            "cookie pair violates RFC 6265 cookie-pair syntax"
        );
        assert!(CookieError::ReservedPrefix.to_string().contains("__Host-"));
        assert!(CookieError::PrefixMismatch.to_string().contains("接頭辞"));
        assert!(CookieError::HostPrefixPath.to_string().contains("'/'"));
        assert!(
            CookieError::SessionSameSiteNone
                .to_string()
                .contains("SameSite=None")
        );
    }

    // --- セッション Cookie（イシュー #827） ---

    #[test]
    fn session_emits_secure_defaults_without_max_age() {
        let v = SetCookie::session("sid", "abc").unwrap().to_header_value();
        assert_eq!(v, "sid=abc; Path=/; SameSite=Lax; Secure; HttpOnly");
        assert!(!v.contains("Max-Age"));
    }

    #[test]
    fn session_with_strict_emits_strict() {
        let v = SetCookie::session_with("sid", "abc", SessionSameSite::Strict)
            .unwrap()
            .to_header_value();
        assert_eq!(v, "sid=abc; Path=/; SameSite=Strict; Secure; HttpOnly");
    }

    #[test]
    fn session_same_site_rejects_none_via_try_from() {
        assert_eq!(
            SessionSameSite::try_from(SameSite::None).unwrap_err(),
            CookieError::SessionSameSiteNone
        );
        assert_eq!(
            SessionSameSite::try_from(SameSite::Lax),
            Ok(SessionSameSite::Lax)
        );
        assert_eq!(
            SessionSameSite::try_from(SameSite::Strict),
            Ok(SessionSameSite::Strict)
        );
    }

    #[test]
    fn session_removal_matches_session_attributes_with_max_age_zero() {
        for ss in [SessionSameSite::Lax, SessionSameSite::Strict] {
            let issued = SetCookie::session_with("sid", "abc", ss)
                .unwrap()
                .to_header_value();
            let removed = SetCookie::session_removal_with("sid", ss)
                .unwrap()
                .to_header_value();
            let expected = issued.replacen("sid=abc", "sid=", 1).replacen(
                "; Path=/",
                "; Path=/; Max-Age=0",
                1,
            );
            assert_eq!(removed, expected);
        }
    }

    #[test]
    fn session_validates_name_and_value() {
        assert_eq!(
            SetCookie::session("", "a").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::session("a b", "a").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::session("sid", "a;b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::session("sid", "a\r\nb").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::session_removal("").unwrap_err(),
            CookieError::InvalidName
        );
    }

    #[test]
    fn session_accepts_host_prefixed_name_and_keeps_host_constraints() {
        for n in ["__Host-sid", "__host-sid"] {
            let c = SetCookie::session(n, "abc").unwrap();
            assert_eq!(
                c.to_header_value(),
                format!("{n}=abc; Path=/; SameSite=Lax; Secure; HttpOnly")
            );
            assert_eq!(
                c.clone().path("/api").unwrap_err(),
                CookieError::HostPrefixPath
            );
            assert!(c.path("/").is_ok());
        }
    }

    #[test]
    fn session_accepts_secure_prefixed_name() {
        let c = SetCookie::session("__Secure-sid", "abc").unwrap();
        assert_eq!(
            c.to_header_value(),
            "__Secure-sid=abc; Path=/; SameSite=Lax; Secure; HttpOnly"
        );
        assert!(c.path("/api").is_ok());
    }

    #[test]
    fn session_removal_with_host_prefixed_name() {
        assert_eq!(
            SetCookie::session_removal("__Host-sid")
                .unwrap()
                .to_header_value(),
            "__Host-sid=; Path=/; Max-Age=0; SameSite=Lax; Secure; HttpOnly"
        );
    }

    // --- cookie 接頭辞（イシュー #810） ---

    #[test]
    fn new_rejects_prefixed_names_case_insensitively() {
        for n in [
            "__Host-a",
            "__Secure-a",
            "__host-a",
            "__HOST-a",
            "__secure-a",
            "__SECURE-a",
            "__hOsT-a",
            "__Host-",
            "__Secure-",
        ] {
            assert_eq!(
                SetCookie::new(n, "v").unwrap_err(),
                CookieError::ReservedPrefix,
                "{n}"
            );
        }
    }

    #[test]
    fn new_accepts_lookalike_names() {
        for n in [
            "_Host-a",
            "__Hostx",
            "x__Host-a",
            "__Host_a",
            "__Secure_a",
            "__Hos",
        ] {
            assert!(SetCookie::new(n, "v").is_ok(), "{n}");
        }
    }

    #[test]
    fn non_ascii_name_does_not_panic() {
        assert_eq!(
            SetCookie::new("__Hoé-a", "v").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::host_prefixed("é__Host-a", "v").unwrap_err(),
            CookieError::InvalidName
        );
    }

    #[test]
    fn host_prefixed_sets_secure_and_root_path() {
        let c = SetCookie::host_prefixed("__Host-sid", "abc").unwrap();
        assert_eq!(c.to_header_value(), "__Host-sid=abc; Path=/; Secure");
        let c = c.http_only().same_site(SameSite::Lax).max_age(60);
        assert_eq!(
            c.to_header_value(),
            "__Host-sid=abc; Path=/; Max-Age=60; SameSite=Lax; Secure; HttpOnly"
        );
        // 大文字小文字違いでも構築でき、名前は与えたまま出力される。
        let c = SetCookie::host_prefixed("__host-sid", "abc").unwrap();
        assert_eq!(c.to_header_value(), "__host-sid=abc; Path=/; Secure");
    }

    #[test]
    fn host_prefixed_path_only_root() {
        let c = SetCookie::host_prefixed("__Host-sid", "v").unwrap();
        assert!(c.clone().path("/").is_ok());
        assert_eq!(
            c.clone().path("/api").unwrap_err(),
            CookieError::HostPrefixPath
        );
        assert_eq!(
            c.clone().path("/a/").unwrap_err(),
            CookieError::HostPrefixPath
        );
        assert_eq!(c.path("/a;b").unwrap_err(), CookieError::InvalidPath);
    }

    #[test]
    fn prefixed_constructors_reject_mismatch() {
        for n in ["__Secure-x", "x", "__Host"] {
            assert_eq!(
                SetCookie::host_prefixed(n, "v").unwrap_err(),
                CookieError::PrefixMismatch
            );
        }
        for n in ["__Host-x", "x"] {
            assert_eq!(
                SetCookie::secure_prefixed(n, "v").unwrap_err(),
                CookieError::PrefixMismatch
            );
        }
    }

    #[test]
    fn secure_prefixed_always_secure_once() {
        let c = SetCookie::secure_prefixed("__Secure-sid", "abc").unwrap();
        assert_eq!(c.to_header_value(), "__Secure-sid=abc; Secure");
        let c = c.secure().same_site(SameSite::None);
        assert_eq!(
            c.to_header_value(),
            "__Secure-sid=abc; SameSite=None; Secure"
        );
        let c = SetCookie::secure_prefixed("__Secure-sid", "v")
            .unwrap()
            .path("/api")
            .unwrap();
        assert_eq!(c.to_header_value(), "__Secure-sid=v; Path=/api; Secure");
    }

    #[test]
    fn prefixed_constructors_validate_name_and_value() {
        assert_eq!(
            SetCookie::host_prefixed("", "v").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::host_prefixed("__Host-a", "a;b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::secure_prefixed("__Secure-a\r\n", "v").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::secure_prefixed("__Secure-a", "a,b").unwrap_err(),
            CookieError::InvalidValue
        );
    }

    #[test]
    fn cookie_error_implements_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<CookieError>();
    }

    // --- SetCookie::new 名前検証（書き込み側） ---

    #[test]
    fn new_rejects_empty_name() {
        assert_eq!(
            SetCookie::new("", "v").unwrap_err(),
            CookieError::InvalidName
        );
    }

    #[test]
    fn new_rejects_name_with_space_semicolon_equals_crlf_non_ascii() {
        assert_eq!(
            SetCookie::new("a b", "v").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::new("a;b", "v").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::new("a=b", "v").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::new("a\r\nb", "v").unwrap_err(),
            CookieError::InvalidName
        );
        assert_eq!(
            SetCookie::new("caf\u{e9}", "v").unwrap_err(),
            CookieError::InvalidName
        );
    }

    // --- SetCookie::new 値検証（書き込み側） ---

    #[test]
    fn new_allows_empty_value() {
        let cookie = SetCookie::new("session", "").unwrap();
        assert_eq!(cookie.to_header_value(), "session=");
    }

    #[test]
    fn new_rejects_value_with_space_dquote_comma_semicolon_backslash() {
        assert_eq!(
            SetCookie::new("session", "a b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a\"b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a,b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a;b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a\\b").unwrap_err(),
            CookieError::InvalidValue
        );
    }

    #[test]
    fn new_rejects_value_with_crlf_nul_control_and_high_bytes() {
        assert_eq!(
            SetCookie::new("session", "a\r\nb").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a\u{0}b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a\u{1}b").unwrap_err(),
            CookieError::InvalidValue
        );
        assert_eq!(
            SetCookie::new("session", "a\u{7f}b").unwrap_err(),
            CookieError::InvalidValue
        );
        // 0x80 以上のバイト（非 ASCII）は cookie-octet 外。
        assert_eq!(
            SetCookie::new("session", "caf\u{e9}").unwrap_err(),
            CookieError::InvalidValue
        );
    }

    #[test]
    fn new_accepts_cookie_octet_boundary_values() {
        // cookie-octet の両端点（0x21 '!' と 0x7e '~'）を受理する。
        assert!(SetCookie::new("session", "!").is_ok());
        assert!(SetCookie::new("session", "~").is_ok());
        // `=` (0x3d) は cookie-value としては許可される（base64 値の想定）。
        assert!(SetCookie::new("session", "a=b").is_ok());
    }

    // --- SetCookie::path 検証（書き込み側） ---

    #[test]
    fn path_rejects_empty_semicolon_crlf_and_control_chars() {
        assert_eq!(
            SetCookie::new("s", "v").unwrap().path("").unwrap_err(),
            CookieError::InvalidPath
        );
        assert_eq!(
            SetCookie::new("s", "v").unwrap().path("/a;b").unwrap_err(),
            CookieError::InvalidPath
        );
        assert_eq!(
            SetCookie::new("s", "v")
                .unwrap()
                .path("/a\r\nb")
                .unwrap_err(),
            CookieError::InvalidPath
        );
        assert_eq!(
            SetCookie::new("s", "v")
                .unwrap()
                .path("/a\u{0}b")
                .unwrap_err(),
            CookieError::InvalidPath
        );
    }

    #[test]
    fn path_accepts_root_and_nested_path() {
        assert!(SetCookie::new("s", "v").unwrap().path("/").is_ok());
        assert!(SetCookie::new("s", "v").unwrap().path("/api/v1").is_ok());
    }

    /// RFC 6265 5.2.4 のクッキーパス抽出アルゴリズムにより、`/` で始まらない
    /// `Path` 属性値はユーザーエージェント側でデフォルトパスへフォールバック
    /// され黙って無視される（Cursor Bugbot 指摘、PR #328）。呼び出し側が
    /// cookie スコープを絞ったつもりでも実際には絞られない不整合を防ぐため、
    /// 構築時点で拒否することを確認する。
    #[test]
    fn path_rejects_values_not_starting_with_slash() {
        assert_eq!(
            SetCookie::new("s", "v").unwrap().path("api").unwrap_err(),
            CookieError::InvalidPath
        );
        assert_eq!(
            SetCookie::new("s", "v")
                .unwrap()
                .path("api/v1")
                .unwrap_err(),
            CookieError::InvalidPath
        );
    }

    // --- SetCookie 属性直列化（書き込み側） ---

    #[test]
    fn to_header_value_omits_unset_attributes() {
        let cookie = SetCookie::new("session", "abc").unwrap();
        assert_eq!(cookie.to_header_value(), "session=abc");
    }

    #[test]
    fn to_header_value_uses_fixed_attribute_order() {
        let cookie = SetCookie::new("session", "abc")
            .unwrap()
            .http_only()
            .secure()
            .same_site(SameSite::Strict)
            .max_age(60)
            .path("/x")
            .unwrap();
        assert_eq!(
            cookie.to_header_value(),
            "session=abc; Path=/x; Max-Age=60; SameSite=Strict; Secure; HttpOnly"
        );
    }

    #[test]
    fn same_site_none_auto_enables_secure() {
        let cookie = SetCookie::new("session", "abc")
            .unwrap()
            .same_site(SameSite::None);
        assert_eq!(
            cookie.to_header_value(),
            "session=abc; SameSite=None; Secure"
        );
    }

    #[test]
    fn max_age_serializes_zero_and_negative_values() {
        let cookie = SetCookie::new("session", "abc").unwrap().max_age(0);
        assert_eq!(cookie.to_header_value(), "session=abc; Max-Age=0");

        let cookie = SetCookie::new("session", "abc").unwrap().max_age(-1);
        assert_eq!(cookie.to_header_value(), "session=abc; Max-Age=-1");
    }
}
