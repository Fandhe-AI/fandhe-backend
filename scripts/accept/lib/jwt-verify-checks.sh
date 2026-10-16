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
# 引数: $1 ディレクトリ
detect_forbidden_io_in_sources() {
    local dir="${1}" f
    while IFS= read -r f; do
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
