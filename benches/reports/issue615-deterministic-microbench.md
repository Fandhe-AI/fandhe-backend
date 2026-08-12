# イシュー #615: 決定的マイクロベンチ（alloc カウンタ）導入時の検証記録

方式選定・カデンツ確定の詳細は `docs/design/deterministic-microbench.md` を参照。
本文書は実装時にローカルで実行した検証手順とその結果のみを記録する。

## 環境

- rustc: `rustc 1.96.0 (ac68faa20 2026-05-25)`（stable、`rust-toolchain.toml` 継承）
- ビルド: `cargo build --release --manifest-path benches/microbench/Cargo.toml`
  （`opt-level = 3` + `lto = true`、`benches/microbench/Cargo.toml` 明示指定）

## 1. 初回ベースライン（`benches/microbench/baseline.json`）

`bash benches/microbench.sh --update-baseline` で生成し、コミット対象とした。

| シナリオ | alloc 回数 | alloc バイト数 |
|---------|-----------|---------------|
| `get_health`（`GET /health`） | 5 | 229 |
| `get_hello_name`（`GET /hello/{name}`） | 8 | 484 |
| `get_users_id`（`GET /users/{id}`） | 8 | 476 |
| `post_echo`（`POST /echo`） | 5 | 378 |

## 2. 決定性検証（受け入れ基準: 同一コミットで再現すること）

`bash benches/microbench.sh`（比較なし、計測結果のみ出力）を同一コミットで 2 回連続
実行し、出力 JSON が完全一致することを確認した（`diff` 差分なし）。

```
$ bash benches/microbench.sh > /tmp/run1.json
$ bash benches/microbench.sh > /tmp/run2.json
$ diff /tmp/run1.json /tmp/run2.json && echo IDENTICAL
IDENTICAL
```

加えて、ベンチ内部の反復一致チェック（`measure_scenario`、各シナリオ 10 反復の
完全一致を検証、不一致なら非 0 終了）も全シナリオで通過することを確認済み
（`bash benches/microbench.sh --check` の exit 0 が前提条件チェック通過を含意する）。

## 3. 検知能力検証（受け入れ基準: alloc 増加パッチを検知すること）

`crates/http/src/request.rs` の `parse_request_head` 内、リクエストライン取得直後に
1 個の余分な alloc（`std::hint::black_box(request_line.to_vec())`）を挿入する
一時パッチを適用し、`bash benches/microbench.sh --check` が退行として exit 1 で
検知することを確認した。パッチは検証後に破棄済み（`git status` で `crates/http` に
差分が残っていないことを確認済み）。

```
$ bash benches/microbench.sh --check
エラー: alloc profile regression detected (ratchet violation):
get_health: allocations 5 -> 6 (baseline -> current), bytes 229 -> 249
get_hello_name: allocations 8 -> 9 (baseline -> current), bytes 484 -> 509
get_users_id: allocations 8 -> 9 (baseline -> current), bytes 476 -> 498
post_echo: allocations 5 -> 6 (baseline -> current), bytes 378 -> 397
（exit 1）
```

パッチ除去後、`bash benches/microbench.sh --check` が再び exit 0 で通過することを
確認した（ベースラインとの差分なし）。

## 4. pay-for-what-you-use 確認

- `cargo metadata --no-deps --format-version 1`（root workspace）の出力に
  `fandhe-backend-microbench`・`stats_alloc`・`serde_json`（本イシューでの新規依存）が
  含まれないことを確認した（`benches/microbench` が `[workspace] exclude` されている
  ことによる構造的な保証）
- `cargo fmt --check`・`cargo clippy --release -- -D warnings`（`benches/microbench`
  の manifest-path 指定）がいずれも通過することを確認した
- `cargo test --release --manifest-path benches/microbench/Cargo.toml`
  （8 テスト: シナリオ応答正当性 4 件・JSON 往復整合性 1 件・ラチェット判定 3 件）が
  全件通過することを確認した

## 5. CI 設定検証

- `bash scripts/actionlint.sh` が通過することを確認した
  （`.github/workflows/ci.yml` 追加分に未登録ラベル・構文エラーなし）
- `grep -rhE "^[[:space:]]*runs-on:" .github/workflows/ | awk '{print $2}' | sort -u |
  grep -vxF ubuntu-latest` の出力が空であることを確認した（`.claude/rules/ci.md` の
  ホステッドランナー既定に準拠）
