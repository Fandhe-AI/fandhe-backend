#!/usr/bin/env bash
# TASK-9.5（#65 / REQ-9）: hub 共通配線の「配線コード削減率」判定ロジック。
#
# `scripts/accept/hub-wiring-accept.sh` 基準 B が本ライブラリの関数群を使う。
# 副作用（ファイル読み込みのみ、cargo・ネットワーク非依存）を持たないため、
# `scripts/tests/run-hub-wiring-accept-tests.sh` が擬似ソース（`scripts/tests/
# fixtures/hub-wiring-accept/`）に対してオフラインで単体テストできる
# （`scripts/accept/lib/nfr6-ratio.sh` と同型の切り出し方針）。
#
# 単体では実行しない（関数定義のみ）。

# `// --- wiring:begin ---` 〜 `// --- wiring:end ---` の区間（マーカー行自体は
# 含まない）から、空行・行コメントのみの行を除いた実 LOC を数える。
# 引数: $1 対象ファイルパス
# 標準出力: 整数（マーカー区間が見つからない場合は 0）
count_wiring_loc() {
    local file="$1"
    # マーカーは行頭コメント `// --- wiring:begin ---` / `// --- wiring:end ---`
    # の厳密一致のみを対象にする。crate doc（`//!`）中でマーカーを引用・説明する
    # 散文（例: 本ファイル冒頭の解説行）は "wiring:begin" という部分文字列を含む
    # ことがあり、緩い部分一致だと doc 内の説明文自体を開始マーカーと誤認して
    # 区間が異常に広がる（1 行に begin・end 両方の言及が出た場合、同一レコードの
    # 処理は最初にマッチしたパターンで `next` するため 2 個目のパターンへ到達
    # できない、という awk の逐次評価順の落とし穴も避ける）。
    awk '
        /^[ \t]*\/\/[ \t]*---[ \t]*wiring:begin[ \t]*---[ \t]*$/ { capture = 1; next }
        /^[ \t]*\/\/[ \t]*---[ \t]*wiring:end[ \t]*---[ \t]*$/   { capture = 0; next }
        capture {
            line = $0
            gsub(/^[ \t]+|[ \t]+$/, "", line)
            if (line == "") next
            if (line ~ /^\/\//) next
            count++
        }
        END { print count + 0 }
    ' "${file}"
}

# 利用側ハンドラ領域（`fn build_router` 〜 次のトップレベル `fn ` 直前まで）に、
# 手書き JWT 検証・JWKS パース等の配線シンボルが現れていないか検査する。
# 現れていれば「配線をプラグインへ集約できていない」ことを意味し、削減率の
# 前提が崩れる（TASK-9.5 受け入れ基準 B）。
# 引数: $1 対象ファイルパス
# 標準出力: 検出した行（1 件以上あれば手書き配線混入、空なら混入なし）
detect_handwritten_auth_in_handlers() {
    local file="$1"
    local region
    region="$(awk '
        /^fn build_router/ { capture = 1 }
        capture { print }
        capture && /^}/ && NR > 1 && found_start { exit }
        /^fn build_router/ { found_start = 1 }
    ' "${file}")"
    if [ -z "${region}" ]; then
        # `build_router` が存在しないファイル（フィクスチャ等）はハンドラ領域全体を対象にする。
        region="$(cat "${file}")"
    fi
    printf '%s\n' "${region}" | grep -nE '\b(verify_token|RsaKeyPair|JwksKeySet|SharedJwks::(new|from_json)|TenantGateConfig::(new|from_jwks_json))\b' || true
}

# 配線 LOC の削減率を PoC-6 基準（207 行）に対して評価する。
# 引数: $1 実測 LOC（マーカー区間）、$2（省略可）基準行数、既定 207
# 標準出力: "PASS <reduction_pct>" | "FAIL <reduction_pct>"
#
# 判定帯: 削減率 90% 以上を「実質 100% 削減」として PASS とする（マーカー区間には
# 環境変数分岐等の付随コードも含み得るため、0 行ちょうどではなく実務的な余裕を持たせる。
# TASK-9.5 実測は 207 行 → 6 行前後 = 削減率 97% 超を想定）。
evaluate_wiring_reduction() {
    local actual_loc="$1"
    local baseline_loc="${2:-207}"
    local reduction_pct
    reduction_pct="$(LC_NUMERIC=C awk -v actual="${actual_loc}" -v baseline="${baseline_loc}" \
        'BEGIN { printf "%.1f", ((baseline - actual) / baseline) * 100 }')"

    if LC_NUMERIC=C awk -v v="${reduction_pct}" 'BEGIN { exit !(v >= 90) }'; then
        echo "PASS ${reduction_pct}"
    else
        echo "FAIL ${reduction_pct}"
    fi
}
