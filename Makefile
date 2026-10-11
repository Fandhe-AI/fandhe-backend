# 開発タスクの入口となる Makefile。CI（.github/workflows/ci.yml）の主要ジョブと同一
# コマンドをローカルで再現し、`make setup` 一発で開発環境（submodule・git hooks）を
# 構築できるようにする。Docker 経由の環境非依存な開発は docker-* ターゲットを使う
# （Dockerfile / compose.yaml）。
#
# Make は薄い入口であり、レシピは scripts/*.sh または言語ネイティブの 1 行コマンド
# （cargo / docker compose 等）の呼び出しに限定する。複数行・条件分岐を要する処理は
# scripts/ 側に置く。Make を使わない場合の同等コマンドは scripts/help.sh を参照。
# ターゲットを追加・改名・削除したら scripts/help.sh も同じ変更で更新すること。
# 変更時は .claude/rules/makefile.md に従う。
#
# GNU Make 3.81（macOS 標準）互換。3.82+ 専用機能（.ONESHELL・.RECIPEPREFIX 等）は使わない。
# 中身の無いターゲット（`@true` 等）は呼び出し元が終了コードを信用してしまうため置かない。

.DEFAULT_GOAL := help

.PHONY: help setup hooks build build-all test test-all fmt fmt-check clippy lint audit doc \
	webrtc-e2e docker-build docker-shell docker-test

help: ## ターゲット一覧を表示する（Make 非依存の一覧: scripts/help.sh）
	@grep -E '^[a-z][a-z0-9-]*:.*##' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-14s %s\n", $$1, $$2}'

setup: hooks ## 開発環境を構築する（submodule 取得 + git hooks 配線）
	git submodule update --init

hooks: ## lefthook で git hooks（pre-commit / commit-msg）を配線する
	@scripts/hooks.sh

build: ## デフォルト feature 構成でビルドする
	cargo build --workspace

build-all: ## 全 feature 有効でビルドする
	cargo build --workspace --all-features

test: ## デフォルト feature 構成でテストする
	cargo test --workspace

test-all: ## 全 feature 有効でテストする（doc test 含む）
	cargo test --workspace --all-features

fmt: ## rustfmt で整形する
	cargo fmt --all

fmt-check: ## 整形差分を検査する（CI fmt ジョブと同一）
	cargo fmt --all --check

clippy: ## clippy lint を検査する（CI clippy ジョブと同一）
	cargo clippy --workspace --all-targets --all-features -- -D warnings

lint: fmt-check clippy ## fmt-check + clippy をまとめて実行する

audit: ## 全 feature 構成の依存監査（cargo audit / cargo deny check）
	@scripts/dep-audit.sh

doc: ## rustdoc を生成する
	cargo doc --workspace --all-features --no-deps

webrtc-e2e: ## RebindHandle::rebind の実接続 force-close e2e テスト（standalone crate、#507）
	@scripts/webrtc-e2e.sh

docker-build: ## 開発用 Docker イメージをビルドする
	docker compose build dev

docker-shell: ## 開発用コンテナのシェルに入る（リポジトリを /work にマウント）
	docker compose run --rm dev

docker-test: ## コンテナ内で test-all を実行する（環境非依存の検証）
	docker compose run --rm dev make test-all
