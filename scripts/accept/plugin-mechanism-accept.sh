#!/usr/bin/env bash
# REQ-2（プラグイン機構）の受け入れ検証オーケストレータ（TASK-2.4、#21。
# 実プラグインペア再検証、イシュー #261）。
#
# `docs/spec/04-requirements.md` REQ-2 の受け入れ基準は「少なくとも 2 種のプラグイン
# （**WebSocket・GraphQL**）を feature flag で着脱できる」ことを名指ししている。
# TASK-2.4（#21）実施当時は実 WebSocket プラグイン（TASK-4.1、#22）が並行実装中だった
# ため、代替ペア（webrtc-proxy + graphql）で暫定実証していたが、両方が実装済みと
# なった現在は仕様名指しのペアで再検証する（#261）。
#
# 対象 feature ペアは環境変数 REQ2_FEATURES（空白区切り、既定 "websocket graphql"）で
# パラメータ化する。旧代替ペアは REQ2_FEATURES="webrtc-proxy graphql" で再現できる
# （後方互換）。入力は許可リスト正規表現 + cargo metadata 実在確認で検証し、
# 未知の文字列・存在しない feature 名は即 FAIL とする（コマンドインジェクション
# 防止・判定不能を PASS と偽らないフェイルクローズ、.claude/rules/security.md）。
#
# 検証する基準:
#   1. 対象 feature（既定: websocket・graphql）が `cargo metadata` に存在する
#      （少なくとも 2 種のプラグインを feature flag で着脱できることの前提）
#   2. `scripts/pay-for-what-you-use-check.sh` を呼び出し、feature 無効時の依存・
#      unsafe・バイナリサイズ完全除外を検証する（動的列挙のため対象追加時も
#      本スクリプトの変更は不要）。加えて対象 feature ペア限定の直接証跡として
#      `cargo tree` で当該プラグインクレートの出現/不出現を確認する
#   3. 各 feature 構成（無効・各 feature 単独・ペア同時有効・全 feature）で
#      `cargo build` / `cargo test` が成功する（動作確認は既存の実プラグイン
#      統合テスト、例: websocket_upgrade.rs・plugin_graphql_boundary.rs に委ねる）。
#      加えて対象プラグインクレート単体の契約テストも実行する
#   4. コンパイル時 vs 実行時動的ロードのトレードオフ設計文書
#      （`docs/design/plugin-loading-tradeoffs.md`）が存在する
#
# 両 feature 無効時のコア性能（REQ-1 基準維持、基準 5）は、計測用バイナリ
# （axum-ref 等価 4 エンドポイント、TASK-1.6-3 / #168 で整備済み）を使った専有計測
# wrapper（`benches/bench-accept-exclusive.sh`、TASK-260 / #260）の実測レポート
# `benches/reports/task-2.4-plugin-accept.md` の「## 結論」セクション内の「総合判定」
# 行（他セクションへの引用は無視し、複数存在時はレポート末尾に近い方＝最新の
# 再計測結果を採用する。基準 5 の実処理・判定ロジックは
# `lib/plugin-mechanism-conclusion-verdict.awk` 参照）を参照して判定する。
# host contention でレポートが BLOCKED のまま・未生成の場合は SKIP とし、
# 判定不能を PASS と偽らない（.claude/rules/security.md のフェイルクローズ原則）。
#
# 判定不能（cargo metadata 失敗・jq 未導入・前提スクリプト不在・未知の
# feature 名等）はフェイルクローズで FAIL とする。
#
# 呼び出し元: 人間が `bash scripts/accept/plugin-mechanism-accept.sh` として直接実行する。
# CI 常設ジョブへの組み込みは既存の `pay-for-what-you-use` ジョブ（TASK-2.2）が
# 本質的に同じ検証を担うため、本スクリプトは受け入れ記録用の手動実行を主目的とする。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
cd "${WORKSPACE_ROOT}"

# ---------------------------------------------------------------------------
# 0: 対象 feature ペアの入力検証（フェイルクローズ、コマンドインジェクション防止）
#    REQ2_FEATURES は空白区切りの feature 名リスト。cargo コマンド引数へ渡る
#    外部入力のため、[a-z0-9-]+ の許可リストで各トークンを検査してから使う
#    （eval 不使用・変数は常にクォート、.claude/rules/security.md A03 対策）。
# ---------------------------------------------------------------------------
# shellcheck disable=SC2206 # 空白区切りの意図的な単語分割（各要素は下記で許可リスト検証する）
REQ2_FEATURES_ARR=(${REQ2_FEATURES:-websocket graphql})

invalid_tokens=()
for f in "${REQ2_FEATURES_ARR[@]}"; do
    if ! [[ "${f}" =~ ^[a-z0-9-]+$ ]]; then
        invalid_tokens+=("${f}")
    fi
done
if [ "${#invalid_tokens[@]}" -gt 0 ]; then
    record_fail "0: REQ2_FEATURES 入力検証" "許可されない文字を含む feature 名: ${invalid_tokens[*]}（[a-z0-9-]+ のみ許可）"
    print_summary "REQ-2、TASK-2.4 / #21（再検証 #261）"
    exit "$(summary_exit_code)"
fi
if [ "${#REQ2_FEATURES_ARR[@]}" -lt 2 ]; then
    record_fail "0: REQ2_FEATURES 入力検証" "対象 feature が 2 種未満（REQ-2 は最低 2 種の着脱実証を要求）: ${REQ2_FEATURES_ARR[*]:-<空>}"
    print_summary "REQ-2、TASK-2.4 / #21（再検証 #261）"
    exit "$(summary_exit_code)"
fi

# ---------------------------------------------------------------------------
# 1: 対象 feature の存在確認（少なくとも 2 種のプラグインを feature flag で
#    着脱できることの前提）。ここで存在しない feature を検知したら、以降の
#    重い cargo build/tree ステップへ進む前に即 FAIL 終了する（未知の
#    feature 名で無駄な cargo 実行を重ねない・判定不能を PASS と偽らない
#    フェイルクローズ、.claude/rules/security.md）。
# ---------------------------------------------------------------------------
if ! command -v jq >/dev/null 2>&1; then
    record_fail "1: 対象プラグイン feature 存在確認" "jq が見つかりません（判定不能、フェイルクローズ）"
    print_summary "REQ-2、TASK-2.4 / #21（再検証 #261）"
    exit "$(summary_exit_code)"
fi
if ! metadata="$(cargo metadata --format-version 1 --no-deps 2>/tmp/plugin-mechanism-accept-metadata.log)"; then
    record_fail "1: 対象プラグイン feature 存在確認" "cargo metadata に失敗しました（/tmp/plugin-mechanism-accept-metadata.log 参照）"
    print_summary "REQ-2、TASK-2.4 / #21（再検証 #261）"
    exit "$(summary_exit_code)"
fi
core_features="$(printf '%s' "${metadata}" | jq -r '
    .packages[] | select(.name == "fandhe-backend-core") | .features | keys[]
' 2>/dev/null || true)"
missing=()
for f in "${REQ2_FEATURES_ARR[@]}"; do
    if ! printf '%s\n' "${core_features}" | grep -qx "${f}"; then
        missing+=("${f}")
    fi
done
if [ "${#missing[@]}" -eq 0 ]; then
    record_pass "1: 対象プラグイン feature 存在確認" "${REQ2_FEATURES_ARR[*]} が fandhe-backend-core に存在（着脱可能な ${#REQ2_FEATURES_ARR[@]} プラグインの前提を充足）"
else
    record_fail "1: 対象プラグイン feature 存在確認" "未検出の feature: ${missing[*]}"
    print_summary "REQ-2、TASK-2.4 / #21（再検証 #261）"
    exit "$(summary_exit_code)"
fi

# ---------------------------------------------------------------------------
# 2: pay-for-what-you-use 機械検証（TASK-2.2、#19）の呼び出し
#    graphql・websocket とも dep:fandhe-backend-plugin-* 命名規約に従うため、
#    scripts/pay-for-what-you-use-check.sh の動的列挙により追加対応なしで
#    検証対象に含まれる（同スクリプトの doc を参照）。
# ---------------------------------------------------------------------------
if [ ! -x "${WORKSPACE_ROOT}/scripts/pay-for-what-you-use-check.sh" ] && [ ! -f "${WORKSPACE_ROOT}/scripts/pay-for-what-you-use-check.sh" ]; then
    record_fail "2: pay-for-what-you-use 機械検証" "scripts/pay-for-what-you-use-check.sh が見つかりません"
elif bash "${WORKSPACE_ROOT}/scripts/pay-for-what-you-use-check.sh" >/tmp/plugin-mechanism-accept-pfwyu.log 2>&1; then
    record_pass "2: pay-for-what-you-use 機械検証" "scripts/pay-for-what-you-use-check.sh が PASS（詳細: /tmp/plugin-mechanism-accept-pfwyu.log）"
else
    record_fail "2: pay-for-what-you-use 機械検証" "scripts/pay-for-what-you-use-check.sh が FAIL（詳細: /tmp/plugin-mechanism-accept-pfwyu.log）"
fi

# 2b: 対象 feature ペア限定の直接証跡（cargo tree）。
#     プラグインクレート名は fandhe-backend-plugin-<feature 名> 規約に従う
#     （dep: 構文で implicit feature を作らない設計、pay-for-what-you-use.md）。
pair_tree_ok=1
pair_tree_detail=""
for f in "${REQ2_FEATURES_ARR[@]}"; do
    plugin_crate="fandhe-backend-plugin-${f}"
    disabled_tree="$(cargo tree -p fandhe-backend-core --no-default-features 2>/tmp/plugin-mechanism-accept-tree-"${f}"-disabled.log || true)"
    if printf '%s\n' "${disabled_tree}" | grep -q "${plugin_crate}"; then
        pair_tree_ok=0
        pair_tree_detail="${pair_tree_detail}${f}: 無効構成で ${plugin_crate} が cargo tree に出現（除外失敗）\n"
        continue
    fi
    enabled_tree="$(cargo tree -p fandhe-backend-core --no-default-features --features "${f}" 2>/tmp/plugin-mechanism-accept-tree-"${f}"-enabled.log || true)"
    if ! printf '%s\n' "${enabled_tree}" | grep -q "${plugin_crate}"; then
        pair_tree_ok=0
        pair_tree_detail="${pair_tree_detail}${f}: 有効構成で ${plugin_crate} が cargo tree に不出現（配線切れの疑い、ポジティブコントロール失敗）\n"
    fi
done
if [ "${pair_tree_ok}" -eq 1 ]; then
    record_pass "2b: 対象ペア cargo tree 直接確認" "${REQ2_FEATURES_ARR[*]} の各 feature で無効時不出現・有効時出現を確認"
else
    record_fail "2b: 対象ペア cargo tree 直接確認" "$(printf '%b' "${pair_tree_detail}")"
fi

# ---------------------------------------------------------------------------
# 3: 各 feature 構成のビルド・テスト（実プラグインの動作確認を兼ねる。
#    websocket 有効時は websocket_upgrade.rs（RFC 6455 ハンドシェイク）、
#    graphql 有効時は plugin_graphql_boundary.rs（実クエリ実行）が走る）
# ---------------------------------------------------------------------------
build_and_test() {
    local label="$1"
    shift
    local feature_args=("$@")
    if cargo build -p fandhe-backend-core "${feature_args[@]}" >/tmp/plugin-mechanism-accept-build-"${label}".log 2>&1 \
        && cargo test -p fandhe-backend-core "${feature_args[@]}" >/tmp/plugin-mechanism-accept-test-"${label}".log 2>&1; then
        record_pass "3: build/test（${label}）" "cargo build/test 成功"
    else
        record_fail "3: build/test（${label}）" "cargo build/test に失敗（詳細: /tmp/plugin-mechanism-accept-{build,test}-${label}.log）"
    fi
}

build_and_test "no-default-features" --no-default-features
for f in "${REQ2_FEATURES_ARR[@]}"; do
    build_and_test "${f}" --features "${f}"
done
pair_csv="$(IFS=,; echo "${REQ2_FEATURES_ARR[*]}")"
build_and_test "pair-${pair_csv}" --features "${pair_csv}"
build_and_test "all-features" --all-features

# 対象プラグインクレート単体の契約テスト（存在する場合のみ実行。
# fandhe-backend-plugin-<feature> 命名規約に従わないクレート・パッケージが
# 存在しない場合は SKIP としてフェイルクローズを維持する）。
for f in "${REQ2_FEATURES_ARR[@]}"; do
    plugin_crate="fandhe-backend-plugin-${f}"
    if cargo metadata --format-version 1 --no-deps 2>/dev/null | jq -e --arg name "${plugin_crate}" '.packages[] | select(.name == $name)' >/dev/null 2>&1; then
        if cargo test -p "${plugin_crate}" >/tmp/plugin-mechanism-accept-test-crate-"${f}".log 2>&1; then
            record_pass "3b: プラグイン単体契約テスト（${plugin_crate}）" "cargo test -p ${plugin_crate} 成功"
        else
            record_fail "3b: プラグイン単体契約テスト（${plugin_crate}）" "cargo test -p ${plugin_crate} に失敗（詳細: /tmp/plugin-mechanism-accept-test-crate-${f}.log）"
        fi
    else
        record_skip "3b: プラグイン単体契約テスト（${plugin_crate}）" "パッケージ ${plugin_crate} が workspace に存在しません"
    fi
done

# ---------------------------------------------------------------------------
# 4: 設計文書の存在確認
# ---------------------------------------------------------------------------
tradeoff_doc="${WORKSPACE_ROOT}/docs/design/plugin-loading-tradeoffs.md"
if [ -f "${tradeoff_doc}" ] && grep -q "実行時動的ロード" "${tradeoff_doc}"; then
    record_pass "4: 安全性トレードオフ設計文書" "docs/design/plugin-loading-tradeoffs.md が存在し、実行時動的ロードとの比較記述を含む"
else
    record_fail "4: 安全性トレードオフ設計文書" "docs/design/plugin-loading-tradeoffs.md が見つからない、または比較記述を確認できません"
fi

# ---------------------------------------------------------------------------
# 5: 性能維持（benches/reports/task-2.4-plugin-accept.md の「## 結論」セクション内の
#    「総合判定」行を参照。TASK-260 / #260。
#
#    単純な `grep -q '総合判定: PASS'` はレポート中に埋め込まれた過去実測の引用
#    （例:「## 判定根拠 1」節に転記された #168 レポートの「総合判定: PASS」）にも
#    ヒットしてしまい、トップレベルの結論を FAIL に変更しても引用側の PASS が先に
#    マッチして誤って PASS 判定になりうる（イシュー #260 Bugbot 指摘）。そのため
#    判定対象を「## 結論」見出し配下の行に限定し、他セクションへの引用は無視する。
#    「## 結論」セクションが複数存在する場合（`benches/bench-accept.sh` が
#    REPORT_MD 指定時に再計測のたびに新しい「## 結論（自動記録: ...）」セクションを
#    追記する設計）は、レポート末尾に最も近いセクションを最新の判定として採用し、
#    レポートを手編集しなくても再計測結果を機械的にゲートへ反映できるようにする
#    （同一セクション内で PASS・FAIL が両方現れる異常系は FAIL を優先し丸め込まない）。
#
#    「## 結論」セクション自体が存在しない・総合判定行が 1 件もない場合や、レポート
#    不在・BLOCKED 記録のみの場合は SKIP とする。PASS への丸め込みは行わない
#    （フェイルクローズ）。判定ロジック本体は `lib/plugin-mechanism-conclusion-verdict.awk`
#    に切り出し、`scripts/tests/run-plugin-mechanism-accept-tests.sh` で
#    cargo・ネットワーク非依存のフィクスチャによる回帰検証を行う。
# ---------------------------------------------------------------------------
plugin_accept_report="${WORKSPACE_ROOT}/benches/reports/task-2.4-plugin-accept.md"
if [ ! -f "${plugin_accept_report}" ]; then
    record_skip "5: 両 feature 無効時の性能維持（REQ-1 基準）" "benches/reports/task-2.4-plugin-accept.md が見つかりません。benches/bench-accept-exclusive.sh を実行して再計測してください"
else
    conclusion_verdict="$(awk -f "${SCRIPT_DIR}/lib/plugin-mechanism-conclusion-verdict.awk" "${plugin_accept_report}")"
    case "${conclusion_verdict}" in
        PASS)
            record_pass "5: 両 feature 無効時の性能維持（REQ-1 基準）" "benches/reports/task-2.4-plugin-accept.md の「## 結論」セクションの総合判定が PASS（詳細はレポート本文を参照）"
            ;;
        FAIL)
            record_fail "5: 両 feature 無効時の性能維持（REQ-1 基準）" "benches/reports/task-2.4-plugin-accept.md の「## 結論」セクションの総合判定が FAIL（詳細はレポート本文を参照）"
            ;;
        *)
            record_skip "5: 両 feature 無効時の性能維持（REQ-1 基準）" "benches/reports/task-2.4-plugin-accept.md の「## 結論」セクションに総合判定 PASS/FAIL の記録がありません（BLOCKED 等、判定不能）。host contention が落ち着いたタイミングで benches/bench-accept-exclusive.sh を再実行してください"
            ;;
    esac
fi

print_summary "REQ-2、TASK-2.4 / #21（再検証 #261）"
exit "$(summary_exit_code)"
