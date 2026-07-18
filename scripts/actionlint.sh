#!/usr/bin/env bash
# ワークフロー静的検証（TASK-15（#180）、docs/spec 由来ではなく PR #98 以降の
# 「actionlint 環境未導入のため未実施」記録を解消する CI 常設化）。
#
# `.github/workflows/*.yml` を actionlint で検証する。式インジェクション
# （untrusted input の `run:` 直展開、OWASP A03）・構文誤り・`needs` 参照切れ等を
# 機械的に検知し、ワークフロー変更の退行を防ぐゲート（.claude/rules/ci.md）。
#
# ローカル（開発者の手元）・CI（ci.yml の actionlint ジョブ）の双方から同一コマンドで
# 呼び出す想定。バージョンは本スクリプトの定数を単一真実源とし、CI 側の
# 「Ensure actionlint」ステップはこの値を読み取って導入する（scripts/fuzz.sh の
# PINNED_NIGHTLY と同じパターン）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

# actionlint バージョン + linux/amd64 tarball の SHA256（単一真実源）。
# 更新する場合は本行と https://github.com/rhysd/actionlint/releases の
# `actionlint_<version>_checksums.txt` を突き合わせてから両方を書き換える。
ACTIONLINT_VERSION="1.7.12"
ACTIONLINT_SHA256_LINUX_AMD64="8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8"

usage() {
    cat <<EOF
使い方: bash scripts/actionlint.sh [ファイル...]

  （引数なし）: .github/workflows/*.yml を actionlint で検証する（CI 既定）。
  ファイル...  : 指定したファイルのみを検証する（セルフテスト・陰性対照用）。
EOF
}

case "${1:-}" in
    -h | --help)
        usage
        exit 0
        ;;
esac

# --------------------------------------------------
# 前提ツールの存在検査（自動インストールしない。.claude/rules/security.md・
# scripts/README.md の既存規約に準拠。CI 側は Ensure ステップが冪等インストールを担う）。
# --------------------------------------------------
if ! command -v actionlint >/dev/null 2>&1; then
    echo "エラー: actionlint が見つかりません。次のコマンドで導入してください（SHA256 検証込み）:" >&2
    echo "  curl -fsSL -o /tmp/actionlint.tar.gz https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}/actionlint_${ACTIONLINT_VERSION}_linux_amd64.tar.gz" >&2
    echo "  echo '${ACTIONLINT_SHA256_LINUX_AMD64}  /tmp/actionlint.tar.gz' | sha256sum -c -" >&2
    echo "  tar xzf /tmp/actionlint.tar.gz -C /tmp actionlint && install -m 755 /tmp/actionlint \"\${HOME}/.local/bin/actionlint\"" >&2
    exit 2
fi

installed_version="$(actionlint -version 2>/dev/null | head -n1 || true)"
if [ "${installed_version}" != "${ACTIONLINT_VERSION}" ]; then
    echo "警告: actionlint のバージョンが pin（${ACTIONLINT_VERSION}）と異なります（検出値: ${installed_version:-不明}）。" >&2
    echo "      CI では Ensure ステップが pin バージョンを保証しますが、ローカル実行では結果が変わる可能性があります。" >&2
fi

if ! command -v shellcheck >/dev/null 2>&1; then
    echo "警告: shellcheck が見つかりません。actionlint の \`run:\` ブロック検査（shellcheck 統合）が縮退します。" >&2
    echo "      導入コマンド（例）: OS のパッケージマネージャで shellcheck を導入してください（例: apt install shellcheck）" >&2
fi

# --------------------------------------------------
# 検証本体。引数指定時はそのファイルのみ（セルフテストの陰性対照・fixture 検査用）、
# 無指定時は .github/workflows 配下を actionlint の既定探索に委ねる。
# --------------------------------------------------
if [ "$#" -gt 0 ]; then
    echo "==> actionlint（指定ファイル: $*）"
    exec actionlint -no-color "$@"
else
    echo "==> actionlint（.github/workflows/*.yml）"
    exec actionlint -no-color
fi
