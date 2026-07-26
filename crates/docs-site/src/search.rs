//! 依存ゼロの全文検索インデックス生成（イシュー #396）。
//!
//! # 役割・呼び出し文脈
//!
//! `crate::build::build_site` がページループの中でページごとの本文
//! （`rewritten_body`。prev/next ナビ・サイドバー・ヘッダーを含まない）から
//! [`extract_plain_text`] でプレーンテキストを抽出し、[`SearchIndex`] へ
//! 蓄積する。全ページ分の収集後に [`serialize_index`] で決定的な JSON へ
//! 直列化し、[`validate_index_size`] でサイズ上限を検証してから
//! `out_dir/`[`INDEX_REL_PATH`] へ書き出す（書き出し自体は `build.rs` が担う。
//! 本モジュールは sans-I/O な純関数のみを提供する）。
//!
//! `crate::layout::docs_page` は [`INDEX_REL_PATH`] を
//! [`crate::layout::asset_href`] 経由で検索入力欄の `data-search-index`
//! 属性へ埋め込み、`crate::script::SITE_JS` が実行時に本インデックスを
//! `fetch` して部分一致検索する（ビルド時生成 + 実行時 fetch という構成を
//! 取ることで、外部 JS ライブラリ・追加クレート依存を一切増やさない、
//! `.claude/rules/pay-for-what-you-use.md` の思想を docs ビルドへ準用する）。
//!
//! # セキュリティ不変条件（`.claude/rules/security.md`）
//!
//! 索引 JSON は HTML へインライン埋め込みせず独立ファイルとして配信するため
//! `<script>` コンテキストへの混入経路は無いが、多層防御として
//! [`escape_json_string`] は JSON 必須エスケープ（`"` `\` 制御文字）に加えて
//! `<` `>` `&`（HTML 混入時の実害を無くす）と `U+2028`/`U+2029`
//! （JS 内 `eval` 相当の文脈でテンプレートリテラル・行分割制御文字として
//! 悪用され得る）もエスケープする。
//!
//! 二段のサイズ上限（[`MAX_PAGE_TEXT_BYTES`]・[`MAX_INDEX_BYTES`]）は
//! 無自覚な索引肥大化・DoS 化を防ぐ fail-closed 設計（`build.rs` が
//! [`validate_index_size`] の `Err` をビルド失敗として扱う）。

use fandhe_frontend_core::Node;

/// 索引スキーマのバージョン。将来スキーマを変更する際、JS 側の互換判定に使う。
pub const INDEX_VERSION: u32 = 1;

/// 1 ページあたりの本文プレーンテキスト上限（バイト）。
///
/// 超過分は [`truncate_at_char_boundary`] で決定的に切り詰める（エラーには
/// しない。ページ全文を索引に含めることは目的ではなく、冒頭からの部分一致・
/// 到達性を確保することが目的のため）。
pub const MAX_PAGE_TEXT_BYTES: usize = 4096;

/// 索引全体（直列化済み JSON）の上限（バイト）。超過はビルド失敗
/// （[`validate_index_size`] が `Err` を返し、`build.rs` が fail-closed で
/// `out_dir` に一切書き出さない）。
pub const MAX_INDEX_BYTES: usize = 1024 * 1024;

/// 索引の出力先（`out_dir` 起点の相対パス）。[`crate::script::SCRIPT_REL_PATH`]
/// と同様に `crate::build` の予約名衝突検証（`site/assets/` 配下に同名の
/// 静的アセットを利用者が置くことを禁止する）対象になる。
pub const INDEX_REL_PATH: &str = "assets/search-index.json";

/// ページ内目次の 1 セクション（索引用）。[`crate::layout::TocEntry`] から
/// 1:1 で写像する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSection {
    /// 見出しレベル（`h2` → 2 / `h3` → 3）。
    pub level: u8,
    /// アンカー先 `id` 属性値。
    pub id: String,
    /// 見出しの表示テキスト。
    pub title: String,
}

/// 索引 1 ページ分のエントリ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPage {
    /// ページの遷移先 href（[`crate::layout::asset_href`] 経由で構築済み）。
    pub href: String,
    /// ページタイトル。
    pub title: String,
    /// ページ内見出し列（出現順）。
    pub sections: Vec<SearchSection>,
    /// 本文プレーンテキスト（[`MAX_PAGE_TEXT_BYTES`] 以下に切り詰め済み）。
    pub text: String,
}

/// サイト全体の検索インデックス。[`serialize_index`] の入力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchIndex {
    /// `site/nav.toml` の `[site] base_path`（JS 側が遷移先 URL を組み立てる
    /// 際の基準にする）。
    pub base_path: String,
    /// 宣言順を保持したページ列。
    pub pages: Vec<SearchPage>,
}

/// `Node` 木から本文プレーンテキストを抽出する。
///
/// [`Node::RawHtml`] は取り込まない（docs-site クレートは `raw_html()` を
/// 使わない方針だが、混入した場合でも索引に生 HTML 断片を含めない防御的
/// 実装。`crate::layout::extract_text` と同じ防御姿勢）。要素の子を処理した
/// 後にブロック境界の区切り（半角スペース 1 個）を挿入し、最後に連続空白を
/// 1 個へ正規化する。区切りを入れないと隣接ブロックのテキストが
/// 「導入本文です」のように癒着し、部分一致・可読性が劣化するため。
///
/// # Examples
///
/// ```
/// use fandhe_backend_docs_site::search::extract_plain_text;
/// use fandhe_frontend_core::{div, el, text};
///
/// let node = div(
///     vec![],
///     vec![
///         el("p", vec![], vec![text("Hello")]),
///         el("p", vec![], vec![text("World")]),
///     ],
/// );
/// assert_eq!(extract_plain_text(&node), "Hello World");
/// ```
pub fn extract_plain_text(node: &Node) -> String {
    let mut out = String::new();
    extract_plain_text_into(node, &mut out);
    normalize_whitespace(&out)
}

/// [`extract_plain_text`] の内部再帰実装。
fn extract_plain_text_into(node: &Node, out: &mut String) {
    match node {
        Node::Text(s) => out.push_str(s),
        Node::Element { children, .. } => {
            for child in children {
                extract_plain_text_into(child, out);
            }
            // ブロック境界の区切り。`normalize_whitespace` が連続空白を
            // 1 個へ畳み込むため、テキストを持たない要素の後でも安全に
            // 挿入できる。
            out.push(' ');
        }
        Node::RawHtml(_) => {}
    }
}

/// 連続する空白（改行・タブを含む ASCII 空白）を半角スペース 1 個へ畳み込み、
/// 先頭・末尾の空白を除去する。
fn normalize_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = true; // 先頭の空白を捨てるため true から開始
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
        } else {
            out.push(c);
            last_was_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// UTF-8 文字境界を跨がずに `s` を `max_bytes` 以下へ決定的に切り詰める。
///
/// `String::truncate`・スライスへのバイト添字直指定は文字境界を跨ぐと
/// panic するため（日本語等のマルチバイト文字を含む本文で実際に起こり得る）、
/// `char_indices()` を走査して `max_bytes` 以下の最大境界を探す。
///
/// # Examples
///
/// ```
/// use fandhe_backend_docs_site::search::truncate_at_char_boundary;
///
/// // 日本語（3 バイト/文字）を含む文字列を境界で切り詰める。
/// let s = "こんにちは"; // 15 バイト（3 バイト × 5 文字）
/// assert_eq!(truncate_at_char_boundary(s, 7), "こん");
///
/// // 上限が全長以上なら無変換。
/// assert_eq!(truncate_at_char_boundary("hello", 100), "hello");
/// ```
pub fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = 0;
    for (idx, ch) in s.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    &s[..end]
}

/// JSON 文字列リテラルへエスケープして `out` へ push する（呼び出し側が
/// `"` で囲む契約。本関数自体は囲み `"` を出力しない）。
///
/// JSON 必須（`"` `\` および `U+0000`〜`U+001F` 制御文字）に加え、多層防御
/// として `<` `>` `&` `U+2028`（LINE SEPARATOR）`U+2029`（PARAGRAPH
/// SEPARATOR）も `\uXXXX` 形式でエスケープする（モジュール doc 参照）。
pub fn escape_json_string(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

/// [`SearchIndex`] をキー順固定の決定的 JSON へ直列化する。
///
/// `serde_json` 等の外部依存は追加しない（`Cargo.toml` の依存方針コメント・
/// 受け入れ条件 4）。キー順は `version` → `base_path` → `pages`、ページは
/// `href` → `title` → `sections` → `text`、セクションは
/// `id` → `level` → `title` に固定する（同一入力に対して常に同一バイト列を
/// 返す決定性。2 回ビルドしてのバイト同一比較で検証可能）。
pub fn serialize_index(index: &SearchIndex) -> String {
    let mut out = String::new();
    out.push('{');
    out.push_str("\"version\":");
    out.push_str(&INDEX_VERSION.to_string());
    out.push_str(",\"base_path\":\"");
    escape_json_string(&index.base_path, &mut out);
    out.push_str("\",\"pages\":[");
    for (i, page) in index.pages.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('{');
        out.push_str("\"href\":\"");
        escape_json_string(&page.href, &mut out);
        out.push_str("\",\"title\":\"");
        escape_json_string(&page.title, &mut out);
        out.push_str("\",\"sections\":[");
        for (j, section) in page.sections.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push('{');
            out.push_str("\"id\":\"");
            escape_json_string(&section.id, &mut out);
            out.push_str("\",\"level\":");
            out.push_str(&section.level.to_string());
            out.push_str(",\"title\":\"");
            escape_json_string(&section.title, &mut out);
            out.push_str("\"}");
        }
        out.push_str("],\"text\":\"");
        escape_json_string(&page.text, &mut out);
        out.push_str("\"}");
    }
    out.push_str("]}");
    out
}

/// 直列化済み索引 JSON の失敗理由（上限超過）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexTooLarge {
    /// 直列化済み JSON の実バイト数。
    pub bytes: usize,
    /// 許容上限（バイト）。
    pub max: usize,
}

/// 直列化済み JSON `json` のバイト長が `max_bytes` 以下かを検証する
/// （fail-closed。上限を引数で受け取る純関数にすることで、実サイトでは
/// 到達しない上限超過経路を小さい上限を注入したテストで直接検証できる）。
///
/// # Examples
///
/// ```
/// use fandhe_backend_docs_site::search::validate_index_size;
///
/// assert!(validate_index_size("{}", 1024).is_ok());
/// assert!(validate_index_size("{}", 1).is_err());
/// ```
pub fn validate_index_size(json: &str, max_bytes: usize) -> Result<(), IndexTooLarge> {
    let bytes = json.len();
    if bytes > max_bytes {
        Err(IndexTooLarge {
            bytes,
            max: max_bytes,
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_frontend_core::{div, el, raw_html, text};

    #[test]
    fn truncate_at_char_boundary_cuts_japanese_text_at_a_char_boundary() {
        let s = "こんにちは"; // 3 バイト/文字 × 5 文字 = 15 バイト
        let truncated = truncate_at_char_boundary(s, 7);
        assert!(s.is_char_boundary(truncated.len()));
        assert_eq!(truncated, "こん");
    }

    #[test]
    fn truncate_at_char_boundary_keeps_string_unchanged_when_within_limit() {
        assert_eq!(truncate_at_char_boundary("hello", 100), "hello");
        assert_eq!(truncate_at_char_boundary("hello", 5), "hello");
    }

    #[test]
    fn escape_json_string_escapes_required_and_defense_in_depth_characters() {
        let mut out = String::new();
        escape_json_string("a\"b\\c<d>e&f\u{2028}g\u{2029}h", &mut out);
        assert_eq!(out, "a\\\"b\\\\c\\u003cd\\u003ee\\u0026f\\u2028g\\u2029h");
    }

    #[test]
    fn escape_json_string_escapes_control_characters() {
        let mut out = String::new();
        escape_json_string("a\nb\tc\rd\u{0}e", &mut out);
        assert_eq!(out, "a\\nb\\tc\\rd\\u0000e");
    }

    #[test]
    fn serialize_index_is_deterministic_with_fixed_key_order() {
        let index = SearchIndex {
            base_path: "/fandhe-backend".to_string(),
            pages: vec![SearchPage {
                href: "/fandhe-backend/guide/".to_string(),
                title: "Guide".to_string(),
                sections: vec![SearchSection {
                    level: 2,
                    id: "intro".to_string(),
                    title: "Intro".to_string(),
                }],
                text: "hello world".to_string(),
            }],
        };
        let first = serialize_index(&index);
        let second = serialize_index(&index);
        assert_eq!(first, second);
        assert!(
            first.starts_with(r#"{"version":1,"base_path":"/fandhe-backend","pages":[{"href":"#)
        );
        assert!(first.contains(r#""sections":[{"id":"intro","level":2,"title":"Intro"}]"#));
    }

    #[test]
    fn validate_index_size_rejects_json_exceeding_a_small_injected_limit() {
        let json = "0123456789";
        assert!(validate_index_size(json, 5).is_err());
        let err = validate_index_size(json, 5).unwrap_err();
        assert_eq!(err.bytes, 10);
        assert_eq!(err.max, 5);
    }

    #[test]
    fn validate_index_size_accepts_json_within_a_sufficient_limit() {
        assert!(validate_index_size("0123456789", 1024).is_ok());
    }

    #[test]
    fn extract_plain_text_ignores_raw_html_and_normalizes_whitespace() {
        let node = div(
            vec![],
            vec![
                el("p", vec![], vec![text("  Hello   world  ")]),
                raw_html("<b>ignored</b>"),
                el("p", vec![], vec![text("Second\nparagraph")]),
            ],
        );
        assert_eq!(extract_plain_text(&node), "Hello world Second paragraph");
    }

    #[test]
    fn extract_plain_text_truncates_long_page_body_to_the_configured_limit() {
        let long = "あ".repeat(2000); // 3 バイト × 2000 文字 = 6000 バイト（> 4096）
        let node = el("p", vec![], vec![text(long)]);
        let extracted = extract_plain_text(&node);
        let truncated = truncate_at_char_boundary(&extracted, MAX_PAGE_TEXT_BYTES);
        assert!(truncated.len() <= MAX_PAGE_TEXT_BYTES);
        assert!(extracted.len() > MAX_PAGE_TEXT_BYTES);
    }
}
