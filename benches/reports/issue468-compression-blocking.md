# イシュー #468: 巨大応答の gzip 圧縮を spawn_blocking へ切り離す — 実測レポート

対象: `crates/plugin-compression`（`apply_compression` の `plan_compression` /
`compress_body` / `attach_compressed` 分割）+ `crates/core`（`finalize_response`
の `spawn_blocking` オフロード）。

実行コマンド: `bash benches/compression-blocking-bench.sh`
（内部で `cargo run --release --example compression_blocking_micro_bench
-p fandhe-backend-core --features compression` を実行）。

実行環境: 本 worktree の開発コンテナ（release ビルド、`benches/README.md` の
「複数回計測・中央値評価」規約に合わせ各 body サイズ 100 回・ディスパッチ
コストは 1000 回計測して中央値を採用）。2 回の再実行でいずれもほぼ同一の
傾向が得られた（下表は 2 回目の実測値）。

## 1. `compress_body`（gzip 圧縮本体）の所要時間

| body サイズ | 中央値（µs） | ディスパッチコスト比（÷ 13.1µs） |
|---|---|---|
| 1024 バイト | 64.8 | 約 4.9 倍 |
| 4096 バイト | 67.8 | 約 5.2 倍 |
| 16384 バイト | 82.5 | 約 6.3 倍 |
| 32768 バイト | 99.5 | 約 7.6 倍 |
| **65536 バイト（採用しきい値）** | **133.8** | **約 10.2 倍** |
| 131072 バイト | 201.1 | 約 15.4 倍 |
| 262144 バイト | 370.7 | 約 28.3 倍 |
| 1048576 バイト | 1088.5 | 約 83.1 倍 |

## 2. `spawn_blocking` ディスパッチ往復コスト（no-op クロージャ）

中央値 13.1µs（1000 回計測、環境ノイズにより実行毎に 9〜13µs 程度の幅が
観測された）。

## 3. しきい値（`blocking_threshold`）の決定

実装計画（4 節）の基準「圧縮時間がディスパッチコストの約 10 倍以上になる
最小サイズ」を採る。上表より 65536 バイト（64 KiB）付近でこの基準（約
10.2 倍）を満たし、それ未満（32 KiB で約 7.6 倍）はディスパッチオーバー
ヘッドの相対的な比重が増える。既定 `blocking_threshold` を **64 KiB** と
決定した（`CompressionConfigBuilder::blocking_threshold` の既定値・
`crates/plugin-compression/src/lib.rs` の `DEFAULT_BLOCKING_THRESHOLD`）。

- 64 KiB 未満はインライン実行を維持する（ディスパッチオーバーヘッドが
  相対的に大きく、`spawn_blocking` へ切り離す利益が薄い）。
- 64 KiB 以上は `spawn_blocking` へ切り離す。圧縮時間が長いほど接続タスクの
  tokio ワーカスレッド占有時間も長くなり、オフロードの効果（他タスクの
  テールレイテンシ保護）が実行時間に対するディスパッチコストの比重に反比例
  して大きくなる。
- 利用者が自身のワークロード特性に応じて `blocking_threshold` を調整
  できるよう、構築時 API（`CompressionConfigBuilder::blocking_threshold`）
  として公開する（受け入れ基準「しきい値設定 API」）。

## 4. ストリーミング（チャンク単位）圧縮への適用要否

`Handler::handle_streaming` の chunked ストリーミング応答は
`StreamingGzipEncoder::encode_chunk` を bounded mpsc（容量既定 4〜8）経由の
チャンクごとに呼ぶ設計であり、1 チャンクのサイズはハンドラ実装依存だが
典型的には SSE・NDJSON 等の逐次送出パターンで数十〜数百バイト〜数 KiB
程度に収まる（`crates/plugin-compression/src/lib.rs` crate doc「チャンク
単位のストリーミング gzip 圧縮」節、逐次配信の意味論を優先する設計）。

上表の計測から、圧縮時間がディスパッチコスト（13.1µs）の 10 倍
（採用しきい値 64 KiB 相当）に達するのは単発の body としては大きな部類
であり、典型的なストリーミング 1 チャンクのサイズでは圧縮時間がディス
パッチコストを下回る、または同程度にとどまるケースが多いと見込まれる
（1 KiB 相当の圧縮時間は約 65µs だが、これは 1024 バイトの `body` 全体を
1 回で圧縮した場合の測定値であり、`encode_chunk` は増分エンコード
（既存バッファへの `write_all` + flush のみ）のため実際のチャンク単位
コストはこれよりさらに小さい）。

**結論: ストリーミング圧縮への `spawn_blocking` オフロードは不採用と
する。** 理由:

1. チャンクは典型的に小さく、`spawn_blocking` 1 回のディスパッチコスト
   （13.1µs）が増分圧縮コストを上回りやすい。
2. チャンクごとに `spawn_blocking` を挟むと、`write_streaming_response`
   の「recv → 圧縮変換 → chunked framing → write」ループの逐次性・
   バックプレッシャ設計（`crates/core/src/streaming.rs` モジュール doc）
   に対して、エンコーダの所有権をチャンクごとに move/return する複雑性
   （`(encoder, result)` の往復）が増える一方、上記の実測から得られる
   レイテンシ改善効果は限定的と見込まれる。
3. 巨大な単一チャンクを送出するハンドラ実装（本来 body 全体を一括圧縮
   する `apply_compression` 経路を使うべきケース）は、ストリーミング API
   ではなく通常応答（今回オフロード対応済み）を使うことを推奨する運用で
   代替可能（`docs/guide/streaming.md` へ案内を追記）。

将来、大きなチャンクを送出するストリーミングワークロードが実運用で
問題になった場合は、本レポートを起点に再検討する（不採用は恒久的な
決定ではなく、実測ベースの現時点の判断）。

## 5. E2E レイテンシ改善（テールレイテンシ保護）の位置づけ

本イシューの受け入れ基準は「採否と根拠（実測データ）の記録」であり、
採用（64 KiB 以上のオフロード）を選んだ場合の E2E テールレイテンシ改善
効果自体は、上記マイクロベンチのみで定性的に裏付けられる（大きな
圧縮処理を接続タスクから切り離すことで、同一 tokio ワーカ上の他リクエスト
処理がブロックされなくなる）。`oha` 等による並行負荷下の E2E p99 比較は
本レポートのスコープ外とし、性能退行監視の枠組み（`benches/
bench-accept-exclusive.sh` の週次実行、`.claude/rules/ci.md`）で継続的に
監視する運用とする（新規の専用 E2E ベンチ追加は本イシューのスコープ外、
`.claude/rules/out-of-scope-tracking.md` に従い必要であれば別イシューで
追跡する）。

## 6. 再現手順

```bash
bash benches/compression-blocking-bench.sh
```

`crates/core/examples/compression_blocking_micro_bench.rs` を編集した
場合は本ファイルの数値を再計測・更新すること。
