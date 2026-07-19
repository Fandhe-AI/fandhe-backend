#!/usr/bin/env bash
# pay-for-what-you-use-check.sh セルフテスト用の cargo geiger モック（常時失敗）。
#
# 毎回、CI 実機で観測された cargo-geiger の一過性 panic（cargo-0.86.0 の PackageSet
# 内部 assertion、Issue #212）を模して stderr へ panic メッセージを出し
# exit 101・stdout 空で終わる。リトライ上限（3 回）到達後に fail-closed で
# FAIL 判定される経路の検証に使う。
#
# 呼び出し元: pay-for-what-you-use-check.sh の PFWU_GEIGER_CMD フック
# （run-pay-for-what-you-use-tests.sh から環境変数経由で注入される。引数は見ない）。
set -euo pipefail

echo "thread 'main' panicked at cargo-0.86.0/src/cargo/core/package.rs:298:9: assertion failed: self.pending_ids.insert(id)" >&2
exit 101
