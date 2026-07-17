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

# 引数: $1 RPS 比（%、例 "95.23"）
# 標準出力: "PASS" | "WARN" | "FAIL"
#
# 判定帯（doc は webrtc-accept.sh 側にも詳細記載。ここでは値のみを保持する）:
#   - 実務許容帯 [95, 105]: NFR-7（ミドルウェア型、RPS 劣化 5% 以内）の先例を踏まえた
#     フェイルクローズ境界。範囲外は FAIL（受け入れ未達）
#   - 狭義 NFR-6 帯 [100.3, 100.8]: 要件書の文言どおりの帯。実務許容帯内だが
#     狭義帯外は WARN（受け入れとしては通すが、乖離を必ず記録し PASS に丸めない）
evaluate_nfr6_ratio() {
    local rps_ratio_pct="$1"
    local practical_min=95
    local practical_max=105
    local strict_min="100.3"
    local strict_max="100.8"

    if awk -v v="${rps_ratio_pct}" -v lo="${practical_min}" -v hi="${practical_max}" 'BEGIN { exit !(v >= lo && v <= hi) }'; then
        if awk -v v="${rps_ratio_pct}" -v lo="${strict_min}" -v hi="${strict_max}" 'BEGIN { exit !(v >= lo && v <= hi) }'; then
            echo "PASS"
        else
            echo "WARN"
        fi
    else
        echo "FAIL"
    fi
}
