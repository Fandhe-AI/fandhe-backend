#!/usr/bin/env bash
# Make を介さない操作一覧。`make help` は Makefile の `## ` コメントから機械的に生成されるのに
# 対し、本ファイルは手動で保守する一覧であり、両者を自動で同期する仕組みは無い。
# Makefile のターゲットを追加・改名・削除したら同じ変更で本ファイルも更新すること
# （.claude/rules/makefile.md）。
set -euo pipefail

if [ "$#" -ne 0 ]; then
  echo "usage: $(basename "$0") (no arguments)" >&2
  exit 2
fi

cat <<'EOF'
Make を使わずに直接実行する方法（make <target> は同じコマンドを呼ぶ薄い入口）:

  setup        git submodule update --init && scripts/hooks.sh   開発環境を構築する
  hooks        scripts/hooks.sh                                   git hooks を配線する
  build        cargo build --workspace
  build-all    cargo build --workspace --all-features
  test         cargo test --workspace
  test-all     cargo test --workspace --all-features
  fmt          cargo fmt --all
  fmt-check    cargo fmt --all --check
  clippy       cargo clippy --workspace --all-targets --all-features -- -D warnings
  lint         fmt-check + clippy
  audit        scripts/dep-audit.sh                               依存監査（cargo audit / cargo deny）
  doc          cargo doc --workspace --all-features --no-deps
  webrtc-e2e   scripts/webrtc-e2e.sh                              RebindHandle::rebind の実接続 e2e
  docker-build docker compose build dev
  docker-shell docker compose run --rm dev
  docker-test  docker compose run --rm dev make test-all
EOF
