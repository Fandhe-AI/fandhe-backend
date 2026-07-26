//! docs サイトが出力する唯一の JS（イシュー #390）。
//!
//! # 役割・呼び出し文脈
//!
//! `crate::layout` は本モジュール追加以前は JS を 1 バイトも出力していな
//! かった。本モジュールはテーマトグル（ダーク/ライト切替）の実装として、
//! 初めて docs サイトへクライアント側 JS を持ち込む。Fandhe-AI/fandhe-frontend
//! の `crates/docs-site/src/script.rs`（イシュー #951 相当）からの部分移植で、
//! スクロールスパイ・検索 UI 等の他機能は本イシューのスコープ外のため含めない
//! （`lib.rs` モジュール doc の移植方針参照）。
//!
//! - [`INLINE_THEME_BOOTSTRAP`]: `crate::layout::docs_page` が `<head>` の
//!   先頭付近（スタイルシートより前）へ同期実行の `<script>` として埋め込む
//!   FOUC 抑止スニペット。`localStorage` に保存済みのテーマがあれば CSS 適用
//!   前に `<html data-theme="...">` を確定させる。
//! - [`SITE_JS`]: `crate::build::build_site` が [`SCRIPT_REL_PATH`]
//!   （`out_dir` 起点）へ書き出す本体。`.docs-theme-toggle` ボタンのラベル・
//!   `aria-pressed` 更新、クリック時の切替・保存、および `hidden` 属性の解除
//!   （配線完了後にのみ可視化する）を担う。イシュー #396 で全文検索の
//!   初期化（`.docs-search-input` の遅延 `fetch`・部分一致検索・結果描画）
//!   も同じ [`SITE_JS`] へ追加した（新規アセットを増やさない方針。
//!   `crate::build` の予約名衝突検証を単純に保つ）。索引 `fetch` が失敗
//!   した場合は `loadFailed` フラグで終端失敗状態を保持し、以降の
//!   `input` イベントでは再 `fetch` を行わない（PR #410 レビュー指摘、
//!   404・ネットワークエラー時にキー入力のたびリクエストが再送される
//!   retry storm を防ぐ）。
//!
//! # セキュリティ不変条件（`.claude/rules/security.md`・`.claude/rules/coding-rust.md`）
//!
//! `Node::Text`（`fandhe_frontend_core`）は `<script>` の中身であっても必ず
//! `escape_html_into` を経由する。`<script>` の中身は HTML パーサが実体参照を
//! 復号しない raw text であるため、エスケープ対象文字（`< > & " '`）を
//! 1 文字でも含む JS ソースを `text()` 経由で埋め込むと構文が壊れる。
//! [`INLINE_THEME_BOOTSTRAP`] / [`SITE_JS`] は文字列リテラルにバッククォート
//! （テンプレートリテラル）のみを使い、`&&` の代わりに `||` を使うことで
//! これらの文字を一切含まない。[`is_escape_safe`] がこの性質を機械検証し、
//! [`inline_theme_bootstrap`] は検証に落ちた場合 `None` を返す fail-closed の
//! アクセサとする（`raw_html()` は導入しない）。
//!
//! `${`（テンプレートリテラル補間）も [`is_escape_safe`] の対象外文字列として
//! 禁止する。本モジュールの定数はすべて `&'static str` で外部入力・
//! `site/nav.toml` 由来の値を一切含まないが、将来の変数補間の混入を
//! テストで機械的にブロックする構造的な防御である。
//!
//! `localStorage` はスクリプトの実行主体（同一オリジンの他スクリプト・
//! 利用者自身）が改変できる非信頼データのため、[`INLINE_THEME_BOOTSTRAP`]・
//! [`SITE_JS`] のいずれも読み出した値を `dark`/`light` の allowlist と
//! 一致した場合のみ `data-theme` へ反映する。

/// [`SITE_JS`] の出力先（`out_dir` 起点の相対パス）。
///
/// `crate::build::build_site` が本パスへ書き出し、`crate::layout::docs_page`
/// が `<script src>`（`defer`）で参照する単一実装点。
pub const SCRIPT_REL_PATH: &str = "assets/site.js";

/// テーマ選択を保存する `localStorage` キー。
///
/// GitHub Pages では fandhe-frontend の docs サイトと同一オリジン
/// （`fandhe-ai.github.io`）になり得るため、移植元の `fandhe-docs-theme` とは
/// 別キーにして衝突を避ける。[`INLINE_THEME_BOOTSTRAP`] と [`SITE_JS`] の
/// 双方が同じキーを参照する契約であることを本モジュールの
/// `script_js_and_inline_bootstrap_share_the_same_storage_key`（`tests`）が
/// 固定する（キー名の二重管理ドリフト検知）。
pub const THEME_STORAGE_KEY: &str = "fandhe-backend-docs-theme";

/// `<head>` の先頭付近（スタイルシートより前）に同期実行で埋め込む FOUC 抑止
/// スニペット。
///
/// `localStorage` から保存済みテーマを読み、`dark`/`light` のいずれかであれば
/// `<html>` の `data-theme` 属性を CSS 適用前に確定させる。`localStorage`
/// アクセス例外（Safari プライベートブラウズ等）は握りつぶし、失敗時は
/// `data-theme` 未設定のまま（`site/assets/site.css` の
/// `@media (prefers-color-scheme: dark)` 経路）へ退避する。
///
/// 責務はここまで（属性設定のみ）。ボタンのイベント配線・ラベル更新はすべて
/// [`SITE_JS`] 側が担う（`site.js` の読み込み失敗時にもこのスニペットだけは
/// 動作し、保存済みテーマの反映は維持される）。
pub const INLINE_THEME_BOOTSTRAP: &str = "try{var t=localStorage.getItem(`fandhe-backend-docs-theme`);if(t===`dark`||t===`light`){document.documentElement.setAttribute(`data-theme`,t);}}catch(e){}";

/// [`SCRIPT_REL_PATH`] へ書き出す `assets/site.js` の全量。
///
/// 責務（テーマトグル、イシュー #390）:
///
/// 1. `document.readyState === "loading"` なら `DOMContentLoaded` を待ち、
///    そうでなければ即座に配線を実行する。
/// 2. `init()` 内で `.docs-theme-toggle` ボタンを取得する（無ければ即
///    return。docs-site 以外のページ・将来の骨格変更で要素が消えても例外を
///    投げない防御的実装）。`querySelector` の呼び出しを `init()` 内に置く
///    ことで、上記 1 の `DOMContentLoaded` 待ちフォールバックが実際に意味を
///    持つ（`init()` 呼び出し前に要素取得・null 判定を済ませてしまうと、
///    まだ解析されていない DOM に対して常に null 判定される死んだ分岐に
///    なる）。
/// 3. 実効テーマを解決する: `<html data-theme>` 属性値（`dark`/`light` のみ
///    採用） → 無ければ `matchMedia("(prefers-color-scheme: dark)")`。
/// 4. ボタンのラベル・`aria-pressed` を実効テーマに合わせて初期化する
///    （この時点では `data-theme` を書き込まない。利用者が未選択なら OS 設定
///    追従のままにする）。
/// 5. `click` で実効テーマの反対側へ切替 → `localStorage` へ保存（例外は
///    握りつぶす） → `data-theme` 属性を更新 → ラベル更新。
/// 6. **すべての配線が完了した後にのみ** `hidden` 属性を解除する。`hidden` の
///    除去を `<head>` のインラインスニペットや CSS 側で行うと、`site.js` の
///    読み込み失敗（ネットワーク断・将来 CSP 等）時に「押しても何も起きない
///    ボタン」が残ってしまう。JS 無効時だけでなく JS が届かなかった場合の
///    受け入れ条件（「非表示 + OS 設定追従」）を満たすため、可視化は配線完了
///    後に限定する（レビューで安易に単純化しないこと）。
///
/// 責務（全文検索、イシュー #396）:
///
/// 1. `.docs-search` / `.docs-search-input` / `#docs-search-results` を
///    取得する（無ければ即 return）。
/// 2. 索引 URL は `data-search-index` 属性から読む（空なら return）。
/// 3. 索引は初回 `focus` または初回 `input` のいずれか早い方で 1 度だけ
///    `fetch` する（`loading` フラグで多重取得を抑止）。取得失敗は
///    `.catch` でサイレントに諦め、UI は結果 0 件のまま壊さない
///    （設計上の意図的なフォールバック。将来の安易な簡略化で消さないこと）。
/// 4. クエリは小文字化して部分一致判定する。スコアはタイトル一致 + 3 /
///    セクション見出し一致 + 2 / 本文一致 + 1 の決定的加算とし、0 点の
///    ページは除外した上でスコア降順に並べ替え、上位 `SEARCH_MAX_RESULTS`
///    （`SITE_JS` 内 JS 定数、8 件）のみを描画する。
/// 5. 結果の描画は `document.createElement` + `textContent` +
///    `setAttribute` のみで行う（`innerHTML` 等は使わない、下記参照）。
///    href は必ず `/` から始まり `//` から始まらないもの（同一オリジンの
///    相対パス）のみを描画する多層防御を行う（索引はビルド時生成の信頼
///    データだが、将来の改変・配信改ざんに対する保険。OWASP A10 SSRF 対策
///    と同種の発想を流用）。
/// 6. `Escape` キーで入力・結果をクリアする。空クエリ・0 件時は結果パネルへ
///    `hidden` を戻し、レイアウトを崩さない。
/// 7. **すべての配線が完了した後にのみ** `.docs-search` の `hidden` を
///    解除する（テーマトグルと同じ fail-closed パターン、上記手順 6 参照）。
///
/// 文字列リテラルはすべてバッククォート（テンプレートリテラル。補間は使わ
/// ない）を使い、`&&` の代わりに `||` またはネストした `if` を、比較演算子
/// `<`/`>` の代わりに `!==`/`===`/`indexOf(...) !== -1`/sort コンパレータを
/// 使うことでエスケープ対象文字（`< > & " '`）を含まない
/// （[`is_escape_safe`] 参照）。`innerHTML` / `insertAdjacentHTML` /
/// `document.write` / `eval` / `new Function` は使わない（DOM 操作は
/// `setAttribute`/`removeAttribute`/`textContent`/`createElement`/
/// `appendChild`/`addEventListener` に限定する）。
pub const SITE_JS: &str = "\
(function () {
  var STORAGE_KEY = `fandhe-backend-docs-theme`;
  var SEARCH_MAX_RESULTS = 8;
  var toggle;

  function effectiveTheme() {
    var attr = document.documentElement.getAttribute(`data-theme`);
    if (attr === `dark` || attr === `light`) {
      return attr;
    }
    var prefersDark = false;
    if (window.matchMedia) {
      prefersDark = window.matchMedia(`(prefers-color-scheme: dark)`).matches;
    }
    return prefersDark ? `dark` : `light`;
  }

  function applyLabel(theme) {
    toggle.setAttribute(`aria-pressed`, theme === `dark` ? `true` : `false`);
    toggle.textContent = theme === `dark` ? `Light` : `Dark`;
  }

  function storeTheme(theme) {
    try {
      window.localStorage.setItem(STORAGE_KEY, theme);
    } catch (err) {
      // localStorage が使えない環境（Safari プライベートブラウズ等）では
      // 保存をあきらめ、今回の切替自体は続行する。
    }
  }

  function init() {
    toggle = document.querySelector(`.docs-theme-toggle`);
    if (!toggle) {
      return;
    }
    applyLabel(effectiveTheme());
    toggle.addEventListener(`click`, function () {
      var next = effectiveTheme() === `dark` ? `light` : `dark`;
      storeTheme(next);
      document.documentElement.setAttribute(`data-theme`, next);
      applyLabel(next);
    });
    // 配線がすべて完了した後にのみ可視化する（上記 doc コメント手順 6）。
    toggle.removeAttribute(`hidden`);
  }

  function initSearch() {
    var container = document.querySelector(`.docs-search`);
    if (!container) {
      return;
    }
    var input = document.querySelector(`.docs-search-input`);
    if (!input) {
      return;
    }
    var results = document.querySelector(`#docs-search-results`);
    if (!results) {
      return;
    }
    var indexUrl = input.getAttribute(`data-search-index`);
    if (!indexUrl) {
      return;
    }

    var indexData = null;
    var loading = false;
    var loadFailed = false;

    function loadIndex() {
      if (indexData) {
        return;
      }
      if (loading) {
        return;
      }
      if (loadFailed) {
        // 直前の fetch が失敗して終端状態に入っている。404 やネットワーク
        // エラー後の再入力のたびに再試行し続けるのを避ける
        // （キー入力ごとの無条件リトライ防止）。
        return;
      }
      loading = true;
      fetch(indexUrl)
        .then(function (res) {
          if (!res.ok) {
            throw new Error(`search index fetch failed`);
          }
          return res.json();
        })
        .then(function (data) {
          indexData = data;
          loading = false;
          renderResults(input.value);
        })
        .catch(function () {
          // 索引取得に失敗しても検索 UI 自体は壊さず、結果 0 件のまま
          // フォールバックする（上記 doc コメント手順 3）。loadFailed を
          // 立てて以降の input イベントでの無条件再試行を止める終端失敗
          // 状態とする。
          loading = false;
          loadFailed = true;
        });
    }

    function clearResults() {
      while (results.firstChild) {
        results.removeChild(results.firstChild);
      }
    }

    function isSafeHref(href) {
      if (typeof href !== `string`) {
        return false;
      }
      if (href.indexOf(`/`) !== 0) {
        return false;
      }
      if (href.indexOf(`//`) === 0) {
        return false;
      }
      return true;
    }

    function scorePage(page, query) {
      var score = 0;
      if (page.title.toLowerCase().indexOf(query) !== -1) {
        score = score + 3;
      }
      page.sections.forEach(function (section) {
        if (section.title.toLowerCase().indexOf(query) !== -1) {
          score = score + 2;
        }
      });
      if (page.text.toLowerCase().indexOf(query) !== -1) {
        score = score + 1;
      }
      return score;
    }

    function renderResults(rawQuery) {
      var query = rawQuery.toLowerCase();
      clearResults();
      if (query.length === 0) {
        results.setAttribute(`hidden`, ``);
        return;
      }
      if (!indexData) {
        results.setAttribute(`hidden`, ``);
        return;
      }
      var matches = [];
      indexData.pages.forEach(function (page) {
        var score = scorePage(page, query);
        if (score !== 0) {
          matches.push({ page: page, score: score });
        }
      });
      matches.sort(function (a, b) {
        return b.score - a.score;
      });
      var top = matches.slice(0, SEARCH_MAX_RESULTS);
      if (top.length === 0) {
        results.setAttribute(`hidden`, ``);
        return;
      }
      var list = document.createElement(`ul`);
      top.forEach(function (entry) {
        var page = entry.page;
        if (!isSafeHref(page.href)) {
          return;
        }
        var item = document.createElement(`li`);
        var link = document.createElement(`a`);
        link.setAttribute(`href`, page.href);
        link.textContent = page.title;
        item.appendChild(link);
        list.appendChild(item);
      });
      results.appendChild(list);
      results.removeAttribute(`hidden`);
    }

    input.addEventListener(`focus`, loadIndex);
    input.addEventListener(`input`, function () {
      loadIndex();
      renderResults(input.value);
    });
    input.addEventListener(`keydown`, function (event) {
      if (event.key === `Escape`) {
        input.value = ``;
        clearResults();
        results.setAttribute(`hidden`, ``);
      }
    });

    // 配線がすべて完了した後にのみ可視化する（上記 doc コメント手順 7）。
    container.removeAttribute(`hidden`);
  }

  function ready() {
    init();
    initSearch();
  }

  if (document.readyState === `loading`) {
    document.addEventListener(`DOMContentLoaded`, ready);
  } else {
    ready();
  }
})();
";

/// `source` が HTML エスケープ対象文字（`< > & " '`）を 1 文字も含まず、かつ
/// テンプレートリテラル補間（`${`）を含まないかを判定する純関数。
///
/// `fandhe_frontend_core::escape_html_into` の変換対象文字と完全一致させる
/// ことで、`<script>` の中身（HTML パーサが実体参照を復号しない raw text）に
/// 埋め込んでも構文が壊れないことを保証する。`${` の禁止は、将来変数補間を
/// 追加しようとした際にこのテストが機械的に検知するための構造的な防御である
/// （変数補間は非信頼データを script コンテキストへ注入する経路になり得るため、
/// docs-site では導入しない方針）。
pub fn is_escape_safe(source: &str) -> bool {
    !source
        .chars()
        .any(|c| matches!(c, '<' | '>' | '&' | '"' | '\''))
        && !source.contains("${")
}

/// [`INLINE_THEME_BOOTSTRAP`] が [`is_escape_safe`] を満たす場合のみ `Some`
/// を返す fail-closed のアクセサ。
///
/// `crate::layout::docs_page` はこの関数が `None` を返した場合 `<script>`
/// 自体を出力しない（壊れた JS を配信するくらいなら `prefers-color-scheme`
/// 追従へ退避する）。
pub fn inline_theme_bootstrap() -> Option<&'static str> {
    if is_escape_safe(INLINE_THEME_BOOTSTRAP) {
        Some(INLINE_THEME_BOOTSTRAP)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_theme_bootstrap_is_escape_safe() {
        assert!(is_escape_safe(INLINE_THEME_BOOTSTRAP));
    }

    #[test]
    fn site_js_is_escape_safe() {
        assert!(is_escape_safe(SITE_JS));
    }

    #[test]
    fn inline_theme_bootstrap_accessor_returns_some_for_the_safe_constant() {
        assert_eq!(inline_theme_bootstrap(), Some(INLINE_THEME_BOOTSTRAP));
    }

    #[test]
    fn is_escape_safe_rejects_html_escape_target_characters() {
        assert!(!is_escape_safe("a<b"));
        assert!(!is_escape_safe("a>b"));
        assert!(!is_escape_safe("a&b"));
        assert!(!is_escape_safe("a\"b"));
        assert!(!is_escape_safe("a'b"));
    }

    #[test]
    fn is_escape_safe_rejects_template_literal_interpolation() {
        assert!(!is_escape_safe("var x = `${y}`;"));
    }

    #[test]
    fn is_escape_safe_accepts_plain_js_without_quotes_or_interpolation() {
        assert!(is_escape_safe(
            "(function () { var x = `plain`; return x; })();"
        ));
    }

    /// キー名の二重管理ドリフト検知: [`INLINE_THEME_BOOTSTRAP`] と
    /// [`SITE_JS`] の双方が [`THEME_STORAGE_KEY`] と同じ文字列を参照する
    /// ことを固定する（片方だけキー名を変更してリロード後の復元が壊れる
    /// 事故を防ぐ）。
    #[test]
    fn script_js_and_inline_bootstrap_share_the_same_storage_key() {
        assert!(INLINE_THEME_BOOTSTRAP.contains(THEME_STORAGE_KEY));
        assert!(SITE_JS.contains(THEME_STORAGE_KEY));
    }

    /// `localStorage` アクセスの例外握りつぶし（try/catch）が消えていないこと
    /// を固定する。Safari プライベートブラウズ等での例外時にスクリプト全体が
    /// 停止し、既存機能まで壊れる回帰を防ぐ回帰テスト。
    #[test]
    fn inline_theme_bootstrap_swallows_localstorage_exceptions() {
        assert!(INLINE_THEME_BOOTSTRAP.contains("try{"));
        assert!(INLINE_THEME_BOOTSTRAP.contains("catch"));
    }

    #[test]
    fn site_js_swallows_localstorage_exceptions() {
        assert!(SITE_JS.contains("try {"));
        assert!(SITE_JS.contains("catch"));
    }

    /// [`SITE_JS`] は `hidden` の解除をイベント配線完了後にのみ行う（上記
    /// doc コメント手順 6）。`removeAttribute` 呼び出しが `init` 関数の最後
    /// （`addEventListener` の後）に位置することを、文字列上の出現順で
    /// 固定する。
    #[test]
    fn site_js_reveals_toggle_only_after_click_handler_is_wired() {
        let listener_pos = SITE_JS
            .find("addEventListener")
            .expect("SITE_JS should wire a click handler");
        let reveal_pos = SITE_JS
            .find("removeAttribute(`hidden`)")
            .expect("SITE_JS should reveal the toggle by removing the hidden attribute");
        assert!(
            listener_pos < reveal_pos,
            "hidden の解除はイベント配線より後である必要がある"
        );
    }

    /// レビュー指摘（イシュー #390）の回帰テスト: `.docs-theme-toggle` の
    /// `querySelector` 呼び出しが `init` 関数の中（`readyState` 分岐より後）
    /// に位置することを固定する。トップレベルで即時実行してしまうと、
    /// 上記 doc コメント手順 1 が説明する「`readyState === "loading"` なら
    /// `DOMContentLoaded` を待つ」フォールバックが、要素取得前に済んだ
    /// null 判定によって意味を持たなくなる（defer 実行時には実害がなくとも
    /// doc コメントの契約とコードが乖離するデッドコード化を防ぐ）。
    #[test]
    fn site_js_queries_toggle_element_inside_init_not_at_top_level() {
        let ready_state_check_pos = SITE_JS
            .find("document.readyState")
            .expect("SITE_JS should branch on document.readyState");
        let query_selector_pos = SITE_JS
            .find("document.querySelector(`.docs-theme-toggle`)")
            .expect("SITE_JS should query the toggle element");
        assert!(
            query_selector_pos < ready_state_check_pos,
            "querySelector の呼び出しは init 関数定義内（readyState 分岐より前のソース位置）にある必要がある"
        );

        let init_fn_pos = SITE_JS
            .find("function init()")
            .expect("SITE_JS should define an init function");
        assert!(
            init_fn_pos < query_selector_pos,
            "querySelector の呼び出しは init 関数の中に位置する必要がある"
        );
    }

    /// [`SITE_JS`] は危険な DOM 操作 API（`innerHTML`/`insertAdjacentHTML`/
    /// `document.write`/`eval`/`new Function`）を使わない（OWASP A03）。
    #[test]
    fn site_js_does_not_use_dangerous_dom_apis() {
        for needle in [
            "innerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
            "new Function",
        ] {
            assert!(!SITE_JS.contains(needle), "SITE_JS should not use {needle}");
        }
    }

    /// [`SITE_JS`] が検索入力欄（`.docs-search-input`）と索引 URL 属性
    /// （`data-search-index`）を参照することを固定する（イシュー #396）。
    #[test]
    fn site_js_references_search_input_and_index_attribute() {
        assert!(SITE_JS.contains(".docs-search-input"));
        assert!(SITE_JS.contains("data-search-index"));
        assert!(SITE_JS.contains("#docs-search-results"));
    }

    /// 検索索引の `fetch` 失敗をサイレントにフォールバックする `catch` が
    /// 存在することを固定する（イシュー #396 計画 5 節手順 3。索引取得失敗時
    /// も UI を壊さない契約の回帰テスト）。
    #[test]
    fn site_js_search_fetch_has_a_silent_catch_fallback() {
        let fetch_pos = SITE_JS
            .find("fetch(indexUrl)")
            .expect("SITE_JS should fetch the search index");
        let catch_pos = SITE_JS
            .find(".catch(function ()")
            .expect("SITE_JS should swallow search index fetch failures");
        assert!(
            fetch_pos < catch_pos,
            "catch は fetch(indexUrl) より後に位置する必要がある"
        );
    }

    /// [`SITE_JS`] は検索 UI（`.docs-search`）の `hidden` 解除を、入力欄への
    /// イベント配線がすべて完了した後にのみ行う（テーマトグルと同じ
    /// fail-closed パターン、上記 doc コメント手順 7）。
    #[test]
    fn site_js_reveals_search_ui_only_after_wiring_is_complete() {
        let keydown_listener_pos = SITE_JS
            .find("input.addEventListener(`keydown`")
            .expect("SITE_JS should wire a keydown handler on the search input");
        let reveal_pos = SITE_JS
            .find("container.removeAttribute(`hidden`)")
            .expect("SITE_JS should reveal the search UI by removing the hidden attribute");
        assert!(
            keydown_listener_pos < reveal_pos,
            "検索 UI の hidden 解除はイベント配線より後である必要がある"
        );
    }

    /// 検索結果の href 検証（`isSafeHref`）が `/` 始まり・`//` 非開始のみを
    /// 受理することを固定する（OWASP A10 SSRF 対策と同種の多層防御、
    /// イシュー #396 計画 5 節手順 5）。
    #[test]
    fn site_js_search_validates_result_hrefs_before_rendering() {
        assert!(SITE_JS.contains("function isSafeHref(href)"));
        assert!(SITE_JS.contains("href.indexOf(`/`) !== 0"));
        assert!(SITE_JS.contains("href.indexOf(`//`) === 0"));
    }

    /// レビュー指摘（PR #410 Bugbot）の回帰テスト: 検索索引の `fetch` が
    /// 失敗した場合、`loadFailed` という終端失敗状態が `catch` 内で
    /// 立てられ、`loadIndex` の先頭（`fetch` 呼び出しより前）でその状態を
    /// 見て早期リターンすることを固定する。この構造がないと、404 や
    /// ネットワークエラー後に検索ボックスへの `input` イベントのたびに
    /// 無条件で `fetch` が再試行されてしまう。
    #[test]
    fn site_js_search_fetch_failure_sets_terminal_state_to_avoid_retry_storm() {
        let load_index_pos = SITE_JS
            .find("function loadIndex()")
            .expect("SITE_JS should define loadIndex");
        let load_failed_check_pos = SITE_JS
            .find("if (loadFailed)")
            .expect("SITE_JS should short-circuit loadIndex once a fetch failure is recorded");
        let fetch_pos = SITE_JS
            .find("fetch(indexUrl)")
            .expect("SITE_JS should fetch the search index");
        let catch_pos = SITE_JS
            .find(".catch(function ()")
            .expect("SITE_JS should swallow search index fetch failures");
        let load_failed_set_pos = SITE_JS
            .rfind("loadFailed = true;")
            .expect("SITE_JS should record a terminal failure state on fetch error");

        assert!(
            load_index_pos < load_failed_check_pos,
            "loadFailed の判定は loadIndex 関数の中に位置する必要がある"
        );
        assert!(
            load_failed_check_pos < fetch_pos,
            "loadFailed の早期リターンは fetch 呼び出しより前に位置する必要がある"
        );
        assert!(
            catch_pos < load_failed_set_pos,
            "loadFailed = true の代入は catch ハンドラの中に位置する必要がある"
        );
    }
}
