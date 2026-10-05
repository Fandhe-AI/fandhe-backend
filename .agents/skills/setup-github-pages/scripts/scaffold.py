#!/usr/bin/env python3
"""対象リポジトリへ docs サイト一式（wrapper・後処理・workflow・初期サイト）を配置・更新する。

# 役割・境界

SKILL.md の Step 1（`--detect` によるモード判定）、新規構築フローの Step N1、更新フローの Step U1
（`--json`。exit 3 の分岐で `--show-diff`）から呼ばれる。ビルドは build-local.sh（Step N3 / U2）の担当。スキル同梱の templates/ と scripts/ を対象リポジトリの所定位置へ
コピーしつつ、`__SGP_*__` プレースホルダーをユーザー入力で置換する（シェルの sed で置換すると入力値の
`/` `&` `\\` 等で置換式が壊れる・注入されるため、値は本スクリプトが検証・エスケープして書き込む）。
同じスクリプトが「新規構築」と「構築済みリポジトリの更新（スキルの最新の構成への追従）」の両方を担う。

# モード

`--detect` で対象リポジトリを判定する（書き込みなし）。`mode=new`（痕跡なし）/ `update`（配置マニフェスト
または旧版配置の痕跡あり）/ `foreign`（上流 fandhe-frontend 自身、またはスキル由来でない同名構成）。
判定の根拠は `kind`（upstream / unrelated / manifest / legacy / none）で機械可読に返す。
通常実行ではモードを自動判定し、update では `--owner/--repo/--title` を省略できる
（所有ファイルは既存の pages.yml の branch から、利用者ファイルは既存のものを保持するため）。

# 契約

- スキル所有ファイル（FILES の OWNED）は、配置時の sha256 を `tools/docs-site-gen/.scaffold-manifest.json` に
  記録する。再実行時の分類（全件を先に分類し、1 件でも競合なら何も書かない）:
  生成予定と一致 → 変更なし / 配置後に未編集（ハッシュがマニフェストと一致）→ 自動更新 /
  それ以外 → 競合（exit 3。`--update` で強制上書き）。競合の内容確認は `--show-diff`（書き込みなし。
  symlink・root 外・特殊ファイルは内容を読まない）。
- pages.yml の `sgp:user-paths` 区間（追加の監視パス）は利用者が編集してよい例外。区間の中身は検証して
  保持し、「未編集」判定と記録ハッシュは区間を空にした正規形で行う。
- 利用者編集ファイル（USER）は常に保持する。更新モードで欠けていても再作成せず「欠落」と報告する
  （再作成は `--owner/--repo/--branch/--title` を明示したときだけ）。
- マニフェストは信頼しない入力として扱う。厳密に検証して 1 つでも違反すれば丸ごと無視（=マニフェストなし。
  自動更新は行わず、不一致は競合になる安全側）。マニフェスト内のパスは FILES の固定パスとの突き合わせに
  のみ使い、書き込み・削除・表示の対象にしない。スキルで廃止された所有ファイルはスキル側の固定リスト
  （DEPRECATED_OWNED）だけで判定し、**表示のみ**で自動削除しない。
- 対象リポジトリ由来の文字列（パス・check_site の断片・origin 等）は出力前に無害化する（`sanitize`）。
- `.gitignore` へは未登録の行だけを追記する。
- 書き込み・読み取りは `--target` 配下の通常ファイルに限る（symlink・`.git` 配下は不可。`write_target_problem`）。

終了コード: 0 成功 / 2 入力不正・書き込み先が不適・適用対象外 / 3 競合（所有ファイルの不一致・配置後に編集）/
4 配置後の check_site 失敗（ファイルは配置済み。指摘箇所を直す）。`--json` 指定時は、どの終了コードでも
JSON を 1 つ標準出力へ出す。
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parent.parent
# append: 同じディレクトリに標準モジュール名のファイルがあっても、標準ライブラリを先に解決させる
sys.path.append(str(Path(__file__).resolve().parent))
from _common import (  # noqa: E402
    BIDI_RE, COLOR_RE, CONTROL_RE, FF_REV_RE, PLACEHOLDER_RE, LANG_RE, LETTER_RE, MAX_TEXT_LEN, UPSTREAM_BRAND, Brand,
    has_upstream_word, is_upstream_repo, resolves_inside, sanitize, write_target_problem, valid_owner, valid_repo_name,
)

BRANCH_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$")

# (テンプレート相対パス, 配置先相対パス, 実行権限, 種別)
# 種別 OWNED: スキルが所有する機械的ファイル。内容はスキルの版と引数から一意に決まり、利用者が編集する前提では
#   ない。既存の内容が生成予定と違えば「競合」とし、何も書かずに中止する（別用途の同名ファイルを黙って
#   「配置済み」と扱うと、必要な構成が無いまま後続 Step へ進んでしまうため）。スキル更新時は --update で上書きする。
# 種別 USER: 利用者が編集する前提のファイル。再実行時に必ず内容が食い違うため、既存なら「保持（利用者編集）」
#   とし、書き換えない。内容の妥当性は check_site.py が検証する。
OWNED, USER = "owned", "user"
PAGES_REL = ".github/workflows/pages.yml"
FILES = [
    ("templates/docs-site-gen/Cargo.toml", "tools/docs-site-gen/Cargo.toml", False, OWNED),
    ("templates/docs-site-gen/src/main.rs", "tools/docs-site-gen/src/main.rs", False, OWNED),
    ("templates/docs-site-gen/FF_REV", "tools/docs-site-gen/FF_REV", False, OWNED),
    ("templates/brand.toml", "tools/docs-site-gen/brand.toml", False, USER),
    ("scripts/build-local.sh", "tools/docs-site-gen/build-local.sh", True, OWNED),
    ("scripts/rebrand_site.py", "tools/docs-site-gen/rebrand_site.py", False, OWNED),
    ("scripts/check_site.py", "tools/docs-site-gen/check_site.py", False, OWNED),
    ("scripts/_common.py", "tools/docs-site-gen/_common.py", False, OWNED),
    ("templates/pages.yml", PAGES_REL, False, OWNED),
    ("templates/nav.toml", "site/nav.toml", False, USER),
    ("templates/index.md", "site/index.md", False, USER),
    ("templates/rust-toolchain.toml", "rust-toolchain.toml", False, USER),
]

# スキル側で廃止された所有ファイル（配置先の相対パス）。廃止の判定は、対象リポジトリのマニフェストではなく
# このスキル側の固定リストだけで行う（マニフェストは信頼しない入力で、任意のパスを「廃止された所有ファイル」
# として指名させると、利用者に無関係なファイルの削除を促せてしまうため）。廃止したら、ここへ追加する。
DEPRECATED_OWNED: tuple[str, ...] = ()

# 終了コード: 0 成功 / 2 入力不正・書き込み先が不適 / 3 競合（OWNED の不一致）/ 4 配置後の check_site 失敗
EXIT_CONFLICT, EXIT_CHECK_FAILED = 3, 4
# --update でも解消しない競合の種別（手動で解消する。案内も --update を勧めない）
UNFIXABLE_KINDS = frozenset({"symlink", "not_regular", "unreadable", "outside_root"})

MANIFEST_REL = "tools/docs-site-gen/.scaffold-manifest.json"
MANIFEST_VERSION = 1
MANIFEST_MAX_BYTES = 64 * 1024
MANIFEST_MAX_FILES = 100
# マニフェスト内のパスとして受理する形（相対・英数字と . _ - / のみ・許可ディレクトリ配下）。
# FILES / DEPRECATED_OWNED の固定パスとの突き合わせにのみ使い、書き込み・削除・表示には使わない。
_MANIFEST_PATH_RE = re.compile(r"^[A-Za-z0-9_.][A-Za-z0-9_./-]{0,200}$")
_MANIFEST_ALLOWED_PREFIXES = ("tools/docs-site-gen/", ".github/workflows/")
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
# 旧版（マニフェスト導入前）の配置痕跡。すべて揃っていれば update 扱いにする
LEGACY_TRACES = (
    "tools/docs-site-gen/FF_REV",
    "tools/docs-site-gen/Cargo.toml",
    "tools/docs-site-gen/src/main.rs",
    "tools/docs-site-gen/build-local.sh",
)

GITIGNORE_LINES = ["_ff/", "tools/docs-site-gen/target/", "tools/docs-site-gen/Cargo.lock", "_site/"]

# 読み取りの上限。対象リポジトリのファイルは信頼できないため、巨大ファイル・特殊ファイルで止まらない・
# メモリを使い切らないようにする。
HASH_READ_CAP = 4 * 1024 * 1024
TEXT_READ_CAP = 1024 * 1024
DIFF_READ_CAP = 256 * 1024
DIFF_MAX_LINES = 200
MARKER_HEAD_BYTES = 4096

# pages.yml の利用者区間（`on.push.paths` へ追加の監視パスを書ける）。区間の行は厳格に検証する。
USER_PATHS_TAG = "sgp:user-paths"
USER_PATH_LINE_RE = re.compile(r'^      - "([A-Za-z0-9_.*/-]{1,200})"$')
# 旧版の paths に見える行のうち、利用者区間の形式（二重引用符）に合わないもの（シングルクォート・引用符なし）。
# 区間へは引き継げないため、競合の理由・警告で知らせる。ステップの行（`- name: …`）は空白・コロンを含むので該当しない。
USER_PATH_LOOSE_RE = re.compile(r"^      - (?:'[^'\n]{1,200}'|[A-Za-z0-9_.*/-]{1,200})[ \t]*$")
USER_PATHS_MAX = 20


def out(msg: object, *, err: bool = False) -> None:
    """対象リポジトリ由来の文字列を含み得る出力は、必ずここを通して無害化する（1 行 = 1 回の呼び出し）。"""
    print(sanitize(msg, 600), file=sys.stderr if err else sys.stdout)


# ---------------------------------------------------------------- 安全な読み取り


# 対象リポジトリのルート（実体）。設定されていれば、対象リポジトリのファイルを開く入口（_open_regular）が、
# 親ディレクトリの symlink を含めた実体が root の外（`.git` 配下を含む）へ解決されるパスを、開く前に拒否する。
# 読み取りの入口をここ 1 か所に集約し、分類・差分・ハッシュ・マニフェスト・pages.yml 解析などの呼び出し側が
# 個別に確認を忘れても、root 外のファイルを読まないようにする。main が最初に設定する。
_ROOT_GUARD: Path | None = None


class OutsideRootError(OSError):
    """読み取り先の実体が対象リポジトリの外（または .git 配下）へ解決される。"""


def exists_inside(root_real: Path, path: Path) -> bool:
    """実体が root 内に解決される場合に限り、存在を判定する（親 symlink を辿って root 外の状態を見ない）。"""
    return resolves_inside(root_real, path) and os.path.lexists(path)


def _open_regular(path: Path) -> int:
    """symlink を辿らず、通常ファイルだけを開く（FIFO 等で読み取りが止まらないよう非ブロッキング）。"""
    if _ROOT_GUARD is not None and not resolves_inside(_ROOT_GUARD, path):
        raise OutsideRootError("対象の外（または .git 配下）へ解決される")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    fd = os.open(path, flags)
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise OSError("通常ファイルではない")
    except OSError:
        os.close(fd)
        raise
    return fd


def read_capped(path: Path, cap: int) -> bytes:
    """上限付きで読む。上限超過は OverflowError、symlink・特殊ファイルは OSError。"""
    fd = _open_regular(path)
    with os.fdopen(fd, "rb") as fh:
        data = fh.read(cap + 1)
    if len(data) > cap:
        raise OverflowError("大きすぎる")
    return data


def read_head(path: Path, n: int) -> bytes:
    fd = _open_regular(path)
    with os.fdopen(fd, "rb") as fh:
        return fh.read(n)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def safe_sha256(path: Path) -> str | None:
    """通常ファイルの sha256（上限付き）。読めない・大きすぎる・symlink は None。"""
    try:
        return sha256_bytes(read_capped(path, HASH_READ_CAP))
    except (OSError, OverflowError):
        return None


def regular_inside(root_real: Path, path: Path) -> bool:
    # 先に実体を検証する（is_file は親 symlink を辿って root 外を stat するため、検証の後に呼ぶ）
    return resolves_inside(root_real, path) and path.is_file() and not path.is_symlink()


# ---------------------------------------------------------------- 入力検証


def toml_escape(value: str) -> str:
    return value.replace("\\", "\\\\").replace('"', '\\"')


def validate_text(name: str, value: str, *, required: bool) -> str:
    if required and not value.strip():
        raise ValueError(f"--{name} は必須")
    if CONTROL_RE.search(value):
        raise ValueError(f"--{name} に制御文字を含められない")
    if len(value) > MAX_TEXT_LEN:
        raise ValueError(f"--{name} は {MAX_TEXT_LEN} 文字以内")
    if BIDI_RE.search(value):
        raise ValueError(f"--{name} に双方向制御文字を含められない")
    if PLACEHOLDER_RE.search(value):
        # 置換結果が再置換されて意図しない値になる経路を入力段階で断つ
        raise ValueError(f"--{name} にプレースホルダー（__SGP_*__）を含められない")
    if has_upstream_word(value):
        raise ValueError(
            f"--{name} に上流名 `{UPSTREAM_BRAND}` を独立した語として含められない"
            "（生成後の残存検査と区別できない。`fandhe-frontend-docs` のような別の語の一部は可。"
            "--copyright の既定値は owner を含むため、必要なら --copyright を明示する）"
        )
    return value


# ---------------------------------------------------------------- マニフェスト


def validate_manifest(obj: object) -> tuple[str, dict[str, str]]:
    """マニフェストを厳密に検証して (ff_rev, files) を返す。1 つでも違反すれば ValueError。

    信頼しない入力として扱うため、未知のキー・未知の版・不正なパス（絶対・`..`・許可ディレクトリ外）・
    不正なハッシュはすべて丸ごと拒否する（部分的に採用しない）。
    """
    if not isinstance(obj, dict) or set(obj) != {"version", "ff_rev", "files"}:
        raise ValueError("キーが {version, ff_rev, files} と一致しない")
    if type(obj["version"]) is not int or obj["version"] != MANIFEST_VERSION:
        raise ValueError("未対応の書式バージョン")
    ff_rev = obj["ff_rev"]
    if not isinstance(ff_rev, str) or not FF_REV_RE.fullmatch(ff_rev):
        raise ValueError("ff_rev が 40 桁の小文字 hex ではない")
    files = obj["files"]
    if not isinstance(files, dict) or len(files) > MANIFEST_MAX_FILES:
        raise ValueError("files が不正（辞書でない、または件数超過）")
    for k, v in files.items():
        if not isinstance(k, str) or not _MANIFEST_PATH_RE.fullmatch(k) or k.startswith("/"):
            raise ValueError("files のパスが不正")
        if any(seg in ("", ".", "..") for seg in k.split("/")):
            raise ValueError("files のパスに空・`.`・`..` セグメントを含む")
        if not k.startswith(_MANIFEST_ALLOWED_PREFIXES):
            raise ValueError("files のパスが許可ディレクトリ配下でない")
        if not isinstance(v, str) or not _SHA256_RE.fullmatch(v):
            raise ValueError("files のハッシュが sha256（小文字 hex 64 桁）でない")
    return ff_rev, dict(files)


def load_manifest(target: Path, root_real: Path) -> tuple[tuple[str, dict[str, str]] | None, list[str]]:
    """(検証済みマニフェストまたは None, 警告) を返す。不正・読めないものは None（=マニフェストなし）扱い。

    無視する側に倒すのが安全側: マニフェストが無ければ自動更新は起きず、内容が違うファイルは競合になる。
    読む前に、実体が root 配下（`.git` 配下を除く）の通常ファイルであることを確認する。
    """
    path = target / MANIFEST_REL
    if not resolves_inside(root_real, path):
        # 親ディレクトリが root の外へ解決される場合は、存在も見ない（外側の状態に左右されない）
        warn = [f"{MANIFEST_REL} が対象の外（または .git 配下）へ解決されるため読まない"] if os.path.lexists(target / "tools") else []
        return None, warn
    if not os.path.lexists(path):
        return None, []
    if path.is_symlink() or not path.is_file():
        return None, [f"{MANIFEST_REL} が通常ファイルでない（symlink など）ため無視した"]
    try:
        obj = json.loads(read_capped(path, MANIFEST_MAX_BYTES).decode("utf-8"))
        return validate_manifest(obj), []
    except OverflowError:
        return None, [f"{MANIFEST_REL} が {MANIFEST_MAX_BYTES} バイトを超えるため無視した"]
    except (OSError, ValueError, RecursionError) as e:   # JSONDecodeError・UnicodeDecodeError は ValueError 系
        return None, [f"{MANIFEST_REL} が不正なため無視した（{type(e).__name__}）"]


# ---------------------------------------------------------------- モード判定

# origin URL の既知形式（URL 全体に対する厳密一致。先頭・末尾に余分な文字を許さない）。
_ORIGIN_FORMS = (
    re.compile(r"^https://github\.com/(?P<o>[A-Za-z0-9-]+)/(?P<r>[A-Za-z0-9_.-]+?)(?:\.git)?/?$"),
    re.compile(r"^ssh://git@github\.com/(?P<o>[A-Za-z0-9-]+)/(?P<r>[A-Za-z0-9_.-]+?)(?:\.git)?/?$"),
    re.compile(r"^git@github\.com:(?P<o>[A-Za-z0-9-]+)/(?P<r>[A-Za-z0-9_.-]+?)(?:\.git)?/?$"),
)


def parse_origin(url: str) -> tuple[str, str] | None:
    """既知の GitHub URL 形式なら (owner, repo)。それ以外は None（生の文字列は出力に使わない）。"""
    url = url.strip()
    if len(url) > 300:
        return None
    for form in _ORIGIN_FORMS:
        m = form.fullmatch(url)
        if m and valid_owner(m.group("o")) and valid_repo_name(m.group("r")):
            return m.group("o"), m.group("r")
    return None


def _origin_owner_repo(target: Path) -> tuple[str, str] | None:
    try:
        # バイト列で受けて復号する（text=True だと、細工した .git/config の不正な UTF-8 で UnicodeDecodeError になり、
        # --detect --json が約束した JSON を出さずに落ちる）。復号できない部分は置換し、origin は不明として扱う。
        r = subprocess.run(["git", "-C", str(target), "config", "--get", "remote.origin.url"],
                           capture_output=True, timeout=5)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if r.returncode != 0:
        return None
    return parse_origin(r.stdout.decode("utf-8", errors="replace"))


_GEN_REL = "tools/docs-site-gen/"


def known_generator_names() -> set[str]:
    """`tools/docs-site-gen/` 直下にあってよい名前（スキルの配置物と、ビルドで生じるもの）。"""
    names = {Path(dst).name for _, dst, _, _ in FILES if dst.startswith(_GEN_REL) and "/" not in dst[len(_GEN_REL):]}
    return names | {Path(MANIFEST_REL).name, "src", "target", "Cargo.lock", "THIRD-PARTY-LICENSES"}


def unknown_generator_entries(target: Path, root_real: Path) -> list[str]:
    """`tools/docs-site-gen/` にある、スキルが配置しないもの（相対パス。名前順）。detect の「痕跡なし」判定用。

    スキル所有の同名ファイルが 1 つも無くても、ディレクトリが別用途で使われていれば新規構築にしない。
    直下は許可リスト（known_generator_names）、`src/` はスキルが置くファイルだけを既知とする。
    ディレクトリ自体が symlink・対象の外へ解決される場合は中を見ない（detect が別途その旨を根拠に載せる）。
    名前だけを見て、内容は読まない。
    """
    gen = target / "tools" / "docs-site-gen"
    if not os.path.lexists(gen) or gen.is_symlink() or not resolves_inside(root_real, gen):
        return []
    if not gen.is_dir():
        return [_GEN_REL.rstrip("/") + "（ディレクトリではない）"]
    known = known_generator_names()
    known_src = {Path(dst).name for _, dst, _, _ in FILES if dst.startswith(_GEN_REL + "src/")}
    found: list[str] = []
    try:
        with os.scandir(gen) as it:
            found += [_GEN_REL + e.name for e in it if e.name not in known]
        src = gen / "src"
        if src.is_dir() and not src.is_symlink():
            with os.scandir(src) as it:
                found += [_GEN_REL + "src/" + e.name for e in it if e.name not in known_src]
    except OSError:
        return [_GEN_REL.rstrip("/") + "（読めない）"]
    return sorted(found)


def detect(target: Path, root_real: Path) -> dict:
    """対象リポジトリを判定する（書き込みなし）。

    mode: new（痕跡なし）/ update（有効なマニフェスト、または旧版配置の痕跡）/ foreign（上流自身、
    スキル由来でない同名ファイルがある、または `tools/docs-site-gen/` にスキルが配置しないファイルがある）。
    kind は根拠の種別で、SKILL.md はこれで分岐する。
    upstream は foreign のうち適用対象外（デザインの出どころ）。
    """
    reasons: list[str] = []
    manifest, warns = load_manifest(target, root_real)
    reasons += warns
    marker = target / "crates" / "docs-site" / "Cargo.toml"
    upstream = False
    try:
        if regular_inside(root_real, marker) and \
                "fandhe-frontend-docs-site" in read_head(marker, MARKER_HEAD_BYTES).decode("utf-8", errors="replace"):
            upstream = True
            reasons.append("crates/docs-site が fandhe-frontend の docs-site（上流リポジトリ自身）")
    except OSError:
        pass
    origin = _origin_owner_repo(target)
    if origin and is_upstream_repo(*origin):
        upstream = True
        reasons.append("git の origin が上流リポジトリ（Fandhe-AI/fandhe-frontend）")
    if upstream:
        reasons.append("上流 fandhe-frontend はデザインの出どころであり、このスキルの適用対象外")
        return {"mode": "foreign", "kind": "upstream", "reasons": reasons}
    if manifest is not None:
        reasons.append(f"配置マニフェスト {MANIFEST_REL}（FF_REV {manifest[0][:12]}）がある")
        return {"mode": "update", "kind": "manifest", "reasons": reasons}
    legacy = [t for t in LEGACY_TRACES if regular_inside(root_real, target / t)]
    if len(legacy) == len(LEGACY_TRACES):
        reasons.append("旧版配置の痕跡（FF_REV・wrapper・build-local.sh）がある（マニフェストなし）")
        return {"mode": "update", "kind": "legacy", "reasons": reasons}
    present = [dst for _, dst, _, kind in FILES if kind == OWNED and exists_inside(root_real, target / dst)]
    outside = [dst for _, dst, _, _ in FILES if not resolves_inside(root_real, target / dst)]
    if outside:
        reasons.append("次の配置先は対象の外（または .git 配下）へ解決されるため読まない（親ディレクトリが symlink 等）: "
                       + ", ".join(outside[:6]) + (f" ほか {len(outside) - 6} 件" if len(outside) > 6 else ""))
    if present:
        reasons.append("スキル所有の配置先に既存ファイルがあるが、スキルの配置とは認められない: " + ", ".join(present))
        return {"mode": "foreign", "kind": "unrelated", "reasons": reasons}
    stray = unknown_generator_entries(target, root_real)
    if stray:
        reasons.append("tools/docs-site-gen/ にスキルが配置しないファイルがある（別用途のディレクトリの可能性）: "
                       + ", ".join(sanitize(s, 80) for s in stray[:6])
                       + (f" ほか {len(stray) - 6} 件" if len(stray) > 6 else ""))
        return {"mode": "foreign", "kind": "unrelated", "reasons": reasons}
    reasons.append("スキルの配置痕跡なし")
    return {"mode": "new", "kind": "none", "reasons": reasons}


# ---------------------------------------------------------------- pages.yml（利用者区間）

# 行ごとに判定する（`\s*` と re.M の組み合わせは、空白と改行だけの巨大な入力で最悪 O(n²) になる）。
_BRANCH_LINE_RE = re.compile(r'[ \t]*branches:[ \t]*\[[ \t]*"([^"]{1,100})"[ \t]*\][ \t]*')


def read_text_capped(path: Path, cap: int = TEXT_READ_CAP) -> str:
    return read_capped(path, cap).decode("utf-8")


def branch_from_pages_yml(target: Path, root_real: Path) -> str | None:
    p = target / PAGES_REL
    try:
        if not regular_inside(root_real, p):
            return None
        for line in read_text_capped(p).split("\n"):
            m = _BRANCH_LINE_RE.fullmatch(line.rstrip("\r")) if len(line) <= 300 else None
            if m and BRANCH_RE.fullmatch(m.group(1)) and ".." not in m.group(1):
                return m.group(1)
    except (OSError, OverflowError, UnicodeDecodeError):
        return None
    return None


def _glob_ok(glob: str) -> bool:
    return bool(glob) and not glob.startswith("/") and all(seg not in ("", "..") for seg in glob.split("/"))


# マーカーはトークンで判定する（行頭の空白 6 つ + `# sgp:user-paths:begin|end`、その後ろは行末か区切りに続く
# 説明文）。テンプレートの説明文を将来変えても、配置済みの pages.yml が不正扱いにならないようにするため。
# 書き込み時はテンプレートの行へ正規化する。
_BEGIN_RE = re.compile(rf"^ {{6}}# {USER_PATHS_TAG}:begin(?=$|[ \t（(:：])")
_END_RE = re.compile(rf"^ {{6}}# {USER_PATHS_TAG}:end(?=$|[ \t（(:：])")


def _template_markers(rendered: str) -> tuple[list[str], int, int]:
    lines = rendered.split("\n")
    b = [i for i, l in enumerate(lines) if _BEGIN_RE.match(l)]
    e = [i for i, l in enumerate(lines) if _END_RE.match(l)]
    if len(b) != 1 or len(e) != 1 or b[0] >= e[0] or e[0] != b[0] + 1:
        raise ValueError("templates/pages.yml の利用者区間マーカーが不正（スキル側の不整合）")
    return lines, b[0], e[0]


# マニフェストへ記録する・未編集判定に使う pages.yml の正規形では、マーカー行を説明文のない固定の行にする
# （説明文はテンプレートの版で変わり得るが、利用者区間の有無・位置だけが「構造」であるため）。
CANON_BEGIN = f"      # {USER_PATHS_TAG}:begin"
CANON_END = f"      # {USER_PATHS_TAG}:end"


def canon_pages(text: str) -> str:
    return "\n".join(CANON_BEGIN if _BEGIN_RE.match(l) else CANON_END if _END_RE.match(l) else l
                     for l in text.split("\n"))


def manifest_hash(rel: str, text: str) -> str:
    """マニフェストの記録ハッシュ。pages.yml はマーカー行の説明文に依存しない正規形で取る。"""
    return sha256_bytes((canon_pages(text) if rel == PAGES_REL else text).encode("utf-8"))


def with_user_paths(rendered: str, globs: list[str]) -> str:
    lines, b, _ = _template_markers(rendered)
    return "\n".join(lines[:b + 1] + [f'      - "{g}"' for g in globs] + lines[b + 1:])


def analyze_pages(cur: str, rendered: str) -> dict:
    """既存の pages.yml を分析する。

    kind=region: 利用者区間が有効（entries=区間の glob、norm=区間を空にしマーカー行をテンプレートの行へ
                 正規化した形。マーカー行の説明文に依存しない）。
    kind=legacy: 区間マーカーが無い旧版。追加された paths（テンプレート既定以外）のうち検証を通ったものを
                 extras に取り出し、通らなかったもの（dropped）は理由付きで報告する。追加 paths を除いた本文が
                 テンプレート（マーカー行を除く）と一致すれば clean。
    kind=invalid: マーカーの欠落・重複・逆順・ネスト・偽マーカー、区間内の不正な行、件数超過
                 （信頼しない入力として競合へ倒す）。
    """
    r_lines, rb, re_ = _template_markers(rendered)
    begin_line, end_line = r_lines[rb], r_lines[re_]
    # `cur` は呼び出し側（main）が `\r\n` → `\n` に正規化済みの文字列（core.autocrlf で CRLF になった pages.yml でも、
    # 区間・追加 paths が LF のファイルと同じ結果になる）。正規化は 1 回だけ行う（ここでも行うと `\r\r\n` の
    # ような単独の `\r` まで消えてしまう）。行の途中・行末に残った単独の `\r` は、従来どおり不正として扱う。
    lines = cur.split("\n")
    hits = [i for i, l in enumerate(lines) if f"{USER_PATHS_TAG}:" in l]
    if not hits:
        defaults = {m.group(1) for l in r_lines if (m := USER_PATH_LINE_RE.match(l))}
        extras: list[str] = []
        dropped: list[tuple[str, str]] = []
        loose = [l for l in lines if USER_PATH_LOOSE_RE.match(l) and not USER_PATH_LINE_RE.match(l)]
        kept = []
        for l in lines:
            m = USER_PATH_LINE_RE.match(l)
            if m and m.group(1) not in defaults:
                g = m.group(1)
                if not _glob_ok(g):
                    dropped.append((g, "不正な glob（絶対パス・`..`・空セグメント）"))
                elif len(extras) >= USER_PATHS_MAX:
                    dropped.append((g, f"上限 {USER_PATHS_MAX} 件を超過"))
                else:
                    extras.append(g)
            else:
                kept.append(l)
        rendered_wo = [l for i, l in enumerate(r_lines) if i not in (rb, re_)]
        return {"kind": "legacy", "extras": extras, "dropped": dropped, "loose": len(loose),
                "clean": kept == rendered_wo}
    b = [i for i, l in enumerate(lines) if _BEGIN_RE.match(l)]
    e = [i for i, l in enumerate(lines) if _END_RE.match(l)]
    if len(hits) != 2 or len(b) != 1 or len(e) != 1 or b[0] >= e[0] or hits != [b[0], e[0]]:
        return {"kind": "invalid", "why": "マーカーの欠落・重複・逆順・ネスト、または偽のマーカー行がある"}
    globs = []
    for l in lines[b[0] + 1:e[0]]:
        m = USER_PATH_LINE_RE.match(l)
        if not m or not _glob_ok(m.group(1)):
            return {"kind": "invalid", "why": "区間内に不正な行がある（`      - \"<glob>\"` 形式・英数字と . _ * / - のみ・`..` 不可）"}
        globs.append(m.group(1))
    if len(globs) > USER_PATHS_MAX:
        return {"kind": "invalid", "why": f"区間内の行が {USER_PATHS_MAX} 件を超える"}
    return {"kind": "region", "entries": globs,
            "norm": "\n".join(lines[:b[0]] + [CANON_BEGIN, CANON_END] + lines[e[0] + 1:])}


# tools/docs-site-gen/ 直下に存在してよい名前（許可リスト）。スキルが配置するもの（FILES の basename）・マニフェスト・
# cargo が作る `target` と `Cargo.lock`・wrapper の `src`・build-local.sh が（--write-third-party で）作る
# `THIRD-PARTY-LICENSES`（実際は対象リポジトリ直下だが、置かれても無害な既知の名前として含める）。
# build-local.sh は python を `-B`（__pycache__ を作らない）で起動するため、`__pycache__` は既知にしない
# （事前に置かれた .pyc が読み込まれ得るので、見つけたら警告する）。
_TOOLCHAIN_PATH_RE = re.compile(rb"(?m)^[ \t]*path[ \t]*=")


def unexpected_buildable(target: Path, root_real: Path) -> list[str]:
    """ローカルビルド（cargo build・python 起動）で実行・読み込まれ得る、スキルが配置していないもの。

    許可リスト方式: `tools/docs-site-gen/` 直下で、既知の名前以外はすべて対象（`argparse/__init__.py`・
    `argparse.pyc`・`*.so`・`build.rs` のような名前判定の回避を許さない）。加えて cargo の設定ディレクトリ `.cargo`
    （リポジトリ直下・`tools/`・`tools/docs-site-gen/`）と、`path` キーを持つ `rust-toolchain(.toml)`。
    見つけても中止はしない（利用者の正当なファイルの可能性があるため）。警告で内容の確認を促す。
    """
    expected = known_generator_names()
    found: list[str] = []

    # この関数の判定はすべて「安全側の警告」を出すためのもの。実体を検証した結果が偽になることを理由に警告を
    # 消してはいけない（cargo・python は symlink を辿る）。存在は内容を読まずに lstat 相当（lexists / is_symlink）で
    # 判定し、symlink は「symlink」「対象の外を指す」とラベルだけ付けて警告する。リンク先の中身は読まない。
    def label(entry: Path) -> str:
        if entry.is_symlink():
            return "（symlink" + ("" if resolves_inside(root_real, entry) else "、対象の外を指す") + "。中身は読まない）"
        return ""

    gen = target / "tools" / "docs-site-gen"
    if os.path.lexists(gen):
        if gen.is_symlink() or not resolves_inside(root_real, gen):
            found.append(_GEN_REL.rstrip("/") + label(gen) if gen.is_symlink() else _GEN_REL.rstrip("/") + "（親が対象の外へ解決される。中身は読まない）")
        elif gen.is_dir():
            try:
                with os.scandir(gen) as it:   # 切り詰めずに全件走査する（ダミーで後ろの名前を隠せない）
                    for e in it:
                        if e.name not in expected:
                            found.append(_GEN_REL + e.name + (label(gen / e.name) if e.is_symlink() else ""))
                        elif e.is_symlink():   # 既知の名前でも symlink なら警告する（外を指し得る）
                            found.append(_GEN_REL + e.name + label(gen / e.name))
            except OSError:
                pass
    # 実行・読み込みに直結しやすい名前（Python・Rust のソース/バイトコード/拡張、ディレクトリ、symlink）を先に並べる。
    # 表示は件数で切るため、ダミーの大量のファイルの後ろに危険な名前が隠れないようにする。
    def risk(rel: str) -> tuple[int, str]:
        n = rel[len(_GEN_REL):].split("（")[0]
        risky = (re.search(r"\.(py|pyc|pyd|so|dylib|rs)$", n) is not None or n == "__pycache__"
                 or "symlink" in rel or (gen / n).is_dir())
        return (0 if risky else 1, n)

    found.sort(key=risk)
    for d in ("", "tools/", _GEN_REL):
        parent = target / d if d else target
        if not resolves_inside(root_real, parent):
            # 親ディレクトリが対象の外へ解決される（symlink）: その下の .cargo・rust-toolchain は確認できないので警告する
            if os.path.lexists(parent):
                found.append(f"{d or './'}（親が対象の外へ解決される。.cargo・rust-toolchain を確認できない）")
            continue
        cargo = parent / ".cargo"
        if os.path.lexists(cargo) and (d + ".cargo") not in found:
            found.append(d + ".cargo" + label(cargo))
        for name in ("rust-toolchain", "rust-toolchain.toml"):
            f = parent / name
            if not os.path.lexists(f):
                continue
            if f.is_symlink():
                found.append(f"{d}{name}{label(f)}")   # 中身を読まずに警告する
                continue
            try:
                if not f.is_file():
                    found.append(f"{d}{name}（通常ファイルではない）")
                elif _TOOLCHAIN_PATH_RE.search(read_capped(f, 64 * 1024)):
                    found.append(f"{d}{name}（path キーを持つ）")
            except (OSError, OverflowError):
                found.append(f"{d}{name}（読めない）")
    return found


# ---------------------------------------------------------------- 差分表示（--show-diff）


def build_diff(root_real: Path, path: Path, rel: str, new_text: str) -> tuple[str, list[str]]:
    """競合ファイルとスキルの新版の unified diff。(状態, 行) を返す。

    読むのは root 内に解決される通常ファイルだけ（symlink・特殊ファイル・root 外・`.git` 配下は内容を読まない）。
    `diff -u` のような外部コマンドは symlink を辿ってリンク先（例: ~/.aws/credentials）を出してしまうため、
    自前で O_NOFOLLOW 読みする。出力は行数・文字数に上限を付け、制御文字・bidi 文字を無害化する。
    """
    if path.is_symlink():
        return "symlink", ["symlink のため内容を表示しない。手動で解消する"]
    if not resolves_inside(root_real, path):
        return "outside", ["対象の外（または .git 配下）へ解決されるため内容を表示しない。手動で解消する"]
    try:
        data = read_capped(path, DIFF_READ_CAP)
    except OverflowError:
        return "too_large", [f"{DIFF_READ_CAP // 1024} KiB を超えるため内容を表示しない。手動で解消する"]
    except OSError:
        return "not_regular", ["通常ファイルとして読めないため内容を表示しない。手動で解消する"]
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return "not_utf8", ["UTF-8 でないため内容を表示しない。手動で解消する"]
    diff = list(difflib.unified_diff(text.splitlines(), new_text.splitlines(),
                                     f"対象/{rel}", f"スキルの新版/{rel}", lineterm="", n=2))
    if not diff:
        # splitlines() は改行コードを無視するため、LF と CRLF の差は diff に出ない。内容が違えば明示する
        if text != new_text:
            return "newline_only", ["改行コードのみの差（行の内容は同じ。core.autocrlf 等の変換の可能性）"]
        return "diff", ["（差分なし）"]
    lines = [sanitize(l.replace("\t", "    "), 300) for l in diff[:DIFF_MAX_LINES]]
    if len(diff) > DIFF_MAX_LINES:
        lines.append(f"…（{len(diff) - DIFF_MAX_LINES} 行省略）")
    return "diff", lines


# ---------------------------------------------------------------- main


class _Parser(argparse.ArgumentParser):
    """`--json` 指定時は、引数エラーでも JSON を 1 つ出す（SKILL.md が JSON を前提にできるように）。"""

    wants_json = False

    def error(self, message: str):   # noqa: D401
        if self.wants_json:
            print(json.dumps({"exit_code": 2, "error": sanitize(message), "mode": None}, ensure_ascii=True))
        self.print_usage(sys.stderr)
        self.exit(2, f"{self.prog}: error: {sanitize(message)}\n")


def main(argv: list[str] | None = None) -> int:
    ap = _Parser(description=__doc__.split("\n")[0])
    ap.wants_json = "--json" in (sys.argv[1:] if argv is None else argv)
    ap.add_argument("--target", required=True, type=Path, help="対象リポジトリのルート（既存ディレクトリ）")
    ap.add_argument("--detect", action="store_true", help="モード（new / update / foreign）と根拠を表示して終了する（書き込みなし）")
    ap.add_argument("--list-paths", action="store_true",
                    help="スキル所有ファイル・利用者編集ファイル・マニフェストの配置先を JSON で出して終了する（書き込みなし。"
                         "update-snapshot.sh が触ってよい対象の許可リストの唯一の定義元として使う）")
    ap.add_argument("--json", action="store_true", help="結果を JSON 1 つで標準出力へ出す（SKILL.md の報告用）")
    ap.add_argument("--show-diff", action="store_true",
                    help="競合した所有ファイルとスキルの新版の差分を表示して終了する（書き込みなし。"
                         "symlink・対象の外・特殊ファイルは内容を読まない）")
    ap.add_argument("--owner", default=None, help="新規構築で必須")
    ap.add_argument("--repo", default=None, help="新規構築で必須")
    ap.add_argument("--branch", default=None,
                    help="既定ブランチ名（gh repo view で解決した値）。新規構築で必須。更新では省略すると既存の pages.yml から読む")
    ap.add_argument("--title", default=None, help="サイトタイトル（nav.toml [site].title・トップ見出し）。新規構築で必須")
    ap.add_argument("--brand", default=None, help="ヘッダーのブランド名（既定: --title）")
    ap.add_argument("--tagline", default="")
    ap.add_argument("--copyright", dest="copyright_", default=None, help="既定: © <年> <owner>")
    ap.add_argument("--lang", default="ja")
    ap.add_argument("--version-badge", default="")
    ap.add_argument("--favicon-letter", default=None, help="既定: ブランド名の先頭英数字")
    ap.add_argument("--favicon-color", default="#2b6cb0")
    ap.add_argument("--update", action="store_true",
                    help="スキル所有ファイル（workflow・wrapper・スクリプト・FF_REV）の不一致を、配置後の編集を含めて"
                         "強制的に上書きする。未編集のものは --update なしでも自動更新される。"
                         "利用者編集ファイル（nav.toml・index.md・brand.toml・rust-toolchain.toml）は触らない")
    ap.add_argument("--year", default=None, help="著作権表記の年（既定: 現在の年）")
    args = ap.parse_args(argv)

    if args.list_paths:
        print(json.dumps({"owned": [d for _, d, _, k in FILES if k == OWNED],
                          "user": [d for _, d, _, k in FILES if k == USER],
                          "manifest": MANIFEST_REL,
                          "extra": [".gitignore", "THIRD-PARTY-LICENSES"]}, ensure_ascii=True))
        return 0

    summary: dict = {
        "mode": None, "kind": None, "detect": [], "error": None,
        "ff_rev": None, "created": [], "updated": [], "same": [], "kept": [], "missing": [],
        "conflicts": [], "deprecated": [], "gitignore_added": [], "manifest_written": False,
        "manifest_recreated": False, "warnings": [], "check": None, "diffs": None, "exit_code": None,
    }

    def finish(code: int, msg: str | None = None) -> int:
        if msg:
            out(msg, err=True)
            summary["error"] = sanitize(msg)
        summary["exit_code"] = code
        if args.json:
            print(json.dumps(summary, ensure_ascii=True))
        return code

    if not args.target.is_dir():
        return finish(2, "エラー: --target が既存ディレクトリではない")
    root_real = Path(os.path.realpath(args.target))
    global _ROOT_GUARD
    _ROOT_GUARD = root_real   # 以後、対象リポジトリのファイルを開く入口が root 外への解決を拒否する

    det = detect(args.target, root_real)
    summary.update(mode=det["mode"], kind=det["kind"], detect=[sanitize(r) for r in det["reasons"]])
    if args.detect:
        if args.json:
            summary["exit_code"] = 0
            print(json.dumps({"mode": det["mode"], "kind": det["kind"], "reasons": summary["detect"], "exit_code": 0},
                             ensure_ascii=True))
        else:
            out(f"mode={det['mode']}")
            out(f"kind={det['kind']}")
            for r in det["reasons"]:
                out(f"根拠: {r}")
        return 0
    if det["kind"] == "upstream":
        return finish(2, "エラー: 適用対象外: " + " / ".join(det["reasons"]))
    mode = "update" if det["mode"] == "update" else "new"
    summary["mode"] = mode

    manifest, manifest_warnings = load_manifest(args.target, root_real)
    m_ff_rev, m_files = manifest if manifest else (None, {})
    warnings: list[str] = list(manifest_warnings)

    # ---- 入力の検証（指定されたものはモードに関わらず検証する）
    full = all(v is not None for v in (args.owner, args.repo, args.branch, args.title))
    try:
        if mode == "new" and not full and not args.show_diff:
            missing = [n for n, v in (("--owner", args.owner), ("--repo", args.repo),
                                      ("--branch", args.branch), ("--title", args.title)) if v is None]
            raise ValueError(f"新規構築には {', '.join(missing)} が必要（更新モードでは省略できる）")
        if args.owner is not None and not valid_owner(args.owner):
            raise ValueError("--owner が GitHub の owner 名として不正")
        if args.repo is not None and not valid_repo_name(args.repo):
            raise ValueError("--repo が GitHub の repo 名として不正")
        if args.owner is not None and args.repo is not None and is_upstream_repo(args.owner, args.repo):
            raise ValueError("--owner/--repo が上流リポジトリ（Fandhe-AI/fandhe-frontend）そのもの。自サイトのリポジトリを指定する")
        existing_branch = branch_from_pages_yml(args.target, root_real)
        branch = args.branch if args.branch is not None else existing_branch
        if branch is None:
            raise ValueError("既定ブランチを決められない（既存の pages.yml から読めない）。--branch を指定する")
        if args.branch is None:
            warnings.append(f"--branch が省略されたため、既存の pages.yml の branches（{branch}）を使った。"
                            "既定ブランチが変わっていても追従しない（SKILL.md の手順では常に --branch を渡す）")
        if not BRANCH_RE.fullmatch(branch) or ".." in branch:
            raise ValueError("--branch が不正（英数字・. _ / - のみ）")
        if args.branch is not None and existing_branch is not None and existing_branch != args.branch:
            warnings.append(f"既定ブランチ（--branch={args.branch}）が既存の pages.yml の branches（{existing_branch}）と食い違う。"
                            "pages.yml は --branch の値で更新される（未編集の場合）")

        subs = None
        title = None
        if full:
            title = validate_text("title", args.title, required=True)
            brand = validate_text("brand", args.brand if args.brand is not None else title, required=True)
            tagline = validate_text("tagline", args.tagline, required=False)
            badge = validate_text("version-badge", args.version_badge, required=False)
            if not LANG_RE.fullmatch(args.lang):
                raise ValueError("--lang は BCP 47 風（例: ja / en）")
            if not COLOR_RE.fullmatch(args.favicon_color):
                raise ValueError("--favicon-color は #RRGGBB 形式")
            letter = args.favicon_letter
            if letter is None:
                m = re.search(r"[A-Za-z0-9]", brand)
                letter = m.group(0).upper() if m else ""
            if not LETTER_RE.fullmatch(letter):
                raise ValueError("--favicon-letter は英数字 1 文字")
            if args.copyright_ is None:
                import datetime
                year = args.year or str(datetime.date.today().year)
                if not re.fullmatch(r"[0-9]{4}", year):
                    raise ValueError("--year は 4 桁の数字")
                copyright_ = f"© {year} {args.owner}"
            else:
                copyright_ = args.copyright_
            copyright_ = validate_text("copyright", copyright_, required=True)
            repository = f"https://github.com/{args.owner}/{args.repo}"
            probe = Brand(brand, repository, args.owner, args.repo, tagline, copyright_, args.lang,
                          badge, letter, args.favicon_color)
            subs = {
                "__SGP_SITE_TITLE__": toml_escape(title),
                "__SGP_BASE_PATH__": probe.base_path,
                "__SGP_BRAND__": toml_escape(brand),
                "__SGP_REPOSITORY__": repository,
                "__SGP_TAGLINE__": toml_escape(tagline),
                "__SGP_COPYRIGHT__": toml_escape(copyright_),
                "__SGP_LANG__": args.lang,
                "__SGP_VERSION_BADGE__": toml_escape(badge),
                "__SGP_FAVICON_LETTER__": letter,
                "__SGP_FAVICON_COLOR__": args.favicon_color,
                "__SGP_DEFAULT_BRANCH__": branch,
            }
        ff_rev = (SKILL_DIR / "templates/docs-site-gen/FF_REV").read_text().strip()
        if not FF_REV_RE.fullmatch(ff_rev):
            raise ValueError("同梱の FF_REV が 40 桁 hex ではない（スキル側の不整合）")
    except ValueError as e:
        return finish(2, f"エラー: {e}")

    owned_table = {"__SGP_DEFAULT_BRANCH__": branch}

    def render(src_rel: str, kind: str) -> str | None:
        text = (SKILL_DIR / src_rel).read_text(encoding="utf-8")
        if kind == OWNED:
            table = owned_table
        elif subs is None:
            return None
        else:
            # 1 パスの re.sub で置換する。key ごとに str.replace を重ねると、先に埋めた値が
            # 後続の置換対象になり得る（値の中のプレースホルダーが再展開される）。
            table = {"__SGP_SITE_TITLE__": title} if src_rel == "templates/index.md" else subs
        res = PLACEHOLDER_RE.sub(lambda m: table.get(m.group(0), m.group(0)), text)
        if kind == OWNED and PLACEHOLDER_RE.search(res):
            raise ValueError(f"{src_rel} に未置換のプレースホルダーが残る（スキル側の不整合: 所有ファイルは branch 以外の値を取らない）")
        return res

    # ---- 全件を分類してから書く（部分書き込みなし）。
    #   create    存在しない → 書く
    #   same      存在し、生成予定の内容と一致 → 何もしない（冪等な再実行）
    #   update    OWNED で不一致かつ (配置後に未編集 = ハッシュがマニフェストと一致 / --update 指定) → 上書き
    #   conflict  OWNED で不一致かつ編集あり・マニフェストなし（--update なし）、または通常ファイルでない → 中止
    #   keep      USER で既存 → 保持（利用者編集）
    #   missing   USER で欠落（更新モード。引数が明示されない限り再作成しない）
    plan: list[tuple[str, str, bool, str, str]] = []   # (dst_rel, text, executable, action, reason)
    same: list[str] = []
    keep: list[str] = []
    missing: list[str] = []
    conflicts: list[tuple[str, str, str]] = []         # (path, kind, reason)
    problems: list[str] = []
    owned_texts: dict[str, str] = {}                   # マニフェストへ記録する正規形
    desired: dict[str, str] = {}                       # 書き込む（または一致を確認する）最終内容

    def plan_write(dst_rel: str, dst: Path, text: str, executable: bool, action: str, reason: str) -> None:
        why = write_target_problem(root_real, dst)
        if why:
            problems.append(f"{dst_rel}（{why}）")
        plan.append((dst_rel, text, executable, action, reason))

    def edited_conflict(dst_rel: str, base_text: str) -> tuple[str, str, str]:
        """利用者が編集した（またはマニフェストが無い）所有ファイルの競合の種別と理由。"""
        if dst_rel in m_files:
            if manifest_hash(dst_rel, base_text) != m_files[dst_rel]:
                return dst_rel, "user_edited_skill_changed", "利用者が編集し、スキル側も変更した"
            return dst_rel, "user_edited_skill_unchanged", "利用者が編集した（スキル側は配置時から変更なし）"
        if manifest is not None:
            return (dst_rel, "not_recorded",
                    "マニフェストに記録が無く、内容が生成予定と異なる（スキルの新版で追加された所有ファイルと同名の別内容、または別用途のファイル）")
        return (dst_rel, "no_manifest",
                "マニフェストなしで内容が生成予定と異なる（旧版からの移行、または別用途の同名ファイル。"
                "所有ファイルを編集していなければ、利用者の了承を得てから --update を 1 回実行すればよい）")

    try:
        for src_rel, dst_rel, executable, kind in FILES:
            dst = args.target / dst_rel
            text = render(src_rel, kind)
            if text is not None:
                desired[dst_rel] = text
            if kind == OWNED:
                owned_texts[dst_rel] = text
            # 先に実体を検証する。親ディレクトリの symlink で root の外（`.git` 配下を含む）へ解決される配置先は、
            # exists / is_file / 読み取りがすべて外側の状態を見るため、存在も内容も見ずに競合として扱う
            # （所有ファイル・利用者編集ファイルとも。--update でも上書きしない。kept / missing の判定も外側に左右されない）。
            if not resolves_inside(root_real, dst):
                conflicts.append((dst_rel, "outside_root",
                                  "対象の外（または .git 配下）へ解決される（親ディレクトリが symlink 等）。内容を読まない。"
                                  "手動で解消する（--update では上書きされない）"))
                continue
            exists = dst.exists() or dst.is_symlink()
            if kind == USER:
                if exists:
                    keep.append(dst_rel)   # symlink でも書かない（リンク先へは決して書かない）
                elif subs is None:
                    missing.append(dst_rel)   # 更新モードでは再作成しない（利用者が意図して消した可能性）
                else:
                    plan_write(dst_rel, dst, text, executable, "create", "新規作成")
                continue
            if not exists:
                plan_write(dst_rel, dst, text, executable, "create", "新規作成")
                continue
            if dst.is_symlink():
                conflicts.append((dst_rel, "symlink", "シンボリックリンク（手動で解消する。--update では上書きされない）"))
                continue
            if not dst.is_file():
                conflicts.append((dst_rel, "not_regular", "通常ファイルではない（手動で解消する。--update では上書きされない）"))
                continue
            if dst_rel == PAGES_REL:
                try:
                    # pages.yml だけは改行を LF とみなして比較する（解析対象の構造化テキストで、利用者区間を持つため）。
                    # CRLF へ変換されただけのファイルは一致として扱い、書き換えない。他の所有ファイルは byte 一致のまま
                    # （改行だけの差は newline_only の競合）。
                    cur = read_text_capped(dst).replace("\r\n", "\n")
                except (OSError, OverflowError, UnicodeDecodeError):
                    conflicts.append((dst_rel, "unreadable", "読み取れない（大きすぎる・UTF-8 でない・特殊ファイル）。手動で解消する（--update では上書きされない）"))
                    continue
                an = analyze_pages(cur, text)
                if an["kind"] == "region":
                    final = with_user_paths(text, an["entries"])
                    desired[dst_rel] = final
                    if cur == final:
                        same.append(dst_rel)
                    elif m_files.get(dst_rel) == sha256_bytes(an["norm"].encode("utf-8")):
                        plan_write(dst_rel, dst, final, executable, "update",
                                   "配置後に未編集（利用者区間を除く）のため、利用者区間を保持してスキルの新版へ自動更新")
                    elif args.update:
                        plan_write(dst_rel, dst, final, executable, "update",
                                   "--update による強制上書き（利用者区間は保持する）")
                    else:
                        conflicts.append(edited_conflict(dst_rel, text))
                elif an["kind"] == "legacy":
                    final = with_user_paths(text, an["extras"])
                    desired[dst_rel] = final
                    if an["dropped"]:
                        warnings.append(
                            f"旧版の pages.yml の追加 paths のうち {len(an['dropped'])} 件を利用者区間へ引き継がなかった: "
                            + "; ".join(f"{sanitize(g, 60)}（{why}）" for g, why in an["dropped"][:5])
                            + ("…" if len(an["dropped"]) > 5 else ""))
                    if an["clean"]:
                        n = len(an["extras"])
                        plan_write(dst_rel, dst, final, executable, "update",
                                   "旧版（利用者区間なし）から移行" + (f"し、追加の paths {n} 件を利用者区間へ引き継ぐ" if n else ""))
                    elif args.update:
                        if an["loose"]:
                            warnings.append(f"旧版の pages.yml の paths のうち {an['loose']} 件（シングルクォート・引用符なし）は"
                                            "利用者区間の形式に合わず、引き継がずに破棄した。必要なら二重引用符の形で利用者区間へ書き足す")
                        plan_write(dst_rel, dst, final, executable, "update",
                                   "--update による強制上書き（旧版の追加 paths のうち検証を通ったものだけ利用者区間へ引き継ぐ）")
                    else:
                        c = edited_conflict(dst_rel, text)
                        if an["loose"]:
                            c = (c[0], c[1], c[2] + f"。--update で引き継がれない行がある（シングルクォート・引用符なしの paths {an['loose']} 件。"
                                 "区間へ移すなら二重引用符の形に直す）")
                        conflicts.append(c)
                else:   # invalid
                    if args.update:
                        warnings.append(f"{PAGES_REL} の利用者区間が不正（{an['why']}）のため、区間の内容を引き継がず上書きした")
                        plan_write(dst_rel, dst, text, executable, "update", "--update による強制上書き（不正な利用者区間は破棄）")
                    else:
                        conflicts.append((dst_rel, "pages_region_invalid", f"利用者区間が不正（{an['why']}）"))
                continue
            cur_hash = safe_sha256(dst)
            if cur_hash is None:
                conflicts.append((dst_rel, "unreadable", "読み取れない（大きすぎる・特殊ファイル）。手動で解消する（--update では上書きされない）"))
            elif cur_hash == sha256_bytes(text.encode("utf-8")):
                same.append(dst_rel)
            elif m_files.get(dst_rel) == cur_hash:
                plan_write(dst_rel, dst, text, executable, "update", "配置後に未編集のため、スキルの新版へ自動更新")
            elif args.update:
                plan_write(dst_rel, dst, text, executable, "update",
                           "--update による強制上書き（配置後の編集・別用途の内容を置き換える）")
            else:
                conflicts.append(edited_conflict(dst_rel, text))
    except ValueError as e:
        return finish(2, f"エラー: {e}")

    # ---- 差分表示（書き込みなし）
    if args.show_diff:
        diffs: dict[str, dict] = {}
        for path, kind, reason in conflicts:
            status, lines = build_diff(root_real, args.target / path, path, desired.get(path, ""))
            diffs[path] = {"conflict_kind": kind, "reason": reason, "status": status, "lines": lines}
        summary["diffs"] = diffs
        # 注意: same は「いま生成する内容と一致するか」であり、「適用直後から変わっていないか」ではない。更新の取り消しの
        # 判定には使わない（scripts/update-snapshot.sh の書き込み直後のハッシュとの比較を使う。references/update-recovery.md）
        summary.update(same=same, kept=keep, missing=missing)
        summary["conflicts"] = [{"path": p, "kind": k, "reason": r} for p, k, r in conflicts]
        if not args.json:
            if not conflicts:
                out("競合なし（表示する差分はない）")
            for path, d in diffs.items():
                out(f"=== {path}: {d['reason']}")
                for line in d["lines"]:
                    out(line)
        return finish(0)

    manifest_path = args.target / MANIFEST_REL
    why = write_target_problem(root_real, manifest_path)
    if why:
        problems.append(f"{MANIFEST_REL}（{why}）")

    # ---- .gitignore は分類フェーズで読み取り・検査する（書き込み後に例外で落ちて部分書き込みを残さない）
    gi = args.target / ".gitignore"
    gi_lines: list[str] = []
    gi_ends_nl = True
    why = write_target_problem(root_real, gi)
    if why:
        problems.append(f".gitignore（{why}）")
    elif os.path.lexists(gi):
        try:
            gi_text = read_text_capped(gi)
            gi_lines = gi_text.splitlines()
            gi_ends_nl = gi_text.endswith("\n") or not gi_text
        except (OSError, OverflowError, UnicodeDecodeError):
            problems.append(".gitignore（UTF-8 として読めない、または大きすぎる）")

    # ---- 廃止された所有ファイル（スキル側の固定リストだけで判定。表示のみで自動削除しない）
    deprecated: list[dict] = []
    for rel in DEPRECATED_OWNED:
        pth = args.target / rel
        if not regular_inside(root_real, pth):
            continue
        h = safe_sha256(pth)
        deprecated.append({"path": rel, "edited": (h != m_files[rel]) if rel in m_files else None})
    unknown = [k for k in m_files if k not in {d for _, d, _, kd in FILES if kd == OWNED}
               and k not in DEPRECATED_OWNED and k != MANIFEST_REL]
    if unknown:
        warnings.append(f"マニフェストの未知のエントリ {len(unknown)} 件を無視した（新しいマニフェストへは引き継がない）: "
                        + ", ".join(sanitize(u, 80) for u in sorted(unknown)[:3]))

    suspicious = unexpected_buildable(args.target, root_real)
    if suspicious:
        warnings.append("ローカルビルドで実行・読み込まれ得る、スキルが配置していないファイルがある: "
                        + ", ".join(sanitize(x, 80) for x in suspicious[:8])
                        + (f" ほか {len(suspicious) - 8} 件" if len(suspicious) > 8 else "")
                        + "。内容を確認する（信頼できないリポジトリではローカルビルドをしない）")

    new_manifest_obj = {
        "version": MANIFEST_VERSION, "ff_rev": ff_rev,
        "files": {**{k: manifest_hash(k, v) for k, v in owned_texts.items()},
                  # 廃止された所有ファイルは、残っている間は記録を保つ（次回以降も削除候補として表示するため）
                  **{d["path"]: m_files[d["path"]] for d in deprecated if d["path"] in m_files}},
    }
    new_manifest = json.dumps(new_manifest_obj, indent=2, sort_keys=True) + "\n"
    manifest_changed = True
    if regular_inside(root_real, manifest_path):
        try:
            manifest_changed = read_capped(manifest_path, MANIFEST_MAX_BYTES).decode("utf-8") != new_manifest
        except (OSError, OverflowError, UnicodeDecodeError):
            manifest_changed = True

    # FF_REV の旧→新（旧はマニフェスト、無ければ既存の FF_REV ファイル）
    old_ff = m_ff_rev
    if old_ff is None:
        p = args.target / "tools/docs-site-gen/FF_REV"
        try:
            if regular_inside(root_real, p):
                cand = read_capped(p, 1024).decode("utf-8").strip()
                old_ff = cand if FF_REV_RE.fullmatch(cand) else None
        except (OSError, OverflowError, UnicodeDecodeError):
            old_ff = None

    summary.update(
        ff_rev={"old": old_ff, "new": ff_rev, "changed": old_ff != ff_rev},
        same=same, kept=keep, missing=missing, deprecated=deprecated, warnings=warnings,
        conflicts=[{"path": p, "kind": k, "reason": r} for p, k, r in conflicts],
    )

    if problems:
        # 部分書き込みを避けるため、1 件でも不適なら何も書かずに中止する
        return finish(2, "エラー: 書き込み先が不適（リンク先の内外を問わず symlink には書かない）、または読み取れない: "
                      + ", ".join(problems))
    if conflicts:
        for w in warnings:
            out(f"警告 {w}", err=True)
        out("エラー: スキルが所有するファイルが既存で、内容が生成予定と一致しない（競合）。何も書かずに中止する:", err=True)
        for path, _kind, reason in conflicts:
            out(f"  - {path}（{reason}）", err=True)
        fixable = [p for p, k, _ in conflicts if k not in UNFIXABLE_KINDS]
        manual = [p for p, k, _ in conflicts if k in UNFIXABLE_KINDS]
        out("  対処: まず --show-diff で差分を確認する（内容を読むのは通常ファイルだけ）。", err=True)
        if fixable:
            out("  別用途・利用者の編集を残すなら手動で統合し、スキルの新版で置き換えてよいなら、同じ引数に --update を付けて"
                "再実行する（上書きされるのは所有ファイルのみ。pages.yml の追加 paths は利用者区間へ書けば更新でも保持される）。"
                + ("" if manifest is not None else
                   "マニフェストが無い旧版配置からの移行も、所有ファイルを編集していなければ、利用者の了承を得てから --update を 1 回実行すれば"
                   "マニフェストが書かれ、以後は未編集の所有ファイルが自動で更新される。"), err=True)
        if manual:
            out("  次は --update でも上書きされない（symlink・通常ファイルでない・読めない）。手動で解消してから再実行する: "
                + ", ".join(manual), err=True)
        return finish(EXIT_CONFLICT)

    for dst_rel, text, executable, action, reason in plan:
        dst = args.target / dst_rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_bytes(text.encode("utf-8"))   # バイト列で書く（プラットフォームの改行変換を通さない）
        if executable:
            dst.chmod(dst.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        summary["created" if action == "create" else "updated"].append(
            dst_rel if action == "create" else {"path": dst_rel, "reason": reason})
    if manifest_changed:
        manifest_path.parent.mkdir(parents=True, exist_ok=True)
        manifest_path.write_bytes(new_manifest.encode("utf-8"))
        summary["manifest_written"] = True
        summary["manifest_recreated"] = manifest is None and mode == "update"

    to_add = [line for line in GITIGNORE_LINES if line not in gi_lines]
    if to_add:
        prefix = "" if not gi_lines or gi_ends_nl else "\n"
        with gi.open("ab") as fh:
            fh.write((prefix + "\n# docs サイト（setup-github-pages）\n" + "\n".join(to_add) + "\n").encode("utf-8"))
    summary["gitignore_added"] = to_add

    # 配置後の検証（利用者編集ファイルを含む構成全体が build の前提を満たすか）
    import check_site
    brand_path = args.target / "tools" / "docs-site-gen" / "brand.toml"
    try:
        errors, check_warnings = check_site.check(root_real, brand_path)
    except ValueError as e:
        errors, check_warnings = [str(e)], []
    summary["check"] = {"ok": not errors, "errors": [sanitize(e) for e in errors],
                        "warnings": [sanitize(w) for w in check_warnings]}

    if not args.json:
        updated_paths = [u["path"] for u in summary["updated"]]
        out(f"モード: {mode}（{'; '.join(det['reasons'])}）")
        for w in warnings:
            out(f"警告 {w}")
        ff = summary["ff_rev"]
        out("FF_REV: " + (f"{ff['old'][:12]} → {ff['new'][:12]}" if ff["old"] and ff["changed"]
                         else "変更なし（" + ff["new"][:12] + "）" if ff["old"] else f"（旧不明）→ {ff['new'][:12]}"))
        out("作成: " + (", ".join(summary["created"]) or "なし"))
        out("更新: " + (", ".join(updated_paths) or "なし"))
        for u in summary["updated"]:
            out(f"  - {u['path']}: {u['reason']}")
        out("一致（変更なし）: " + (", ".join(same) or "なし"))
        out("保持（利用者編集）: " + (", ".join(keep) or "なし"))
        if missing:
            out("欠落（再作成しない。必要なら --owner/--repo/--branch/--title を付けて再実行）: " + ", ".join(missing))
        out("追記した .gitignore 行: " + (", ".join(to_add) or "なし"))
        out("マニフェスト: " + ("再作成した（マニフェストが無かった旧版配置からの移行）" if summary["manifest_recreated"]
                              else "書き込んだ" if summary["manifest_written"] else "変更なし"))
        if deprecated:
            out("削除候補（スキルで廃止された所有ファイル。自動削除しない。内容を確認して手動で削除する）:")
            for d in deprecated:
                state = "未編集" if d["edited"] is False else "配置後に編集あり" if d["edited"] else "編集の有無は不明"
                out(f"  - {d['path']}（{state}）")
        if keep:
            out("注: 保持したファイルの内容（brand.toml・nav.toml 等）は生成予定と一致する保証がない。"
                "下の check_site で検証する（失敗したら該当ファイルを直す）。")
        for w in check_warnings:
            out(f"警告 {w}")
    for e in errors:
        out(f"NG {e}", err=True)
    if errors:
        return finish(EXIT_CHECK_FAILED, "エラー: 配置後の検証（check_site）に失敗した。上の項目を直してから再実行する")
    if not args.json:
        out("check_site ok")
    return finish(0)


if __name__ == "__main__":
    sys.exit(main())
