# 開発用コンテナイメージ（compose.yaml から使用）。ホスト環境に依存せず build / test /
# fmt / clippy を再現するための最小構成。rust-toolchain.toml が stable を指すため、
# stable 追従の公式 slim イメージを基底にする（本番配布用イメージではない。公開物は
# crates.io のクレートのみ、docs/design/crates-io-release.md 参照）。
FROM rust:slim

# git: submodule（docs/spec）取得・lefthook が参照 / make: Makefile ターゲット実行 /
# jq: .claude hooks・scripts が使用 / pkg-config, curl: ビルド・スクリプトの汎用前提
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        git make jq curl pkg-config ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# rust-toolchain.toml と同じ components を事前導入し、初回実行時の rustup 取得を省く
RUN rustup component add rustfmt clippy

WORKDIR /work

CMD ["bash"]
