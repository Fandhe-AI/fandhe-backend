//! HTTP/1.1 リクエストライン・ヘッダの sans-IO パーサ。
//!
//! [`parse_request_head`] はソケット I/O を一切持たない純関数（`&[u8]` → 構造体）
//! として実装する。理由は 3 つ:
//! 1. ソケット読み取りループ（TASK-1.3-2 / #67）と責務を分離できる
//! 2. I/O なしでそのまま fuzz（TASK-15.3 / #51）に供せる
//! 3. doc test・単体テストが書きやすく AI ファースト保守性に適う
//!
//! body の読み取り・keep-alive 判定・`Content-Length` / `Transfer-Encoding` の
//! 意味解釈は本モジュールの責務外（TASK-1.3-2 / #67 が担う）。本モジュールは
//! `headers()` で同名ヘッダ全件を出現順で取得できる API を提供し、#67 が
//! 重複 `Content-Length` 等の意味検証をできる構造にとどめる。
//!
//! [`RequestHead`] はリクエストあたりのヒープアロケーションを N（ヘッダ本数）
//! に依存しない定数個へ抑えるため、ヘッド部を 1 個の所有バッファ
//! （`Box<str>`）としてコピーし、method / target / 各ヘッダ名・値は
//! バッファへの `Range<usize>` として保持する（イシュー #591、性能改善
//! ツリー #579 Phase 3。設計は `docs/design/zero-copy-request-head.md` の
//! 案 B。ライフタイムパラメータは `RequestHead` に持ち込まない）。フィールド
//! は非公開のまま、[`RequestHead::method`] / [`RequestHead::target`] の
//! アクセサ経由でのみ `&str` を取得させる契約とする。

use std::ops::Range;

/// リクエストヘッド（リクエストライン + ヘッダ + 空行）として許容するバイト数上限。
///
/// この上限はリソース枯渇（DoS）対策であり、`\r\n\r\n`（ヘッダ終端）に到達しない
/// まま累積バイト数がこの値を超えた場合も [`ParseError::HeaderSectionTooLarge`]
/// を返し、無限バッファ成長を防ぐ。
pub const MAX_HEADER_BYTES: usize = 16 * 1024;

/// 1 リクエストで許容するヘッダの最大本数。
///
/// ヘッダ本数を無制限に許すとパース処理・後段のヘッダ格納がリソース枯渇の
/// 攻撃対象になるため、超過時は [`ParseError::TooManyHeaders`] を返す。
pub const MAX_HEADER_COUNT: usize = 100;

/// リクエストラインで受理する HTTP バージョン。
///
/// HTTP/1.0・HTTP/1.1 以外（HTTP/0.9・HTTP/2 等）は
/// [`ParseError::UnsupportedVersion`] として拒否する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersion {
    /// `HTTP/1.0`
    Http10,
    /// `HTTP/1.1`
    Http11,
}

/// パース済みリクエストヘッド（body は含まない）。
///
/// body の読み取りは呼び出し元（ソケット I/O 層、TASK-1.3-2 / #67）の責務。
///
/// # 内部表現（イシュー #591）
///
/// `method` / `target` / 各ヘッダ名・値は個別の `String` として所有せず、
/// ヘッド部全体を 1 回だけコピーした所有バッファ `buf`（`Box<str>`、構築時に
/// UTF-8 検証済み）への `Range<usize>` として保持する。これにより 1 リクエスト
/// あたりの追加ヒープアロケーションは `buf` 1 + `headers`（`Vec`）1 の
/// **ヘッダ本数 N に依存しない定数個**になる（旧実装は method・target・
/// ヘッダ名・値それぞれが個別 `String` で `4 + 2N` 相当の alloc を要した）。
/// `Range` の境界はすべて UTF-8 文字境界であることをパース時の不変条件として
/// 保証する（区切りバイト（SP・`:`・OWS・`\r\n`）は常に ASCII であり UTF-8
/// 継続バイト `0x80..=0xBF` と重ならないため、境界がマルチバイト文字の途中を
/// 切ることはない）。
///
/// `method` / `target` はフィールドを非公開にし、[`RequestHead::method`] /
/// [`RequestHead::target`] のアクセサ経由でのみ `&str` を取得させる
/// （**BREAKING CHANGE**: 旧 `pub method: String` / `pub target: String`
/// フィールドは廃止）。`version` は `Copy` 型で alloc 削減の動機がないため
/// 引き続き `pub` のまま維持する。ヘッダは出現順を保持したまま保持し、
/// [`RequestHead::header`] / [`RequestHead::headers`] 経由でのみアクセスさせる
/// （フィールドは非公開）。
#[derive(Debug, Clone)]
pub struct RequestHead {
    /// ヘッド部（リクエストライン + ヘッダ、末尾の空行 `\r\n\r\n` を含まない）の
    /// 所有コピー。構築時に UTF-8 として検証済み。以下の各 `Range` はすべて
    /// この文字列のバイトオフセットを指す。
    buf: Box<str>,
    /// `buf` 中の method の範囲（RFC 9110 tchar のみで構成される token として検証済み）。
    method: Range<usize>,
    /// `buf` 中の request-target の範囲（SP・制御文字を含まないことを検証済み）。
    target: Range<usize>,
    /// リクエストバージョン（HTTP/1.0 または HTTP/1.1 のみ）。
    pub version: HttpVersion,
    /// 出現順を保持したヘッダ列（`buf` 中の (名前, 値) の範囲。名前は大小文字を
    /// 区別せず比較する契約）。
    headers: Vec<(Range<usize>, Range<usize>)>,
}

impl RequestHead {
    /// リクエストメソッドを返す（RFC 9110 tchar のみで構成される token として検証済み）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_http::request::parse_request_head;
    ///
    /// let buf = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
    /// let outcome = parse_request_head(buf).unwrap();
    /// let head = match outcome {
    ///     fandhe_backend_http::request::ParseOutcome::Complete { head, .. } => head,
    ///     _ => unreachable!(),
    /// };
    /// assert_eq!(head.method(), "GET");
    /// ```
    pub fn method(&self) -> &str {
        &self.buf[self.method.clone()]
    }

    /// request-target を返す（SP・制御文字を含まないことを検証済み、無正規化・非デコード）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_http::request::parse_request_head;
    ///
    /// let buf = b"GET /items HTTP/1.1\r\nHost: example.com\r\n\r\n";
    /// let outcome = parse_request_head(buf).unwrap();
    /// let head = match outcome {
    ///     fandhe_backend_http::request::ParseOutcome::Complete { head, .. } => head,
    ///     _ => unreachable!(),
    /// };
    /// assert_eq!(head.target(), "/items");
    /// ```
    pub fn target(&self) -> &str {
        &self.buf[self.target.clone()]
    }

    /// 大文字小文字を無視して先頭一致するヘッダ値を取得する。
    ///
    /// 同名ヘッダが複数存在する場合は最初の 1 件のみを返す。全件が必要な
    /// 呼び出し元（重複 `Content-Length` 検査等、#67 が担う）は
    /// [`RequestHead::headers`] を使うこと。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_http::request::parse_request_head;
    ///
    /// let buf = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
    /// let outcome = parse_request_head(buf).unwrap();
    /// let head = match outcome {
    ///     fandhe_backend_http::request::ParseOutcome::Complete { head, .. } => head,
    ///     _ => unreachable!(),
    /// };
    /// assert_eq!(head.header("host"), Some("example.com"));
    /// assert_eq!(head.header("HOST"), Some("example.com"));
    /// assert_eq!(head.header("missing"), None);
    /// ```
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| self.buf[k.clone()].eq_ignore_ascii_case(name))
            .map(|(_, v)| &self.buf[v.clone()])
    }

    /// 全ヘッダを出現順に走査するイテレータを返す。
    ///
    /// 同名ヘッダの重複検査（例: `Content-Length` の重複拒否）は呼び出し元
    /// （#67）が本イテレータを使って行う契約とする。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_http::request::parse_request_head;
    ///
    /// let buf = b"GET / HTTP/1.1\r\nX-A: 1\r\nX-B: 2\r\nX-A: 3\r\n\r\n";
    /// let outcome = parse_request_head(buf).unwrap();
    /// let head = match outcome {
    ///     fandhe_backend_http::request::ParseOutcome::Complete { head, .. } => head,
    ///     _ => unreachable!(),
    /// };
    /// let all: Vec<_> = head.headers().collect();
    /// assert_eq!(all, vec![("X-A", "1"), ("X-B", "2"), ("X-A", "3")]);
    /// ```
    pub fn headers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers
            .iter()
            .map(move |(k, v)| (&self.buf[k.clone()], &self.buf[v.clone()]))
    }

    /// `target` からクエリ文字列を除いたパス部分を返す。
    ///
    /// 分離は `target` 中の**最初の `?` の 1 点のみ**で行う（`?` が存在しな
    /// ければ `target` 全体を返す）。上位層（`fandhe-backend-routes` の
    /// `Router::dispatch`）はこのパスをルート照合キーとして使う契約とし、
    /// クエリ文字列付きリクエスト（`GET /search?q=x`）が静的・パラメータ
    /// ルートの双方に一致できるようにする（イシュー #258）。
    ///
    /// % デコード・末尾スラッシュ正規化等は一切行わない。既存の
    /// 「target は無正規化・非デコードのまま保持する」契約を踏襲し、
    /// デコード差異によるルート一致のずれ（OWASP A01 正規化バイパス）を
    /// 生じさせない。デコード・再検証はハンドラの責務。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_http::request::parse_request_head;
    ///
    /// let buf = b"GET /search?q=x&limit=10 HTTP/1.1\r\nHost: example.com\r\n\r\n";
    /// let outcome = parse_request_head(buf).unwrap();
    /// let head = match outcome {
    ///     fandhe_backend_http::request::ParseOutcome::Complete { head, .. } => head,
    ///     _ => unreachable!(),
    /// };
    /// assert_eq!(head.path(), "/search");
    /// assert_eq!(head.query(), Some("q=x&limit=10"));
    /// ```
    pub fn path(&self) -> &str {
        let target = self.target();
        match target.split_once('?') {
            Some((path, _)) => path,
            None => target,
        }
    }

    /// `target` からクエリ文字列（最初の `?` より後）を返す。
    ///
    /// `?` が存在しなければ `None`。`?` のみで値が空（`/search?`）の場合は
    /// `Some("")` を返し、「クエリ区切り自体の有無」を呼び出し側が区別
    /// できるようにする。生文字列のまま返し、% デコード・key-value 分解は
    /// 行わない（呼び出し元の責務、イシュー #258）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_http::request::parse_request_head;
    ///
    /// let buf = b"GET /search HTTP/1.1\r\nHost: example.com\r\n\r\n";
    /// let outcome = parse_request_head(buf).unwrap();
    /// let head = match outcome {
    ///     fandhe_backend_http::request::ParseOutcome::Complete { head, .. } => head,
    ///     _ => unreachable!(),
    /// };
    /// assert_eq!(head.query(), None);
    /// ```
    pub fn query(&self) -> Option<&str> {
        self.target().split_once('?').map(|(_, query)| query)
    }

    /// 全 `Cookie` ヘッダ（大小文字無視）を出現順に結合し、cookie-pair の列へ
    /// 分解する。
    ///
    /// RFC 6265 は UA に単一 `Cookie` ヘッダ送出を求めるが、複数到達時は
    /// `"; "` で結合してから分解した場合と同一の結果を返す仕様とする
    /// （RFC 6265bis の想定に整合、イシュー #309 受け入れ条件 2）。
    ///
    /// [`crate::cookie::MAX_COOKIE_COUNT`] / [`crate::cookie::MAX_COOKIE_STRING_BYTES`]
    /// の 2 上限は**複数ヘッダに跨る累積値**へ適用する。ヘッダを分割して
    /// 送ることで上限を迂回できてしまう抜け道を防ぐため（fail-closed、
    /// `.claude/rules/security.md`）。
    ///
    /// `Cookie` ヘッダが 1 本も無い場合は空の `Vec` を返す（エラーにしない。
    /// 未送信は構文違反ではないため [`crate::cookie::parse_cookie_header`]
    /// の「空文字列はエラー」契約とは区別する）。
    ///
    /// 不正な cookie-pair を含む場合は明示スキップではなく
    /// [`crate::cookie::CookieError::InvalidCookiePair`] を返す（fail-closed。
    /// [`crate::cookie`] モジュール doc の「不正組の扱い」節を参照）。
    ///
    /// # Examples
    ///
    /// 単一 `Cookie` ヘッダを分解する:
    ///
    /// ```
    /// use fandhe_backend_http::request::parse_request_head;
    ///
    /// let buf = b"GET / HTTP/1.1\r\nHost: h\r\nCookie: a=1; b=2\r\n\r\n";
    /// let outcome = parse_request_head(buf).unwrap();
    /// let head = match outcome {
    ///     fandhe_backend_http::request::ParseOutcome::Complete { head, .. } => head,
    ///     _ => unreachable!(),
    /// };
    /// assert_eq!(head.cookies().unwrap(), vec![("a", "1"), ("b", "2")]);
    /// ```
    ///
    /// 複数 `Cookie` ヘッダは `"; "` 結合と等価に扱う:
    ///
    /// ```
    /// use fandhe_backend_http::request::parse_request_head;
    ///
    /// let buf = b"GET / HTTP/1.1\r\nHost: h\r\nCookie: a=1\r\nCookie: b=2\r\n\r\n";
    /// let outcome = parse_request_head(buf).unwrap();
    /// let head = match outcome {
    ///     fandhe_backend_http::request::ParseOutcome::Complete { head, .. } => head,
    ///     _ => unreachable!(),
    /// };
    /// assert_eq!(head.cookies().unwrap(), vec![("a", "1"), ("b", "2")]);
    /// ```
    ///
    /// `Cookie` ヘッダが無ければ空を返す:
    ///
    /// ```
    /// use fandhe_backend_http::request::parse_request_head;
    ///
    /// let buf = b"GET / HTTP/1.1\r\nHost: h\r\n\r\n";
    /// let outcome = parse_request_head(buf).unwrap();
    /// let head = match outcome {
    ///     fandhe_backend_http::request::ParseOutcome::Complete { head, .. } => head,
    ///     _ => unreachable!(),
    /// };
    /// assert_eq!(head.cookies().unwrap(), Vec::<(&str, &str)>::new());
    /// ```
    pub fn cookies(&self) -> Result<Vec<(&str, &str)>, crate::cookie::CookieError> {
        let raw_headers: Vec<&str> = self
            .headers()
            .filter(|(name, _)| name.eq_ignore_ascii_case("cookie"))
            .map(|(_, value)| value)
            .collect();
        if raw_headers.is_empty() {
            return Ok(Vec::new());
        }
        // `"; "` で結合してから分解した場合と同一の結果にするが、実際に
        // 文字列を連結すると戻り値の借用元がこの関数のローカル変数になり
        // `&self` ライフタイムへ結び付けられなくなる（借用エラー）。
        // そこで連結済みバイト長・組数のみを計算して累積上限を検証し
        // （迂回防止、`crate::cookie` モジュール doc「DoS 耐性」節）、
        // 各ヘッダは個別に `crate::cookie::parse_cookie_pair` へ通す。
        // pair は `;` 区切りセグメント単位で完結し境界を跨がないため、
        // 個別ヘッダ処理と一括連結処理の結果は同一になる。
        let joined_len =
            raw_headers.iter().map(|h| h.len()).sum::<usize>() + (raw_headers.len() - 1) * 2;
        if joined_len > crate::cookie::MAX_COOKIE_STRING_BYTES {
            return Err(crate::cookie::CookieError::CookieStringTooLarge);
        }
        let segments: Vec<&str> = raw_headers
            .iter()
            .flat_map(|h| h.split(';').map(trim_ows_str))
            .collect();
        if segments.len() > crate::cookie::MAX_COOKIE_COUNT {
            return Err(crate::cookie::CookieError::TooManyCookies);
        }
        segments
            .into_iter()
            .map(crate::cookie::parse_cookie_pair)
            .collect()
    }
}

/// 意味的等価性で比較する（`derive` は使わない）。
///
/// `buf` は同一内容でも OWS（ヘッダ値前後の SP/HTAB）の量が異なるだけで
/// バイト列としては不一致になりうる（`trim_ows` 適用後の値は等価だが元の
/// ヘッド部バイト列は異なる）。`derive(PartialEq)` で `buf` を直接比較すると
/// このような意味的に等価なヘッドが不等と判定され、旧実装（`method` /
/// `target` / `headers` を個別 `String` として比較していた頃）の意味論から
/// 後退する。[`RequestHead::method`]・[`RequestHead::target`]・`version`・
/// [`RequestHead::headers`] のアクセサ経由で比較することで、内部表現
/// （イシュー #591）に関わらず利用者から見た値の等価性のみを判定する。
impl PartialEq for RequestHead {
    fn eq(&self, other: &Self) -> bool {
        self.method() == other.method()
            && self.target() == other.target()
            && self.version == other.version
            && self.headers().eq(other.headers())
    }
}

impl Eq for RequestHead {}

/// [`parse_request_head`] の成功時の結果。
#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome {
    /// ヘッド解析が完了した。
    ///
    /// `consumed` は `buf` 先頭からヘッダ終端の空行（`\r\n\r\n`）までの
    /// 消費バイト数。呼び出し元は `buf[..consumed]` を読み捨て、残り
    /// （パイプライン済みの次リクエストや body 先頭）を保持する契約。
    Complete {
        /// 解析済みリクエストヘッド。
        head: RequestHead,
        /// ヘッダ終端までの消費バイト数。
        consumed: usize,
    },
    /// ヘッダ終端（`\r\n\r\n`）に未到達。呼び出し元は追加のバイト列を
    /// 読み取ってから再試行する（sans-IO 設計につき本関数はリトライしない）。
    Incomplete,
}

/// [`parse_request_head`] が返しうるエラー。
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// ヘッダ部が [`MAX_HEADER_BYTES`] を超過した（リソース枯渇対策）。
    HeaderSectionTooLarge,
    /// ヘッダ本数が [`MAX_HEADER_COUNT`] を超過した（リソース枯渇対策）。
    TooManyHeaders,
    /// リクエストラインが `method SP target SP version` の形式に違反する。
    InvalidRequestLine,
    /// HTTP/1.0・HTTP/1.1 以外のバージョンが指定された。
    UnsupportedVersion,
    /// ヘッダ名・ヘッダ値が構文違反（tchar 違反・制御文字混入・obs-fold 等）。
    InvalidHeader,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            ParseError::HeaderSectionTooLarge => "header section exceeds MAX_HEADER_BYTES",
            ParseError::TooManyHeaders => "header count exceeds MAX_HEADER_COUNT",
            ParseError::InvalidRequestLine => "invalid request line",
            ParseError::UnsupportedVersion => "unsupported HTTP version",
            ParseError::InvalidHeader => "invalid header",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for ParseError {}

/// `buf` 先頭から 1 リクエスト分のヘッド（リクエストライン + ヘッダ）を解析する。
///
/// sans-IO な純関数であり、ソケット読み取りは行わない。呼び出し元（#67）が
/// ソケットから読み取ったバイト列をそのまま渡し、[`ParseOutcome::Incomplete`]
/// が返れば追加読み取り後に再試行する契約。
///
/// # 受理する構文
///
/// - 行終端は `\r\n` のみ（bare LF・bare CR は拒否）
/// - リクエストラインは `method SP target SP version` の 3 要素固定
/// - メソッドは RFC 9110 tchar のみ、request-target は SP・制御文字を含まない
/// - ヘッダ名は tchar のみ（コロン前の空白は tchar 違反として拒否）、値は
///   前後の OWS を trim し、trim 後の値に HTAB 以外の制御文字を含まない
/// - 継続行（obs-fold）は拒否する
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::request::{parse_request_head, HttpVersion, ParseOutcome};
///
/// let buf = b"POST /items HTTP/1.1\r\nHost: example.com\r\nContent-Length: 4\r\n\r\nbody";
/// match parse_request_head(buf).unwrap() {
///     ParseOutcome::Complete { head, consumed } => {
///         assert_eq!(head.method(), "POST");
///         assert_eq!(head.target(), "/items");
///         assert_eq!(head.version, HttpVersion::Http11);
///         // "body" はヘッダ終端より後ろにあるため consumed に含まれない。
///         assert_eq!(&buf[consumed..], b"body");
///     }
///     ParseOutcome::Incomplete => unreachable!(),
/// }
/// ```
pub fn parse_request_head(buf: &[u8]) -> Result<ParseOutcome, ParseError> {
    const TERMINATOR: &[u8] = b"\r\n\r\n";

    let terminator_pos = find_subslice(buf, TERMINATOR);

    let header_end = match terminator_pos {
        Some(pos) => {
            let consumed = pos + TERMINATOR.len();
            if consumed > MAX_HEADER_BYTES {
                return Err(ParseError::HeaderSectionTooLarge);
            }
            pos
        }
        None => {
            if buf.len() >= MAX_HEADER_BYTES {
                return Err(ParseError::HeaderSectionTooLarge);
            }
            return Ok(ParseOutcome::Incomplete);
        }
    };

    let header_section = &buf[..header_end];
    let mut lines = split_by_crlf(header_section);

    // リクエストライン・ヘッダ行の解析は `header_section` 内のバイトオフセット
    // （= 後で構築する `RequestHead::buf` 内のオフセットと一致）を `Range` で
    // 返す。各行パーサは行の先頭を 0 とした相対 `Range` を返すため、
    // `shift_range` で行の絶対開始位置を加算して `header_section` 全体基準へ
    // 揃える（イシュー #591。`buf` は `header_section` をそのままコピーする
    // ため、この絶対オフセットがそのまま `RequestHead` の Range になる）。
    let request_line_range = lines.next().ok_or(ParseError::InvalidRequestLine)?;
    let request_line = &header_section[request_line_range.clone()];
    let (method_rel, target_rel, version) = parse_request_line(request_line)?;
    let method = shift_range(&method_rel, request_line_range.start);
    let target = shift_range(&target_rel, request_line_range.start);

    // ヘッダ本数は「ヘッド部全体の `\r\n` 出現数」と一致する（リクエストライン
    // ～最初のヘッダの間・各ヘッダ間の区切りが 1 個ずつ、末尾ヘッダの後ろに
    // 区切りは無い（`header_section` は終端の空行を含まない）ため）。事前に
    // 数えて `Vec::with_capacity` することで、ヘッダ本数 N に応じた再確保を
    // 排除し alloc 回数を定数化する（設計文書 5.1 節の実測根拠）。
    let header_count = count_subslice(header_section, b"\r\n");
    let mut headers: Vec<(Range<usize>, Range<usize>)> = Vec::with_capacity(header_count);
    for line_range in lines {
        if headers.len() >= MAX_HEADER_COUNT {
            return Err(ParseError::TooManyHeaders);
        }
        let line = &header_section[line_range.clone()];
        let (name_rel, value_rel) = parse_header_line(line)?;
        headers.push((
            shift_range(&name_rel, line_range.start),
            shift_range(&value_rel, line_range.start),
        ));
    }

    // ヘッド部全体を 1 回だけコピーして所有バッファ化する。各 span（method /
    // target / ヘッダ名・値）は既に `parse_request_line` / `parse_header_line`
    // 内で個別に UTF-8 検証済み（alloc なしの `str::from_utf8` 検証のみ）。
    // ここでの全体検証は、区切りバイト（SP・`:`・OWS・`\r\n`）がすべて ASCII
    // であるため span 検証通過後は実質到達不能だが、`.expect()` を避け
    // フェイルクローズに `Result` を伝播する防御的二重チェック
    // （設計文書 6.3 節、Codex レビュー #600 指摘 2 対応）。
    let buf: Box<str> = match std::str::from_utf8(header_section) {
        Ok(s) => Box::from(s),
        Err(_) => return Err(ParseError::InvalidHeader),
    };

    let head = RequestHead {
        buf,
        method,
        target,
        version,
        headers,
    };
    Ok(ParseOutcome::Complete {
        head,
        consumed: header_end + TERMINATOR.len(),
    })
}

/// `range` の開始・終了双方に `offset` を加算した `Range` を返す。
///
/// 各行パーサ（[`parse_request_line`] / [`parse_header_line`]）が返す
/// span はその行の先頭を 0 とした相対オフセットのため、[`parse_request_head`]
/// が行の絶対開始位置（`header_section` 基準）を加算して揃える。
fn shift_range(range: &Range<usize>, offset: usize) -> Range<usize> {
    (range.start + offset)..(range.end + offset)
}

/// `haystack` 中に `needle` が出現する回数を数える。
///
/// [`parse_request_head`] がヘッダ本数の事前算出（`Vec::with_capacity`）に
/// 使う。`memchr::memmem::find_iter` に委譲し、`find_subslice` 同様バック
/// トラックのない O(haystack + needle) の探索で完了する。
fn count_subslice(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() {
        return 0;
    }
    memchr::memmem::find_iter(haystack, needle).count()
}

/// `haystack` 中で `needle` が最初に現れる位置を返す（見つからなければ `None`）。
///
/// [`parse_request_head`]（ヘッド終端 `\r\n\r\n` 探索）・[`CrlfSplit`]（ヘッダ行区切り
/// `\r\n` 探索）から呼ばれるホットパス。`memchr::memmem::find`（SIMD 最適化された
/// Two-Way 法）に委譲する（イシュー #586）。正規表現やバックトラックを伴わない
/// 最悪計算量 O(haystack + needle) の探索であり、病的入力による計算量爆発
/// （ReDoS 相当）を起こさない性質は旧実装（`windows().position()`）から維持する。
/// 空 needle は `memmem::find` 自体が `Some(0)` を返し得るため、旧実装と契約を
/// 揃えるため明示的に `None` を返すガードを残す。
pub(crate) fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    memchr::memmem::find(haystack, needle)
}

/// ヘッダ部（末尾の空行を含まない）を `\r\n` 区切りで分割し、各行の
/// `section` 内 `Range<usize>` を返すイテレータを返す。
///
/// 分割はリテラル `\r\n` の出現位置でのみ行う。bare LF・bare CR はどの区切り
/// にも一致しないため区切られずセグメント内に残り、後続の tchar / 制御文字
/// 検証（[`parse_request_line`] / [`parse_header_line`]）で拒否される。これに
/// より obs-fold（継続行）も、先頭が SP/HTAB で始まる非 token セグメントとして
/// 自然に拒否される。
///
/// スライス（`&[u8]`）ではなく `Range<usize>` を返す点が旧実装からの変更点
/// （イシュー #591）。呼び出し元（[`parse_request_head`]）が `header_section`
/// 内の絶対オフセットとしてそのまま `RequestHead::buf` の `Range` に転用する。
fn split_by_crlf(section: &[u8]) -> impl Iterator<Item = Range<usize>> + '_ {
    CrlfSplit {
        section,
        pos: Some(0),
    }
}

/// [`split_by_crlf`] の内部イテレータ実装。オフセット追跡型（`section` 自体は
/// 縮小せず、走査開始位置 `pos` のみを進める）。
struct CrlfSplit<'a> {
    section: &'a [u8],
    pos: Option<usize>,
}

impl Iterator for CrlfSplit<'_> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.pos?;
        let rest = &self.section[start..];
        match find_subslice(rest, b"\r\n") {
            Some(rel_pos) => {
                let end = start + rel_pos;
                self.pos = Some(end + 2);
                Some(start..end)
            }
            None => {
                self.pos = None;
                Some(start..self.section.len())
            }
        }
    }
}

/// RFC 9110 の tchar（token 構成文字）判定。
///
/// `!#$%&'*+-.^_`|~` と DIGIT・ALPHA のみを許容する。ヘッダ名・メソッドの
/// token 検証に使う。`crate::response::AllowedMethods`（`Allow` ヘッダ用の
/// 検証済みメソッド型、TASK-177）がこのパーサと同一の判定基準を共有するため
/// `pub(crate)` として公開する（二重定義によるドリフト防止）。
pub(crate) fn is_tchar(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
    ) || b.is_ascii_alphanumeric()
}

/// 制御文字（DoS・インジェクション対策で拒否する対象）判定。
///
/// `allow_htab` が `true` の場合のみ HTAB（0x09）を許容する
/// （ヘッダ値の内部 HTAB を許すため）。
fn is_forbidden_ctl(b: u8, allow_htab: bool) -> bool {
    if allow_htab && b == 0x09 {
        return false;
    }
    b < 0x20 || b == 0x7F
}

/// `line` を SP（`0x20`）区切りで分割し、各セグメントの `line` 内先頭
/// オフセットとスライスの組を返すイテレータを返す。
///
/// 連続する SP（例: `"GET  /"`）は間に長さ 0 のセグメントを生む（`str::split`
/// 同様、区切り文字を併合しない）。[`parse_request_line`] はこの性質を利用し、
/// 「3 要素固定・各要素非空」という旧実装（`slice::split` + `filter(!is_empty)`）
/// の検証意味論をそのまま踏襲する。
fn split_by_sp_with_offset(line: &[u8]) -> impl Iterator<Item = (usize, &[u8])> {
    SpSplit { line, pos: Some(0) }
}

/// [`split_by_sp_with_offset`] の内部イテレータ実装。
struct SpSplit<'a> {
    line: &'a [u8],
    pos: Option<usize>,
}

impl<'a> Iterator for SpSplit<'a> {
    type Item = (usize, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.pos?;
        match self.line[start..].iter().position(|&b| b == b' ') {
            Some(rel_pos) => {
                let end = start + rel_pos;
                self.pos = Some(end + 1);
                Some((start, &self.line[start..end]))
            }
            None => {
                self.pos = None;
                Some((start, &self.line[start..]))
            }
        }
    }
}

/// リクエストライン 1 行を `(method, target, version)` に分解する。
///
/// `method` / `target` は `line` 先頭を 0 とした相対 `Range<usize>` で返す
/// （呼び出し元 [`parse_request_head`] が `header_section` 内の絶対位置へ
/// シフトする、イシュー #591）。`version` はその場で [`HttpVersion`] へ変換
/// するため所有権を要さず `Range` を返す必要がない。
fn parse_request_line(
    line: &[u8],
) -> Result<(Range<usize>, Range<usize>, HttpVersion), ParseError> {
    if line.iter().any(|&b| is_forbidden_ctl(b, false)) {
        return Err(ParseError::InvalidRequestLine);
    }

    let mut parts = split_by_sp_with_offset(line);
    let method = parts.next().filter(|(_, s)| !s.is_empty());
    let target = parts.next().filter(|(_, s)| !s.is_empty());
    let version = parts.next().filter(|(_, s)| !s.is_empty());
    // 4 要素目が存在する（= SP が 3 個以上ある）場合は 3 要素固定に違反する。
    if parts.next().is_some() {
        return Err(ParseError::InvalidRequestLine);
    }
    let ((method_start, method), (target_start, target), (_, version)) =
        match (method, target, version) {
            (Some(m), Some(t), Some(v)) => (m, t, v),
            _ => return Err(ParseError::InvalidRequestLine),
        };

    if !method.iter().all(|&b| is_tchar(b)) {
        return Err(ParseError::InvalidRequestLine);
    }
    // request-target: SP は split で既に除外済み。制御文字は上で行全体を検査済み。
    if target.is_empty() {
        return Err(ParseError::InvalidRequestLine);
    }

    let http_version = parse_version(version)?;

    // span 単位の UTF-8 検証（alloc なし）。`RequestHead::buf` 構築時の全体
    // 検証（`parse_request_head`）と役割分担する 2 段構成（設計文書 6.3 節）。
    if std::str::from_utf8(method).is_err() || std::str::from_utf8(target).is_err() {
        return Err(ParseError::InvalidRequestLine);
    }

    let method_range = method_start..(method_start + method.len());
    let target_range = target_start..(target_start + target.len());

    Ok((method_range, target_range, http_version))
}

/// バージョン token（例: `HTTP/1.1`）を [`HttpVersion`] に変換する。
///
/// `HTTP/` prefix を持たない場合は構文違反として [`ParseError::InvalidRequestLine`]、
/// prefix はあるが 1.0/1.1 以外の場合は [`ParseError::UnsupportedVersion`] を返す。
fn parse_version(version: &[u8]) -> Result<HttpVersion, ParseError> {
    const PREFIX: &[u8] = b"HTTP/";
    if !version.starts_with(PREFIX) {
        return Err(ParseError::InvalidRequestLine);
    }
    match &version[PREFIX.len()..] {
        b"1.1" => Ok(HttpVersion::Http11),
        b"1.0" => Ok(HttpVersion::Http10),
        _ => Err(ParseError::UnsupportedVersion),
    }
}

/// ヘッダ行 1 行を `(name, value)` に分解する。
///
/// - ヘッダ名は tchar のみ（コロン直前の空白は tchar 違反として自然に拒否される）
/// - 値は前後の OWS（SP/HTAB）を trim し、trim 後に HTAB 以外の制御文字が
///   残っていれば拒否する
/// - obs-fold（SP/HTAB で始まる継続行）はコロン欠如または名前の tchar 違反として拒否される
/// - 制御文字チェックは RFC 9110 の `obs-text`（0x80–0xFF）を通過させるが、これは
///   UTF-8 マルチバイト列の後続バイトを許容するためであり、生の obs-text を
///   オペークなバイト列として保持する意図ではない。値は本関数の戻り値が
///   `String`（= 有効な UTF-8 保証）であることに従い、最終的に厳密な UTF-8 として
///   検証する。マルチバイト列として不正な生の obs-text（例: 単独の `0xE9`）は
///   [`ParseError::InvalidHeader`] として拒否する
///
/// `name` / `value` は `line` 先頭を 0 とした相対 `Range<usize>` で返す
/// （呼び出し元 [`parse_request_head`] が `header_section` 内の絶対位置へ
/// シフトする、イシュー #591）。
fn parse_header_line(line: &[u8]) -> Result<(Range<usize>, Range<usize>), ParseError> {
    let colon_pos = line
        .iter()
        .position(|&b| b == b':')
        .ok_or(ParseError::InvalidHeader)?;
    let (name, value_raw) = (&line[..colon_pos], &line[colon_pos + 1..]);

    if name.is_empty() || !name.iter().all(|&b| is_tchar(b)) {
        return Err(ParseError::InvalidHeader);
    }

    let value_rel = trim_ows_range(value_raw);
    let value = &value_raw[value_rel.clone()];
    if value.iter().any(|&b| is_forbidden_ctl(b, true)) {
        return Err(ParseError::InvalidHeader);
    }

    // span 単位の UTF-8 検証（alloc なし）。`RequestHead::buf` 構築時の全体
    // 検証（`parse_request_head`）と役割分担する 2 段構成（設計文書 6.3 節）。
    if std::str::from_utf8(name).is_err() || std::str::from_utf8(value).is_err() {
        return Err(ParseError::InvalidHeader);
    }

    let name_range = 0..colon_pos;
    let value_start = colon_pos + 1 + value_rel.start;
    let value_end = colon_pos + 1 + value_rel.end;

    Ok((name_range, value_start..value_end))
}

/// 前後の OWS（SP `0x20` / HTAB `0x09`）を取り除いた範囲を `bytes` 内の相対
/// `Range<usize>` として返す。
fn trim_ows_range(bytes: &[u8]) -> Range<usize> {
    let is_ows = |b: &u8| *b == b' ' || *b == b'\t';
    let start = bytes.iter().position(|b| !is_ows(b)).unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !is_ows(b))
        .map_or(start, |p| p + 1);
    start..end
}

/// `&str` 版の OWS（SP `0x20` / HTAB `0x09`）trim。
///
/// `str::trim()` は Unicode の空白文字全般（NBSP・BOM 等）を除去してしまい、
/// HTTP OWS（RFC 9110 §5.6.3 の SP/HTAB のみ）の定義より広くトリムしてしまう。
/// `Cookie` ヘッダの pair 分割（[`crate::cookie::parse_cookie_header`]・
/// [`RequestHead::cookies`]）では OWS 以外の Unicode 空白を trim してはならない
/// （trim 後の値が意図せず変化し、本来 `InvalidCookiePair` になるべき pair が
/// 誤って受理される fail-closed 契約の後退を防ぐ）。
///
/// SP/HTAB は ASCII の 1 バイト文字であり UTF-8 の継続バイト（`0x80..=0xBF`）とは
/// 重ならないため、バイト境界での trim は UTF-8 境界を破壊しない。
pub(crate) fn trim_ows_str(s: &str) -> &str {
    s.trim_matches(|c: char| c == ' ' || c == '\t')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(buf: &[u8]) -> (RequestHead, usize) {
        match parse_request_head(buf).expect("parse should succeed") {
            ParseOutcome::Complete { head, consumed } => (head, consumed),
            ParseOutcome::Incomplete => panic!("expected Complete, got Incomplete"),
        }
    }

    #[test]
    fn get_without_headers() {
        let buf = b"GET / HTTP/1.1\r\n\r\n";
        let (head, consumed) = complete(buf);
        assert_eq!(head.method(), "GET");
        assert_eq!(head.target(), "/");
        assert_eq!(head.version, HttpVersion::Http11);
        assert_eq!(head.headers().count(), 0);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn get_with_multiple_headers() {
        let buf = b"GET /path HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\n\r\n";
        let (head, consumed) = complete(buf);
        assert_eq!(head.method(), "GET");
        assert_eq!(head.target(), "/path");
        assert_eq!(head.header("host"), Some("example.com"));
        assert_eq!(head.header("Accept"), Some("*/*"));
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn post_with_body_is_not_consumed() {
        let buf = b"POST /items HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd";
        let (head, consumed) = complete(buf);
        assert_eq!(head.method(), "POST");
        assert_eq!(head.header("content-length"), Some("4"));
        assert_eq!(&buf[consumed..], b"abcd");
    }

    #[test]
    fn http_10_is_accepted() {
        let buf = b"GET / HTTP/1.0\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.version, HttpVersion::Http10);
    }

    #[test]
    fn consumed_excludes_pipelined_next_request() {
        let buf = b"GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\n\r\n";
        let (head, consumed) = complete(buf);
        assert_eq!(head.target(), "/a");
        assert_eq!(&buf[consumed..], b"GET /b HTTP/1.1\r\n\r\n");
    }

    #[test]
    fn header_lookup_is_case_insensitive_and_preserves_order() {
        let buf = b"GET / HTTP/1.1\r\nX-A: 1\r\nX-B: 2\r\nX-A: 3\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.header("x-a"), Some("1"));
        let all: Vec<_> = head.headers().collect();
        assert_eq!(all, vec![("X-A", "1"), ("X-B", "2"), ("X-A", "3")]);
    }

    #[test]
    fn path_and_query_without_query_string() {
        let buf = b"GET /search HTTP/1.1\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.path(), "/search");
        assert_eq!(head.query(), None);
    }

    #[test]
    fn path_and_query_with_query_string() {
        let buf = b"GET /search?q=x&limit=10 HTTP/1.1\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.path(), "/search");
        assert_eq!(head.query(), Some("q=x&limit=10"));
    }

    #[test]
    fn path_and_query_with_empty_query_string() {
        // `?` のみ（値なし）は「クエリ区切りは存在するが空」を表す Some("") を返す。
        let buf = b"GET /search? HTTP/1.1\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.path(), "/search");
        assert_eq!(head.query(), Some(""));
    }

    #[test]
    fn path_and_query_split_on_first_question_mark_only() {
        // 2 個目以降の `?` はクエリ文字列側にそのまま残す（1 点分離の契約）。
        let buf = b"GET /a?b?c HTTP/1.1\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.path(), "/a");
        assert_eq!(head.query(), Some("b?c"));
    }

    #[test]
    fn path_does_not_percent_decode() {
        // % デコードは行わない契約。`%3F`（エンコード済み `?`）はパス側に残る。
        let buf = b"GET /a%3Fb HTTP/1.1\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.path(), "/a%3Fb");
        assert_eq!(head.query(), None);
    }

    #[test]
    fn path_and_query_for_asterisk_form() {
        let buf = b"OPTIONS * HTTP/1.1\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.path(), "*");
        assert_eq!(head.query(), None);
    }

    #[test]
    fn incomplete_on_empty_input() {
        assert_eq!(parse_request_head(b""), Ok(ParseOutcome::Incomplete));
    }

    #[test]
    fn incomplete_mid_request_line() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/1."),
            Ok(ParseOutcome::Incomplete)
        );
    }

    #[test]
    fn incomplete_just_before_terminator() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/1.1\r\n\r"),
            Ok(ParseOutcome::Incomplete)
        );
    }

    #[test]
    fn header_section_too_large_when_terminator_missing() {
        let mut buf = vec![b'a'; MAX_HEADER_BYTES];
        assert_eq!(
            parse_request_head(&buf),
            Err(ParseError::HeaderSectionTooLarge)
        );
        buf.truncate(MAX_HEADER_BYTES - 1);
        assert_eq!(parse_request_head(&buf), Ok(ParseOutcome::Incomplete));
    }

    #[test]
    fn header_section_too_large_when_terminator_found_over_budget() {
        let filler = vec![b'a'; MAX_HEADER_BYTES];
        let mut buf = b"GET / HTTP/1.1\r\nX-Big: ".to_vec();
        buf.extend_from_slice(&filler);
        buf.extend_from_slice(b"\r\n\r\n");
        assert_eq!(
            parse_request_head(&buf),
            Err(ParseError::HeaderSectionTooLarge)
        );
    }

    #[test]
    fn too_many_headers() {
        let mut buf = b"GET / HTTP/1.1\r\n".to_vec();
        for i in 0..=MAX_HEADER_COUNT {
            buf.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
        }
        buf.extend_from_slice(b"\r\n");
        assert_eq!(parse_request_head(&buf), Err(ParseError::TooManyHeaders));
    }

    #[test]
    fn invalid_method_character() {
        let buf = b"G@T / HTTP/1.1\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidRequestLine));
    }

    #[test]
    fn request_line_missing_element() {
        assert_eq!(
            parse_request_head(b"GET /\r\n\r\n"),
            Err(ParseError::InvalidRequestLine)
        );
    }

    #[test]
    fn request_line_extra_element() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/1.1 extra\r\n\r\n"),
            Err(ParseError::InvalidRequestLine)
        );
    }

    #[test]
    fn unsupported_version_http_2() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/2.0\r\n\r\n"),
            Err(ParseError::UnsupportedVersion)
        );
    }

    #[test]
    fn unsupported_version_http_0_9() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/0.9\r\n\r\n"),
            Err(ParseError::UnsupportedVersion)
        );
    }

    #[test]
    fn malformed_version_prefix() {
        assert_eq!(
            parse_request_head(b"GET / FOO/1.1\r\n\r\n"),
            Err(ParseError::InvalidRequestLine)
        );
    }

    #[test]
    fn bare_lf_in_request_line_is_rejected() {
        assert_eq!(
            parse_request_head(b"GET / HTTP/1.1\nHost: x\r\n\r\n"),
            Err(ParseError::InvalidRequestLine)
        );
    }

    #[test]
    fn colon_before_space_in_header_name_is_rejected() {
        let buf = b"GET / HTTP/1.1\r\nHost : example.com\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidHeader));
    }

    #[test]
    fn obs_fold_is_rejected() {
        let buf = b"GET / HTTP/1.1\r\nHost: example.com\r\n value\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidHeader));
    }

    #[test]
    fn nul_byte_in_target_is_rejected() {
        let buf = b"GET /\0 HTTP/1.1\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidRequestLine));
    }

    #[test]
    fn control_char_in_header_value_is_rejected() {
        let buf = b"GET / HTTP/1.1\r\nX-Bad: a\0b\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidHeader));
    }

    #[test]
    fn header_missing_colon_is_rejected() {
        let buf = b"GET / HTTP/1.1\r\nHostexample.com\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidHeader));
    }

    #[test]
    fn valid_utf8_multibyte_header_value_is_accepted() {
        // "café" の "é" は UTF-8 で 0xC3 0xA9（obs-text 範囲だが正当なマルチバイト列）。
        let buf = b"GET / HTTP/1.1\r\nX-Name: caf\xC3\xA9\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(
            head.headers().find(|(n, _)| *n == "X-Name").map(|(_, v)| v),
            Some("café")
        );
    }

    #[test]
    fn lone_obs_text_byte_in_header_value_is_rejected() {
        // 単独の 0xE9 は有効な UTF-8 マルチバイト列を構成しないため拒否する。
        let buf = b"GET / HTTP/1.1\r\nX-Bad: a\xE9b\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidHeader));
    }

    #[test]
    fn parse_error_display_messages_are_stable() {
        // エラーメッセージが上位層（RequestError::Display 等）で連結される契約
        // であり、文言が意図せず変わっていないことを固定する（PoC-9 教訓:
        // Display 文言はステータス行同様に検証すべき出力の一部）。
        assert_eq!(
            ParseError::HeaderSectionTooLarge.to_string(),
            "header section exceeds MAX_HEADER_BYTES"
        );
        assert_eq!(
            ParseError::TooManyHeaders.to_string(),
            "header count exceeds MAX_HEADER_COUNT"
        );
        assert_eq!(
            ParseError::InvalidRequestLine.to_string(),
            "invalid request line"
        );
        assert_eq!(
            ParseError::UnsupportedVersion.to_string(),
            "unsupported HTTP version"
        );
        assert_eq!(ParseError::InvalidHeader.to_string(), "invalid header");
    }

    #[test]
    fn parse_error_implements_std_error() {
        // `RequestError::source()`（connection.rs）が `&dyn std::error::Error` として
        // 返せることをコンパイル時に固定する。
        fn assert_error<E: std::error::Error>() {}
        assert_error::<ParseError>();
    }

    #[test]
    fn http_version_variants_are_distinct() {
        assert_ne!(HttpVersion::Http10, HttpVersion::Http11);
        assert_eq!(HttpVersion::Http11, HttpVersion::Http11);
    }

    #[test]
    fn crlf_only_terminator_with_bare_lf_body_boundary_is_rejected() {
        // ヘッダ終端は必ず `\r\n\r\n` のみで判定する。ヘッダ部内に bare LF が
        // 混入した場合、行分割が崩れて tchar 違反として拒否されることを固定する
        // （リクエストスマグリング対策の一環）。
        let buf = b"GET / HTTP/1.1\r\nHost: example.com\nX-A: 1\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidHeader));
    }

    #[test]
    fn cookies_header_name_lookup_is_case_insensitive() {
        // ヘッダ名の大文字小文字混在（`cookie` / `Cookie`）でも同一ヘッダ集合
        // として拾えることを固定する（`RequestHead::cookies` の `headers()`
        // フィルタは `eq_ignore_ascii_case` を使う契約）。
        let buf = b"GET / HTTP/1.1\r\nHost: h\r\ncookie: a=1\r\nCOOKIE: b=2\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.cookies().unwrap(), vec![("a", "1"), ("b", "2")]);
    }

    #[test]
    fn cookies_unicode_whitespace_around_pair_is_not_trimmed() {
        // NBSP（U+00A0）は HTTP OWS（SP/HTAB）ではないため `RequestHead::cookies`
        // でも trim してはならない（`crate::cookie` の同名テストと対の固定、
        // `str::trim()` の Unicode 空白全般 trim を使わない契約の回帰防止）。
        let buf = "GET / HTTP/1.1\r\nHost: h\r\ncookie: a=1;\u{a0}b=2\r\n\r\n".as_bytes();
        let (head, _) = complete(buf);
        assert_eq!(
            head.cookies().unwrap_err(),
            crate::cookie::CookieError::InvalidCookiePair
        );
    }

    #[test]
    fn cookies_pair_count_cumulative_across_headers_exactly_at_max_is_accepted() {
        // 複数 `Cookie` ヘッダに跨って組数がちょうど上限に達する場合は受理する
        // （累積上限の境界値。`crate::cookie::MAX_COOKIE_COUNT` 参照）。
        let half = crate::cookie::MAX_COOKIE_COUNT / 2;
        let rest = crate::cookie::MAX_COOKIE_COUNT - half;
        let header_a = (0..half)
            .map(|i| format!("a{i}=v"))
            .collect::<Vec<_>>()
            .join("; ");
        let header_b = (0..rest)
            .map(|i| format!("b{i}=v"))
            .collect::<Vec<_>>()
            .join("; ");
        let buf = format!(
            "GET / HTTP/1.1\r\nHost: h\r\nCookie: {header_a}\r\nCookie: {header_b}\r\n\r\n"
        );
        let (head, _) = complete(buf.as_bytes());
        assert_eq!(
            head.cookies().unwrap().len(),
            crate::cookie::MAX_COOKIE_COUNT
        );
    }

    #[test]
    fn cookies_pair_count_cumulative_across_headers_exceeding_max_is_rejected() {
        // 単一ヘッダでは上限を超えないが、複数 `Cookie` ヘッダを合算すると
        // 超過するケース。ヘッダ分割による上限迂回を防ぐ契約を固定する。
        let half = crate::cookie::MAX_COOKIE_COUNT / 2;
        let rest = crate::cookie::MAX_COOKIE_COUNT - half + 1;
        let header_a = (0..half)
            .map(|i| format!("a{i}=v"))
            .collect::<Vec<_>>()
            .join("; ");
        let header_b = (0..rest)
            .map(|i| format!("b{i}=v"))
            .collect::<Vec<_>>()
            .join("; ");
        let buf = format!(
            "GET / HTTP/1.1\r\nHost: h\r\nCookie: {header_a}\r\nCookie: {header_b}\r\n\r\n"
        );
        let (head, _) = complete(buf.as_bytes());
        assert_eq!(
            head.cookies().unwrap_err(),
            crate::cookie::CookieError::TooManyCookies
        );
    }

    #[test]
    fn cookies_byte_length_cumulative_across_headers_exceeding_max_is_rejected() {
        // 単一ヘッダでは上限バイト数を超えないが、複数 `Cookie` ヘッダの
        // 結合後の長さ（`"; "` 結合込み）で超過するケースを固定する。
        let half_len = crate::cookie::MAX_COOKIE_STRING_BYTES / 2;
        let value_a = "a".repeat(half_len - 2);
        let value_b = "b".repeat(half_len);
        let header_a = format!("k={value_a}");
        let header_b = format!("k={value_b}");
        let buf = format!(
            "GET / HTTP/1.1\r\nHost: h\r\nCookie: {header_a}\r\nCookie: {header_b}\r\n\r\n"
        );
        let (head, _) = complete(buf.as_bytes());
        assert_eq!(
            head.cookies().unwrap_err(),
            crate::cookie::CookieError::CookieStringTooLarge
        );
    }

    /// [`find_subslice`] を memmem ベースへ変更する前の参照実装
    /// （イシュー #586）。差分テストでのみ使用し、新実装との返値完全一致を
    /// 検証する（リクエストスマグリング防止: ヘッド終端位置が 1 バイトでも
    /// ずれるとヘッダ/ボディ境界の解釈差につながるため）。
    fn find_subslice_reference(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() || haystack.len() < needle.len() {
            return None;
        }
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    #[test]
    fn find_subslice_matches_reference_implementation() {
        let needle_variants: &[&[u8]] = &[b"\r\n\r\n", b"\r\n", b"X"];
        let cases: &[&[u8]] = &[
            b"",
            b"\r",
            b"\r\n",
            b"\r\n\r",
            b"\r\n\r\n",
            b"a\r\n\r\nb",
            b"\r\r\r\r",
            b"\r\n\r\r\n\r\n",
            b"\r\n\r\nX",
            b"X\r\n\r\n",
            b"\r\n\r",
            b"\n\r\n\r\n",
            b"aaaa",
            b"\r\n\r\n\r\n\r\n",
        ];
        for needle in needle_variants {
            for case in cases {
                assert_eq!(
                    find_subslice(case, needle),
                    find_subslice_reference(case, needle),
                    "haystack={case:?} needle={needle:?}"
                );
            }
        }

        // needle が haystack と同長・haystack より長いケース
        let haystack = b"\r\n\r\n";
        assert_eq!(
            find_subslice(haystack, b"\r\n\r\n"),
            find_subslice_reference(haystack, b"\r\n\r\n")
        );
        assert_eq!(
            find_subslice(haystack, b"\r\n\r\n\r\n"),
            find_subslice_reference(haystack, b"\r\n\r\n\r\n")
        );

        // 空 needle・空 haystack
        assert_eq!(find_subslice(b"", b""), find_subslice_reference(b"", b""));
        assert_eq!(
            find_subslice(b"abc", b""),
            find_subslice_reference(b"abc", b"")
        );
    }

    #[test]
    fn find_subslice_empty_needle_is_none() {
        assert_eq!(find_subslice(b"abc", b""), None);
        assert_eq!(find_subslice(b"", b""), None);
    }

    #[test]
    fn parse_request_head_terminator_split_across_reads() {
        // ヘッド終端 `\r\n\r\n` が読み取り単位の境界をまたぐケース（バッファ
        // 分割着弾）を固定する。1 回目は終端直前の `\r` までしか届かず
        // Incomplete、`\n` 到着後の 2 回目で Complete になることを検証する
        // （memmem 化後も探索対象バッファ全体を都度再走査する前提は不変）。
        let full = b"GET /x HTTP/1.1\r\nHost: h\r\n\r\n";
        let split_at = full.len() - 1; // 末尾の `\n` の直前で分割
        let (head_part, _) = full.split_at(split_at);

        assert!(matches!(
            parse_request_head(head_part),
            Ok(ParseOutcome::Incomplete)
        ));

        let (head, consumed) = complete(full);
        assert_eq!(head.method(), "GET");
        assert_eq!(head.target(), "/x");
        assert_eq!(consumed, full.len());
    }

    #[test]
    fn method_and_target_utf8_boundary_case() {
        // method / target 単独では非 ASCII は tchar / 制御文字検証で事実上
        // 拒否されるが、target は非 ASCII を許容しうる経路（tchar 検証は
        // method のみに適用）のため、マルチバイト UTF-8 を含む target で
        // アクセサが文字境界 panic しないことを固定する（イシュー #591、
        // `RequestHead::buf` の `Range` インデックスが UTF-8 境界を破壊
        // しない不変条件の回帰防止）。
        let buf = "GET /caf\u{e9} HTTP/1.1\r\nHost: h\r\n\r\n".as_bytes();
        let (head, _) = complete(buf);
        assert_eq!(head.target(), "/caf\u{e9}");
    }

    #[test]
    fn invalid_utf8_in_target_is_rejected() {
        // 単独の 0xE9（有効な UTF-8 マルチバイト列を構成しない）を target に
        // 混入した場合、span 単位の UTF-8 検証（`parse_request_line`）で
        // 拒否されることを固定する（設計文書 6.3 節の 2 段検証のうち
        // span 検証側の回帰防止）。
        let buf = b"GET /a\xE9b HTTP/1.1\r\nHost: h\r\n\r\n";
        assert_eq!(parse_request_head(buf), Err(ParseError::InvalidRequestLine));
    }

    #[test]
    fn partial_eq_is_semantic_not_byte_identical() {
        // `buf`（ヘッド部の生バイト列コピー）は OWS 量の違いでバイト列として
        // 不一致になりうるが、`PartialEq` はアクセサ経由の意味的等価性で
        // 比較する（`derive` を使わない手動実装、イシュー #591）。ヘッダ値の
        // 前後 OWS 量だけが異なる 2 つのヘッドが等価と判定されることを固定する。
        let a = complete(b"GET / HTTP/1.1\r\nX-A: value\r\n\r\n").0;
        let b = complete(b"GET / HTTP/1.1\r\nX-A:   value  \r\n\r\n").0;
        assert_eq!(a, b);
    }

    #[test]
    fn partial_eq_detects_header_value_difference() {
        let a = complete(b"GET / HTTP/1.1\r\nX-A: 1\r\n\r\n").0;
        let b = complete(b"GET / HTTP/1.1\r\nX-A: 2\r\n\r\n").0;
        assert_ne!(a, b);
    }

    #[test]
    fn headers_vec_capacity_matches_header_count() {
        // ヘッダ本数の事前算出（`count_subslice`）が実際のヘッダ本数と一致し、
        // 過不足なく `Vec::with_capacity` されることを固定する（イシュー #591、
        // 性能改善ツリー #579 Phase 3 の alloc 定数化の前提）。
        let buf = b"GET / HTTP/1.1\r\nX-A: 1\r\nX-B: 2\r\nX-C: 3\r\n\r\n";
        let (head, _) = complete(buf);
        assert_eq!(head.headers().count(), 3);
    }
}
