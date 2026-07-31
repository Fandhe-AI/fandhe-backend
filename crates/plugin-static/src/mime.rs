//! 拡張子 → `Content-Type` の静的推定テーブル（イシュー #318 受け入れ基準）。
//!
//! 外部依存（`mime_guess` 等）を追加せず、crate 内蔵の `&'static str` テーブル
//! で完結させる（`.claude/rules/pay-for-what-you-use.md`）。[`super::try_handle_static`]
//! が解決済みファイルパスの拡張子から呼ぶ想定で、未知拡張子・拡張子なしは
//! [`content_type_for_extension`] が既定値 `application/octet-stream` を返す
//! （MIME スニッフィングを避けるため、`super` モジュールが常時
//! `X-Content-Type-Options: nosniff` を併せて付与する契約と対）。
//!
//! イシュー #423 で、内蔵テーブルに存在しない拡張子を利用者が
//! `StaticFilesConfigBuilder::mime` で個別に補える経路を追加した。
//! [`content_type_for_extension`] はオーバーライド表を内蔵 [`TABLE`] より
//! 優先して線形探索する（利用者による上書きを許容する）。

/// 拡張子（小文字、`.` を含まない）→ `Content-Type` の対応表。
///
/// SPA フロントエンド配信で頻出する形式に絞って収録する（HTML/CSS/JS/画像/
/// フォント/wasm 等）。網羅性より「未知の場合は安全な既定値へ倒す」
/// フェイルクローズ設計を優先する。イシュー #423 で PWA・SSG 配信頻出の
/// 形式（`.webmanifest` 等）を追加した。
const TABLE: &[(&str, &str)] = &[
    ("html", "text/html; charset=utf-8"),
    ("htm", "text/html; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("js", "text/javascript; charset=utf-8"),
    ("mjs", "text/javascript; charset=utf-8"),
    ("json", "application/json"),
    ("map", "application/json"),
    ("webmanifest", "application/manifest+json"),
    ("xml", "application/xml"),
    ("txt", "text/plain; charset=utf-8"),
    ("md", "text/markdown; charset=utf-8"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("ico", "image/x-icon"),
    ("wasm", "application/wasm"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
    ("pdf", "application/pdf"),
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
];

/// 既定 `Content-Type`（未知拡張子・拡張子なしの場合）。
///
/// MIME スニッフィングによる意図しない実行（HTML/JS と誤認されるアップロード
/// 済みファイル等）を避けるための保守的な既定値
/// （`.claude/rules/security.md` A05・[`super::try_handle_static`] の
/// `X-Content-Type-Options: nosniff` 併用と対）。
pub(crate) const DEFAULT_CONTENT_TYPE: &str = "application/octet-stream";

/// ファイル名（またはパス）の拡張子から `Content-Type` を推定する。
///
/// 拡張子の大文字小文字は無視する（`.PNG` も `image/png` に一致）。
/// `overrides`（[`StaticFilesConfigBuilder::mime`]、イシュー #423）を内蔵
/// [`TABLE`] より優先して線形探索し、一致すればその値を返す（利用者による
/// 内蔵エントリの上書きを許容する）。`overrides` はキーが正規化済み（先頭
/// `.` 剥離・小文字化済み）である前提（呼び出し元の `build()` が保証する）。
/// 拡張子がない、またはどちらのテーブルにも存在しない場合は
/// [`DEFAULT_CONTENT_TYPE`] を返す。
pub(crate) fn content_type_for_extension(
    file_name: &str,
    overrides: &[(String, &'static str)],
) -> &'static str {
    let Some(ext) = file_name.rsplit('.').next().filter(|ext| *ext != file_name) else {
        return DEFAULT_CONTENT_TYPE;
    };
    let ext_lower = ext.to_ascii_lowercase();
    overrides
        .iter()
        .find(|(key, _)| *key == ext_lower)
        .map(|(_, value)| *value)
        .or_else(|| {
            TABLE
                .iter()
                .find(|(key, _)| *key == ext_lower)
                .map(|(_, value)| *value)
        })
        .unwrap_or(DEFAULT_CONTENT_TYPE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_OVERRIDES: &[(String, &str)] = &[];

    #[test]
    fn resolves_known_extensions() {
        assert_eq!(
            content_type_for_extension("index.html", NO_OVERRIDES),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type_for_extension("app.js", NO_OVERRIDES),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type_for_extension("logo.PNG", NO_OVERRIDES),
            "image/png"
        );
        assert_eq!(
            content_type_for_extension("bundle.wasm", NO_OVERRIDES),
            "application/wasm"
        );
    }

    #[test]
    fn resolves_webmanifest_extension() {
        // イシュー #423 の受け入れ基準本丸: 内蔵テーブルのみで
        // `.webmanifest` が `application/manifest+json` に解決される。
        assert_eq!(
            content_type_for_extension("manifest.webmanifest", NO_OVERRIDES),
            "application/manifest+json"
        );
    }

    #[test]
    fn resolves_newly_added_modern_extensions() {
        assert_eq!(
            content_type_for_extension("doc.pdf", NO_OVERRIDES),
            "application/pdf"
        );
        assert_eq!(
            content_type_for_extension("track.mp3", NO_OVERRIDES),
            "audio/mpeg"
        );
        assert_eq!(
            content_type_for_extension("clip.webm", NO_OVERRIDES),
            "video/webm"
        );
    }

    #[test]
    fn falls_back_to_octet_stream_for_unknown_or_missing_extension() {
        assert_eq!(
            content_type_for_extension("data.unknownext", NO_OVERRIDES),
            DEFAULT_CONTENT_TYPE
        );
        assert_eq!(
            content_type_for_extension("Makefile", NO_OVERRIDES),
            DEFAULT_CONTENT_TYPE
        );
        assert_eq!(
            content_type_for_extension("", NO_OVERRIDES),
            DEFAULT_CONTENT_TYPE
        );
    }

    #[test]
    fn dotfile_without_further_extension_is_treated_as_no_extension() {
        // `.gitignore` は「拡張子 `gitignore` を持つ拡張子なしファイル」ではなく
        // 「拡張子なしのドットファイル」として扱う（`rsplit('.')` の素朴な適用だと
        // `gitignore` を拡張子と誤認しうるため、`file_name` 全体と一致する場合は
        // 拡張子なし扱いにするガードを設けている）。
        assert_eq!(
            content_type_for_extension(".gitignore", NO_OVERRIDES),
            DEFAULT_CONTENT_TYPE
        );
    }

    #[test]
    fn override_takes_priority_over_builtin_table() {
        let overrides = vec![("json".to_string(), "application/geo+json")];
        assert_eq!(
            content_type_for_extension("data.json", &overrides),
            "application/geo+json"
        );
    }

    #[test]
    fn override_is_used_when_extension_unknown_to_builtin_table() {
        let overrides = vec![("custom".to_string(), "application/x-custom")];
        assert_eq!(
            content_type_for_extension("file.custom", &overrides),
            "application/x-custom"
        );
    }

    #[test]
    fn override_lookup_falls_back_to_builtin_table_when_unmatched() {
        let overrides = vec![("custom".to_string(), "application/x-custom")];
        assert_eq!(
            content_type_for_extension("app.js", &overrides),
            "text/javascript; charset=utf-8"
        );
    }

    #[test]
    fn override_match_is_case_insensitive_on_extension() {
        let overrides = vec![("custom".to_string(), "application/x-custom")];
        assert_eq!(
            content_type_for_extension("FILE.CUSTOM", &overrides),
            "application/x-custom"
        );
    }
}
