# TASK-3.3 性能受け入れ計測レポート — OpenAPI 生成有無での `GET /health` 性能有意差

Issue #32 の成果物。REQ-3 受け入れ基準「OpenAPI 生成の有無で `GET /health` 相当の
ランタイム性能指標（RPS・p95）に有意差がない（±5% 以内）」の検証結果。

## 実施日時・環境

- 実施日時: 2026-07-17（JST）
- OS: Linux 7.0.0-27-generic x86_64（Ubuntu）
- CPU コア数: 12（`nproc`）
- rustc / cargo: 1.96.0（stable, 2026-05-25）
- 計測ツール: `oha`（導入済み、`which oha` で確認済み）

## 判定結果: BLOCKED

`crates/core` にサーバ側 `openapi` feature（`openapi = ["dep:bf-plugin-openapi"]`
相当、`GET /openapi.json` 静的サービングハンドラ）が本イシュー着手時点で存在しない
ため、「OpenAPI 生成有効／無効」の 2 構成を A/B 比較する計測そのものが実施不能である。

```
$ cargo metadata --format-version 1 --no-deps | \
    jq -r '.packages[] | select(.name == "backend-framework-core") | .features | keys[]'
webrtc-proxy
websocket
graphql
default
```

`openapi` feature が一覧に存在しない。`crates/plugin-openapi/src/lib.rs`・
`src/embed.rs` の doc comment は当該配線を TASK-2.1（#18）のスコープとして委ねて
いるが、TASK-2.1 は `webrtc-proxy` feature のみを確立してクローズしており、
`openapi` の配線・後継 Issue のいずれも存在しない（詳細な経緯は
`docs/acceptance/req3-openapi-generation.md` を参照）。

**理由**: `openapi` feature が存在しない以上、「OpenAPI 生成有効（`--features
openapi` でビルドし `GET /openapi.json` を実サービングする構成）」を作れず、
比較対象の片方を欠く。加えて `crates/core/examples/minimal.rs` は現状 `GET /` の
みを登録しており、`GET /health` 自体も未登録である（対象パスの計測用エンドポイント
も別途 #32 スコープ内で追加が必要）。両者が揃わない状態で「有意差なし」を実測せずに
PASS と記録することは、`.claude/rules/security.md` のフェイルクローズ原則（判定不能を
PASS と偽らない）に反するため、`task-1.6-1-performance.md`（#71）の前例に倣い
BLOCKED として正直に記録する。

## 再計測手順（`openapi` feature 配線後）

フォローアップ Issue（`docs/acceptance/req3-openapi-generation.md` 「フォローアップ」
節 1）で `crates/core` に `openapi` feature が配線された後、以下の手順で再計測する:

```bash
# baseline: openapi feature 無効
cargo build --release --example minimal -p backend-framework-core --no-default-features
cp target/release/examples/minimal /tmp/minimal-baseline

# 対象: openapi feature 有効
cargo build --release --example minimal -p backend-framework-core --features openapi
cp target/release/examples/minimal /tmp/minimal-openapi

# 各構成で GET /health を RUNS=5 中央値方式で計測し RPS・p95 の相対差を確認する
# （benches/lib/common.sh の中央値算出関数を再利用する想定。ポートは分離する）
RUNS=5 /tmp/minimal-baseline &
oha -z 15s -c 128 --no-tui --output-format json http://127.0.0.1:3000/health

RUNS=5 /tmp/minimal-openapi &
oha -z 15s -c 128 --no-tui --output-format json http://127.0.0.1:3000/health
```

RPS・p95 の相対差が ±5% 以内であることを確認し、本レポートを更新して判定結果を
BLOCKED から PASS/FAIL に更新すること。`OPENAPI_JSON` は `include_str!` によるコンパ
イル時埋め込み定数の参照のみで実行時コストがない設計（`embed.rs` の doc comment）の
ため、`GET /openapi.json` のハンドラが `GET /health` のリクエスト処理経路に一切関与
しない限り有意差はほぼゼロになる見込みだが、実測せずに結論づけない。
