#!/usr/bin/env bash
# イシュー #468: `plugin-compression` の gzip 圧縮 spawn_blocking オフロード
# しきい値決定・ストリーミング適用要否判定のためのマイクロベンチ実行ラッパー。
#
# `crates/core/examples/compression_blocking_micro_bench.rs`（release ビルド）を
# 実行し、`compress_body`（gzip 圧縮本体）の body サイズ別所要時間と
# `tokio::task::spawn_blocking` 1 回のディスパッチ往復コストを比較する
# マークダウン表を標準出力へ書き出す。結果は `benches/reports/
# issue468-compression-blocking.md` へ手動転記し、`docs/design/
# plugin-boundary.md` 5.10.7 節の採否根拠として記録する（受け入れ基準）。
#
# HTTP 負荷生成（oha 等）を使わないマイクロベンチのため、他の `bench-*.sh`
# と異なり `oha`/サーバ起動は不要。`benches/README.md` の「複数回計測・
# 中央値評価」規約は example 内部の `median_micros` ヘルパーで満たす。

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

echo "# イシュー #468 圧縮 spawn_blocking マイクロベンチ" >&2
echo "# 実行日時: $(date -u '+%Y-%m-%dT%H:%M:%SZ')" >&2
echo "# コミット: $(git rev-parse --short HEAD 2>/dev/null || echo 'unknown')" >&2

cargo run --release \
    --example compression_blocking_micro_bench \
    -p fandhe-backend-core \
    --features compression
