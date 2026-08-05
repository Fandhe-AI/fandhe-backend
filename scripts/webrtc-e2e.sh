#!/usr/bin/env bash
# WebRTC rebind force-close e2e テスト実行スクリプト（イシュー #507）。
#
# `crates/plugin-webrtc/tests-e2e`（root workspace から exclude 済みの standalone
# crate、`crates/http/fuzz` と同パターン）を独立 workspace としてビルド・テストする。
# `RebindHandle::rebind` が確立済み `RTCPeerConnection` を実際に force-close する
# ことを実 ICE/DTLS シグナリングで検証する（`RebindHandle::rebind` の doc 契約の
# end-to-end カバレッジ）。stable ツールチェーンのみで動作する（scripts/fuzz.sh と
# 異なり nightly・追加ツールのインストール不要）。
#
# CI からは .github/workflows/ci.yml の `webrtc-e2e` ジョブ経由で呼ばれ、
# `ci-complete` の判定対象に含まれる。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
E2E_MANIFEST="${REPO_ROOT}/crates/plugin-webrtc/tests-e2e/Cargo.toml"

if [ ! -f "${E2E_MANIFEST}" ]; then
    echo "エラー: ${E2E_MANIFEST} が見つかりません" >&2
    exit 1
fi

cargo test --manifest-path "${E2E_MANIFEST}"
