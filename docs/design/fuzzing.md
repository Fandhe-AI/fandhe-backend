# fuzz 実行環境（nightly / 代替 fuzzer 整備）

TASK-15.3-1（#87、docs/spec/05-tasks.md TASK-15.3、Conditional Go 条件(4)）対応。
親イシュー #51（TASK-15.3）は「fuzz 実行環境の整備」（本イシュー #87）と「fuzz スクリーニング
本実行・検出欠陥の修正」（#88、TASK-15.3-2）に分割されている。本ドキュメントは環境整備側の
設計判断を記録する。

## 背景

`crates/http/src/request.rs` の doc コメントが明記するとおり、`parse_request_head` は
sans-IO な純関数（`&[u8] -> Result<ParseOutcome, ParseError>`）として設計されており、
「そのまま fuzz に供せる」ことを最初から意図している。PoC 環境が stable のみだったため、
nightly 限定のサニタイザ計装を要する `cargo-fuzz` を未実施のまま据え置かれていた。

## fuzzer 選定: cargo-fuzz（libFuzzer）を採用、afl.rs は不採用

- fuzz 対象が `&[u8] -> Result` の純関数であり、libFuzzer ハーネス（`fuzz_target!`）に
  最も自然に載る。ソケット I/O・状態を持たないため、永続プロセス型（afl.rs の
  fork server）による恩恵が薄い
- ASan（AddressSanitizer）併用によるメモリ不正検出が `cargo fuzz run` の既定で得られる
- `.claude/agents/testing/fuzz-runner.md` が「cargo-fuzz（または afl.rs）・nightly 前提」
  と定義しており、cargo-fuzz が第一候補
- afl.rs へのフォールバック判断点: self-hosted runner（Linux）で nightly + サニタイザ
  計装のビルドが通らない場合（C++ ツールチェーン欠如等）のみ検討する。
  `scripts/fuzz.sh` は C コンパイラ（`cc` / `clang`）の存在検査で早期に検知し、
  見つからない場合は afl.rs 検討を促すメッセージを表示してフェイルクローズする

## nightly バージョン pin

- `rust-toolchain.toml` はリポジトリ既定（stable）のまま変更しない
  （`.claude/rules/coding-rust.md` 「fuzz / サニタイザは nightly を明示的に使う」）
- `scripts/fuzz.sh` の `PINNED_NIGHTLY` 定数を単一真実源とし、CI（`ci.yml` の
  `fuzz-smoke` ジョブ）はこの定数をスクリプトから読み取って
  `rustup toolchain install` する（ci.yml 側に日付を重複記載しない）
- 検証済み組み合わせ: `nightly-2026-07-15` + `cargo-fuzz 0.13.2`（実装時点で
  `cargo search cargo-fuzz` が返した最新版）

## fuzz target 一覧と対象 API

`crates/http/fuzz/fuzz_targets/` に 2 本配置する。

| target | 対象 API | 検証範囲 |
|--------|---------|---------|
| `parse_request_head` | `bf_http::request::parse_request_head(&[u8])` | 構文解析層単体。任意バイト列を直接投入し、パニック・メモリ不正が起きないことを検証する |
| `head_semantics` | `parse_request_head` → `Complete` の場合のみ `bf_http::body::body_length` / `bf_http::connection::should_keep_alive` | 意味解釈層のパイプライン。構文的に妥当なヘッダ列を前提に動く `Content-Length` 解析・`Connection` トークン走査のパニック要因（オーバーフロー等）を検証する |

いずれも戻り値の Ok/Err・Complete/Incomplete の意味的正しさは検証しない（それは
`crates/http/src/*.rs` の `#[cfg(test)]`・doc test の責務）。fuzz target は「パニックしない
こと」「メモリ不正を起こさないこと」のみを libFuzzer に判定させる。

`Transfer-Encoding`（chunked）は本マイルストーンで一律拒否されるため fuzz target の対象
としない。chunked 対応後に別 target を追加検討する（out-of-scope、下記参照）。

## pay-for-what-you-use の担保

`crates/http/fuzz/` は root `Cargo.toml` の `[workspace] exclude` に加え、`libfuzzer-sys`
依存を持つ独立クレートとして構成する。これにより:

- `cargo build` / `cargo tree`（workspace ルートでの実行）に `crates/http/fuzz` も
  `libfuzzer-sys` も一切現れない
- `crates/http/fuzz/Cargo.toml` は `bf-http` を `path = ".."` 依存として個別参照する
  独立ビルド単位であり、`cargo +<pinned-nightly> fuzz run <target>`（`scripts/fuzz.sh`
  経由）でのみビルド・実行される

## smoke（CI 常設）と本実行（#88）の 2 段構え

- **smoke**: `ci.yml` の `fuzz-smoke` ジョブが PR/push のたびに各 target を
  `-max_total_time=60`（秒）で実行し、パーサへの回帰（パニック・メモリ不正の再混入）を
  短時間で検知する。`ci-complete` の判定対象に含める
- **本実行**: 長時間（分〜時間オーダー）のスクリーニングによる未知の欠陥検出は #88
  （TASK-15.3-2）のスコープ。`bash scripts/fuzz.sh --max-total-time <長い秒数>` で
  ローカル/専用ジョブから実行する想定

## corpus・artifacts の取り扱い

- **corpus**（`crates/http/fuzz/corpus/<target>/`）: 既存テスト（`crates/http/tests/http_flow.rs`・
  `crates/http/src/*.rs` の `#[cfg(test)]`・doc test）由来の正常系・異常系リクエストを
  シードとしてコミットする。機密は含まない
  - 注意: `cargo fuzz run` はカバレッジ増加した新規入力を実行のたびに `corpus/<target>/`
    へ追記する（libFuzzer の標準動作）。ローカルで `scripts/fuzz.sh` を実行した後は、
    コミット前に `git status` で corpus ディレクトリの意図しない肥大化がないか確認し、
    キュレーションされたシードのみを残すこと
- **artifacts**（`crates/http/fuzz/artifacts/<target>/`）: クラッシュ再現入力。実行の
  たびに生成される一時生成物であり `.gitignore` でコミット対象外とする
- **coverage**（`crates/http/fuzz/coverage/`）: カバレッジ計測生成物。同様に
  `.gitignore` でコミット対象外とする

## フェイルクローズ検証

`scripts/fuzz.sh` は crash/hang を検知すると非 0 終了し、artifacts のパスを表示する。
実装時に一時的な `panic!` を仕込んだ target で本挙動を確認済み（確認後に破棄、
コミットには含まれない）。

## #88（TASK-15.3-2）fuzz 本実行結果

TASK-15.3-2（#88）で `bash scripts/fuzz.sh --max-total-time 240` により両 target
（`parse_request_head`・`head_semantics`）を実行した。

- 実行結果: いずれも crash/hang を検出せず正常終了（`parse_request_head` 約 4,600 万
  実行、`head_semantics` 約 4,700 万実行、各 240 秒）。パーサ自体（`crates/http/src/*.rs`）
  に対する欠陥は検出されなかった
- 検出・修正した欠陥（ビルド基盤側）: `crates/http/fuzz/Cargo.toml` に空 `[workspace]`
  テーブルがなく、cargo がディレクトリツリーを上方に辿って祖先の `Cargo.toml` を
  workspace root とみなそうとする挙動により、本体 workspace root から見て
  `crates/http/fuzz` がさらに深い階層に置かれる環境（例: nested git worktree）で
  `cargo +<nightly> fuzz run` がビルドエラーになっていた。空 `[workspace]` テーブルを
  追加し、本クレートを独立 workspace root として固定することで解消した（本体
  `cargo build` / `cargo tree` からの不可視性・`crates/http/fuzz` の exclude 設定には
  影響しない）
- corpus の肥大化防止: 実行のたびに libFuzzer が追記する `corpus/<target>/` の新規
  入力は、キュレーションされたシードのみを残す既存方針（本書「corpus・artifacts の
  取り扱い」節）に従い、コミット前に非追跡分を削除した

## TASK-15.3 / REQ 対応

- 出典: `docs/spec/05-tasks.md` TASK-15.3【Conditional Go 条件(4)】
- 安全性方針: `.claude/rules/security.md`「パーサは fuzz-runner でファジング検証する」を
  充足する基盤
- pay-for-what-you-use: `.claude/rules/pay-for-what-you-use.md`（fuzz 専用依存の
  workspace 除外）

## スコープ外（out-of-scope-tracking）

- chunked Transfer-Encoding 対応後の fuzz target 追加（現状は一律拒否のため対象外）
- corpus の永続化・自動最小化（`cargo fuzz cmin`）・カバレッジ計測の CI 化、
  OSS-Fuzz 等の外部基盤連携（必要になれば #88 レビュー時に Issue 化を検討）
