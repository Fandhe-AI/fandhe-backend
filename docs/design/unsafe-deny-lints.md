# 危険な unsafe パターンの deny lint 設定（TASK-14.2、#40、REQ-14）

## 対応する仕様

- `docs/spec/04-requirements.md` REQ-14「AI 改修の検証ゲート」
- `docs/spec/05-tasks.md` TASK-14.2「危険 unsafe パターンの deny lint 設定」
- `docs/spec/03-poc/ai-first-maintainability/README.md`（PoC-9）: `reserve` 直後の
  `set_len` による未初期化領域露出（`clippy::uninit_vec` 相当）を、AI の判断を待たず
  ビルド段階で機械的にブロックする多層防御の必要性を実測で示した

## 課題

TASK-11.2-2（#76）までの時点で、ルート `Cargo.toml` の `[workspace.lints.rust]` には
`unsafe_code = "warn"` があるが、`[workspace.lints.clippy]` は未定義だった。危険な
`unsafe` パターンの検出は clippy のデフォルト重大度（correctness 系は既定で deny）と
CI の `cargo clippy -- -D warnings` に暗黙依存しており、次の抜け穴があった。

- AI（または人間）がコード側に `#[allow(clippy::uninit_vec)]` を書けば、CI の
  `-D warnings` を素通りできる。PoC-9 は「`#[allow]` で黙らせるべきではない」と
  明記しており、単一層（CI コマンドラインの `-D warnings`）だけでは防げない。
- `.claude/rules/coding-rust.md` が要求する「`unsafe` には `// SAFETY:` コメント必須」も
  規約止まりで、機械強制されていなかった。

## 設計: 2 層の lint テーブル

`Cargo.toml`（workspace ルート）の `[workspace.lints.clippy]` に、役割の異なる 2 層で
lint を設定する。全クレートは既に `[lints] workspace = true` で継承済みのため、
本タスクは workspace ルートの変更のみで全クレートに反映される。

### 第 1 層: `forbid`（`#[allow]` による抑制自体を禁止）

メモリ安全性を直接壊す correctness 系 lint。`forbid` は `#[allow(...)]` によるコード側
での抑制をコンパイルエラー（`E0453: allow(...) incompatible with previous forbid`）に
するため、PoC-9 が指摘した「AI が `#[allow]` を書いて CI を素通りする」経路を封じる。
CI の `-D warnings`（コマンドラインの一時的な重大度指定）にのみ依存せず、コードに
埋め込まれた lint テーブル自体が抑制不可能な形で防御することが目的。

```toml
uninit_vec = "forbid"                  # PoC-9 実測の代表パターン（reserve 直後の set_len）
uninit_assumed_init = "forbid"         # MaybeUninit::uninit().assume_init() の誤用
mem_replace_with_uninit = "forbid"     # mem::replace に未初期化値を渡す
transmuting_null = "forbid"            # NULL ポインタを参照へ transmute
wrong_transmute = "forbid"             # 型サイズ不一致の transmute
unsound_collection_transmute = "forbid" # Vec<T> の要素型を安全でない形で transmute
eager_transmute = "forbid"             # 条件分岐前の transmute（無効値を経由しうる）
cast_slice_different_sizes = "forbid"  # スライスの要素サイズを無視した cast
zst_offset = "forbid"                  # ゼロサイズ型への offset 計算（未定義動作）
out_of_bounds_indexing = "forbid"      # コンパイル時に境界外と判定できる添字アクセス
not_unsafe_ptr_arg_deref = "forbid"    # 安全な fn がポインタ引数を無条件に逆参照
```

### 第 2 層: `deny`（正当理由があれば局所 `#[allow]` + レビューで例外化可能）

`unsafe` の記述規律を強制する restriction 系 lint。forbid 層と異なり `#[allow(...)]`
自体は許容するため、正当な理由がある個別ブロックはレビューを経て例外化できる
（restriction 系は誤検知や過剰検出があり得るため、forbid ほど強い制約は課さない）。

```toml
undocumented_unsafe_blocks = "deny"    # unsafe ブロックへの // SAFETY: コメント必須
                                        # （coding-rust.md の機械強制）
unnecessary_safety_comment = "deny"    # SAFETY コメントの陳腐化・誤用防止
multiple_unsafe_ops_per_block = "deny" # 1 unsafe ブロックにつき危険操作を 1 つに限定
```

### rust 側

```toml
[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"        # unsafe fn 内でも unsafe ブロック明示を要求
```

edition 2024 では既定で deny だが、意図を明示するため workspace lints 表に記載する。

## 選定・除外の根拠

- **lint 名の実在確認**: 実装時点のピン留めツールチェーン（`rust-toolchain.toml`:
  `channel = "stable"`、`rustc 1.96.0` / `clippy 0.1.96`）で
  `cargo clippy --workspace --all-targets --all-features -- -D warnings` を実行し、
  `unknown_lints` / `renamed_and_removed_lints` 警告が出ないことを確認済み（0 件）。
- **`transmute_null_to_fn` を除外**: 実装時点の clippy に存在しない、または改名・統合済みの
  lint であるため設定に含めていない。個別 lint 指定のみでグループ指定はしていないため、
  存在しない lint 名を書くと `unknown_lints` として CI が fail-closed で検知する
  （= 改名・削除の追随漏れは黙って見逃されず、必ず CI が赤くなる設計）。
- **`priority` キーは不要**: 個別 lint 名のみを指定しており、lint グループ
  （`clippy::all` 等）とのオーバーライドが発生しないため。
- **既存の `unsafe_code = "warn"` / `missing_docs = "warn"` は変更しない**:
  TASK-11.2-2 で意図的に warn 維持と判断済みの既存決定であり、本タスクのスコープ外。
- **`clippy.toml` は追加しない**: 現状ワークスペース内の自コード `unsafe` は 0 件で
  デフォルト設定のまま `undocumented_unsafe_blocks` が機能するため。将来 SAFETY コメント
  位置の許容設定（`allow-mixed-uninlined-format-args` 相当の類）が必要になった時点で導入する。

## ビルドへの非影響

`[lints.clippy]` はツール（clippy）専用の lint テーブルであり、通常の `cargo build` /
`cargo test`（rustc 単体実行）では無視される。本タスクの変更は clippy 実行時にのみ
効果を持ち、通常ビルド・実行時性能・バイナリサイズには影響しない
（`cargo build --workspace --all-features` で警告・エラーなしを確認済み）。

## ネガティブ検証（実施記録）

受け入れ条件「危険な unsafe パターンが機械的にブロックされる」ことを、`crates/http/src/lib.rs`
末尾に一時的に PoC-9 の模擬パターンを注入して実証した（検証後に revert 済み、コミットには
含まれない）。

1. **uninit_vec パターンの注入**:
   ```rust
   fn __neg_test_uninit_vec(n: usize) -> Vec<u8> {
       let mut v: Vec<u8> = Vec::with_capacity(n);
       unsafe {
           v.reserve(n);
           v.set_len(n);
       }
       v
   }
   ```
   `cargo clippy -p fandhe-backend-http -- -D warnings` → `clippy::uninit_vec` および
   `clippy::undocumented_unsafe_blocks` の 2 件でエラー（期待どおり）。

2. **`#[allow]` による抑制の試行**: 上記関数に
   `#[allow(dead_code, clippy::uninit_vec, clippy::undocumented_unsafe_blocks)]` を付与して
   再実行 → `error[E0453]: allow(clippy::uninit_vec) incompatible with previous forbid` で
   **それでもエラー**（forbid 層が `#[allow]` による抑制を許さないことを実証）。
   `undocumented_unsafe_blocks` は deny 層のため `#[allow]` で抑制でき、この lint のみは
   通過した（設計どおりの挙動: deny 層は局所例外を許容する）。

3. **注入コードを revert** し、クリーンツリーで
   `cargo clippy --workspace --all-targets --all-features -- -D warnings` が全通過することを
   再確認（本タスクの変更のみでは既存コードに新規エラーが出ないことの確認）。

## TASK-14.3 との責務分界

本タスク（TASK-14.2）は「危険パターンの機械的検出」までを担う。次を含まない。

- 受け入れテストのスクリプト化・自動実行記録 → TASK-14.3（#41）で
  `scripts/tests/run-review-gate-tests.sh` として実装済み（`docs/design/review-gate.md`）
- 自律実装のマージ条件としてのレビューゲート運用定義 → TASK-14.3（#41）、
  `docs/design/review-gate.md` で定義済み
- `cargo geiger` による unsafe 件数計測の CI 常設化（TASK-15 系、`docs/dep-impact/` 運用）

## lint 追加・改名時の運用

- 新しい危険パターンを追加検出したい場合は、forbid（抑制不可）にすべきか deny
  （局所例外可）にすべきかを判断した上で、上記いずれかのテーブルに 1 行追加する。
- clippy のバージョンアップで lint が改名・削除された場合、`unknown_lints` /
  `renamed_and_removed_lints` として CI の `clippy -- -D warnings` が fail-closed で検知する
  （`rust-toolchain.toml` が stable の浮動 channel であるため、ツールチェーン更新時に
  自動的に検証される）。検知した場合は改名先へ差し替えるか、rustc 側の deny-by-default で
  既に担保されていることを確認した上でリストから除外し、本ファイルに理由を追記する。

## 受け入れ検証レポート

受け入れ検証レポート: `docs/acceptance/req14-verification-gate.md`（#264）。
REQ-14 受け入れ基準 3 項目のうち基準 2（危険 `unsafe` パターンの機械的検出）の証跡として
本文書（2 層 lint テーブル・ネガティブ検証）を集約転記している。
