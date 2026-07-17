#!/usr/bin/env bash
# NFR-6（`docs/spec/04-requirements.md`）の RPS 比判定ロジック単体
# （TASK-8.4 / #29）。
#
# `scripts/accept/webrtc-accept.sh` の基準 E が `evaluate_nfr6_ratio` を呼ぶ。
# 判定ロジックのみを独立ファイルへ切り出す理由: `webrtc-accept.sh` 本体は
# 末尾で `check_*` 関数群を実行する副作用（cargo tree・cargo audit・oha 実行等）を
# 持つため、`scripts/tests/run-webrtc-accept-tests.sh` が cargo/ネットワーク非依存で
# 判定ロジックだけを source してオフライン検証できるようにする
# （`scripts/tests/run-triage-tests.sh` 等、他の受け入れ系オフラインテストと同様に
# 副作用のあるスクリプト本体を直接 source しない方針）。
#
# 単体では実行しない（関数定義のみ、副作用なし）。

# 引数: $1 RPS 比（%、例 "95.23"）、$2 p95 レイテンシ比（%、例 "106.57"、省略可）
# 標準出力: "PASS" | "WARN" | "FAIL"
#
# NFR-6（docs/spec/04-requirements.md）は「無関係なパスへの RPS・レイテンシ影響」を
# 要求範囲に含むため、RPS 比だけでなく p95 レイテンシ比も判定に用いる。$2 省略時は
# RPS 比のみで判定する（呼び出し元が p95 を計測できない場合のフォールバック）。
#
# 判定帯（doc は webrtc-accept.sh 側にも詳細記載。ここでは値のみを保持する）:
#   - RPS 比: 実務許容帯 [95, 105]・狭義 NFR-6 帯 [100.3, 100.8]（両側判定。低すぎても
#     高すぎても悪化とみなす）
#   - p95 比: 実務許容帯 [0, 105]・狭義 NFR-6 帯 [0, 100.8]（片側判定。レイテンシは
#     低い方向への乖離を問題にしないため下限を設けない）
#   いずれも実務許容帯外は FAIL（受け入れ未達、フェイルクローズ）、実務許容帯内・
#   狭義帯外は WARN（受け入れとしては通すが乖離を必ず記録し PASS に丸めない）。
#   RPS・p95 双方の判定のうち悪い方（FAIL > WARN > PASS）を総合判定として採用する。
evaluate_nfr6_ratio() {
    local rps_ratio_pct="$1"
    local p95_ratio_pct="${2:-}"
    local practical_min=95
    local practical_max=105
    local strict_min="100.3"
    local strict_max="100.8"

    # NOTE: awk はロケールのデフォルト小数点文字（カンマ小数点ロケール等）に影響され、
    # "100.3" 等のリテラルを数値として正しく比較できない場合がある。LC_NUMERIC=C を
    # 明示してロケール非依存の小数点判定にする（兄弟スクリプト
    # `benches/webrtc-nfr6-bench.sh` の比率計算と同じ対策）。
    local rps_verdict
    if LC_NUMERIC=C awk -v v="${rps_ratio_pct}" -v lo="${practical_min}" -v hi="${practical_max}" 'BEGIN { exit !(v >= lo && v <= hi) }'; then
        if LC_NUMERIC=C awk -v v="${rps_ratio_pct}" -v lo="${strict_min}" -v hi="${strict_max}" 'BEGIN { exit !(v >= lo && v <= hi) }'; then
            rps_verdict="PASS"
        else
            rps_verdict="WARN"
        fi
    else
        rps_verdict="FAIL"
    fi

    local p95_verdict="PASS"
    if [ -n "${p95_ratio_pct}" ]; then
        if LC_NUMERIC=C awk -v v="${p95_ratio_pct}" -v hi="${practical_max}" 'BEGIN { exit !(v <= hi) }'; then
            if LC_NUMERIC=C awk -v v="${p95_ratio_pct}" -v hi="${strict_max}" 'BEGIN { exit !(v <= hi) }'; then
                p95_verdict="PASS"
            else
                p95_verdict="WARN"
            fi
        else
            p95_verdict="FAIL"
        fi
    fi

    # 悪い方（FAIL > WARN > PASS）を総合判定として採用する。
    if [ "${rps_verdict}" = "FAIL" ] || [ "${p95_verdict}" = "FAIL" ]; then
        echo "FAIL"
    elif [ "${rps_verdict}" = "WARN" ] || [ "${p95_verdict}" = "WARN" ]; then
        echo "WARN"
    else
        echo "PASS"
    fi
}
