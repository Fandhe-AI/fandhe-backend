#!/usr/bin/env bash
# 決定的マイクロベンチ実行スクリプト（イシュー #615）。
#
# `benches/microbench`（per-request alloc カウンタ、root workspace から exclude
# 済みの standalone crate、`crates/http/fuzz` と同パターン）をビルド・実行する。
# 比較・判定ロジック本体は `benches/microbench/src/main.rs` の
# `compare_with_baseline` に集約されており、本スクリプトはビルド・引数選択の
# みを担う薄いラッパー（`scripts/webrtc-e2e.sh` と同型の位置づけ）。
#
# CI からは .github/workflows/ci.yml の `microbench` ジョブ経由で
# `--check` 付きで呼ばれ、`ci-complete` の判定対象に含まれる。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MICROBENCH_DIR="${SCRIPT_DIR}/microbench"
MANIFEST="${MICROBENCH_DIR}/Cargo.toml"
BASELINE="${MICROBENCH_DIR}/baseline.json"

usage() {
    cat <<EOF
使い方: bash benches/microbench.sh [--check | --update-baseline]

  --check            計測結果を ${BASELINE} と比較し、退行があれば非 0 終了する
                      （CI の microbench ジョブと同一挙動）
  --update-baseline  ${BASELINE} を現在の計測値で上書きする
                      （レビュー承認前提、`.claude/rules/improvement-proposal.md`
                      の「自動更新提案」相当。ベースライン縮小・toolchain 更新
                      起因の再計測時のみ使う想定）
  引数なし           計測結果を JSON で stdout へ出力するのみ（比較・更新なし）
EOF
}

if [ ! -f "${MANIFEST}" ]; then
    echo "エラー: ${MANIFEST} が見つかりません" >&2
    exit 1
fi

MODE="run"
case "${1:-}" in
    --check)
        MODE="check"
        ;;
    --update-baseline)
        MODE="update-baseline"
        ;;
    "")
        MODE="run"
        ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        echo "エラー: 不明な引数です: $1" >&2
        usage >&2
        exit 1
        ;;
esac

# `cargo build` 後にバイナリパスを決め打ちで探すと、`CARGO_TARGET_DIR`（環境変数・
# `.cargo/config.toml` の `build.target-dir`）が設定された環境で成果物が別の
# ディレクトリに出て見つからない事象を起こす（イシュー #480、
# `benches/lib/common.sh` の `BENCH_TARGET_DIR` 導出が対処した問題と同型）。
# `cargo run` は cargo 自身がビルド・実行対象の解決を担うため、この決め打ちパス
# 問題が構造的に発生しない。子プロセス（本体バイナリ）の標準出力・終了コードは
# `cargo run` がそのまま透過する（cargo 自身の進捗表示は標準エラーへ出るため、
# `bash benches/microbench.sh > out.json` の呼び出し側契約は変わらない）。
echo "microbench: ビルド・実行中（release）..." >&2

# `--locked` でコミット済み `benches/microbench/Cargo.lock` からの依存解決を強制する
# （codex-review PR #619 P1 指摘対応）。決定的マイクロベンチは「同一コード・同一環境
# での厳密比較」が運用契約であり、lockfile 非固定だとクリーンな CI 実行のたびに
# `serde_json` 等が再解決されて alloc 特性が変わりうる（PR 差分なしでラチェットが
# 揺れる／逆方向の変化を隠す）。Cargo.lock 更新（依存追加・バージョン更新）時は
# `cargo generate-lockfile --manifest-path benches/microbench/Cargo.toml` で明示的に
# 再生成しコミットする。
case "${MODE}" in
    run)
        cargo run --release --locked --manifest-path "${MANIFEST}"
        ;;
    check)
        cargo run --release --locked --manifest-path "${MANIFEST}" -- --check "${BASELINE}"
        ;;
    update-baseline)
        cargo run --release --locked --manifest-path "${MANIFEST}" -- --update-baseline "${BASELINE}"
        ;;
esac
