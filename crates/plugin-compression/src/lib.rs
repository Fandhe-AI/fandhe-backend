//! `fandhe-backend-plugin-compression`: レスポンス圧縮プラグイン（イシュー #321）。
//!
//! 拡張点対応: レスポンス後処理型（finalize_response）
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
//! `spawn_blocking` 化はスコープ外（後続課題として追跡、
//! `.claude/rules/out-of-scope-tracking.md`）。
//!
//! # チャンク単位のストリーミング gzip 圧縮（イシュー #461）
//!
//! [`apply_compression`] は `Response::body` 全体を前提とする通常応答専用で
//! あり、`Handler::handle_streaming`（#319）の chunked ストリーミング応答
//! （body を一括保持せず bounded mpsc 経由で逐次供給する設計）には使えない
//! （`crate::plugin::finalize_streaming_head` の doc・
//! `docs/design/plugin-boundary.md` 5.9.7 節・5.10.3 節が示す既存の見送り
//! 判断）。本イシューは body 全体のバッファリングを避けたまま、
//! [`StreamingGzipEncoder`]（`flate2::write::GzEncoder<Vec<u8>>` を内包し
//! チャンクごとに sync flush（Z_SYNC_FLUSH 相当）した圧縮済みバイト列を
//! 即時取り出す）で「recv → 圧縮変換 → chunked framing → write」を
//! 1 チャンクずつ処理できるようにし、ストリーミング応答の設計（バック
//! プレッシャ・応答完全性契約）を壊さずに圧縮を接続する。
//!
//! ## 採否の設計判断
//!
//! - **opt-in（既定 OFF）**: [`CompressionConfigBuilder::compress_streaming`]
//!   は既定 `false`。既定 ON にすると既存の `Server::compression` 登録
//!   利用者のストリーミング応答挙動が暗黙に変わるうえ、SSE 等で秘密情報を
//!   流す利用者に BREACH 類似リスク（本モジュール冒頭の該当節を参照）を
//!   黙って背負わせるため、フェイルセーフ側（明示 opt-in）へ倒す。
//! - **`min_size` は非適用**: ストリーミングでは総 body 長が事前に不明で
//!   閾値判定ができない。ストリーミング応答は実運用上大きいことが
//!   ほとんどであり、閾値なしで割り切る。
//! - **チャンクごと sync flush**: `BodyWriter::send` 1 回分が即座にクライアント
//!   でデコード可能になることを優先し（SSE 的な逐次配信の意味論の保存）、
//!   チャンクごとに flush する。flush ごとに約 5 バイトのオーバーヘッドが
//!   生じ、バッファリング一括圧縮より圧縮率は劣る（レイテンシとのトレード
//!   オフ）。
//! - **HTTP/1.1 chunked 経路のみ対象**: HTTP/1.0（EOF 終端・フレーミング
//!   なし）は対象外とし identity のまま返す（クライアント希少・スコープ
//!   最小化、コア側 `write_streaming_response` の doc を参照）。
//! - **エンコーダ失敗時は接続クローズ**（fail-closed）: `Content-Encoding:
//!   gzip` を広告した後にエンコーダがエラーを返した場合、identity バイトへ
//!   切り替えるとストリーム破壊になるため、通常の書き込みエラーと同様に
//!   接続を打ち切る（コア側 `write_streaming_response` 内の呼び出し箇所の
//!   doc を参照）。

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
    compress_streaming: bool,
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

    /// チャンク単位のストリーミング gzip 圧縮（イシュー #461）が有効かを
    /// 返す。既定は `false`（opt-in、[`CompressionConfigBuilder::compress_streaming`]
    /// の doc を参照）。
    #[must_use]
    pub fn compress_streaming(&self) -> bool {
        self.compress_streaming
    }
}

/// [`CompressionConfig`] のビルダー。
#[derive(Debug, Clone)]
pub struct CompressionConfigBuilder {
    min_size: usize,
    compressible_types: Vec<String>,
    compress_streaming: bool,
}

impl Default for CompressionConfigBuilder {
    fn default() -> Self {
        Self {
            min_size: DEFAULT_MIN_SIZE,
            compressible_types: DEFAULT_COMPRESSIBLE_TYPES
                .iter()
                .map(|s| s.to_ascii_lowercase())
                .collect(),
            // 既定 OFF（opt-in）。crate doc「チャンク単位のストリーミング
            // gzip 圧縮」節の設計判断を参照。
            compress_streaming: false,
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

    /// チャンク単位のストリーミング gzip 圧縮（イシュー #461）を有効化する。
    /// 既定は `false`（opt-in）。
    ///
    /// `Handler::handle_streaming`（#319）による chunked ストリーミング応答
    /// にのみ影響し、通常応答（[`apply_compression`]）の挙動は変えない。
    /// `min_size` はストリーミングには適用されない（総 body 長が事前に
    /// 不明なため、crate doc の「採否の設計判断」を参照）。HTTP/1.0 応答
    /// （EOF 終端）には適用されず常に identity のまま返る。
    ///
    /// # BREACH 類似リスクへの注意
    ///
    /// SSE 等で秘密情報と攻撃者制御の入力を同一ストリームに混在させる場合は
    /// 本フラグを有効化しない、または対象エンドポイントを
    /// [`CompressionConfigBuilder::compressible_types`] から除外すること
    /// （モジュール冒頭の「BREACH 類似の情報漏洩リスク」節を参照）。
    ///
    /// ```
    /// use fandhe_backend_plugin_compression::CompressionConfig;
    ///
    /// let config = CompressionConfig::builder().compress_streaming(true).build();
    /// assert!(config.compress_streaming());
    ///
    /// // 既定は無効（opt-in）。
    /// let default_config = CompressionConfig::builder().build();
    /// assert!(!default_config.compress_streaming());
    /// ```
    #[must_use]
    pub fn compress_streaming(mut self, enabled: bool) -> Self {
        self.compress_streaming = enabled;
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
            compress_streaming: self.compress_streaming,
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

/// `Vary` へ `token`（`Accept-Encoding` 想定）を欠落なく反映するヘルパ
/// （イシュー #461 レビュー指摘の修正）。
///
/// `Response::header` は同名ヘッダのうち挿入順で最初の 1 件しか返さない
/// （`crates/http` の doc を参照）ため、`response.header("vary").is_none()`
/// による「Vary が 1 件でも既にあれば付与しない」判定は、CORS プラグイン
/// （`crates/plugin-cors`）が `finalize_response` / `finalize_streaming_head`
/// で先に `Vary: Origin` を確定させる構成（`Server::cors` +
/// `Server::compression` 併用）で `Vary: Accept-Encoding` を取りこぼす。
/// 共有キャッシュが `Origin` のみでバリアント判定し、`Accept-Encoding` を
/// 送らないクライアントへ圧縮済みバイト列を配信しうる不具合につながる。
///
/// 本ヘルパは既存 `Vary` 値（先頭 1 件）を `,` 区切りトークンとして走査し
/// `token` を大文字小文字無視で含むかのみを確認する。含まれなければ
/// `try_add_header` で新規 `Vary` ヘッダ行を追加する（`Set-Cookie` と同様、
/// 同名ヘッダの複数行追加は `Response::with_header` が許容する設計であり、
/// RFC 9110 上も単一ヘッダのカンマ区切り列挙と等価。既存値への上書き
/// マージではなく別行追加に留めるのは、`extra_headers` フィールドが
/// `crates/http` 非公開でこのクレートから書き換えられないため）。
fn add_vary_token(response: Response, token: &str) -> Response {
    let already_present = response.header("vary").is_some_and(|existing| {
        existing
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case(token))
    });
    if already_present {
        response
    } else {
        try_add_header(response, "Vary", token)
    }
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
    // （圧縮の成否に関わらず、共有キャッシュでの変異混同防止）。既存の
    // Vary に Accept-Encoding トークンが含まれる場合のみ重複付与しない
    // （CORS 由来の `Vary: Origin` 等、別トークンの既存 Vary とは共存
    // させる、`add_vary_token` の doc・イシュー #461 レビュー指摘を参照）。
    let response = add_vary_token(response, "Accept-Encoding");

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

/// チャンク単位のストリーミング gzip 圧縮エンコーダ（イシュー #461）。
///
/// `flate2::write::GzEncoder<Vec<u8>>` を内包し、[`encode_chunk`][Self::encode_chunk]
/// 呼び出しごとに `write_all` + sync flush（`Write::flush` が Z_SYNC_FLUSH
/// 相当を行う、flate2 の `zio::Writer::flush` 実装を参照）した圧縮済み
/// バイト列を即時取り出す。body 全体を保持しないため、
/// `Handler::handle_streaming` の bounded mpsc バックプレッシャ・応答完全性
/// 契約（`crate::streaming` モジュール doc）と両立できる
/// （モジュール冒頭「チャンク単位のストリーミング gzip 圧縮」節を参照）。
///
/// `crates/core` の第 5 のシーム（`prepare_streaming_compression`）からのみ
/// 構築される（コンストラクタは非公開、[`begin_streaming_compression`] 経由）。
pub struct StreamingGzipEncoder {
    encoder: flate2::write::GzEncoder<Vec<u8>>,
}

impl StreamingGzipEncoder {
    fn new() -> Self {
        Self {
            encoder: flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default()),
        }
    }

    /// `data` を圧縮し、この呼び出し時点までにデコード可能な圧縮済み
    /// バイト列を返す（sync flush、チャンクごとに約 5 バイトの
    /// オーバーヘッド。モジュール冒頭の「チャンクごと sync flush」節を
    /// 参照）。空 `data` は無出力（`Ok(Vec::new())`）で `write_header` すら
    /// 呼ばない。
    ///
    /// メモリ上の `Vec<u8>` への書き込みのみで I/O エラー要因（ディスク・
    /// ネットワーク）が存在しないため失敗は実質起こり得ないが、`Result` を
    /// 呼び出し元（コア側 `write_streaming_response`）へそのまま伝播し、
    /// 万一失敗した場合は接続クローズ（fail-closed）で処理する契約
    /// （モジュール冒頭「エンコーダ失敗時は接続クローズ」節を参照）。
    pub fn encode_chunk(&mut self, data: &[u8]) -> std::io::Result<Vec<u8>> {
        use std::io::Write;

        if data.is_empty() {
            return Ok(Vec::new());
        }
        self.encoder.write_all(data)?;
        self.encoder.flush()?;
        Ok(std::mem::take(self.encoder.get_mut()))
    }

    /// 残余データ + gzip trailer（CRC32・展開後の長さ）を取り出し、
    /// エンコーダを終端する。
    pub fn finish(self) -> std::io::Result<Vec<u8>> {
        self.encoder.finish()
    }
}

/// ストリーミング応答（`crates/core` の第 5 のシーム、
/// `crate::plugin::prepare_streaming_compression`）から呼ばれる、圧縮確定
/// 判定 + ヘッド改変本体（イシュー #461）。
///
/// 以下の全条件を満たす場合のみ `Content-Encoding: gzip` を付与し
/// `Some(StreamingGzipEncoder)` を返す（モジュール冒頭「採否の設計判断」を
/// 参照）:
///
/// (a) `config.compress_streaming()` が有効（既定 `false`、opt-in）
/// (b) `response` のステータスが 2xx かつ非 bodyless（[`Response::is_bodyless_status`]、
///     1xx・204・304 を包含）
/// (c) 実効 `Content-Type` が `config` の圧縮対象リストに一致
/// (d) `Content-Encoding` が未設定（二重圧縮防止）
/// (e) [`accepts_gzip`] が `true`
///
/// 条件 (a)〜(c) が成立した時点で圧縮成否に関わらず `Vary: Accept-Encoding`
/// を付与する（[`apply_compression`] と同一規則。共有キャッシュでの変異
/// 混同防止）。`min_size`（[`CompressionConfigBuilder::min_size`]）は
/// ストリーミングでは総 body 長が未知のため適用しない。
///
/// # 呼び出し契約（HTTP/1.1 chunked 経路専用）
///
/// 呼び出し元はヘッド直列化（`Response::serialize_chunked_head`）の**前**に
/// 本関数を呼び、`Content-Encoding` 決定後の `Response` をヘッド直列化に
/// 使う。HTTP/1.0（EOF 終端）経路では呼び出さない契約（モジュール冒頭の
/// 「HTTP/1.1 chunked 経路のみ対象」節を参照。呼び出し元が feature 有効時
/// でも HTTP/1.0 分岐へは配線しないことで担保する）。
///
/// ```
/// use fandhe_backend_http::request::{parse_request_head, ParseOutcome};
/// use fandhe_backend_http::response::Response;
/// use fandhe_backend_plugin_compression::{begin_streaming_compression, CompressionConfig};
///
/// let config = CompressionConfig::builder().compress_streaming(true).build();
/// let buf = b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n";
/// let ParseOutcome::Complete { head, .. } = parse_request_head(buf).unwrap() else {
///     unreachable!()
/// };
/// let response = Response::empty(200).with_content_type("text/event-stream");
/// let (response, encoder) = begin_streaming_compression(&head, &config, response);
/// assert_eq!(response.header("content-encoding"), Some("gzip"));
/// assert!(encoder.is_some());
/// ```
#[must_use]
pub fn begin_streaming_compression(
    head: &RequestHead,
    config: &CompressionConfig,
    response: Response,
) -> (Response, Option<StreamingGzipEncoder>) {
    // 条件 (a): opt-in。
    if !config.compress_streaming {
        return (response, None);
    }

    // 条件 (b): 2xx かつ非 bodyless（204 は is_bodyless_status に含まれる）。
    if !(200..300).contains(&response.status) || Response::is_bodyless_status(response.status) {
        return (response, None);
    }

    // 条件 (c): Content-Type 一致。
    let content_type_ok = response
        .header("content-type")
        .is_some_and(|ct| config.matches_content_type(ct));
    if !content_type_ok {
        return (response, None);
    }

    // 表現が Accept-Encoding に依存して変わりうる時点で Vary を付与する
    // （`apply_compression` と同一規則、圧縮の成否に関わらず付与）。CORS
    // （`finalize_streaming_head`）が先に `Vary: Origin` を確定していても
    // `add_vary_token` が別トークンとして共存させる（イシュー #461
    // レビュー指摘の修正）。
    let response = add_vary_token(response, "Accept-Encoding");

    // 条件 (d): 二重圧縮防止。
    if response.header("content-encoding").is_some() {
        return (response, None);
    }

    // 条件 (e): Accept-Encoding が gzip を受理。
    if !accepts_gzip(head) {
        return (response, None);
    }

    let response = try_add_header(response, "Content-Encoding", "gzip");
    (response, Some(StreamingGzipEncoder::new()))
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

    /// 直列化済みレスポンスからヘッド部分（`\r\n\r\n` より前）だけを文字列
    /// として切り出す。gzip 圧縮済み body は UTF-8 として解釈できないため、
    /// ヘッダのみ検証したいテストで使う。
    fn head_text(raw: &[u8]) -> String {
        let sep = b"\r\n\r\n";
        let pos = raw
            .windows(sep.len())
            .position(|w| w == sep)
            .expect("レスポンスに空行区切りがない");
        String::from_utf8(raw[..pos].to_vec()).unwrap()
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

    #[test]
    fn apply_compression_preserves_pre_existing_vary_and_adds_accept_encoding() {
        // イシュー #461 レビュー指摘の回帰テスト: CORS 等が `finalize_response`
        // で先に `Vary: Origin` を確定済みの場合でも、圧縮側は
        // `Accept-Encoding` トークンを別 Vary 行として追加し、既存の
        // `Origin` を失わないこと（`add_vary_token` の doc を参照）。
        let config = CompressionConfig::builder().min_size(1).build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let body = "x".repeat(64);
        let response = Response::new(200, body.into_bytes())
            .with_content_type("text/plain")
            .with_header("Vary", "Origin")
            .unwrap();
        let result = apply_compression(&head, &config, response);

        assert_eq!(result.header("content-encoding"), Some("gzip"));
        // `header()` は挿入順で最初の一致のみを返すため、既存の
        // `Vary: Origin` が保持されていることをここで確認する。
        assert_eq!(result.header("vary"), Some("Origin"));
        // gzip 圧縮済み body は UTF-8 として解釈できないため、ヘッド部分
        // （`\r\n\r\n` より前）のみ切り出して検証する。
        let text = head_text(&result.serialize(true));
        assert!(text.contains("Vary: Origin\r\n"), "text: {text}");
        assert!(text.contains("Vary: Accept-Encoding\r\n"), "text: {text}");
    }

    #[test]
    fn apply_compression_does_not_duplicate_vary_when_already_present() {
        let config = CompressionConfig::builder().min_size(1).build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let body = "x".repeat(64);
        let response = Response::new(200, body.into_bytes())
            .with_content_type("text/plain")
            .with_header("Vary", "Accept-Encoding")
            .unwrap();
        let result = apply_compression(&head, &config, response);

        let text = head_text(&result.serialize(true));
        assert_eq!(text.matches("Vary:").count(), 1, "text: {text}");
    }

    #[test]
    fn begin_streaming_compression_preserves_pre_existing_vary_and_adds_accept_encoding() {
        // イシュー #461 レビュー指摘の回帰テスト（ストリーミング経路側）。
        let config = CompressionConfig::builder()
            .compress_streaming(true)
            .build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let response = Response::empty(200)
            .with_content_type("text/event-stream")
            .with_header("Vary", "Origin")
            .unwrap();
        let (response, encoder) = begin_streaming_compression(&head, &config, response);

        assert!(encoder.is_some());
        assert_eq!(response.header("content-encoding"), Some("gzip"));
        assert_eq!(response.header("vary"), Some("Origin"));
        let text = String::from_utf8(response.serialize_streaming_head_http10()).unwrap();
        assert!(text.contains("Vary: Origin\r\n"), "text: {text}");
        assert!(text.contains("Vary: Accept-Encoding\r\n"), "text: {text}");
    }

    #[test]
    fn streaming_gzip_encoder_roundtrip_multi_chunk() {
        use std::io::Read;

        let mut encoder = StreamingGzipEncoder::new();
        let chunks = ["hello, ", "streaming ", "gzip ", "world"];
        let mut framed = Vec::new();
        for chunk in chunks {
            framed.extend(encoder.encode_chunk(chunk.as_bytes()).unwrap());
        }
        framed.extend(encoder.finish().unwrap());

        let mut decoder = flate2::read::GzDecoder::new(framed.as_slice());
        let mut decoded = String::new();
        decoder.read_to_string(&mut decoded).unwrap();
        assert_eq!(decoded, chunks.concat());
    }

    #[test]
    fn streaming_gzip_encoder_each_chunk_output_independently_decodable() {
        // sync flush（Z_SYNC_FLUSH 相当）の検証: 各 encode_chunk 呼び出しまでの
        // 累積出力（trailer 未送出）を都度 decode すると、そこまでに書き込んだ
        // 平文全体が復元できる（途中経過のバイト列が deflate の flush point で
        // 完結しており、次チャンクを待たずに decode 可能）。
        use std::io::Read;

        let mut encoder = StreamingGzipEncoder::new();
        let mut accumulated_compressed = Vec::new();
        let mut accumulated_plain = String::new();
        for chunk in ["first-chunk-", "second-chunk"] {
            accumulated_compressed.extend(encoder.encode_chunk(chunk.as_bytes()).unwrap());
            accumulated_plain.push_str(chunk);

            let mut decoder = flate2::read::GzDecoder::new(accumulated_compressed.as_slice());
            let mut decoded = String::new();
            // trailer（CRC32・長さ）未送出のため `read_to_string` は
            // `UnexpectedEof` になりうるが、エラー到達までに読めたバイト列は
            // ここまでに書き込んだ平文全体と一致するはずである。
            let _ = decoder.read_to_string(&mut decoded);
            assert_eq!(decoded, accumulated_plain);
        }
    }

    #[test]
    fn streaming_gzip_encoder_empty_chunk_produces_no_output() {
        let mut encoder = StreamingGzipEncoder::new();
        assert_eq!(encoder.encode_chunk(b"").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn streaming_gzip_encoder_finish_without_chunks_is_valid_empty_gzip() {
        use std::io::Read;

        let encoder = StreamingGzipEncoder::new();
        let framed = encoder.finish().unwrap();
        let mut decoder = flate2::read::GzDecoder::new(framed.as_slice());
        let mut decoded = String::new();
        decoder.read_to_string(&mut decoded).unwrap();
        assert_eq!(decoded, "");
    }

    #[test]
    fn begin_streaming_compression_disabled_by_default() {
        let config = CompressionConfig::builder().build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let response = Response::empty(200).with_content_type("text/event-stream");
        let (result, encoder) = begin_streaming_compression(&head, &config, response.clone());
        assert_eq!(result, response);
        assert!(encoder.is_none());
    }

    #[test]
    fn begin_streaming_compression_enabled_adds_headers_and_encoder() {
        let config = CompressionConfig::builder()
            .compress_streaming(true)
            .build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let response = Response::empty(200).with_content_type("text/event-stream");
        let (result, encoder) = begin_streaming_compression(&head, &config, response);
        assert_eq!(result.header("content-encoding"), Some("gzip"));
        assert_eq!(result.header("vary"), Some("Accept-Encoding"));
        assert!(encoder.is_some());
    }

    #[test]
    fn begin_streaming_compression_skips_without_accept_encoding() {
        let config = CompressionConfig::builder()
            .compress_streaming(true)
            .build();
        let head = head_from(b"GET / HTTP/1.1\r\n\r\n");
        let response = Response::empty(200).with_content_type("text/event-stream");
        let (result, encoder) = begin_streaming_compression(&head, &config, response);
        assert_eq!(result.header("content-encoding"), None);
        // Content-Type 一致は満たすため Vary は付与される。
        assert_eq!(result.header("vary"), Some("Accept-Encoding"));
        assert!(encoder.is_none());
    }

    #[test]
    fn begin_streaming_compression_skips_non_matching_content_type() {
        let config = CompressionConfig::builder()
            .compress_streaming(true)
            .build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let response = Response::empty(200).with_content_type("image/png");
        let (result, encoder) = begin_streaming_compression(&head, &config, response);
        assert_eq!(result.header("content-encoding"), None);
        assert_eq!(result.header("vary"), None);
        assert!(encoder.is_none());
    }

    #[test]
    fn begin_streaming_compression_skips_bodyless_status() {
        let config = CompressionConfig::builder()
            .compress_streaming(true)
            .build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let response = Response::empty(204).with_content_type("text/event-stream");
        let (result, encoder) = begin_streaming_compression(&head, &config, response.clone());
        assert_eq!(result, response);
        assert!(encoder.is_none());
    }

    #[test]
    fn begin_streaming_compression_skips_already_encoded() {
        let config = CompressionConfig::builder()
            .compress_streaming(true)
            .build();
        let head = head_from(b"GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
        let response = Response::empty(200)
            .with_content_type("text/event-stream")
            .with_header("Content-Encoding", "br")
            .unwrap();
        let (result, encoder) = begin_streaming_compression(&head, &config, response.clone());
        assert_eq!(result.header("content-encoding"), Some("br"));
        assert_eq!(result.header("vary"), Some("Accept-Encoding"));
        assert!(encoder.is_none());
    }
}
