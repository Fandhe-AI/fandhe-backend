#!/usr/bin/env bash
# REQ-16（汎用 JWT 検証プラグイン、イシュー #823）受け入れ検証の判定用純関数群。
#
# 役割: `scripts/accept/jwt-verify-accept.sh`（オーケストレータ）が cargo・git 等の
# 副作用を持つのに対し、本ファイルは「文字列・ファイルを入力にして判定結果を標準出力へ
# 出すだけ」の関数だけを置く。cargo・ネットワーク非依存のセルフテスト
# `scripts/tests/run-jwt-verify-accept-tests.sh` から単体で source して回帰検証する。
#
# 呼び出し元: jwt-verify-accept.sh / run-jwt-verify-accept-tests.sh。単体では実行しない。

set -euo pipefail

# 許可する `[dependencies]`（基準 4: 新規外部依存なし・暗号は ring のみ）。
JWT_ALLOWED_DEPS="fandhe-backend-core fandhe-backend-http base64 serde serde_json ring"

# 基準 3 の禁止パターン（同期ゲートに現れてはならない I/O・非同期の痕跡）。
JWT_FORBIDDEN_IO_PATTERN='async fn|\.await|tokio::|std::fs|std::net|thread::sleep|std::process'

# ディレクトリ配下の *.rs から行コメント（`//`・`///`・`//!`）だけの行を除き、
# 禁止パターンに当たった行を `file:line:text` で出す。0 件なら何も出さない。
# gate.rs の module doc に「`.await` なし」と書かれており、コメントを除かないと
# 基準 3 が偽陽性で FAIL する（REQ-1 レポートの基準 E・F と同型の誤検知の前例）。
# 引数: $1 ディレクトリ、$2.. 走査から除くパス接頭辞（任意）。
# 除外は「ゲートの外」の `jwks-refresh` 取り直しタスク（`src/refresh/`、設計 8.1。NFR-7 は
# `RequestGate::check` の契約で、タスクは設計上ゲートの外）に限って使う。除外した範囲が
# feature で閉じていることは `module_is_cfg_gated` で別途確かめる（fail-closed）。
detect_forbidden_io_in_sources() {
    local dir="${1}" f ex skip
    shift
    while IFS= read -r f; do
        skip=0
        for ex in "$@"; do
            case "${f}" in "${ex}"*) skip=1 ;; esac
        done
        [ "${skip}" = "1" ] && continue
        { grep -vnE '^[[:space:]]*//' "${f}" || true; } \
            | { grep -E "${JWT_FORBIDDEN_IO_PATTERN}" || true; } \
            | sed "s|^|${f}:|"
    done < <(find "${dir}" -type f -name '*.rs' | sort)
}

# Cargo.toml の `[dependencies]` 節のキーのうち、許可集合の外にあるものを出す。
# `[dev-dependencies]` など他の節は対象外。
# 引数: $1 Cargo.toml
check_dependency_allowlist() {
    local toml="${1}" dep
    awk '
        /^\[/ { in_deps = ($0 == "[dependencies]"); next }
        in_deps && /^[A-Za-z0-9_-]+[[:space:]]*=/ { sub(/[[:space:]]*=.*/, ""); print }
    ' "${toml}" | while IFS= read -r dep; do
        case " ${JWT_ALLOWED_DEPS} " in
            *" ${dep} "*) ;;
            *) echo "${dep}" ;;
        esac
    done
}


# `cargo test` 出力に、指定した各テスト名が `... ok` で現れるか確認し、
# 現れなかった名前を出す（件数一致だけで判定しない）。
# 引数: $1 cargo test 出力を入れたファイル、$2.. テスト名
check_test_names_ok() {
    local out="${1}" name
    shift
    for name in "$@"; do
        grep -qE "^test ${name} \.\.\. ok$" "${out}" || echo "${name}"
    done
}

# `cargo tree --prefix none` 出力中で、指定クレートの行数を数える。
# 引数: $1 出力ファイル、$2 クレート名
count_pkg_in_tree() {
    grep -cE "^${2} " "${1}" || true
}

# fuzz_targets 直下の target 名（*.rs の拡張子なし）を列挙する。
# 引数: $1 fuzz_targets ディレクトリ
list_fuzz_targets() {
    find "${1}" -maxdepth 1 -type f -name '*.rs' -exec basename {} .rs \; | sort
}

# Cargo.toml の `[dependencies]` 表に、指定名のキーが宣言されているか判定する（終了コード 0/1）。
# description・コメント・`[dev-dependencies]` 中の同名文字列には反応しない
# （REQ-1 レポートの基準 E・F と同型の「コメント・文言ヒットによる偽陽性」対策）。
# 引数: $1 Cargo.toml、$2 依存名
toml_has_normal_dependency() {
    awk -v name="${2}" '
        /^\[/ { in_deps = ($0 == "[dependencies]"); next }
        in_deps && /^[[:space:]]*#/ { next }
        in_deps && /^[A-Za-z0-9_-]+[[:space:]]*=/ {
            key = $0; sub(/[[:space:]]*=.*/, "", key)
            if (key == name) found = 1
        }
        END { exit(found ? 0 : 1) }
    ' "${1}"
}

# scripts/fuzz.sh の `FUZZ_DIRS=(...)` 代入行（コメント行を除く）に指定ディレクトリが
# 登録されているか判定する（終了コード 0/1）。ヘッダコメントの言及にはヒットさせない。
# 引数: $1 fuzz.sh、$2 fuzz クレートのディレクトリ
fuzz_dirs_registers() {
    grep -vE '^[[:space:]]*#' "${1}" | grep -E '^FUZZ_DIRS=\(' | grep -qF "\"${2}\""
}

# ワークフロー YAML の指定ジョブ（`jobs:` 直下、2 スペースインデントのキー）の範囲内で、
# コメント行を除いた `run:` 行が指定スクリプトを実行しているか判定する（終了コード 0/1）。
# 引数: $1 ワークフロー YAML、$2 ジョブ ID、$3 スクリプトパス
workflow_job_runs_script() {
    awk -v job="${2}" -v script="${3}" '
        /^  [A-Za-z0-9_-]+:/ { in_job = ($0 ~ "^  " job ":"); next }
        in_job && /^[[:space:]]*#/ { next }
        in_job && /run:/ && index($0, script) { found = 1 }
        END { exit(found ? 0 : 1) }
    ' "${1}"
}

# `[dependencies]` の指定依存が feature でのみ有効になる optional 依存かを判定する（終了コード 0/1）。
# 次の 3 点をすべて満たすときだけ成功する（`jwks-refresh` の tokio、設計 8.2）:
#   1. `[dependencies]` の `<dep>` が `optional = true`
#   2. `[features]` の `<feature>` が `"dep:<dep>"` を含む
#   3. `default` に `<feature>` が含まれない
# 引数: $1 Cargo.toml、$2 依存名、$3 feature 名
optional_dep_gated_by_feature() {
    awk -v dep="${2}" -v feat="${3}" '
        /^\[/ { sect = $0; next }
        /^[[:space:]]*#/ { next }
        sect == "[dependencies]" && $0 ~ "^" dep "[[:space:]]*=" {
            if ($0 ~ /optional[[:space:]]*=[[:space:]]*true/) optional = 1
        }
        sect == "[features]" && $0 ~ "^" feat "[[:space:]]*=" {
            if (index($0, "\"dep:" dep "\"")) dep_in_feature = 1
        }
        sect == "[features]" && /^default[[:space:]]*=/ {
            if (index($0, "\"" feat "\"")) in_default = 1
        }
        END { exit((optional && dep_in_feature && !in_default) ? 0 : 1) }
    ' "${1}"
}

# lib.rs で指定モジュールの宣言が `#[cfg(feature = "<feature>")]` の直後に置かれているか判定する
# （終了コード 0/1）。コメント行は無視する。
# 引数: $1 lib.rs、$2 モジュール名、$3 feature 名
module_is_cfg_gated() {
    awk -v mod="${2}" -v feat="${3}" '
        /^[[:space:]]*\/\// { next }
        $0 ~ "^(pub )?mod " mod ";" { if (prev_cfg) ok = 1; prev_cfg = 0; next }
        { prev_cfg = (index($0, "#[cfg(feature = \"" feat "\")]") > 0) }
        END { exit(ok ? 0 : 1) }
    ' "${1}"
}

# `cargo tree -e features -i tokio --prefix none` の出力から、tokio 自身の feature 行
# （`tokio feature "..."`）だけを重複なく並べて出す。feature 有無の比較に使う。
# 引数: $1 出力ファイル
tokio_feature_lines() {
    { grep -E '^tokio feature "' "${1}" || true; } | sed 's/ (\*)$//' | sort -u
}
