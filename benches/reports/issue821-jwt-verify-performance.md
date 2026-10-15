# plugin-jwt の RS256・ES256 検証コストとキャッシュ効果の計測（イシュー #821）

## 1. 背景

- 仕様の要望整理（`requests/fandhe-auth.md`）に「検証だけを見ると ES256 は RS256 より遅く、その差は
  署名の検証結果のキャッシュで吸収する」という注記がある。本計測はこれを実測で確かめる。
- 旧計測 `benches/reports/task-9.3-*`（hub ラッパー越し・RS256 のみ）は歴史的記録として残し、改変しない。
  計測方式は `docs/design/jwt-verify-plugin.md` 2.6 節に従い `fandhe-backend-plugin-jwt` へ移した。

## 2. 計測方法

### 2.1 プロセス内マイクロベンチ（受け入れ条件 1）

`crates/plugin-jwt/examples/jwt_verify_bench.rs`。RS256・ES256 それぞれ、同一トークンに対し
20,000 ops x 7 試行を行い、試行ごとの ns/op の中央値を採る。3 回実行した（各回の代表値は 7 試行の中央値）。

| 記号 | シナリオ |
|---|---|
| A | `verify` を直接呼ぶ（毎回署名検証） |
| B | `JwtAuthenticator::authenticate`（キャッシュヒットのみ。`cache_hits == ops`、`cache_misses == 1` を assert） |
| C | `JwtGate::check`（実ゲート経路のヒット。毎回 `Allow` を assert） |

### 2.2 HTTP 負荷での NFR-6 計測（受け入れ条件 2）

`benches/jwt-nfr6-bench.sh` と `crates/plugin-jwt/examples/jwt_gate_server.rs`。

- `JwtGate::check` は経路を問わず全リクエストに介入し、トークンが無ければ 401 を返す。ヘッダ無しの
  `GET /` を叩くと baseline の 200 とゲートの 401 という異なる応答形状を比べてしまうため、サーバが
  起動時に作る有効トークンを全系統（baseline を含む）に同じ `Authorization: Bearer` として付ける。
- 系統は (a) baseline（`minimal`）、(b) リンクのみ（ゲート未登録）、(c) ゲート有効 + 有効トークン
  （ES256 が主判定、RS256 も 1 系列）。
- 計測前に、ゲート有効でトークン付き 200・ヘッダ無し 401、ゲート無効と baseline でトークン付き 200 を確認し、
  外れれば中止する。
- 判定は `evaluate_nfr6_ratio`。NFR-6 の狭義帯は「一致したときだけ介入する拡張点」向けで、全パス介入の
  ゲートは NFR-7（標準ミドルウェア有効時の劣化 5% 以内）に近い。このため合否は実務許容帯
  （95〜105%）で見て、狭義帯外は WARN として記録する。

## 3. 実行環境

| 項目 | 値 |
|---|---|
| 日時 | 2026-10-15 |
| 対象コミット | `e4e1740`（origin/main）+ 本変更（example・スクリプトのみ追加。`src/**` は無変更） |
| CPU | 13th Gen Intel Core i7-13700K |
| カーネル | Linux 7.0.0-34-generic |
| rustc / cargo | 1.99.0 / 1.99.0 |
| ビルド | `--release` |
| 負荷（`/proc/loadavg` 1 分） | 計測前後とも 1.1 前後。専有環境ではない（他の並列ワークフローのノイズの可能性あり） |
| oha | 未導入（4 節参照） |

## 4. 結果

### 4.1 マイクロベンチ（中央値、ns/op）

| アルゴリズム | A 検証 | B キャッシュヒット | C ゲート経由ヒット | A/B |
|---|---|---|---|---|
| RS256 | 14,021 / 14,011 / 14,090 | 322.3 / 317.4 / 326.5 | 322.2 / 315.9 / 325.4 | 約 43.2〜44.1x |
| ES256 | 38,471 / 38,315 / 38,531 | 218.9 / 218.5 / 227.1 | 218.0 / 222.0 / 227.5 | 約 169.7〜175.7x |

（各セルは 3 回分の実行の並び。代表値は 3 回の中央値とし、RS256 は A=14,021・B=322.3、
ES256 は A=38,471・B=218.9 とする。）

- ES256/RS256 比は、検証（A）で約 2.73、キャッシュヒット（B）で約 0.69。
- 試行ごとの生値は `jwt_verify_bench` の標準出力（`per trial` 行）に出る。

### 4.2 NFR-6（HTTP 負荷）

**未計測。** 本計測環境に `oha` が無く、`benches/lib/common.sh` の方針（サプライチェーン考慮・自動取得しない）に
従って導入しなかった。実施できたのは次の範囲に限る。

- スクリプトの前提検査が `oha` 不在を検知して非 0 で終了すること。
- 偽の `oha`（固定 JSON を返すスタブ）でスクリプト全体が最後まで通り、機械可読な出力が得られること。
- 実サーバでの正常性確認（ゲート有効 + 有効トークンで 200、ヘッダ無しで 401、ゲート無効と baseline で 200）。
  これは本物の `jwt_gate_server`・`minimal` に対して通った。

スタブの値は性能値ではないため、本レポートには載せない。

## 5. 考察

- 注記の前半は正しい。ES256 の署名検証は RS256 の約 2.7 倍遅い（RSA は公開鍵の指数が小さく検証が軽い）。
- 後半も成立する。キャッシュヒット時は署名検証を行わないため、ES256 のヒットは約 220 ns と、検証の約 1/175 になる。
  ヒット側は RS256 のほうが遅い（約 320 ns）。ヒットの主要コストはトークン全体の SHA-256 で、RS256 のトークンは
  署名が 256 バイトと長く、ES256（64 バイト）より入力が大きいため。ES256 が既定の `fandhe-auth` では有利な方向に働く。
- ゲート経由のヒット（C）は認証器単体（B）とほぼ同じで、`JwtGate::check` 自体の追加コストは測定誤差内。
- 1 リクエストあたりの予算が数 µs の最小サーバでは、ヒット約 0.2〜0.3 µs は無視できない割合になりうる。
  この影響の有無は 4.2 の HTTP 計測でしか確かめられず、現時点では未確認。

## 6. 受け入れ条件との対応

| 条件 | 状態 |
|---|---|
| 1. RS256・ES256 の署名検証とキャッシュヒットの ns/op を複数試行の中央値で測り、レポートに置く | 達成（4.1） |
| 2. ゲート有効構成で無関係パスへの影響が NFR-6 の範囲に収まるか確かめる | **未達（未計測）**。スクリプトと正常性確認は整備済みだが、`oha` が無く数値は無い。人手での計測が必要 |
| 3. 計測環境と再現手順を記録する | 達成（3 節・7 節）。NFR 計測時は `oha --version` と専有・非専有の別も追記すること |

## 7. 再現手順

```bash
# ビルド
cargo build --release -p fandhe-backend-plugin-jwt --example jwt_verify_bench --example jwt_gate_server
cargo build --release -p fandhe-backend-core --example minimal --no-default-features

# マイクロベンチ（受け入れ条件 1）。FANDHE_BACKEND_JWT_VERIFY_BENCH_OPS / _TRIALS で調整可
cargo run --release -p fandhe-backend-plugin-jwt --example jwt_verify_bench

# NFR-6（受け入れ条件 2。oha・jq・curl が必要）
bash benches/jwt-nfr6-bench.sh
# 専有枠（flock・静穏確認付き）
TARGETS=jwt bash benches/nfr6-exclusive.sh
```

`nfr6-exclusive.sh` が BLOCKED（exit 2）を返したら、その事実を記録したうえで直接実行し、
「非専有環境での参考値」と明記する。PASS を得るまで再試行を繰り返さない。

## 8. スコープ外と改善候補（起票は承認後）

- NFR-6 が実務許容帯を外れた場合の候補:
  - `JwtGate::check` は Allow かどうかだけを要るのに、`authenticate` が `Claims` を clone している。
    ゲート専用の軽いヒット経路を設ける。
  - キャッシュキーの SHA-256 計算コストの見直し。
- ライブラリ本体の最適化は本イシューの範囲外で、計測だけに留めた。
