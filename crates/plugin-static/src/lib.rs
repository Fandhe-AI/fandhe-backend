//! `fandhe-backend-plugin-static`: 静的ファイル配信プラグイン（イシュー #318）。
//!
//! 拡張点対応: パスインターセプト型（try_intercept）
//! （3 拡張点 trait には非該当。固定シグネチャシームへの閉包根拠は
//! `docs/design/extension-closure-verification.md`、機械可読宣言の規約は
//! `docs/design/dependency-graph-contract.md` 3 節、イシュー #318）
//!
//! # 背景・境界設計
//!
//! `crates/routes` の `Router` ハンドラは同期シグネチャ（`Fn(&RequestHead,
//! &[u8]) -> Response`）のため、ファイル読み込みのようなブロッキング I/O を
//! `spawn_blocking` で await スレッド外へ逃がす操作を Router 経由では表現
//! できない（`.claude/rules/coding-rust.md` の「Tokio 上でブロッキング処理を
//! await スレッドで実行しない」規約）。そのため `graphql`・`openapi` と同じ
//! 「設定登録型」のパスインターセプト型プラグインとして配線し、`crates/core`
//! の `Server::static_files(config)` に登録した場合のみ `crate::plugin::
//! try_intercept`（async）経由で本クレートの [`try_handle_static`] を呼ぶ。
//! 未登録時は `static` feature が有効でもフォールスルーする。
//!
//! # コアへの配線について（循環依存の回避）
//!
//! 本クレート単体は `fandhe-backend-core` に依存しない。コアが本クレートへ
//! `optional = true` + `dep:` 構文の依存を張る（`static` feature 有効時のみ）
//! ため、逆方向の依存を張ると循環依存になる（`crates/plugin-websocket`・
//! `crates/plugin-cors` と同一の非循環パターン、`docs/design/plugin-boundary.md`
//! 6.1 節・`scripts/dep-direction-check.sh` が機械的に検証する）。workspace
//! 全体の依存方向は次の一方向を維持する（依存方向: server → routes → http::*）。
//!
//! # フェイルクローズ設計（OWASP A01/A03/A04/A05、`.claude/rules/security.md`）
//!
//! - **二層防御**: (1) I/O 前の字句検証（末尾パスをセグメント分割し、空・
//!   `.`・`..`・NUL・`\`・先頭が `.` のセグメント（ドットファイル・
//!   ドットディレクトリ、`.env`・`.git/config`・`.htpasswd` 等の機密
//!   ファイルが公開 root 配下に置かれた場合の意図しない配信を一律拒否する。
//!   イシュー #318 レビュー指摘対応）を含むセグメントを拒否。パーセント
//!   デコードは行わず `Router` のパス照合方針と同じ「正規化しない」判断を
//!   踏襲する）、
//!   (2) `std::fs::canonicalize` 後の正規化済み実パスが正規化済み root 配下
//!   （`starts_with`）であることの検証（シンボリックリンク経由の脱出を拒否）
//! - ファイル未検出・検証失敗・権限エラー・サイズ超過は**一律 404**
//!   （存在オラクル・列挙を作らないフェイルクローズ。理由の異なる 403/500 は
//!   返さない）
//! - ディレクトリリスティングは実装しない（A05 対策、恒久方針）
//! - ファイル I/O（`canonicalize`・`metadata`・`read`）は単一の
//!   `spawn_blocking` クロージャ内に閉じる（`.claude/rules/coding-rust.md`）
//! - 末尾スラッシュ 1 個は「ディレクトリ要求」として受理し `index.html` を
//!   解決する（SSG が生成する `/posts/hello/` 形式の URL 互換、イシュー
//!   #418）。除去は 1 個のみに限定し、連続スラッシュ（`//`）は引き続き
//!   空セグメントとして一律拒否する（「正規化しない」方針を後退させない）。
//!   末尾スラッシュ付き要求が通常ファイルへ解決された場合も一律 404
//!   （フェイルクローズ）。301 リダイレクトによる URL 正規化は本クレートの
//!   スコープ外（拡張点でレスポンス改変ができない制約、イシュー #420）
//!
//! # 既知の限界
//!
//! `canonicalize` と実際の読み込みの間に、対象ファイルがシンボリックリンクへ
//! 差し替わる TOCTOU の残余リスクがある（同一 `spawn_blocking` 内での連続
//! 実行により窓を最小化するが、完全には排除できない。`O_NOFOLLOW` 系の
//! 強化は本イシューのスコープ外）。

mod mime;

use std::path::{Path, PathBuf};

use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;

/// [`StaticFilesConfigBuilder::max_file_bytes`] の既定値（8 MiB）。
///
/// SPA フロントエンドの典型的なアセット（HTML/CSS/JS バンドル・画像）は
/// この範囲に収まる想定。応答は `spawn_blocking` 内で `Vec<u8>` へ丸ごと
/// 読み込む実装のため、この上限は 1 リクエストあたりのメモリ使用量の
/// 上限そのものになる（レスポンスストリーミング連携は別イシューのスコープ、
/// モジュール冒頭 doc を参照）。
pub const DEFAULT_MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// [`StaticFilesConfig::builder`] の `build()` が返す構築失敗理由。
///
/// `fandhe_backend_plugin_cors::CorsConfigError`（`crates/plugin-cors`）相当の
/// フェイルクローズ設計。`root` の不在・非ディレクトリは起動前（構築時）に
/// 検出し、実行時に初めて失敗する経路を作らない。
#[derive(Debug)]
pub enum StaticConfigError {
    /// `mount` が空、先頭が `/` でない、末尾が `/`（`"/"` 単体を除く）、
    /// `//` を含む、または制御文字・`?`・`#` を含む。
    InvalidMount(String),
    /// `root` の [`std::fs::canonicalize`] が失敗した（不在・アクセス不可等）。
    RootNotAccessible(std::io::Error),
    /// `root` は存在するがディレクトリではない。
    RootNotADirectory,
}

impl std::fmt::Display for StaticConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMount(reason) => write!(f, "mount が不正: {reason}"),
            Self::RootNotAccessible(err) => write!(f, "root にアクセスできない: {err}"),
            Self::RootNotADirectory => f.write_str("root がディレクトリではない"),
        }
    }
}

impl std::error::Error for StaticConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RootNotAccessible(err) => Some(err),
            _ => None,
        }
    }
}

/// 検証済み静的ファイル配信設定（[`StaticFilesConfig::builder`] 経由でのみ構築できる）。
///
/// [`try_handle_static`] が本設定を参照して配信対象パスを判定・解決する。
#[derive(Debug, Clone)]
pub struct StaticFilesConfig {
    /// マウントプレフィックス（例 `"/static"`）。正規化済み（末尾 `/` なし、
    /// `"/"` 単体を除く）。
    mount: String,
    /// 配信対象ディレクトリ（構築時に [`std::fs::canonicalize`] 済み）。
    root: PathBuf,
    /// 配信を許可する 1 ファイルあたりの最大バイト数（既定
    /// [`DEFAULT_MAX_FILE_BYTES`]）。
    max_file_bytes: u64,
}

impl StaticFilesConfig {
    /// [`StaticFilesConfigBuilder`] を返す。
    ///
    /// `mount` はリクエストパスの照合に使うプレフィックス（例 `"/static"`）、
    /// `root` は配信対象ディレクトリ。`root` の実在確認は `build()` 呼び出し
    /// 時に行う（本メソッド自体は失敗しない）。
    #[must_use]
    pub fn builder(mount: impl Into<String>, root: impl Into<PathBuf>) -> StaticFilesConfigBuilder {
        StaticFilesConfigBuilder {
            mount: mount.into(),
            root: root.into(),
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

/// [`StaticFilesConfig`] のビルダー。既定値は「1 ファイル [`DEFAULT_MAX_FILE_BYTES`]
/// まで」。
#[derive(Debug, Clone)]
pub struct StaticFilesConfigBuilder {
    mount: String,
    root: PathBuf,
    max_file_bytes: u64,
}

impl StaticFilesConfigBuilder {
    /// 配信を許可する 1 ファイルあたりの最大バイト数を設定する（既定
    /// [`DEFAULT_MAX_FILE_BYTES`]）。超過ファイルは 404 として拒否される
    /// （[`try_handle_static`] の doc を参照）。
    #[must_use]
    pub fn max_file_bytes(mut self, max: u64) -> Self {
        self.max_file_bytes = max;
        self
    }

    /// 検証付きで [`StaticFilesConfig`] を構築する（フェイルクローズ）。
    ///
    /// - `mount` の形式検証（本モジュール冒頭 doc・[`StaticConfigError::InvalidMount`]）
    /// - `root` の [`std::fs::canonicalize`]（不在・アクセス不可は
    ///   [`StaticConfigError::RootNotAccessible`]）
    /// - `root` がディレクトリであることの確認（[`StaticConfigError::RootNotADirectory`]）
    ///
    /// ```
    /// use fandhe_backend_plugin_static::StaticFilesConfig;
    ///
    /// // std::env::temp_dir() は必ず存在するディレクトリのため構築に成功する。
    /// let config = StaticFilesConfig::builder("/static", std::env::temp_dir())
    ///     .build()
    ///     .unwrap();
    /// let _ = config;
    /// ```
    pub fn build(self) -> Result<StaticFilesConfig, StaticConfigError> {
        let mount = validate_mount(&self.mount)?;
        let root =
            std::fs::canonicalize(&self.root).map_err(StaticConfigError::RootNotAccessible)?;
        if !root.is_dir() {
            return Err(StaticConfigError::RootNotADirectory);
        }
        Ok(StaticFilesConfig {
            mount,
            root,
            max_file_bytes: self.max_file_bytes,
        })
    }
}

/// `mount` の形式を検証し、正規化済み文字列を返す（[`StaticConfigError::InvalidMount`]、
/// フェイルクローズ）。
///
/// 許容形式: 先頭が `/`、末尾が `/` でない（`"/"` 単体は例外）、`//` を
/// 含まない、制御文字・`?`・`#` を含まない。
fn validate_mount(mount: &str) -> Result<String, StaticConfigError> {
    if mount.is_empty() || !mount.starts_with('/') {
        return Err(StaticConfigError::InvalidMount(
            "先頭が '/' の非空文字列である必要がある".to_string(),
        ));
    }
    if mount != "/" && mount.ends_with('/') {
        return Err(StaticConfigError::InvalidMount(
            "'/' 単体を除き末尾に '/' を含められない".to_string(),
        ));
    }
    if mount.contains("//") {
        return Err(StaticConfigError::InvalidMount(
            "連続する '/' を含められない".to_string(),
        ));
    }
    if mount.contains('?') || mount.contains('#') {
        return Err(StaticConfigError::InvalidMount(
            "'?' / '#' を含められない（クエリ・フラグメントとの混同防止）".to_string(),
        ));
    }
    if mount.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(StaticConfigError::InvalidMount(
            "制御文字を含められない".to_string(),
        ));
    }
    Ok(mount.to_string())
}

/// リクエストパスからマウントプレフィックスを除いた「末尾パス」を返す。
///
/// `path` が `mount` に一致しない（プレフィックスでない）場合は `None`
/// （呼び出し元はこれを「対象外パス、既定 `Handler` へフォールスルー」と
/// 解釈する）。`mount` が `"/"` の場合は先頭の `/` のみを取り除く。
/// パーセントデコードは行わない（本モジュール冒頭 doc の「正規化しない」
/// 方針を参照）。
fn strip_mount<'a>(path: &'a str, mount: &str) -> Option<&'a str> {
    if mount == "/" {
        return path.strip_prefix('/');
    }
    if path == mount {
        return Some("");
    }
    path.strip_prefix(mount)?.strip_prefix('/')
}

/// 末尾パスの 1 セグメントが安全（パス走査対策を通過する）かどうかを判定する。
///
/// `crates/routes/src/pattern.rs` の `is_safe_segment_value` と同一方針
/// （非空・`.`/`..` 不一致）に加え、本クレートはファイルシステムパスへ
/// 直接連結するため NUL・`\`（Windows パス区切り誤用対策）も拒否する
/// （計画書 5 節の拒否テスト一覧に対応）。さらに先頭が `.` のセグメント
/// （ドットファイル・ドットディレクトリ）も一律拒否する。`.` と `..` は
/// この条件に包含されるが可読性のため明示判定も残す。公開 root 配下に
/// `.env`・`.git/config`・`.htpasswd` 等の機密ファイルが置かれた場合の
/// 意図しない配信（OWASP A01/A05）を防ぐフェイルクローズ判断（イシュー
/// #318 レビュー指摘対応、`docs/design/plugin-boundary.md` 5.10 節）。
fn is_safe_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.starts_with('.')
        && !segment.contains('\0')
        && !segment.contains('\\')
}

/// `try_handle_static` の `spawn_blocking` クロージャ内で完結するファイル
/// 解決本体（同期・sans-async、ユニットテストからも直接呼べる）。
///
/// `root` は構築時に canonicalize 済みの前提（[`StaticFilesConfigBuilder::build`]）。
/// `segments` は [`is_safe_segment`] を通過済みの前提（呼び出し元が保証）。
/// `dir_request` は URL が末尾スラッシュ付き（「ディレクトリ要求」、イシュー
/// #418）だったかを表す。`true` の場合、解決先が通常ファイルであれば配信
/// しない（一般的な静的サーバーと同挙動、フェイルクローズ）。
/// 戻り値 `Some((bytes, content_type))` は配信対象を確定できたことを意味し、
/// `None` はあらゆる失敗（未検出・root 外・ディレクトリで index.html なし・
/// サイズ超過・I/O エラー）を一律に表す（呼び出し元が 404 へ変換する、
/// モジュール冒頭 doc の「一律 404」方針）。
fn resolve_and_read(
    root: &Path,
    segments: &[&str],
    dir_request: bool,
    max_file_bytes: u64,
) -> Option<(Vec<u8>, &'static str)> {
    let mut candidate = root.to_path_buf();
    for segment in segments {
        candidate.push(segment);
    }

    let canonical = std::fs::canonicalize(&candidate).ok()?;
    // シンボリックリンク経由の root 脱出を拒否する（モジュール冒頭 doc の
    // 「二層防御」(2)）。`root` 自身は既に canonicalize 済みのため、
    // `starts_with` は正規化済み同士の比較になる。
    if !canonical.starts_with(root) {
        return None;
    }

    let metadata = std::fs::metadata(&canonical).ok()?;
    // 末尾スラッシュ付き要求（ディレクトリ要求）が通常ファイルへ解決された
    // 場合は配信しない（イシュー #418、一般的な静的サーバーと同挙動の
    // フェイルクローズ）。
    if dir_request && !metadata.is_dir() {
        return None;
    }
    let final_path = if metadata.is_dir() {
        // ディレクトリ解決時は index.html を試す（SPA ユースケース向けの
        // 最小既定。ディレクトリリスティングは実装しない、モジュール冒頭 doc）。
        let index = canonical.join("index.html");
        let index_canonical = std::fs::canonicalize(&index).ok()?;
        if !index_canonical.starts_with(root) {
            return None;
        }
        index_canonical
    } else if metadata.is_file() {
        canonical
    } else {
        // ソケット・デバイスファイル等の特殊ファイルは配信対象外。
        return None;
    };

    let final_metadata = std::fs::metadata(&final_path).ok()?;
    if !final_metadata.is_file() || final_metadata.len() > max_file_bytes {
        return None;
    }

    let bytes = std::fs::read(&final_path).ok()?;
    // TOCTOU 残余リスク（モジュール冒頭 doc の「既知の限界」）への追加防御:
    // metadata 確認後の読み込みでサイズが上限を超えていないか再確認する。
    if bytes.len() as u64 > max_file_bytes {
        return None;
    }

    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let content_type = mime::content_type_for_extension(file_name);
    Some((bytes, content_type))
}

/// `Response::with_header` の `Err` を「当該ヘッダを付与しない」側へ倒す
/// ヘルパ（`crates/plugin-cors/src/lib.rs` の `try_add_header` と同一原則）。
fn try_add_header(response: Response, name: &str, value: &str) -> Response {
    let fallback = response.clone();
    response.with_header(name, value).unwrap_or(fallback)
}

/// `crate::plugin::try_intercept`（`crates/core`）から呼ばれる静的ファイル
/// 配信のエントリポイント。
///
/// `GET` メソッドかつ `config` の `mount` プレフィックスに一致するパスのみを
/// 処理対象とする。それ以外（`GET` 以外のメソッド、`mount` 不一致パス）は
/// `None` を返し、呼び出し元は既定 `Handler`（未登録時 404）等へフォール
/// スルーする（`graphql`・`openapi` と同じ「設定登録型」パターン、
/// モジュール冒頭 doc を参照）。
///
/// `mount` に一致した場合は必ず `Some` を返す（`try_intercept` が処理を
/// 完結させたことを意味する）。ファイル未検出・パス走査試行・サイズ超過等の
/// あらゆる失敗は一律 404（モジュール冒頭 doc のフェイルクローズ方針）。
///
/// 末尾スラッシュ 1 個（`<mount>/dir/`）は「ディレクトリ要求」として
/// `dir/index.html` を解決する（イシュー #418、モジュール冒頭 doc 参照）。
///
/// # ブロッキング I/O の隔離（`.claude/rules/coding-rust.md`）
///
/// `canonicalize`・`metadata`・`read` はすべて `tokio::task::spawn_blocking`
/// のクロージャ内（非公開関数 `resolve_and_read`）に閉じ、tokio の非同期
/// ランタイムスレッドをブロックしない。`spawn_blocking` 自体が失敗する
/// （内部で panic した）場合もフェイルクローズで 404 を返す。
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// use fandhe_backend_plugin_static::{StaticFilesConfig, try_handle_static};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// // 一意な一時ディレクトリへ index.html を書き込み、配信されることを確認する。
/// let mut dir = std::env::temp_dir();
/// dir.push(format!("fandhe-plugin-static-doctest-{}", std::process::id()));
/// std::fs::create_dir_all(&dir).unwrap();
/// std::fs::write(dir.join("index.html"), b"<h1>hi</h1>").unwrap();
///
/// let config = StaticFilesConfig::builder("/static", &dir).build().unwrap();
///
/// let buf = b"GET /static/index.html HTTP/1.1\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// let response = try_handle_static(&head, &config).await.unwrap();
/// assert_eq!(response.status, 200);
/// assert_eq!(response.body, b"<h1>hi</h1>");
///
/// // 末尾スラッシュ付きディレクトリ URL も index.html を解決する（イシュー #418）。
/// let buf = b"GET /static/ HTTP/1.1\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// let response = try_handle_static(&head, &config).await.unwrap();
/// assert_eq!(response.status, 200);
/// assert_eq!(response.body, b"<h1>hi</h1>");
///
/// // 対象外パス（mount 不一致）は None（フォールスルー）。
/// let buf = b"GET /api/todos HTTP/1.1\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// assert!(try_handle_static(&head, &config).await.is_none());
///
/// // パストラバーサル試行は 404（一律フェイルクローズ）。
/// let buf = b"GET /static/../secret HTTP/1.1\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// let response = try_handle_static(&head, &config).await.unwrap();
/// assert_eq!(response.status, 404);
///
/// std::fs::remove_dir_all(&dir).unwrap();
/// # }
/// ```
pub async fn try_handle_static(head: &RequestHead, config: &StaticFilesConfig) -> Option<Response> {
    if !head.method.eq_ignore_ascii_case("GET") {
        return None;
    }

    let path = head.path();
    let tail = strip_mount(path, &config.mount)?;

    // 末尾スラッシュ 1 個は「ディレクトリ要求」（SSG が生成する `/posts/hello/`
    // 形式の URL 互換、イシュー #418）として受理し、`index.html` 解決を試みる。
    // 除去は 1 個のみに限定し、連続スラッシュ由来の空セグメント拒否（下記
    // `is_safe_segment` 検証・「正規化しない」方針）は後退させない。
    let (tail, dir_request) = match tail.strip_suffix('/') {
        Some(stripped) => (stripped, true),
        None => (tail, false),
    };
    // `/static//`（mount 直後が連続スラッシュ）は除去後も tail が空のまま
    // 残る。mount 直下の `/static/`（既存挙動、tail が最初から空）とは区別し、
    // 一律 404 で連続スラッシュ拒否方針を維持する。
    if dir_request && tail.is_empty() {
        return Some(Response::empty(404));
    }

    let segments: Vec<&str> = if tail.is_empty() {
        Vec::new()
    } else {
        tail.split('/').collect()
    };
    if !segments.iter().all(|segment| is_safe_segment(segment)) {
        return Some(Response::empty(404));
    }

    let root = config.root.clone();
    let max_file_bytes = config.max_file_bytes;
    // `Vec<&str>` は 'static ではないため、所有権を持つ `String` へ変換して
    // spawn_blocking クロージャへ move する。
    let owned_segments: Vec<String> = segments.iter().map(|s| (*s).to_string()).collect();

    let outcome = tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = owned_segments.iter().map(String::as_str).collect();
        resolve_and_read(&root, &refs, dir_request, max_file_bytes)
    })
    .await;

    match outcome {
        // `spawn_blocking` 自体の失敗（内部 panic 等）もフェイルクローズで 404。
        Err(_) => Some(Response::empty(404)),
        Ok(None) => Some(Response::empty(404)),
        Ok(Some((bytes, content_type))) => {
            let response = Response::new(200, bytes).with_content_type(content_type);
            // MIME スニッフィング対策（`.claude/rules/security.md` A05、
            // `crates/plugin-static/src/mime.rs` の doc と対）。
            Some(try_add_header(
                response,
                "X-Content-Type-Options",
                "nosniff",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn head_from(buf: &[u8]) -> RequestHead {
        match parse_request_head(buf).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            other => panic!("unexpected parse outcome: {other:?}"),
        }
    }

    /// テスト専用の一意な一時ディレクトリ（`Drop` で自動削除、std のみで実装）。
    struct TempDir(PathBuf);

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    impl TempDir {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!(
                "fandhe-plugin-static-test-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, rel: &str, contents: &[u8]) -> PathBuf {
            let target = self.0.join(rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&target, contents).unwrap();
            target
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn config_for(dir: &TempDir) -> StaticFilesConfig {
        StaticFilesConfig::builder("/static", dir.path())
            .build()
            .unwrap()
    }

    // --- StaticFilesConfig::builder / build ---

    #[test]
    fn build_rejects_missing_leading_slash_mount() {
        let dir = TempDir::new();
        let err = StaticFilesConfig::builder("static", dir.path())
            .build()
            .unwrap_err();
        assert!(matches!(err, StaticConfigError::InvalidMount(_)));
    }

    #[test]
    fn build_rejects_trailing_slash_mount() {
        let dir = TempDir::new();
        let err = StaticFilesConfig::builder("/static/", dir.path())
            .build()
            .unwrap_err();
        assert!(matches!(err, StaticConfigError::InvalidMount(_)));
    }

    #[test]
    fn build_accepts_root_mount() {
        let dir = TempDir::new();
        let config = StaticFilesConfig::builder("/", dir.path()).build().unwrap();
        assert_eq!(config.mount, "/");
    }

    #[test]
    fn build_rejects_nonexistent_root() {
        let mut missing = std::env::temp_dir();
        missing.push("fandhe-plugin-static-does-not-exist");
        let err = StaticFilesConfig::builder("/static", missing)
            .build()
            .unwrap_err();
        assert!(matches!(err, StaticConfigError::RootNotAccessible(_)));
    }

    #[test]
    fn build_rejects_root_that_is_a_file() {
        let dir = TempDir::new();
        let file = dir.write("not-a-dir.txt", b"x");
        let err = StaticFilesConfig::builder("/static", file)
            .build()
            .unwrap_err();
        assert!(matches!(err, StaticConfigError::RootNotADirectory));
    }

    // --- フォールスルー（対象外パス・非 GET） ---

    #[tokio::test]
    async fn falls_through_when_prefix_does_not_match() {
        let dir = TempDir::new();
        let config = config_for(&dir);
        let head = head_from(b"GET /api/todos HTTP/1.1\r\n\r\n");
        assert!(try_handle_static(&head, &config).await.is_none());
    }

    #[tokio::test]
    async fn falls_through_for_non_get_method() {
        let dir = TempDir::new();
        dir.write("index.html", b"hi");
        let config = config_for(&dir);
        let head = head_from(b"POST /static/index.html HTTP/1.1\r\n\r\n");
        assert!(try_handle_static(&head, &config).await.is_none());
    }

    // --- 許可系 ---

    #[tokio::test]
    async fn serves_regular_file_with_content_type_and_nosniff() {
        let dir = TempDir::new();
        dir.write("app.js", b"console.log('hi')");
        let config = config_for(&dir);
        let head = head_from(b"GET /static/app.js HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"console.log('hi')");
        let text = String::from_utf8(response.serialize(false)).unwrap();
        assert!(text.contains("Content-Type: text/javascript; charset=utf-8\r\n"));
        assert!(text.contains("X-Content-Type-Options: nosniff\r\n"));
    }

    #[tokio::test]
    async fn serves_directory_index_html() {
        let dir = TempDir::new();
        dir.write("index.html", b"<h1>root</h1>");
        let config = config_for(&dir);
        let head = head_from(b"GET /static HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"<h1>root</h1>");
    }

    #[tokio::test]
    async fn serves_subdirectory_index_html() {
        let dir = TempDir::new();
        dir.write("docs/index.html", b"<h1>docs</h1>");
        let config = config_for(&dir);
        let head = head_from(b"GET /static/docs HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"<h1>docs</h1>");
    }

    // --- 末尾スラッシュ（ディレクトリ要求、イシュー #418） ---

    #[tokio::test]
    async fn serves_subdirectory_index_html_with_trailing_slash() {
        let dir = TempDir::new();
        dir.write("docs/index.html", b"<h1>docs</h1>");
        let config = config_for(&dir);
        let head = head_from(b"GET /static/docs/ HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"<h1>docs</h1>");
    }

    #[tokio::test]
    async fn serves_root_index_html_with_trailing_slash() {
        let dir = TempDir::new();
        dir.write("index.html", b"<h1>root</h1>");
        let config = config_for(&dir);
        let head = head_from(b"GET /static/ HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"<h1>root</h1>");
    }

    #[tokio::test]
    async fn serves_directory_index_with_trailing_slash_on_root_mount() {
        let dir = TempDir::new();
        dir.write("docs/index.html", b"<h1>docs</h1>");
        let config = StaticFilesConfig::builder("/", dir.path()).build().unwrap();
        let head = head_from(b"GET /docs/ HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"<h1>docs</h1>");
    }

    #[tokio::test]
    async fn trailing_slash_on_regular_file_returns_404() {
        let dir = TempDir::new();
        dir.write("app.js", b"console.log('hi')");
        let config = config_for(&dir);
        let head = head_from(b"GET /static/app.js/ HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn double_trailing_slash_returns_404() {
        let dir = TempDir::new();
        dir.write("docs/index.html", b"<h1>docs</h1>");
        let config = config_for(&dir);
        let head = head_from(b"GET /static/docs// HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn trailing_slash_only_after_mount_double_slash_returns_404() {
        let dir = TempDir::new();
        dir.write("index.html", b"<h1>root</h1>");
        let config = config_for(&dir);
        let head = head_from(b"GET /static// HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn directory_without_index_html_with_trailing_slash_returns_404() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join("empty")).unwrap();
        let config = config_for(&dir);
        let head = head_from(b"GET /static/empty/ HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_directory_escaping_root_with_trailing_slash_is_rejected() {
        let outside = TempDir::new();
        outside.write("index.html", b"<h1>secret</h1>");
        let dir = TempDir::new();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        let config = config_for(&dir);
        let head = head_from(b"GET /static/escape/ HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn unknown_extension_falls_back_to_octet_stream() {
        let dir = TempDir::new();
        dir.write("data.unknownext", b"binary");
        let config = config_for(&dir);
        let head = head_from(b"GET /static/data.unknownext HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 200);
        let text = String::from_utf8(response.serialize(false)).unwrap();
        assert!(text.contains("Content-Type: application/octet-stream\r\n"));
    }

    #[tokio::test]
    async fn file_exactly_at_max_bytes_is_served() {
        let dir = TempDir::new();
        dir.write("exact.bin", &[0u8; 16]);
        let config = StaticFilesConfig::builder("/static", dir.path())
            .max_file_bytes(16)
            .build()
            .unwrap();
        let head = head_from(b"GET /static/exact.bin HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 200);
    }

    // --- 拒否系（一律 404） ---

    #[tokio::test]
    async fn rejects_literal_dotdot_segment() {
        let dir = TempDir::new();
        let config = config_for(&dir);
        let head = head_from(b"GET /static/../secret HTTP/1.1\r\n\r\n");
        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn rejects_dotdot_in_middle_segment() {
        let dir = TempDir::new();
        let config = config_for(&dir);
        let head = head_from(b"GET /static/a/../../b HTTP/1.1\r\n\r\n");
        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn percent_encoded_traversal_is_treated_as_literal_and_not_found() {
        // パーセントデコードしない方針のため `%2e%2e` はリテラルなファイル名
        // として扱われ、そのような名前のファイルは存在しないため 404 になる
        // （バイパスではなく通常の未検出、モジュール冒頭 doc の「正規化しない」方針）。
        let dir = TempDir::new();
        let config = config_for(&dir);
        let head = head_from(b"GET /static/%2e%2e/secret HTTP/1.1\r\n\r\n");
        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn rejects_empty_segment_from_consecutive_slashes() {
        let dir = TempDir::new();
        dir.write("a/b.txt", b"x");
        let config = config_for(&dir);
        let head = head_from(b"GET /static/a//b.txt HTTP/1.1\r\n\r\n");
        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[test]
    fn is_safe_segment_rejects_backslash_and_nul() {
        assert!(!is_safe_segment("a\\b"));
        assert!(!is_safe_segment("a\0b"));
        assert!(is_safe_segment("normal-file.txt"));
    }

    #[test]
    fn is_safe_segment_rejects_leading_dot() {
        assert!(!is_safe_segment(".env"));
        assert!(!is_safe_segment(".git"));
        assert!(!is_safe_segment(".htpasswd"));
    }

    #[tokio::test]
    async fn rejects_dotfile_at_root() {
        // 公開 root 直下に `.env` が置かれた設定ミスがあっても配信しない
        // （OWASP A01/A05、イシュー #318 レビュー指摘対応）。
        let dir = TempDir::new();
        dir.write(".env", b"SECRET=leak");
        let config = config_for(&dir);
        let head = head_from(b"GET /static/.env HTTP/1.1\r\n\r\n");
        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn rejects_dotdir_nested_file() {
        // `.git/config` のようにドットディレクトリ配下のファイルも同様に
        // 一律 404 とする（先頭セグメントで拒否するため到達しない）。
        let dir = TempDir::new();
        dir.write(".git/config", b"[core]\n");
        let config = config_for(&dir);
        let head = head_from(b"GET /static/.git/config HTTP/1.1\r\n\r\n");
        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn missing_file_returns_404() {
        let dir = TempDir::new();
        let config = config_for(&dir);
        let head = head_from(b"GET /static/does-not-exist.txt HTTP/1.1\r\n\r\n");
        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn directory_without_index_html_returns_404() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join("empty")).unwrap();
        let config = config_for(&dir);
        let head = head_from(b"GET /static/empty HTTP/1.1\r\n\r\n");
        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn file_over_max_bytes_returns_404() {
        let dir = TempDir::new();
        dir.write("big.bin", &[0u8; 32]);
        let config = StaticFilesConfig::builder("/static", dir.path())
            .max_file_bytes(16)
            .build()
            .unwrap();
        let head = head_from(b"GET /static/big.bin HTTP/1.1\r\n\r\n");
        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_within_root_is_served() {
        let dir = TempDir::new();
        let target = dir.write("real.txt", b"hello");
        std::os::unix::fs::symlink(&target, dir.path().join("link.txt")).unwrap();
        let config = config_for(&dir);
        let head = head_from(b"GET /static/link.txt HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"hello");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_escaping_root_is_rejected() {
        let dir = TempDir::new();
        let outside = TempDir::new();
        outside.write("secret.txt", b"top-secret");
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("escape.txt"),
        )
        .unwrap();
        let config = config_for(&dir);
        let head = head_from(b"GET /static/escape.txt HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_directory_escaping_root_is_rejected() {
        let dir = TempDir::new();
        let outside = TempDir::new();
        outside.write("index.html", b"outside-index");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape-dir")).unwrap();
        let config = config_for(&dir);
        let head = head_from(b"GET /static/escape-dir HTTP/1.1\r\n\r\n");

        let response = try_handle_static(&head, &config).await.unwrap();
        assert_eq!(response.status, 404);
    }
}
