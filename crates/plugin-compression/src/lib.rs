//! `fandhe-backend-plugin-compression`: レスポンス圧縮プラグイン（イシュー #321）。
//!
//! 拡張点対応: レスポンス後処理型（`finalize_response`）
//! （3 拡張点 trait には非該当。固定シグネチャシームへの閉包根拠は
//! `docs/design/plugin-boundary.md` 5.9 節、機械可読宣言の規約は
//! `docs/design/dependency-graph-contract.md` 3 節。`crates/plugin-cors`
//! （イシュー #305）が確立した「レスポンス後処理型」シームの第 2 インスタンス）
//!
//! # 背景・境界設計
//!
//! `Middleware::on_response` はレスポンスへの参照を持たない観測専用契約の
//! ため、body の圧縮のような書き換えには使えない（`crates/plugin-cors` の
//! doc と同じ理由）。コア側（`crates/core`）が `compression` feature 有効時
//! のみ [`apply_compression`] を「レスポンス後処理型」シーム経由で、CORS の
//! 後（`crate::plugin::finalize_response` の逐次適用順）に呼び出す。
//! 圧縮は「最終 body を確定させる後処理」であり、CORS はヘッダのみで body に
//! 触れないため順序が結果へ影響することはないが、規約として「圧縮は必ず
//! 最後」を明文化する。
//!
//! # コアへの配線について（循環依存の回避）
//!
//! 本クレート単体は `fandhe-backend-core` に依存しない。コアが本クレートへ
//! `optional = true` + `dep:` 構文の依存を張る（`compression` feature 有効時
//! のみ）ため、逆方向の依存を張ると循環依存になる（`crates/plugin-cors` と
//! 同一の非循環パターン、`docs/design/plugin-boundary.md` 6.1 節・
//! `scripts/dep-direction-check.sh` が機械的に検証する）。依存方向は
//! `server → routes → http::*` の一方向であり、本クレートは `http` のみに
//! 依存する下位層として振る舞う。
//!
//! # 圧縮判定（すべて満たす場合のみ圧縮、フェイルセーフ設計）
//!
//! [`apply_compression`] は以下をすべて満たす場合のみ body を gzip 圧縮し、
//! `Content-Encoding: gzip` を付与する。判定不能・条件不足は常に「圧縮しない
//! （無改変で返す）」側へ倒す（フェイルセーフ。認可系のフェイルクローズとは
//! 逆向きの安全側であることに注意 —— identity 応答は常に安全であり、
//! 誤って圧縮しないことによる実害はない）:
//!
//! 1. 呼び出し元が [`CompressionConfig`] を登録済み（feature + opt-in、
//!    コア側 `Server::compression` の doc を参照。本クレート自体はこの
//!    判定に関与しない）
//! 2. レスポンスのステータスが 2xx かつ 204 以外
//! 3. レスポンスの実効 `Content-Type`（[`fandhe_backend_http::response::Response::header`]
//!    で取得）が [`CompressionConfig`] の圧縮対象リストに一致（`; charset=`
//!    等のパラメータは無視して type/subtype 部分のみ比較）
//! 4. body 長が [`CompressionConfig`] の最小サイズ閾値以上
//! 5. リクエストの `Accept-Encoding` が gzip を受理（[`accepts_gzip`]）
//! 6. レスポンスに `Content-Encoding` が未設定（二重圧縮防止）
//!
//! 条件 3・4 を満たした時点（= 表現が `Accept-Encoding` に依存して変わり
//! うる時点）で、圧縮の成否に関わらず `Vary: Accept-Encoding` を付与する
//! （共有キャッシュでの変異混同防止。重複付与はしない）。圧縮結果が元 body
//! より大きい場合は元のレスポンスをそのまま返す。
//!
//! # BREACH 類似の情報漏洩リスク（受け入れ基準、OWASP A02 隣接）
//!
//! 秘密情報（CSRF トークン・セッション情報等）と攻撃者制御の入力が同一の
//! 圧縮応答に混在すると、圧縮後サイズの観測から秘密が漏洩しうる（BREACH
//! 類似の攻撃）。本プラグインは opt-in（`Server::compression` の明示登録
//! 時のみ動作）であり、既定の圧縮対象 `Content-Type` にも「秘密を含み
//! やすい応答は利用者判断で対象から除外する」運用が必要である。該当
//! エンドポイントでは [`CompressionConfigBuilder::compressible_types`] で
//! 対象 `Content-Type` から除外するか、[`CompressionConfigBuilder::min_size`]
//! の運用、または `compression` feature 自体を無効化して対処すること
//! （攻撃の具体的手順はここに記載しない、`.claude/rules/feasibility-guardrail.md`
//! の方針に準拠）。
//!
//! # CPU コストの扱い
//!
//! gzip 圧縮は同期 CPU 処理であり、`finalize_response` を呼ぶ接続タスク内で
//! 実行される。`Middleware` の「同期ブロッキング I/O 禁止」規約
//! （`.claude/rules/coding-rust.md`）は I/O が対象であり、本件は CPU 処理の
//! ため対象外。作業量はコア既存のリクエストボディサイズ上限・ハンドラの
//! 生成物サイズにより有界（無制限ではない）。巨大応答向けの
//! `spawn_blocking` 化・ストリーミング圧縮はスコープ外（後続課題として
//! 追跡、`.claude/rules/out-of-scope-tracking.md`）。

use fandhe_backend_http::request::RequestHead;
use fandhe_backend_http::response::Response;

/// 既定の圧縮対象 `Content-Type` パターン。
///
/// 末尾が `/` のパターンは type プレフィックス一致（`text/` は
/// `text/plain`・`text/html` 等すべてに一致）、それ以外は type/subtype の
/// 完全一致として扱う（[`CompressionConfig::matches_content_type`] を参照）。
const DEFAULT_COMPRESSIBLE_TYPES: &[&str] = &[
    "text/",
    "application/json",
    "application/javascript",
    "application/xml",
    "application/xhtml+xml",
    "image/svg+xml",
];

/// 既定の最小圧縮対象サイズ（バイト）。これ未満の body は圧縮しない
/// （小さい応答は圧縮のオーバーヘッドが利益を上回りやすいための閾値）。
const DEFAULT_MIN_SIZE: usize = 1024;

/// レスポンス圧縮の設定（[`CompressionConfigBuilder`] で構築する）。
///
/// `crates/core` の `Server::compression(config)` に登録した場合のみ
/// [`apply_compression`] が実リクエストへの圧縮を行う（`compression`
/// feature の opt-in、他の設定登録型プラグインと同一パターン）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressionConfig {
    min_size: usize,
    compressible_types: Vec<String>,
}

impl CompressionConfig {
    /// [`CompressionConfigBuilder`]（既定値: 閾値 1024 バイト・
    /// `text/*`・`application/json` 等の圧縮対象 `Content-Type` リスト）を
    /// 返す。
    ///
    /// ```
    /// use fandhe_backend_plugin_compression::CompressionConfig;
    ///
    /// let config = CompressionConfig::builder().build();
    /// assert!(config.matches_content_type("application/json"));
    /// assert!(config.matches_content_type("text/html; charset=utf-8"));
    /// assert!(!config.matches_content_type("image/png"));
    /// ```
    #[must_use]
    pub fn builder() -> CompressionConfigBuilder {
        CompressionConfigBuilder::default()
    }

    /// `content_type` が圧縮対象リストに一致するか判定する。
    ///
    /// `;` 以降のパラメータ（`; charset=utf-8` 等）は無視し、type/subtype
    /// 部分のみを大文字小文字無視で比較する。登録パターンが末尾 `/` の
    /// 場合は type プレフィックス一致、それ以外は完全一致。
    #[must_use]
    pub fn matches_content_type(&self, content_type: &str) -> bool {
        let essence = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if essence.is_empty() {
            return false;
        }
        self.compressible_types.iter().any(|pattern| {
            if let Some(prefix) = pattern.strip_suffix('/') {
                essence.starts_with(&format!("{prefix}/"))
            } else {
                essence == *pattern
            }
        })
    }
}

/// [`CompressionConfig`] のビルダー。
#[derive(Debug, Clone)]
pub struct CompressionConfigBuilder {
    min_size: usize,
    compressible_types: Vec<String>,
}

impl Default for CompressionConfigBuilder {
    fn default() -> Self {
        Self {
            min_size: DEFAULT_MIN_SIZE,
            compressible_types: DEFAULT_COMPRESSIBLE_TYPES
                .iter()
                .map(|s| s.to_ascii_lowercase())
                .collect(),
        }
    }
}

impl CompressionConfigBuilder {
    /// 圧縮対象とする body の最小サイズ（バイト）を設定する。既定は 1024。
    ///
    /// `min_size` は非公開フィールドのため、[`apply_compression`] の
    /// 挙動（閾値未満の body は無圧縮のまま返る）で間接的に確認する。
    ///
    /// ```
    /// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
    /// use fandhe_backend_http::response::Response;
    /// use fandhe_backend_plugin_compression::{CompressionConfig, apply_compression};
    ///
    /// let config = CompressionConfig::builder().min_size(2048).build();
    /// let buf = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n";
    /// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
    ///     unreachable!()
    /// };
    /// // 64 バイトの body は閾値 2048 未満のため圧縮されない。
    /// let response = Response::new(200, "x".repeat(64).into_bytes()).with_content_type("text/plain");
    /// let result = apply_compression(&head, &config, response);
    /// assert_eq!(result.header("content-encoding"), None);
    /// ```
    #[must_use]
    pub fn min_size(mut self, min_size: usize) -> Self {
        self.min_size = min_size;
        self
    }

    /// 圧縮対象 `Content-Type` パターンのリストを既定値から丸ごと差し替える。
    ///
    /// 各パターンは大文字小文字を無視して比較する。末尾 `/` は type
    /// プレフィックス一致（例: `"text/"`）、それ以外は type/subtype の完全
    /// 一致（例: `"application/json"`）として扱う
    /// （[`CompressionConfig::matches_content_type`] を参照）。
    ///
    /// # BREACH 類似リスクへの注意
    ///
    /// 秘密情報を含みやすいレスポンス（動的トークンを埋め込む HTML 断片
    /// 等）を返す `Content-Type` は、必要な場合を除き対象から外すことを
    /// 推奨する（本モジュール冒頭 doc の「BREACH 類似の情報漏洩リスク」を
    /// 参照）。
    ///
    /// ```
    /// use fandhe_backend_plugin_compression::CompressionConfig;
    ///
    /// let config = CompressionConfig::builder()
    ///     .compressible_types(vec!["application/json".to_string()])
    ///     .build();
    /// assert!(config.matches_content_type("application/json"));
    /// assert!(!config.matches_content_type("text/plain"));
    /// ```
    #[must_use]
    pub fn compressible_types(mut self, types: Vec<String>) -> Self {
        self.compressible_types = types.into_iter().map(|s| s.to_ascii_lowercase()).collect();
        self
    }

    /// 圧縮対象 `Content-Type` パターンを 1 件追加する（既定リストへの追記）。
    ///
    /// ```
    /// use fandhe_backend_plugin_compression::CompressionConfig;
    ///
    /// let config = CompressionConfig::builder()
    ///     .add_compressible_type("application/x-custom+json")
    ///     .build();
    /// assert!(config.matches_content_type("application/x-custom+json"));
    /// // 既定リストも維持される。
    /// assert!(config.matches_content_type("text/plain"));
    /// ```
    #[must_use]
    pub fn add_compressible_type(mut self, content_type: impl Into<String>) -> Self {
        self.compressible_types
            .push(content_type.into().to_ascii_lowercase());
        self
    }

    /// [`CompressionConfig`] を構築する。
    ///
    /// 現時点で構築時検証が必要な不正な組み合わせは存在しないため
    /// infallible（`compressible_types` を空リストへ差し替えた場合も
    /// 「常に圧縮しない設定」として有効な状態であり、エラーではない）。
    #[must_use]
    pub fn build(self) -> CompressionConfig {
        CompressionConfig {
            min_size: self.min_size,
            compressible_types: self.compressible_types,
        }
    }
}

/// リクエストの `Accept-Encoding` が gzip を受理するか判定する。
///
/// `RequestHead::headers()` で同名ヘッダ全件を走査し、`,` 区切りの各
/// コーディングトークンを `;q=` パラメータとともに解釈する。
///
/// - `gzip` / `x-gzip`（大文字小文字無視）に明示的なエントリがあれば、
///   その `q` 値（省略時 1.0）で受理／拒絶を決める
/// - 明示エントリがなければ `*` エントリの `q` 値にフォールバックする
/// - どちらも存在しなければ「受理しない」（フェイルセーフ）
/// - `q=0` / `q=0.0` 等ゼロの場合は明示的な拒絶として扱う
/// - `q` 値が数値としてパースできない、または `0.0`〜`1.0` の範囲外の場合は
///   そのトークン自体を無視する（解釈不能は「受理しない」側に効く）
///
/// ```
/// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// use fandhe_backend_plugin_compression::accepts_gzip;
///
/// let buf = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip, deflate\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// assert!(accepts_gzip(&head));
///
/// let buf = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip;q=0\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// assert!(!accepts_gzip(&head));
///
/// let buf = b"GET / HTTP/1.1\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// assert!(!accepts_gzip(&head));
/// ```
#[must_use]
pub fn accepts_gzip(head: &RequestHead) -> bool {
    let mut gzip_q: Option<f32> = None;
    let mut star_q: Option<f32> = None;

    for (name, value) in head.headers() {
        if !name.eq_ignore_ascii_case("accept-encoding") {
            continue;
        }
        for token in value.split(',') {
            let mut parts = token.split(';');
            let coding = parts.next().unwrap_or("").trim();
            if coding.is_empty() {
                continue;
            }
            let mut q: f32 = 1.0;
            let mut valid = true;
            for param in parts {
                let param = param.trim();
                let Some(raw) = param
                    .strip_prefix("q=")
                    .or_else(|| param.strip_prefix("Q="))
                else {
                    continue;
                };
                match raw.trim().parse::<f32>() {
                    Ok(v) if (0.0..=1.0).contains(&v) => q = v,
                    _ => valid = false,
                }
            }
            if !valid {
                // 解釈不能な q 値は「受理しない」側へ倒すため、この
                // トークンからの明示的な許可情報を採用しない（無視）。
                continue;
            }
            if coding.eq_ignore_ascii_case("gzip") || coding.eq_ignore_ascii_case("x-gzip") {
                gzip_q = Some(q);
            } else if coding == "*" {
                star_q = Some(q);
            }
        }
    }

    match gzip_q {
        Some(q) => q > 0.0,
        None => star_q.is_some_and(|q| q > 0.0),
    }
}

/// `Response::with_header` の `Err` を「当該ヘッダを付与しない」側へ倒す
/// ヘルパ（`crates/plugin-cors` の同名ヘルパと同一パターン）。
/// `with_header` は失敗時に `self` を返さない契約のため、呼び出し前に
/// `Response` を複製し、失敗時はその複製（変更前の状態）を返す。
fn try_add_header(response: Response, name: &str, value: impl Into<String>) -> Response {
    let fallback = response.clone();
    response.with_header(name, value).unwrap_or(fallback)
}

/// gzip コンテナ形式で `data` を圧縮する。
fn gzip_compress(data: &[u8]) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    let mut encoder = GzEncoder::new(Vec::with_capacity(data.len() / 2), Compression::default());
    // メモリ上の `Vec<u8>` への書き込みのみで I/O エラー要因（ディスク・
    // ネットワーク）が存在しないため、`write_all`/`finish` の失敗は
    // 実質的に起こり得ない。それでも `.unwrap()` は避け、失敗時は空の
    // 圧縮結果を返して呼び出し元（[`apply_compression`]）の「圧縮結果が
    // 元 body 以上なら無圧縮のまま返す」フェイルセーフ経路に委ねる
    // （ライブラリコードでの panic 回避、`.claude/rules/coding-rust.md`）。
    if encoder.write_all(data).is_err() {
        return Vec::new();
    }
    encoder.finish().unwrap_or_default()
}

/// レスポンス後処理型シーム（`crate::plugin::finalize_response`）から呼ばれる
/// 圧縮適用の本体。
///
/// `head`（受理済みリクエスト）・`config`（登録済み設定）・確定済み
/// `response` を受け取り、モジュール冒頭 doc の圧縮判定（条件 2〜6。
/// 条件 1 の「登録済みか」は呼び出し元がこの関数を呼ぶかどうかで判定
/// 済みという契約）をすべて満たす場合のみ body を gzip 圧縮して返す。
/// いずれか不足時は無改変（ただし `Vary` 付与のみ行いうる）で返す。
///
/// ```
/// use fandhe_backend_http::request::{ParseOutcome, parse_request_head};
/// use fandhe_backend_http::response::Response;
/// use fandhe_backend_plugin_compression::{CompressionConfig, apply_compression};
///
/// let config = CompressionConfig::builder().min_size(1).build();
/// let buf = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// let body = "x".repeat(64);
/// let response = Response::new(200, body.clone().into_bytes()).with_content_type("text/plain");
/// let compressed = apply_compression(&head, &config, response);
/// assert_eq!(compressed.header("content-encoding"), Some("gzip"));
/// assert_eq!(compressed.header("vary"), Some("Accept-Encoding"));
/// assert!(compressed.body.len() < body.len());
///
/// // 解凍結果は元の body と一致する（応答同一性、受け入れ基準）。
/// use std::io::Read;
/// let mut decoder = flate2::read::GzDecoder::new(compressed.body.as_slice());
/// let mut decoded = String::new();
/// decoder.read_to_string(&mut decoded).unwrap();
/// assert_eq!(decoded, body);
/// ```
#[must_use]
pub fn apply_compression(
    head: &RequestHead,
    config: &CompressionConfig,
    response: Response,
) -> Response {
    // 条件 2: 2xx かつ 204 以外。
    if response.status == 204 || !(200..300).contains(&response.status) {
        return response;
    }

    // 条件 3・4: Content-Type 一致 + 閾値以上のサイズ。
    let content_type_ok = response
        .header("content-type")
        .is_some_and(|ct| config.matches_content_type(ct));
    let size_ok = response.body.len() >= config.min_size;
    if !(content_type_ok && size_ok) {
        return response;
    }

    // 表現が Accept-Encoding に依存して変わりうる時点で Vary を付与する
    // （圧縮の成否に関わらず、共有キャッシュでの変異混同防止）。重複付与は
    // しない。
    let response = if response.header("vary").is_none() {
        try_add_header(response, "Vary", "Accept-Encoding")
    } else {
        response
    };

    // 条件 6: 二重圧縮防止。
    if response.header("content-encoding").is_some() {
        return response;
    }

    // 条件 5: Accept-Encoding が gzip を受理。
    if !accepts_gzip(head) {
        return response;
    }

    let compressed = gzip_compress(&response.body);
    if compressed.is_empty() || compressed.len() >= response.body.len() {
        // 圧縮失敗、または圧縮結果が元 body 以上（既に圧縮済みに近い等）は
        // 元のレスポンスをそのまま返す（フェイルセーフ）。
        return response;
    }

    let mut response = try_add_header(response, "Content-Encoding", "gzip");
    response.body = compressed;
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_backend_http::request::{ParseOutcome, parse_request_head};

    fn head_from(buf: &[u8]) -> RequestHead {
        match parse_request_head(buf).unwrap() {
            ParseOutcome::Complete { head, .. } => head,
            other => panic!("unexpected parse outcome: {other:?}"),
        }
    }

    #[test]
    fn matches_content_type_ignores_params() {
        let config = CompressionConfig::builder().build();
        assert!(config.matches_content_type("application/json; charset=utf-8"));
        assert!(config.matches_content_type("TEXT/HTML"));
        assert!(!config.matches_content_type("image/png"));
        assert!(!config.matches_content_type(""));
    }

    #[test]
    fn accepts_gzip_multiple_headers_combine() {
        let head = head_from(
            b"GET / HTTP/1.1\r\nAccept-Encoding: deflate\r\nAccept-Encoding: gzip\r\n\r\n",
        );
        assert!(accepts_gzip(&head));
    }

    #[test]
    fn accepts_gzip_wildcard_fallback() {
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: *\r\n\r\n");
        assert!(accepts_gzip(&head));

        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: *;q=0\r\n\r\n");
        assert!(!accepts_gzip(&head));
    }

    #[test]
    fn accepts_gzip_explicit_zero_overrides_wildcard() {
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: *, gzip;q=0\r\n\r\n");
        assert!(!accepts_gzip(&head));
    }

    #[test]
    fn accepts_gzip_invalid_q_value_ignored() {
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip;q=bogus\r\n\r\n");
        assert!(!accepts_gzip(&head));
    }

    #[test]
    fn accepts_gzip_case_insensitive_x_gzip() {
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: X-GZIP\r\n\r\n");
        assert!(accepts_gzip(&head));
    }

    #[test]
    fn apply_compression_skips_below_threshold() {
        let config = CompressionConfig::builder().min_size(1024).build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let response = Response::new(200, b"small".to_vec()).with_content_type("text/plain");
        let result = apply_compression(&head, &config, response.clone());
        assert_eq!(result, response);
    }

    #[test]
    fn apply_compression_skips_non_matching_content_type() {
        let config = CompressionConfig::builder().min_size(1).build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let body = "x".repeat(64);
        let response = Response::new(200, body.into_bytes()).with_content_type("image/png");
        let result = apply_compression(&head, &config, response.clone());
        assert_eq!(result, response);
        assert_eq!(result.header("vary"), None);
    }

    #[test]
    fn apply_compression_skips_without_accept_encoding() {
        let config = CompressionConfig::builder().min_size(1).build();
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        let body = "x".repeat(64);
        let response =
            Response::new(200, body.clone().into_bytes()).with_content_type("text/plain");
        let result = apply_compression(&head, &config, response);
        assert_eq!(result.body, body.into_bytes());
        assert_eq!(result.header("content-encoding"), None);
        // Content-Type 一致・閾値超過は満たすため Vary は付与される。
        assert_eq!(result.header("vary"), Some("Accept-Encoding"));
    }

    #[test]
    fn apply_compression_skips_already_encoded() {
        let config = CompressionConfig::builder().min_size(1).build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let body = "x".repeat(64);
        let response = Response::new(200, body.into_bytes())
            .with_content_type("text/plain")
            .with_header("Content-Encoding", "br")
            .unwrap();
        let result = apply_compression(&head, &config, response.clone());
        // 二重圧縮防止（条件 6）で body・Content-Encoding は無改変。ただし
        // 条件 3・4 は満たすため Vary はこの経路でも付与される
        // （モジュール冒頭 doc の「圧縮の成否に関わらず付与」を参照）。
        assert_eq!(result.body, response.body);
        assert_eq!(result.header("content-encoding"), Some("br"));
        assert_eq!(result.header("vary"), Some("Accept-Encoding"));
    }

    #[test]
    fn apply_compression_skips_non_2xx_and_204() {
        let config = CompressionConfig::builder().min_size(1).build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let body = "x".repeat(64);

        let response =
            Response::new(404, body.clone().into_bytes()).with_content_type("text/plain");
        let result = apply_compression(&head, &config, response.clone());
        assert_eq!(result, response);

        let response = Response::new(204, body.into_bytes()).with_content_type("text/plain");
        let result = apply_compression(&head, &config, response.clone());
        assert_eq!(result, response);
    }

    #[test]
    fn apply_compression_falls_back_when_compressed_is_larger() {
        // 極小 body（圧縮しても gzip ヘッダ分だけ大きくなる）は圧縮しない。
        let config = CompressionConfig::builder().min_size(1).build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let response = Response::new(200, b"a".to_vec()).with_content_type("text/plain");
        let result = apply_compression(&head, &config, response.clone());
        assert_eq!(result.body, response.body);
        assert_eq!(result.header("content-encoding"), None);
    }

    #[test]
    fn apply_compression_roundtrip_matches_original_body() {
        use std::io::Read;

        let config = CompressionConfig::builder().min_size(1).build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let body = "The quick brown fox jumps over the lazy dog. ".repeat(50);
        let response =
            Response::new(200, body.clone().into_bytes()).with_content_type("application/json");
        let result = apply_compression(&head, &config, response);
        assert_eq!(result.header("content-encoding"), Some("gzip"));

        let mut decoder = flate2::read::GzDecoder::new(result.body.as_slice());
        let mut decoded = String::new();
        decoder.read_to_string(&mut decoded).unwrap();
        assert_eq!(decoded, body);
    }
}
