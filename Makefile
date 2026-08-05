# 開発タスクの入口となる Makefile。CI（.github/workflows/ci.yml）の主要ジョブと同一
# コマンドをローカルで再現し、`make setup` 一発で開発環境（submodule・git hooks）を
# 構築できるようにする。Docker 経由の環境非依存な開発は docker-* ターゲットを使う
# （Dockerfile / compose.yaml）。

.DEFAULT_GOAL := help

.PHONY: help setup hooks build build-all test test-all fmt fmt-check clippy lint audit doc \
	webrtc-e2e docker-build docker-shell docker-test

help: ## ターゲット一覧を表示する
	@grep -E '^[a-z][a-z-]*:.*##' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-14s %s\n", $$1, $$2}'

setup: hooks ## 開発環境を構築する（submodule 取得 + git hooks 配線）
	git submodule update --init

hooks: ## lefthook で git hooks（pre-commit / commit-msg）を配線する
	@if command -v lefthook >/dev/null 2>&1; then \
		lefthook install; \
	else \
		npx --yes lefthook@2 install; \
	fi

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
	bash scripts/dep-audit.sh

doc: ## rustdoc を生成する
	cargo doc --workspace --all-features --no-deps

webrtc-e2e: ## RebindHandle::rebind の実接続 force-close e2e テスト（standalone crate、#507）
	bash scripts/webrtc-e2e.sh

docker-build: ## 開発用 Docker イメージをビルドする
	docker compose build dev

docker-shell: ## 開発用コンテナのシェルに入る（リポジトリを /work にマウント）
	docker compose run --rm dev

docker-test: ## コンテナ内で test-all を実行する（環境非依存の検証）
	docker compose run --rm dev make test-all
