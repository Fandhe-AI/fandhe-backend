//! `fandhe_backend_docs_site::markdown::render_markdown` の統合テスト（イシュー #466）。
//!
//! ここで検証する不変条件は 2 つ:
//! 1. 各ブロック構文（見出し / 段落 / リスト / フェンスコード / 引用 / テーブル）が
//!    仕様どおりのノード木（`fandhe_frontend_core::render` の HTML 文字列出力で
//!    厳密比較）へレンダリングされること
//! 2. 全経路が既定エスケープ（REQ-1）を維持し、`<script>` 等の生 HTML がテキストと
//!    してエスケープされること（XSS 回帰）
//!
//! アサーションは決定性・機械検証可能性のため文字列の厳密一致で行う
//! （`.claude/rules/coding-rust.md` の一般規約に沿う）。

use fandhe_backend_docs_site::markdown::render_markdown;
use fandhe_frontend_core::render;

/// `render_markdown` の出力を結合して 1 つの HTML 文字列にする補助関数。
fn render_all(input: &str) -> String {
    render_markdown(input)
        .iter()
        .map(render)
        .collect::<Vec<_>>()
        .join("")
}

// ---------------------------------------------------------------------
// 見出し
// ---------------------------------------------------------------------

#[test]
fn heading_levels_h1_to_h6() {
    let input = "# H1\n\n## H2\n\n### H3\n\n#### H4\n\n##### H5\n\n###### H6\n";
    assert_eq!(
        render_all(input),
        "<h1>H1</h1><h2>H2</h2><h3>H3</h3><h4>H4</h4><h5>H5</h5><h6>H6</h6>"
    );
}

#[test]
fn seven_hashes_falls_back_to_paragraph() {
    assert_eq!(
        render_all("####### not a heading"),
        "<p>####### not a heading</p>"
    );
}

#[test]
fn hash_without_space_falls_back_to_paragraph() {
    // "#tag" のように # 直後に空白がない場合は見出しと認識しない。
    assert_eq!(render_all("#tag"), "<p>#tag</p>");
}

// ---------------------------------------------------------------------
// 段落
// ---------------------------------------------------------------------

#[test]
fn multiline_paragraph_joins_with_space() {
    assert_eq!(
        render_all("line one\nline two\nline three"),
        "<p>line one line two line three</p>"
    );
}

#[test]
fn blank_lines_separate_paragraphs() {
    assert_eq!(
        render_all("first paragraph\n\nsecond paragraph"),
        "<p>first paragraph</p><p>second paragraph</p>"
    );
}

#[test]
fn empty_input_produces_no_nodes() {
    assert_eq!(render_markdown("").len(), 0);
    assert_eq!(render_markdown("\n\n\n").len(), 0);
}

// ---------------------------------------------------------------------
// リスト
// ---------------------------------------------------------------------

#[test]
fn bullet_list_basic() {
    assert_eq!(
        render_all("- one\n- two\n- three"),
        "<ul><li>one</li><li>two</li><li>three</li></ul>"
    );
}

#[test]
fn ordered_list_basic() {
    assert_eq!(
        render_all("1. one\n2. two\n3. three"),
        "<ol><li>one</li><li>two</li><li>three</li></ol>"
    );
}

#[test]
fn bullet_list_star_marker() {
    assert_eq!(
        render_all("* alpha\n* beta"),
        "<ul><li>alpha</li><li>beta</li></ul>"
    );
}

#[test]
fn list_item_continuation_line_merges_into_previous_item() {
    // マーカーなしの継続行（インデント付き）は直前の li のテキストへ結合する。
    assert_eq!(
        render_all("- one\n  continued text\n- two"),
        "<ul><li>one continued text</li><li>two</li></ul>"
    );
}

#[test]
fn nested_list_one_level() {
    assert_eq!(
        render_all("- outer one\n  - inner a\n  - inner b\n- outer two"),
        "<ul><li>outer one<ul><li>inner a</li><li>inner b</li></ul></li><li>outer two</li></ul>"
    );
}

#[test]
fn list_nesting_beyond_max_depth_falls_back_to_continuation_text() {
    // MAX_DEPTH（16）を超えるネストはスタックオーバーフロー防止のため打ち切られ、
    // それ以降のマーカー行は子リストにならず直前アイテムへの継続テキストとして
    // 結合される（境界値のリグレッション回帰、非ブロッキング指摘への対応）。
    //
    // 18 段のマーカー行（インデント 2 段刻み）を用意すると、0〜16 段目
    // （`parse_list` が `depth < MAX_DEPTH` を満たす限り再帰する範囲）までは
    // 実際に `<ul>` としてネストされ（計 17 個）、17 段目のマーカー行は
    // `depth(16) < MAX_DEPTH(16)` が偽になるため子リスト化されず、
    // 直前の `<li>` テキストへ生のマーカー文字列（`- deepest`）ごと結合される。
    let mut lines: Vec<String> = Vec::new();
    for level in 0..18 {
        lines.push(format!("{}- L{level}", "  ".repeat(level)));
    }
    let input = lines.join("\n");

    let output = render_all(&input);

    assert_eq!(
        output.matches("<ul>").count(),
        17,
        "MAX_DEPTH（16）に対応する <ul> ネスト段数は 17 段（0〜16）であるべき: {output}"
    );
    assert!(
        output.contains("L16 - L17"),
        "17 段目のマーカー行は子リスト化されず、生の \"- L17\" ごと直前 li の継続テキストへ \
         結合されるべき: {output}"
    );
    assert!(
        !output.contains("<ul><li>L17"),
        "17 段目が独立した <ul><li> を形成してはならない（MAX_DEPTH 打ち切りの回帰）: {output}"
    );
}

// ---------------------------------------------------------------------
// フェンスコードブロック
// ---------------------------------------------------------------------

#[test]
fn fence_with_lang_info_string() {
    assert_eq!(
        render_all("```rust\nfn main() {}\n```"),
        "<pre><code class=\"language-rust\">fn main() {}</code></pre>"
    );
}

#[test]
fn fence_with_comma_info_string_uses_first_token() {
    assert_eq!(
        render_all("```rust,ignore\nlet x = 1;\n```"),
        "<pre><code class=\"language-rust\">let x = 1;</code></pre>"
    );
}

#[test]
fn fence_with_leading_space_before_lang_token_is_recognized() {
    // フェンス直後に空白を挟んだ info string（```` ``` rust ````）でも言語
    // トークンを正しく認識する（先に trim してから分割する回帰テスト）。
    assert_eq!(
        render_all("``` rust\nfn main() {}\n```"),
        "<pre><code class=\"language-rust\">fn main() {}</code></pre>"
    );
}

#[test]
fn indented_fence_strips_body_indent_matching_open_fence() {
    // 開始フェンスが（最大 3 スペースまで）字下げされている場合、本文行から
    // 同じ幅の字下げを取り除く（字下げをそのまま残すと `pre`/`code` の
    // 空白が意味を持つ出力に余分な字下げが混入する）。
    assert_eq!(
        render_all("  ```\n  fn main() {}\n  ```"),
        "<pre><code>fn main() {}</code></pre>"
    );
}

#[test]
fn closing_fence_with_trailing_whitespace_is_recognized() {
    // 閉じフェンス行の末尾に空白・タブが付いていても閉じ行として認識する。
    // 認識できないと開始フェンスが閉じられず、残り全入力がコードとして
    // 飲み込まれてしまう（見出し等の後続構文が描画から消える回帰）。
    assert_eq!(
        render_all("```\ncode\n```   \n# heading"),
        "<pre><code>code</code></pre><h1>heading</h1>"
    );
}

#[test]
fn fence_without_info_string_has_no_class() {
    assert_eq!(
        render_all("```\nplain text\n```"),
        "<pre><code>plain text</code></pre>"
    );
}

#[test]
fn fence_with_invalid_info_string_has_no_class() {
    // ホワイトリスト外の文字（"）を含む info string は class を付与しない。
    assert_eq!(
        render_all("```\"onerror=alert(1)\nbody\n```"),
        "<pre><code>body</code></pre>"
    );
}

#[test]
fn unclosed_fence_consumes_rest_of_input() {
    assert_eq!(
        render_all("```rust\nfn main() {\n    unclosed"),
        "<pre><code class=\"language-rust\">fn main() {\n    unclosed</code></pre>"
    );
}

#[test]
fn fence_content_is_not_reinterpreted_as_markdown() {
    assert_eq!(
        render_all("```\n# not a heading\n- not a list\n```"),
        "<pre><code># not a heading\n- not a list</code></pre>"
    );
}

// ---------------------------------------------------------------------
// 引用
// ---------------------------------------------------------------------

#[test]
fn blockquote_single_line() {
    assert_eq!(
        render_all("> hello"),
        "<blockquote><p>hello</p></blockquote>"
    );
}

#[test]
fn blockquote_multiline_joins_as_paragraph() {
    assert_eq!(
        render_all("> line one\n> line two"),
        "<blockquote><p>line one line two</p></blockquote>"
    );
}

#[test]
fn blockquote_with_nested_list() {
    assert_eq!(
        render_all("> - item one\n> - item two"),
        "<blockquote><ul><li>item one</li><li>item two</li></ul></blockquote>"
    );
}

#[test]
fn blockquote_nesting_beyond_max_depth_falls_back_to_paragraph() {
    // MAX_DEPTH（16、引用とリストで共有）を超える引用ネストはスタックオーバーフロー
    // 防止のため打ち切られ、それ以降の `>` は再帰的に剥がされず、残った `>` ごと
    // 単一の段落テキストとして扱われる（境界値のリグレッション回帰、
    // 非ブロッキング指摘への対応）。
    //
    // `>` を 20 個連続させると、`parse_quote` は depth 0〜16（計 17 回）呼び出され
    // その都度先頭の `>` を 1 個ずつ剥がして `blockquote` を生成するが、
    // 17 回目（depth=16）の呼び出しは `depth(16) >= MAX_DEPTH(16)` が真になるため
    // それ以上再帰せず、残り 3 個の `>` を含む本文をそのまま段落へ格納する
    // （エスケープにより `&gt;&gt;&gt;x` として出力される）。
    let input = format!("{}x", ">".repeat(20));

    let output = render_all(&input);

    assert_eq!(
        output.matches("<blockquote>").count(),
        17,
        "MAX_DEPTH（16）に対応する <blockquote> ネスト段数は 17 段（0〜16）であるべき: {output}"
    );
    let expected = format!(
        "{}<p>&gt;&gt;&gt;x</p>{}",
        "<blockquote>".repeat(17),
        "</blockquote>".repeat(17)
    );
    assert_eq!(output, expected);
}

// ---------------------------------------------------------------------
// テーブル
// ---------------------------------------------------------------------

#[test]
fn table_basic() {
    let input = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |";
    assert_eq!(
        render_all(input),
        "<table><thead><tr><th>a</th><th>b</th></tr></thead><tbody><tr><td>1</td><td>2</td></tr><tr><td>3</td><td>4</td></tr></tbody></table>"
    );
}

#[test]
fn table_with_alignment_markers_is_ignored() {
    let input = "| a | b |\n|:---|---:|\n| 1 | 2 |";
    assert_eq!(
        render_all(input),
        "<table><thead><tr><th>a</th><th>b</th></tr></thead><tbody><tr><td>1</td><td>2</td></tr></tbody></table>"
    );
}

#[test]
fn delimiter_row_requires_at_least_one_dash_per_cell() {
    // GFM 仕様上、区切りセルは `:` のみでは不成立（最低 1 つの `-` が必須）。
    // `::` のみのセルを許容すると通常テキスト行が誤ってテーブル区切り行と
    // 判定され、後続の `|` 開始行が意図せずテーブルへ取り込まれてしまう
    // （誤爆防止の回帰テスト）。
    // 区切り行と誤認されなくなった結果、テーブルとして開始されず、後続の
    // 通常行と合わせて 1 つの段落（改行はスペース結合）として扱われる。
    assert_eq!(
        render_all("| a | b |\n|::|::|\n| 1 | 2 |"),
        "<p>| a | b | |::|::| | 1 | 2 |</p>"
    );
}

#[test]
fn pipe_line_without_delimiter_row_falls_back_to_paragraph() {
    // 直後が区切り行でない `|` 開始行はテーブルと認識せず段落として扱う（誤爆防止）。
    assert_eq!(render_all("| not a table |"), "<p>| not a table |</p>");
}

#[test]
fn table_missing_cells_are_padded_with_empty_td() {
    let input = "| a | b | c |\n|---|---|---|\n| 1 |";
    assert_eq!(
        render_all(input),
        "<table><thead><tr><th>a</th><th>b</th><th>c</th></tr></thead><tbody><tr><td>1</td><td></td><td></td></tr></tbody></table>"
    );
}

#[test]
fn table_extra_cells_are_not_dropped() {
    let input = "| a | b |\n|---|---|\n| 1 | 2 | 3 |";
    assert_eq!(
        render_all(input),
        "<table><thead><tr><th>a</th><th>b</th></tr></thead><tbody><tr><td>1</td><td>2</td><td>3</td></tr></tbody></table>"
    );
}

// ---------------------------------------------------------------------
// 複合フィクスチャ（既存 docs の構文実態に近い入力）
// ---------------------------------------------------------------------

#[test]
fn fixture_mixed_document_renders_expected_html() {
    let input = include_str!("fixtures/mixed.md");
    let expected = include_str!("fixtures/mixed.expected.html").trim_end();
    assert_eq!(render_all(input), expected);
}

// ---------------------------------------------------------------------
// XSS 回帰（ブロック経路すべてで既定エスケープが保たれること、REQ-1）
// ---------------------------------------------------------------------

const SCRIPT_PAYLOAD: &str = "<script>alert(1)</script>";
const IMG_PAYLOAD: &str = "<img src=x onerror=alert(1)>";

#[test]
fn xss_payload_in_heading_is_escaped() {
    let out = render_all("# <script>alert(1)</script>");
    assert!(!out.contains("<script"));
    assert!(out.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert_eq!(out, "<h1>&lt;script&gt;alert(1)&lt;/script&gt;</h1>");
}

#[test]
fn xss_payload_in_paragraph_is_escaped() {
    let out = render_all(SCRIPT_PAYLOAD);
    assert!(!out.contains("<script"));
    assert_eq!(out, "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>");
}

#[test]
fn xss_payload_in_list_item_is_escaped() {
    // "onerror=" という文字列自体はエスケープ後もテキストとして残り得るが、
    // 危険なのは実際の属性として解釈されることなので、タグ開始マーカー
    // （`<img`）が出力されないこと・全体が仕様どおりエスケープされていること
    // を厳密一致で確認する。
    let out = render_all(&format!("- {IMG_PAYLOAD}"));
    assert!(!out.contains("<img"));
    assert_eq!(out, "<ul><li>&lt;img src=x onerror=alert(1)&gt;</li></ul>");
}

#[test]
fn xss_payload_in_table_cell_is_escaped() {
    let input = format!("| a |\n|---|\n| {SCRIPT_PAYLOAD} |");
    let out = render_all(&input);
    assert!(!out.contains("<script"));
    assert!(out.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
}

#[test]
fn xss_payload_in_blockquote_is_escaped() {
    let out = render_all(&format!("> {SCRIPT_PAYLOAD}"));
    assert!(!out.contains("<script"));
    assert_eq!(
        out,
        "<blockquote><p>&lt;script&gt;alert(1)&lt;/script&gt;</p></blockquote>"
    );
}

#[test]
fn xss_payload_in_fence_content_is_escaped() {
    // フェンス内容も text() 経由で出力されるため、コードブロック脱出（`</code></pre><script>`）
    // を狙った入力もエスケープされ、構造は崩れない。
    let input = "```\n</code></pre><script>alert(1)</script>\n```";
    let out = render_all(input);
    assert!(!out.contains("<script"));
    assert_eq!(
        out,
        "<pre><code>&lt;/code&gt;&lt;/pre&gt;&lt;script&gt;alert(1)&lt;/script&gt;</code></pre>"
    );
}

#[test]
fn xss_payload_in_fence_info_string_does_not_inject_attribute() {
    // info string に " を含めて class 属性からの脱出を試みても、ホワイトリスト
    // 判定（is_valid_lang_token）で class 自体が付与されない。
    let input = "```\"><script>alert(1)</script>\nbody\n```";
    let out = render_all(input);
    assert!(!out.contains("<script"));
    assert!(!out.contains("class="));
    assert_eq!(out, "<pre><code>body</code></pre>");
}

#[test]
fn raw_html_tag_in_paragraph_is_treated_as_text() {
    // HTML ブロックは構文として解釈しない。<div> はテキストとしてエスケープされる。
    let out = render_all("<div class=\"x\">hello</div>");
    assert!(!out.contains("<div"));
    assert_eq!(
        out,
        "<p>&lt;div class=&quot;x&quot;&gt;hello&lt;/div&gt;</p>"
    );
}

// ---------------------------------------------------------------------
// インライン構文（イシュー #467）
// ---------------------------------------------------------------------

#[test]
fn inline_code_basic() {
    assert_eq!(
        render_all("this is `code` here"),
        "<p>this is <code>code</code> here</p>"
    );
}

#[test]
fn inline_code_escapes_special_characters() {
    assert_eq!(
        render_all("`<a & b>`"),
        "<p><code>&lt;a &amp; b&gt;</code></p>"
    );
}

#[test]
fn inline_code_with_double_backtick_can_contain_backtick() {
    // 開始と同じ本数（2 個）の連続で閉じるため、コード中に単独のバッククォート
    // （`` ` ``）を 1 個含められる（CommonMark 簡略版）。
    assert_eq!(render_all("``a`b``"), "<p><code>a`b</code></p>");
}

#[test]
fn inline_code_unclosed_falls_back_to_literal_backtick() {
    assert_eq!(render_all("`unclosed"), "<p>`unclosed</p>");
}

#[test]
fn inline_code_content_is_not_reinterpreted() {
    // コード内はリテラル（リンク・強調を解釈しない）。
    assert_eq!(
        render_all("`[not a link](x) **not strong**`"),
        "<p><code>[not a link](x) **not strong**</code></p>"
    );
}

#[test]
fn emphasis_strong() {
    assert_eq!(
        render_all("**bold** text"),
        "<p><strong>bold</strong> text</p>"
    );
}

#[test]
fn emphasis_em() {
    assert_eq!(render_all("*em* text"), "<p><em>em</em> text</p>");
}

#[test]
fn emphasis_strong_containing_em() {
    assert_eq!(
        render_all("**a *b* c**"),
        "<p><strong>a <em>b</em> c</strong></p>"
    );
}

#[test]
fn emphasis_unclosed_star_falls_back_to_literal() {
    assert_eq!(render_all("*unclosed"), "<p>*unclosed</p>");
}

#[test]
fn emphasis_underscore_is_not_interpreted() {
    // `_`/`__` によるアンダースコア強調は意図的に非対応（既存 docs で不使用、
    // かつ識別子中の `_` を誤解釈するリスクがあるため）。
    assert_eq!(
        render_all("raw_html_lint_e2e is a test name"),
        "<p>raw_html_lint_e2e is a test name</p>"
    );
}

#[test]
fn link_relative_url() {
    assert_eq!(
        render_all("[docs](/guide)"),
        "<p><a href=\"/guide\">docs</a></p>"
    );
}

#[test]
fn link_https_url() {
    assert_eq!(
        render_all("[site](https://example.com)"),
        "<p><a href=\"https://example.com\">site</a></p>"
    );
}

#[test]
fn link_text_can_contain_code_and_emphasis() {
    assert_eq!(
        render_all("[`code` and **bold**](/x)"),
        "<p><a href=\"/x\"><code>code</code> and <strong>bold</strong></a></p>"
    );
}

#[test]
fn emphasis_closing_marker_inside_inline_code_is_not_treated_as_closer() {
    // レビュー指摘イシュー #467: find_closing_run はコードスパンの中身を
    // 読み飛ばさずに走査すると、コード内の `*` を外側の強調の閉じ
    // マーカーと誤認識し、`` `b*c` `` の途中で強調が閉じてしまっていた。
    // コードスパンを丸ごと読み飛ばすことで、外側の強調は末尾の `*` まで
    // 正しく開いたままになる。
    assert_eq!(
        render_all("*a `b*c` d*"),
        "<p><em>a <code>b*c</code> d</em></p>"
    );
}

#[test]
fn strong_closing_marker_inside_inline_code_is_not_treated_as_closer() {
    assert_eq!(
        render_all("**a `b**c` d**"),
        "<p><strong>a <code>b**c</code> d</strong></p>"
    );
}

#[test]
fn link_label_closing_bracket_inside_inline_code_is_not_treated_as_closer() {
    // レビュー指摘イシュー #467: find_char がコードスパンの中身を読み飛ば
    // さずに走査すると、コード内の `]` をリンクラベルの閉じ括弧と誤認識
    // し、`` `a]b` `` の途中でラベルが閉じてしまっていた。
    assert_eq!(
        render_all("[`a]b`](/x)"),
        "<p><a href=\"/x\"><code>a]b</code></a></p>"
    );
}

#[test]
fn link_nesting_is_disallowed_inner_bracket_is_literal() {
    // 最初に完成した [text](url) パターンが優先され、内側の `[` はリンクの
    // 一部（リテラル）として取り込まれる（リンクのネスト禁止、設計どおり）。
    assert_eq!(render_all("[a[b](/u)"), "<p><a href=\"/u\">a[b</a></p>");
}

#[test]
fn link_unclosed_bracket_falls_back_to_literal() {
    assert_eq!(render_all("[not a link"), "<p>[not a link</p>");
}

#[test]
fn link_missing_paren_falls_back_to_literal_bracket_text() {
    assert_eq!(render_all("[text] no parens"), "<p>[text] no parens</p>");
}

#[test]
fn inline_syntax_applies_in_heading() {
    assert_eq!(
        render_all("# **Bold** heading with `code`"),
        "<h1><strong>Bold</strong> heading with <code>code</code></h1>"
    );
}

#[test]
fn inline_syntax_applies_in_list_item() {
    assert_eq!(
        render_all("- [link](/a) and **bold**"),
        "<ul><li><a href=\"/a\">link</a> and <strong>bold</strong></li></ul>"
    );
}

#[test]
fn inline_syntax_applies_in_table_cell() {
    let input = "| a |\n|---|\n| **bold** |";
    assert_eq!(
        render_all(input),
        "<table><thead><tr><th>a</th></tr></thead><tbody><tr><td><strong>bold</strong></td></tr></tbody></table>"
    );
}

#[test]
fn inline_syntax_applies_in_blockquote() {
    assert_eq!(
        render_all("> `code` inside quote"),
        "<blockquote><p><code>code</code> inside quote</p></blockquote>"
    );
}

// ---------------------------------------------------------------------
// XSS 回帰（インライン経路、REQ-1・受け入れ条件対応）
// ---------------------------------------------------------------------

#[test]
fn xss_payload_in_link_text_is_escaped() {
    let out = render_all(&format!("[{SCRIPT_PAYLOAD}](/x)"));
    assert!(!out.contains("<script"));
    assert_eq!(
        out,
        "<p><a href=\"/x\">&lt;script&gt;alert(1)&lt;/script&gt;</a></p>"
    );
}

#[test]
fn xss_payload_in_emphasis_is_escaped() {
    let out = render_all(&format!("**{SCRIPT_PAYLOAD}**"));
    assert!(!out.contains("<script"));
    assert_eq!(
        out,
        "<p><strong>&lt;script&gt;alert(1)&lt;/script&gt;</strong></p>"
    );
}

#[test]
fn xss_payload_in_inline_code_is_escaped() {
    let out = render_all(&format!("`{SCRIPT_PAYLOAD}`"));
    assert!(!out.contains("<script"));
    assert_eq!(
        out,
        "<p><code>&lt;script&gt;alert(1)&lt;/script&gt;</code></p>"
    );
}

#[test]
fn xss_link_attribute_injection_is_confined_to_href_value() {
    // href への属性 breakout（" 注入による onerror= の混入）を試みても、
    // core の属性値エスケープにより " が &quot; へエスケープされ href 値内に
    // 閉じ込められる。onerror が実際の属性として出力されないことを確認する。
    let out = render_all("[x](/a\" onerror=\"y)");
    assert!(!out.contains(" onerror=\""));
    assert_eq!(out, "<p><a href=\"/a&quot; onerror=&quot;y\">x</a></p>");
}

#[test]
fn xss_javascript_scheme_link_does_not_generate_anchor() {
    // url 内の丸括弧はネストを数えず最初の ")" で閉じる簡略実装のため、
    // url は "javascript:alert(1"（末尾の ")" 欠落）として切り出され、続く
    // 単独の ")" はリンク外のリテラルとして出力される。丸括弧の対応追跡は
    // 既存 docs で URL 内に丸括弧を使う実態がないためスコープ外（計画参照）。
    // 本テストの主眼である「javascript: スキームは <a> を生成しない」という
    // 安全性は url の断片化に関わらず成立する。
    let out = render_all("[click](javascript:alert(1))");
    assert!(!out.contains("<a "));
    assert!(!out.contains("javascript:"));
    assert_eq!(out, "<p>click)</p>");
}

#[test]
fn xss_javascript_scheme_uppercase_is_rejected() {
    let out = render_all("[click](JAVASCRIPT:alert(1))");
    assert!(!out.contains("<a "));
}

#[test]
fn xss_javascript_scheme_tab_obfuscation_is_rejected() {
    let out = render_all("[click](java\tscript:alert(1))");
    assert!(!out.contains("<a "));
}

#[test]
fn xss_javascript_scheme_leading_space_is_rejected() {
    let out = render_all("[click]( javascript:alert(1))");
    assert!(!out.contains("<a "));
}

#[test]
fn xss_data_scheme_link_is_rejected() {
    let out = render_all("[click](data:text/html,alert(1))");
    assert!(!out.contains("<a "));
}

#[test]
fn xss_vbscript_scheme_link_is_rejected() {
    let out = render_all("[click](vbscript:alert(1))");
    assert!(!out.contains("<a "));
}

#[test]
fn xss_mailto_scheme_link_is_rejected() {
    // core の is_safe_url は mailto: を許可するが、docs-site 独自の第 1 層は
    // 受け入れ条件（http/https/相対のみ）どおりより厳しく拒否する。
    let out = render_all("[mail](mailto:a@example.com)");
    assert!(!out.contains("<a "));
    assert_eq!(out, "<p>mail</p>");
}

#[test]
fn xss_tel_scheme_link_is_rejected() {
    let out = render_all("[call](tel:0123456789)");
    assert!(!out.contains("<a "));
    assert_eq!(out, "<p>call</p>");
}

#[test]
fn safe_http_scheme_link_is_allowed() {
    assert_eq!(
        render_all("[a](http://example.com)"),
        "<p><a href=\"http://example.com\">a</a></p>"
    );
}

#[test]
fn safe_protocol_relative_link_is_allowed() {
    assert_eq!(
        render_all("[a](//example.com/x)"),
        "<p><a href=\"//example.com/x\">a</a></p>"
    );
}

// ---------------------------------------------------------------------
// インライン閉じマーカー探索の走査幅上限
// （アルゴリズム的計算量 DoS 対策、レビュー指摘イシュー #467）
// ---------------------------------------------------------------------
//
// find_closing_run / find_char は開始位置ごとに閉じマーカーを前方走査する
// ため、上限なしでは「閉じマーカーが見つからない `*`/`` ` ``/`[` の連続」
// に対し最悪 O(n^2) の計算量になる（対策前の実測: 全て `*` の debug ビルド
// で n=262,144 のとき約 33 秒）。MAX_INLINE_SCAN_WINDOW による走査幅の
// 打ち切りで 1 回の探索コストを定数に抑え、全体を O(n) に落とす。
//
// 時間上限は 20 秒とする。線形時間なら共有 self-hosted runner の高負荷時
// でも debug ビルドで数秒（実測 6〜7 秒弱）に収まる一方、O(n^2) へ退行
// すると n=400,000 では 33 秒（n=262,144 実測）を大きく超える 70 秒以上
// かかるため、20 秒でも退行検知力は保たれる。当初の 5 秒上限は runner の
// 並行負荷でわずかに超過する flaky（CI 実測 5.9〜6.6 秒）を起こした。

#[test]
fn oversized_unclosed_emphasis_run_completes_within_bounded_time() {
    // 全て `*` の入力（強調の閉じマーカーがほぼ見つからない最悪ケース）でも
    // 走査幅上限により処理時間が入力サイズに対して線形にとどまることを
    // 確認する。対策前は O(n^2) で発散し、同サイズの入力は debug ビルドでも
    // 数十秒かかった（本テストの回帰対象）。
    let huge_input = "*".repeat(400_000);
    let start = std::time::Instant::now();
    let out = render_all(&huge_input);
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "走査幅上限が機能していれば debug ビルドでも数秒以内に完了するはず: {elapsed:?}"
    );
    assert!(!out.is_empty());
}

#[test]
fn oversized_unclosed_link_bracket_run_completes_within_bounded_time() {
    // `[` が閉じ `]` を伴わず大量に連続する入力（try_link の find_char が
    // 各開始位置で走査幅ぶん走査する最悪ケース）でも走査幅上限により線形に
    // とどまることを確認する。閉じ `]` が存在しないため <a> は一切生成
    // されない。
    let huge_input = "[".repeat(400_000);
    let start = std::time::Instant::now();
    let out = render_all(&huge_input);
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "走査幅上限が機能していれば debug ビルドでも数秒以内に完了するはず: {elapsed:?}"
    );
    assert!(!out.contains("<a "));
}

#[test]
fn emphasis_marker_run_longer_than_scan_window_does_not_panic_or_hang() {
    // find_closing_run の境界処理の回帰: 走査幅上限（2,000 文字）をまたぐ
    // 長さの `*` 連続に対して、走査打ち切り時（同じ文字がまだ続いている
    // 状態で上限に達した場合）に誤って `Some` を返し panic や無限ループを
    // 引き起こさないことを確認する（本文字列に対し `**` を直後に置いた
    // ネストした呼び出しも発生するため、境界条件の組み合わせを踏む）。
    //
    // なお、走査幅の内側に真に閉じる 2 連続が偶然出現した場合に <strong>
    // を生成すること自体は、走査開始位置ごとに再試行する既存の貪欲な
    // バックトラック設計（本モジュール導入時点から変わらない挙動）による
    // ものであり、本テストが検証する不変条件ではない。
    let input = format!("**{}X", "*".repeat(2001));
    let out = render_all(&input);
    assert!(!out.is_empty());
}

#[test]
fn oversized_unclosed_backtick_run_completes_within_bounded_time() {
    // レビュー指摘イシュー #467: try_inline_code は開始バッククォート連続
    // の本数カウントに走査幅上限がなかったため、closing-marker 側の探索
    // （find_closing_run）に上限があるにも関わらず、長いバッククォート
    // 連続一つに対して開始位置ごとに O(n) の再カウントが発生し全体で
    // O(n^2) になっていた（本テストの回帰対象）。上限を適用した後は
    // 他の閉じマーカー走査幅上限テストと同様に線形時間で完了する。
    // 先頭を `a` にして行頭バッククォート連続によるフェンスコードブロック
    // 判定（`fence_open`、ブロックレベル）を回避し、インライン経路
    // （try_inline_code）を確実に通す。
    let huge_input = format!("a{}", "`".repeat(400_000));
    let start = std::time::Instant::now();
    let out = render_all(&huge_input);
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "開始バッククォート連続のカウントに走査幅上限が機能していれば debug ビルドでも数秒以内に完了するはず: {elapsed:?}"
    );
    assert!(!out.contains("<code>"));
}

// ---------------------------------------------------------------------
// 3 連続 `*` による strong+em のネスト
// （レビュー指摘イシュー #467: `***bold***` が常にリテラルへ
// フォールバックしていた不具合の回帰）
// ---------------------------------------------------------------------

#[test]
fn triple_star_emphasis_nests_em_and_strong() {
    // CommonMark 同様、em が strong を包む（`<em><strong>...</strong></em>`）。
    assert_eq!(
        render_all("***bold***"),
        "<p><em><strong>bold</strong></em></p>"
    );
}

#[test]
fn triple_star_emphasis_inside_sentence() {
    assert_eq!(
        render_all("a ***bold*** b"),
        "<p>a <em><strong>bold</strong></em> b</p>"
    );
}

#[test]
fn mismatched_triple_and_double_star_closer_does_not_nest_em_and_strong() {
    // 開始 `***`・閉じ `**` のように本数が一致しない混在ケースはスコープ外
    // （find_closing_run は開始と過不足なく一致する本数の閉じ連続のみを
    // 受理するため、marker_len=3 での照合はここでは成立しない）。GFM の
    // 非対称デリミタ解決（flanking rule）は実装しないため、本関数が
    // 3 連続として厳密解釈することはない。開始位置ごとに再試行する既存の
    // 貪欲な走査（`try_emphasis` 呼び出し自体は本テストの対象外）により、
    // 先頭の `*` 1 文字がリテラルへ落ち、続く `**bold**` 相当の部分が
    // `<strong>` として解釈される場合がある。「常にリテラル」ではなく
    // 「em+strong のネストにはならない」ことが本テストで固定したい不変
    // 条件である。
    assert_eq!(render_all("***bold**"), "<p>*<strong>bold</strong></p>");
}
