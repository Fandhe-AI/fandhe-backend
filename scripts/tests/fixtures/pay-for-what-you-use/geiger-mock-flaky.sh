#!/usr/bin/env bash
# pay-for-what-you-use-check.sh セルフテスト用の cargo geiger モック（一過性失敗の再現）。
#
# 環境変数 GEIGER_MOCK_STATE が指す状態ファイルで呼び出し回数を数え、2 回目までは
# CI 実機で観測された cargo-geiger の一過性 panic（cargo-0.86.0 の PackageSet 内部
# assertion、Issue #212）を模して stderr へ panic メッセージを出し exit 101・stdout 空、
# 3 回目で `.packages[].package.id.name` の jq クエリで解析可能な最小 JSON を stdout へ
# 出して成功する。リトライループの「失敗 → 回復」経路の検証に使う。
#
# 呼び出し元: pay-for-what-you-use-check.sh の PFWU_GEIGER_CMD フック
# （run-pay-for-what-you-use-tests.sh から環境変数経由で注入される。引数は見ない）。
set -euo pipefail

state_file="${GEIGER_MOCK_STATE:?GEIGER_MOCK_STATE（呼び出し回数の状態ファイルパス）が未設定です}"
count=0
if [ -f "${state_file}" ]; then
    count="$(cat "${state_file}")"
fi
count=$((count + 1))
printf '%s' "${count}" >"${state_file}"

if [ "${count}" -lt 3 ]; then
    echo "thread 'main' panicked at cargo-0.86.0/src/cargo/core/package.rs:298:9: assertion failed: self.pending_ids.insert(id)" >&2
    exit 101
fi

# geiger_packages が空にならないよう（fail-closed 判定を避けるため）、プラグイン
# クレートを含まないパッケージ 1 件を返す。
printf '%s' '{"packages":[{"package":{"id":{"name":"fandhe-backend-core"}}}]}'
exit 0
