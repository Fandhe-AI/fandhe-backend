"""setup-github-pages 共通部品: TOML サブセットのパーサと brand.toml の検証。

# 役割・境界

`rebrand_site.py`（生成 dist の後処理）と `check_site.py`（生成前の事前検証）が
同じ解釈で `nav.toml` / `brand.toml` を読み、同じ入力検証を通すための共有モジュール。
対象リポジトリの `tools/docs-site-gen/` へ 3 つの .py を一緒に配置する前提で、同じ
ディレクトリからの `import _common` で読み込まれる。標準ライブラリのみに依存する。

# なぜ tomllib を使わないか

生成器（fandhe-frontend docs-site の nav.rs）は TOML の厳密なサブセット
（`key = "文字列"` のみ。整数・bool・配列・inline table は不可）しか受理しない。
tomllib は上位互換のため「tomllib では通るが生成器が落とす」入力を事前検知できない。
事前検証の判定を生成器と揃えるため、nav.rs の文法を同じ制約で再実装している。
"""

from __future__ import annotations

import os
import re
import stat
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path

# nav.rs の MAX_INPUT_BYTES と同値。
MAX_INPUT_BYTES = 1024 * 1024

# 生成物のブランド表示に使う文字列の上限。ヘッダー・フッターのレイアウトが崩れる長さを防ぐ。
MAX_TEXT_LEN = 120

# scaffold.py が書き込むプレースホルダー。他の記法と衝突しない固有接頭辞にして、
# ユーザーの Markdown 本文に偶然現れる `__INIT__` 等を誤検出しないようにしている。
PLACEHOLDER_RE = re.compile(r"__SGP_[A-Z0-9_]+__")

# 表示順を偽装する双方向制御文字（Trojan Source）。ブランド表示・タイトルに混入させない。
BIDI_RE = re.compile("[\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]")

# C0・DEL・C1（NEL=\x85 を含む）と行・段落区切り（U+2028/2029）。行ベースの解釈が
# 実装（Python / Rust / TOML 系パーサ）で食い違う文字を、表示値へ入れさせない。
CONTROL_RE = re.compile("[\x00-\x1f\x7f-\x9f\u2028\u2029]")

# GitHub の命名規則を 1 箇所で定義する（scaffold.py・check_repo（SKILL.md）・brand.toml 検証が共有）。
# owner: 英数字とハイフン、先頭・末尾ハイフン不可、連続ハイフン不可、39 文字以内。
# repo: 英数字・`-`・`_`・`.`、100 文字以内。`.` / `..` 単独と `.git` 終端は不可
# （`_example` `.github` `a..b` は有効な名前）。
OWNER_RE = re.compile(r"^[A-Za-z0-9]+(?:-[A-Za-z0-9]+)*$")
REPO_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]{1,100}$")


def valid_owner(owner: str) -> bool:
    return len(owner) <= 39 and OWNER_RE.fullmatch(owner) is not None


def valid_repo_name(name: str) -> bool:
    return (
        REPO_NAME_RE.fullmatch(name) is not None
        and name not in (".", "..")
        and not name.endswith(".git")
    )


REPOSITORY_RE = re.compile(r"^https://github\.com/(?P<owner>[^/]+)/(?P<repo>[^/]+)$")
LANG_RE = re.compile(r"^[a-z]{2,3}(-[A-Za-z0-9]{1,8})*$")
COLOR_RE = re.compile(r"^#[0-9a-fA-F]{6}$")
LETTER_RE = re.compile(r"^[A-Za-z0-9]$")
FF_REV_RE = re.compile(r"^[0-9a-f]{40}$")

# 出力に上流ブランド名が残らないことを検査する語（大文字小文字を区別しない）。
UPSTREAM_BRAND = "fandhe-frontend"

# 上流を指す表示だけを「残存」「入力拒否」の対象にする。単純な部分一致にすると、利用者のリポジトリ名が
# `fandhe-frontend-docs` のような場合に、自サイトの正当なリンク（base_path・GitHub URL）まで
# 上流の残存と誤検出する。語境界で区別する。
#
# _WORD_TEXT: 表示文字列（brand / tagline / title 等）用。`fandhe-frontend` が独立した語として現れたら拒否
#   （`fandhe-frontend-docs` のような別の語の一部は許可）。
# _WORD_ATTR: 生成 HTML の残存検査用。上の語境界に加え、直前が `/` `.` のもの（`/fandhe-frontend-docs/`
#   や `github.com/acme/fandhe-frontend` のような URL・パスの一部）は利用者のものとして除外する。
_WORD_TEXT_RE = re.compile(r"(?<![A-Za-z0-9_-])fandhe-frontend(?![A-Za-z0-9_-])", re.I)
RESIDUAL_RE = re.compile(
    r"(?<![A-Za-z0-9_/.-])fandhe-frontend(?![A-Za-z0-9_-])"
    r"|github\.com/Fandhe-AI/fandhe-frontend(?![A-Za-z0-9_.-])"
    r"|crates\.io/crates/fandhe-frontend",
    re.I,
)


# 人間には見えず LLM には読める文字は、指示文を仕込む経路になる（Unicode タグ文字・ゼロ幅文字など）。
# 一般カテゴリ Cc（制御）・Cf（書式: ゼロ幅・bidi・タグ文字・U+00AD・U+2060〜2064 など）・Cs・Co・Cn・
# Zl・Zp に加え、カテゴリは Mn / Lo だが不可視な変異セレクタ・結合用の文字・フィラーを明示的に含める。
_INVISIBLE_CATEGORIES = frozenset({"Cc", "Cf", "Cs", "Co", "Cn", "Zl", "Zp"})
_INVISIBLE_EXTRA = frozenset(
    [0x034F, 0x115F, 0x1160, 0x17B4, 0x17B5, 0x2800, 0x3164, 0xFFA0, 0xFEFF]
    + list(range(0x180B, 0x180F)) + list(range(0xFE00, 0xFE10)) + list(range(0xE0100, 0xE01F0))
)


def _is_hidden(ch: str) -> bool:
    return unicodedata.category(ch) in _INVISIBLE_CATEGORIES or ord(ch) in _INVISIBLE_EXTRA


def sanitize(value: object, limit: int = 300) -> str:
    """対象リポジトリ由来の文字列を、端末・ログ・エージェントの文脈へ出す前に無害化する（1 行にする）。

    制御文字（ESC・改行・タブ・NEL 等）と、人間に見えない文字（双方向制御・ゼロ幅・Unicode タグ・変異セレクタ等）は
    `\\uXXXX`（BMP 外は `\\UXXXXXXXX`）へ置換し、長さを制限する。nav.toml の title・差分の行・パース失敗の
    断片・git の origin など、攻撃者が内容を決められる文字列は、出力先がターミナルでも、読み手が AI エージェント
    でも、指示文や偽の行として働かないようにする（出力は常に「データ」であり、指示として扱わない）。
    """
    out = []
    for ch in str(value):
        if _is_hidden(ch):
            out.append(f"\\u{ord(ch):04x}" if ord(ch) <= 0xFFFF else f"\\U{ord(ch):08x}")
        else:
            out.append(ch)
    res = "".join(out)
    return res if len(res) <= limit else res[:limit] + "…"


def read_bounded_text(path: Path, cap: int) -> str:
    """通常ファイルを上限付きで UTF-8 として読む。上限超過・通常ファイルでない・UTF-8 でないは ValueError / OSError。

    呼び出し側が事前に `resolves_inside` で読み取り先を確かめる（symlink を辿るかどうかは呼び出し側の方針）。
    """
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0))
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise ValueError("通常ファイルではない")
        with os.fdopen(fd, "rb") as fh:
            fd = -1
            data = fh.read(cap + 1)
    finally:
        if fd >= 0:
            os.close(fd)
    if len(data) > cap:
        raise ValueError(f"{cap} バイトを超える")
    return data.decode("utf-8")


def has_upstream_word(text: str) -> bool:
    """表示文字列に上流名が独立した語として含まれるか（入力検証用）。"""
    return _WORD_TEXT_RE.search(text) is not None


def is_upstream_repo(owner: str, repo: str) -> bool:
    """上流リポジトリそのもの（Fandhe-AI/fandhe-frontend。大文字小文字・.git を正規化）か。"""
    r = repo.lower()
    if r.endswith(".git"):
        r = r[:-4]
    return owner.lower() == "fandhe-ai" and r == UPSTREAM_BRAND


def _resolve_real(path: Path) -> Path:
    """`path`（未作成でもよい）の symlink 解決後の実体。未作成の末端は、存在する最も近い祖先の realpath に連結する。"""
    probe = path
    rest: list[str] = []
    while not os.path.lexists(probe):
        rest.append(probe.name)
        probe = probe.parent
    return Path(os.path.realpath(probe)).joinpath(*reversed(rest))


def _in_git_dir(root_real: Path, real: Path) -> bool:
    git_dir = root_real / ".git"
    return real == git_dir or git_dir in real.parents


def resolves_inside(root_real: Path, path: Path) -> bool:
    """`path`（未作成でもよい）を symlink 解決した実体が root_real の配下（`.git` 配下を除く）に収まるか。

    書き込み先・読み取り先を限定する不変条件の Python 側の唯一の実装（bash 側は build-local.sh の guard_path）。
    祖先の途中が root の外を指す symlink だと、mkdir・write・read が root の外へ到達するため、
    操作の前にここで検証する。`.git` 配下（設定・フック・オブジェクト）はスキルの読み書き対象ではないため
    拒否する（`.gitignore -> .git/config` のようなリンクや、親ディレクトリ経由の到達を防ぐ）。
    """
    real = _resolve_real(path)
    if _in_git_dir(root_real, real):
        return False
    return real == root_real or root_real in real.parents


def write_target_problem(root_real: Path, path: Path) -> str | None:
    """root 配下への書き込み先として不適なら理由、問題なければ None。

    Python 側で「書く」前に必ず通す唯一のゲート。bash 側 guard_path と同じ不変条件
    （実体が root 配下・末端が symlink でない）に加え、既存の宛先は通常ファイルに限る
    （ディレクトリ・デバイス等へ書かない）。末端 symlink は、リンク先が root の内か外かを問わず拒否する
    （リポジトリ内を指す `.gitignore -> .git/config` でも、追記が別の設定ファイルを壊すため）。
    """
    if path.is_symlink():
        return "シンボリックリンク"
    real = _resolve_real(path)
    if _in_git_dir(root_real, real):
        return "`.git` 配下は書き込み対象にできない"
    if not (real == root_real or root_real in real.parents):
        return "対象の外へ解決される（親ディレクトリが symlink の可能性）"
    if os.path.lexists(path) and not path.is_file():
        return "通常ファイルではない"
    return None


class SubsetError(ValueError):
    """TOML サブセットの構文違反。メッセージには行番号を含める。"""


@dataclass
class Table:
    header: str
    line: int
    values: dict[str, str] = field(default_factory=dict)


def _parse_quoted(value_part: str, line: int) -> tuple[str, str]:
    if not value_part.startswith('"'):
        raise SubsetError(f"line {line}: 二重引用符の文字列のみ使用できる")
    out: list[str] = []
    i = 1
    while i < len(value_part):
        c = value_part[i]
        if c == '"':
            return "".join(out), value_part[i + 1 :]
        if c == "\\":
            i += 1
            if i >= len(value_part):
                raise SubsetError(f"line {line}: エスケープが途中で終わっている")
            e = value_part[i]
            mapping = {'"': '"', "\\": "\\", "n": "\n", "t": "\t"}
            if e not in mapping:
                raise SubsetError(f"line {line}: 未対応のエスケープ \\{e}")
            out.append(mapping[e])
        else:
            out.append(c)
        i += 1
    raise SubsetError(f"line {line}: 文字列が閉じていない")


def _check_trailing(rest: str, line: int) -> None:
    rest = rest.lstrip()
    if rest and not rest.startswith("#"):
        raise SubsetError(f"line {line}: 末尾に余分な内容 `{rest}`")


def parse_subset(text: str, allowed_headers: set[str]) -> list[Table]:
    """TOML サブセットを `Table` のリスト（出現順）へ変換する。

    `[a]` / `[[a.b]]` は同じく `Table(header="a" / "a.b")` として並べる。
    ヘッダー名は `allowed_headers` に含まれるものだけを受理する（未知は SubsetError）。
    """
    if len(text.encode("utf-8")) > MAX_INPUT_BYTES:
        raise SubsetError("入力が 1 MiB 上限を超えている")
    tables: list[Table] = []
    # str.splitlines() は \x0b \x0c \x1c-\x1e \x85 U+2028/2029 でも分割するが、生成器（Rust の
    # str::lines）は \n のみで分割し末尾の \r を落とす。解釈を揃えるため \n 分割 + 末尾 \r 除去にする。
    for no, raw in enumerate(text.split("\n"), start=1):
        raw = raw[:-1] if raw.endswith("\r") else raw
        s = raw.strip()
        if not s or s.startswith("#"):
            continue
        if s.startswith("[["):
            end = s.find("]]")
            if end < 0:
                raise SubsetError(f"line {no}: `]]` が無い")
            header = s[2:end].strip()
            _check_trailing(s[end + 2 :], no)
            if header not in allowed_headers:
                raise SubsetError(f"line {no}: 未知のテーブル [[{header}]]")
            tables.append(Table(header, no))
            continue
        if s.startswith("["):
            end = s.find("]")
            if end < 0:
                raise SubsetError(f"line {no}: `]` が無い")
            header = s[1:end].strip()
            _check_trailing(s[end + 1 :], no)
            if header not in allowed_headers:
                raise SubsetError(f"line {no}: 未知のテーブル [{header}]")
            tables.append(Table(header, no))
            continue
        eq = s.find("=")
        if eq < 0:
            raise SubsetError(f"line {no}: `key = \"value\"` の形式ではない")
        key = s[:eq].strip()
        if not key or not re.fullmatch(r"[A-Za-z0-9_]+", key):
            raise SubsetError(f"line {no}: 不正なキー `{key}`")
        if not tables:
            raise SubsetError(f"line {no}: テーブルの外にキーがある")
        value, rest = _parse_quoted(s[eq + 1 :].lstrip(), no)
        _check_trailing(rest, no)
        if key in tables[-1].values:
            raise SubsetError(f"line {no}: キー `{key}` が重複している")
        tables[-1].values[key] = value
    return tables


NAV_HEADERS = {
    "site",
    "section",
    "section.page",
    "section.group",
    "section.group.page",
    "menu",
    "menu.item",
}


def parse_nav(text: str) -> list[Table]:
    return parse_subset(text, NAV_HEADERS)


# ---------------------------------------------------------------- brand.toml


@dataclass
class Brand:
    brand: str
    repository: str
    owner: str
    repo: str
    tagline: str
    copyright: str
    lang: str
    version_badge: str
    favicon_letter: str
    favicon_color: str

    @property
    def base_path(self) -> str:
        """GitHub Pages の公開パス。`<owner>.github.io` リポジトリ（User/Org サイト）はルート配信。"""
        if self.repo.lower() == f"{self.owner.lower()}.github.io":
            return ""
        return f"/{self.repo}"


class BrandError(ValueError):
    pass




def _text(values: dict[str, str], key: str, *, required: bool) -> str:
    v = values.get(key, "")
    if required and not v.strip():
        raise BrandError(f"brand.toml: `{key}` は必須")
    if CONTROL_RE.search(v):
        raise BrandError(f"brand.toml: `{key}` に制御文字を含められない")
    if len(v) > MAX_TEXT_LEN:
        raise BrandError(f"brand.toml: `{key}` は {MAX_TEXT_LEN} 文字以内")
    if has_upstream_word(v):
        # 後処理後の残存検査（出力に上流ブランド名が無いこと）と区別できなくなるため入力段階で拒否する。
        raise BrandError(
            f"brand.toml: `{key}` に上流名 `{UPSTREAM_BRAND}` を独立した語として含められない"
            "（生成後の残存検査と区別できない。`fandhe-frontend-docs` のような別の語の一部は可）"
        )
    if BIDI_RE.search(v):
        raise BrandError(f"brand.toml: `{key}` に双方向制御文字を含められない")
    if PLACEHOLDER_RE.search(v):
        raise BrandError(f"brand.toml: `{key}` にプレースホルダー（__SGP_*__）が残っている")
    return v


# brand.toml の必須キーと追記例の既定値。スキルのテンプレートに新しい必須キーが増えたとき、
# 既存の brand.toml（利用者編集のため scaffold は上書きしない）に不足があれば、汎用の検証エラーではなく
# 「不足キーと追記例」を案内する。キーを増やすときはここへ追加する（tests が不足の案内を検査する）。
BRAND_REQUIRED_KEYS: dict[str, str] = {
    "brand": "<ヘッダーのブランド名>",
    "repository": "https://github.com/<owner>/<repo>",
    "copyright": "© <年> <名義>",
    "lang": "ja",
    "favicon_letter": "<英数字1文字>",
    "favicon_color": "#2b6cb0",
}


def load_brand(path: Path) -> Brand:
    try:
        text = read_bounded_text(path, 64 * 1024)
        tables = parse_subset(text, {"brand"})
    except (OSError, ValueError, SubsetError) as e:
        raise BrandError(f"brand.toml を読めない: {e}") from e
    if len(tables) != 1:
        raise BrandError("brand.toml: [brand] テーブルがちょうど 1 つ必要")
    v = tables[0].values
    allowed = {
        "brand", "repository", "tagline", "copyright", "lang",
        "version_badge", "favicon_letter", "favicon_color",
    }
    unknown = set(v) - allowed
    if unknown:
        raise BrandError(f"brand.toml: 未知のキー {sorted(unknown)}")

    missing = [k for k in BRAND_REQUIRED_KEYS if k not in v]
    if missing:
        example = ", ".join(f'{k} = "{BRAND_REQUIRED_KEYS[k]}"' for k in missing)
        raise BrandError(
            f"brand.toml: 必須キーが不足している: {', '.join(missing)}。[brand] テーブルへ追記する"
            f"（スキルのテンプレートに新しいキーが増えた場合は templates/brand.toml を参照）。追記例: {example}"
        )

    brand = _text(v, "brand", required=True)
    tagline = _text(v, "tagline", required=False)
    copyright_ = _text(v, "copyright", required=True)
    version_badge = _text(v, "version_badge", required=False)

    repository = v.get("repository", "")
    m = REPOSITORY_RE.fullmatch(repository)
    if not m or not valid_owner(m.group("owner")) or not valid_repo_name(m.group("repo")):
        raise BrandError(
            "brand.toml: `repository` は https://github.com/<owner>/<repo> 形式のみ許可"
            "（GitHub の命名規則。`.` `..` 単独・.git 終端は不可）"
        )
    repo = m.group("repo")
    if is_upstream_repo(m.group("owner"), repo):
        raise BrandError(
            "brand.toml: `repository` が上流リポジトリ（Fandhe-AI/fandhe-frontend）そのもの。"
            "自サイトのリポジトリ URL を指定する"
        )

    lang = v.get("lang", "")
    if not LANG_RE.fullmatch(lang):
        raise BrandError("brand.toml: `lang` は BCP 47 風（例: ja / en / en-US）")
    letter = v.get("favicon_letter", "")
    if not LETTER_RE.fullmatch(letter):
        raise BrandError("brand.toml: `favicon_letter` は英数字 1 文字")
    color = v.get("favicon_color", "")
    if not COLOR_RE.fullmatch(color):
        raise BrandError("brand.toml: `favicon_color` は #RRGGBB 形式")

    return Brand(
        brand=brand,
        repository=repository,
        owner=m.group("owner"),
        repo=repo,
        tagline=tagline,
        copyright=copyright_,
        lang=lang,
        version_badge=version_badge,
        favicon_letter=letter,
        favicon_color=color,
    )
