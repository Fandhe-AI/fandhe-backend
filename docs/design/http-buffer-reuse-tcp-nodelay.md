# bf-http 読み取りバッファ再利用・TCP_NODELAY 最適化

TASK-1.3-3（#68、`docs/spec/05-tasks.md`）の実装設計。TASK-1.3-2（#67）が確立した
`crates/http` の `read_request` 呼び出し契約（1 コネクションにつき読み取りバッファを
1 つ保持して繰り返し呼ぶ）を維持したまま、memmove・ゼロ埋めコストを削減し、
`TCP_NODELAY` を設定可能にする。

## 背景

`crates/http/src/connection.rs`（#67 時点）は `buf: Vec<u8>` を直接扱っており、
以下のコストがあった。

1. ヘッド消費後・body 読了後の `buf.drain(..consumed)` による先頭詰めコピー
   （パイプライン残余の有無にかかわらず発生）
2. `read_chunk` が `buf.resize(start + READ_CHUNK_BYTES, 0)` で毎回 8 KiB をゼロ埋め
3. `read_body` が `buf[..n].to_vec()` + `drain` で二重コピー
4. 大 body 処理後も keep-alive 接続の容量が無制限に保持される（メモリ滞留）
5. `TCP_NODELAY` の設定箇所がどこにもない

## 設計

### `RecvBuffer`（`crates/http/src/buffer.rs`）

`Vec<u8>` + 読み取りカーソル `pos` を持つ接続単位バッファ型。不変条件は
`pos <= buf.len()`。

- **遅延コンパクション**: `consume(n)` はカーソル前進のみ（`drain` 相当のコピーを
  行わない）。次回読み取り直前（`reserve_for_read`）に、未読領域が残っている場合
  （パイプライン残余）のみ `copy_within` で先頭詰めする。非パイプラインの典型
  ケースでは memmove が発生しない
- **ゼロ埋め回避**: `read_chunk` は `Vec::reserve` + `AsyncReadExt::read_buf` で
  スペア容量へ直接書き込む。`resize` によるゼロ埋めは行わず、`unsafe` も使わない
  （`tokio` の `io-util` が内包する `bytes::BufMut` 実装に委譲）
- **body 抽出の最適化**: 未読領域が body ちょうど（`pos == 0` かつ
  `unread().len() == n`）の典型ケースでは `take_exact` が `mem::take` で
  コピーなしに取り出す。この場合、取り出した `Vec` は呼び出し元（`Request::body`）
  の所有物になり、`RecvBuffer` 側は空の新規 `Vec` に入れ替わる（＝次リクエストで
  容量が再確保される）。パイプライン残余がある部分一致ケースはコピーで対応する。
  「典型ケースの body コピー回避」と「keep-alive の容量再利用」はこの点で
  トレードオフ関係にあり、意図した仕様である（無 body・ヘッドのみの keep-alive
  connection では容量が再利用され続ける。`http_flow.rs` の
  `keep_alive_requests_reuse_buffer_capacity` はこのケースを検証する）
- **容量有界化**: `shrink_if_oversized` はリクエスト処理完了時に呼ばれ、容量が
  `MAX_RETAINED_CAPACITY`（64 KiB = `MAX_HEADER_BYTES` 16 KiB + 余裕）を超えていれば
  消費済みプレフィックスを捨てたうえで `shrink_to` する。大 body
  （最大 `MAX_BODY_BYTES` = 1 MiB）を処理した keep-alive 接続でのメモリ滞留を
  接続単位で有界化する（`.claude/rules/security.md` リソース枯渇対策）

`read_request` / `read_head` / `read_body`（`connection.rs`）は `buf: &mut Vec<u8>`
から `buf: &mut RecvBuffer` へシグネチャを変更した。呼び出し元契約
（同じバッファを次呼び出しへ渡す・パイプライン残余は保持される）は不変。

### `TCP_NODELAY`（`crates/http/src/socket.rs`、feature `net`）

`crates/http` は pay-for-what-you-use のため既定で tokio `net` を持たない。
オプション feature `net`（`net = ["tokio/net"]`、既定 off）を追加し、
`#[cfg(feature = "net")] pub mod socket;` で `configure_stream(&TcpStream)` を提供する。
`set_nodelay(true)` を設定し、失敗は `io::Error` として呼び出し元へ伝播する
（握りつぶさない、`.claude/rules/security.md` フェイルセーフ）。

feature 無効時は `mio`/`socket2`/`libc` が `cargo tree -p bf-http` に一切出ない
（`--features net` を付けたときのみ増える）。

## 未実施（TASK-1.4 / #70 への引き継ぎ）

実装着手時点（2026-07-17）で `crates/core/src/server.rs` に接続受理ループ
（TASK-1.4-2、#70）はまだ実装されていない（`crates/core/src` は
`extension.rs` / `lib.rs` のみ）。そのため本タスクは `crates/http` 側の提供
（`RecvBuffer` + `socket::configure_stream`）と単体・統合テストまでを範囲とし、
コア側の配線は #70 の実装側が引き継ぐ。

#70 の accept ループ実装時に必要な変更（この設計ドキュメントを参照して実施する）:

1. 接続 accept 直後、feature `net` を有効化した `bf-http` を使い
   `bf_http::socket::configure_stream(&stream)` を呼ぶ。エラー時は当該接続のみ
   クローズし、accept ループ全体は継続する
2. 1 コネクションにつき `bf_http::buffer::RecvBuffer::new()` を 1 つ保持し、
   `read_request` へ繰り返し渡す（旧 `Vec::new()` からの置き換え）
3. `crates/core/Cargo.toml` に `bf-http = { path = "../http", features = ["net"] }`
   を追加する

## 検証

- `cargo tree -p bf-http` / `cargo tree -p bf-http --features net`:
  デフォルト構成で `mio`/`socket2`/`libc` が出ないこと、`--features net` でのみ
  増えることを確認済み
- `cargo nextest run --workspace --all-features --profile ci` /
  `cargo test --doc --workspace --all-features`: 全通過
- `cargo test -p bf-http`（default feature、`net` 除外構成）: 全通過
- `cargo fmt --all --check` / `cargo clippy --workspace --all-targets --all-features -- -D warnings`:
  警告なし
- `cargo audit` / `cargo deny check`: 問題なし
- `scripts/unsafe-triage.sh`: baseline から変化なし（`unsafe` 追加なし）
