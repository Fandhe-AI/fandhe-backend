# benches/reports/task-2.4-plugin-accept.md（または同形式のレポート）から、
# 「## 結論」見出し配下の「総合判定」行のみを判定材料として抽出する
# （`scripts/accept/plugin-mechanism-accept.sh` 基準 5、TASK-260 / #260 Bugbot 指摘対応）。
#
# 単純な `grep -q '総合判定: PASS'` はレポート中の他セクション（「## 判定根拠 1」等）に
# 引用として埋め込まれた過去実測の「総合判定: PASS」にもヒットしてしまい、トップレベルの
# 結論を FAIL に変更しても引用側の PASS が先にマッチして誤って PASS 判定になりうる。
# 本ロジックは判定対象を「## 結論」見出し配下の行に限定し、他セクションへの引用は無視する。
#
# `benches/bench-accept.sh` は REPORT_MD 指定時、再計測のたびに新しい
# 「## 結論（自動記録: ...）」セクションを追記する設計（同見出しは前方一致で判定対象に
# 含まれる）。「## 結論」セクションが複数存在する場合はレポート末尾に最も近いセクションを
# 最新の判定として採用し、レポートを手編集しなくても再計測結果を機械的にゲートへ
# 反映できるようにする。同一セクション内で PASS・FAIL が両方現れる異常系は FAIL を
# 優先し丸め込まない（フェイルクローズ）。
#
# 出力: 標準出力に "PASS" / "FAIL" / 空文字（総合判定の記録なし）のいずれか 1 行。
#
# 「## 結論」セクションが検出されるたびに、そのセクションの判定（PASS/FAIL/空文字の
# いずれか）で無条件に `final` を上書きする（`insec == 1` であれば `verdict` が空でも
# 上書きする）。`benches/bench-accept.sh` は BLOCKED（baseline / CORE_BIN 未整備・
# 専有ロック取得不能・静穏未達）等、総合判定を確定できない再計測でも必ず新しい「## 結論」
# セクションを追記する設計のため、この無条件上書きがないと BLOCKED な再計測の後に
# 古い「## 結論」セクションの PASS/FAIL がそのまま権威として残ってしまう
# （stale PASS 問題、イシュー #260 Bugbot 指摘対応）。
#
# 呼び出し元: `scripts/accept/plugin-mechanism-accept.sh` が
# `awk -f "${SCRIPT_DIR}/lib/plugin-mechanism-conclusion-verdict.awk" <report.md>` として
# 呼び出す。`scripts/tests/run-plugin-mechanism-accept-tests.sh` が cargo・ネットワーク
# 非依存のフィクスチャで単体検証する。

function flush() {
    if (insec == 1) {
        final = verdict
    }
}

/^## / {
    flush()
    insec = ($0 ~ /^## 結論/) ? 1 : 0
    verdict = ""
    next
}

insec == 1 && /^\*\*総合判定: FAIL\*\*/ { verdict = "FAIL" }
insec == 1 && /^\*\*総合判定: PASS\*\*/ { if (verdict != "FAIL") { verdict = "PASS" } }

END {
    flush()
    print final
}
