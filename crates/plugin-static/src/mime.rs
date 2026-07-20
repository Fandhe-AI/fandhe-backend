//! 拡張子 → `Content-Type` の静的推定テーブル（イシュー #318 受け入れ基準）。
//!
//! 外部依存（`mime_guess` 等）を追加せず、crate 内蔵の `&'static str` テーブル
//! で完結させる（`.claude/rules/pay-for-what-you-use.md`）。[`super::try_handle_static`]
//! が解決済みファイルパスの拡張子から呼ぶ想定で、未知拡張子・拡張子なしは
//! [`content_type_for_extension`] が既定値 `application/octet-stream` を返す
//! （MIME スニッフィングを避けるため、`super` モジュールが常時
//! `X-Content-Type-Options: nosniff` を併せて付与する契約と対）。

/// 拡張子（小文字、`.` を含まない）→ `Content-Type` の対応表。
///
/// SPA フロントエンド配信で頻出する形式に絞って収録する（HTML/CSS/JS/画像/
/// フォント/wasm 等）。網羅性より「未知の場合は安全な既定値へ倒す」
/// フェイルクローズ設計を優先する。
const TABLE: &[(&str, &str)] = &[
    ("html", "text/html; charset=utf-8"),
    ("htm", "text/html; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("js", "text/javascript; charset=utf-8"),
    ("mjs", "text/javascript; charset=utf-8"),
    ("json", "application/json"),
    ("map", "application/json"),
    ("xml", "application/xml"),
    ("txt", "text/plain; charset=utf-8"),
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
/// 拡張子の大文字小文字は無視する（`.PNG` も `image/png` に一致）。拡張子が
/// ない、またはテーブルに存在しない場合は [`DEFAULT_CONTENT_TYPE`] を返す。
pub(crate) fn content_type_for_extension(file_name: &str) -> &'static str {
    let Some(ext) = file_name.rsplit('.').next().filter(|ext| *ext != file_name) else {
        return DEFAULT_CONTENT_TYPE;
    };
    let ext_lower = ext.to_ascii_lowercase();
    TABLE
        .iter()
        .find(|(key, _)| *key == ext_lower)
        .map_or(DEFAULT_CONTENT_TYPE, |(_, value)| *value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_known_extensions() {
        assert_eq!(
            content_type_for_extension("index.html"),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type_for_extension("app.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type_for_extension("logo.PNG"), "image/png");
        assert_eq!(
            content_type_for_extension("bundle.wasm"),
            "application/wasm"
        );
    }

    #[test]
    fn falls_back_to_octet_stream_for_unknown_or_missing_extension() {
        assert_eq!(
            content_type_for_extension("data.unknownext"),
            DEFAULT_CONTENT_TYPE
        );
        assert_eq!(content_type_for_extension("Makefile"), DEFAULT_CONTENT_TYPE);
        assert_eq!(content_type_for_extension(""), DEFAULT_CONTENT_TYPE);
    }

    #[test]
    fn dotfile_without_further_extension_is_treated_as_no_extension() {
        // `.gitignore` は「拡張子 `gitignore` を持つ拡張子なしファイル」ではなく
        // 「拡張子なしのドットファイル」として扱う（`rsplit('.')` の素朴な適用だと
        // `gitignore` を拡張子と誤認しうるため、`file_name` 全体と一致する場合は
        // 拡張子なし扱いにするガードを設けている）。
        assert_eq!(
            content_type_for_extension(".gitignore"),
            DEFAULT_CONTENT_TYPE
        );
    }
}
