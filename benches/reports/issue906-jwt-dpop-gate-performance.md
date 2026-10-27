# DPoP を有効にした JwtGate の検証コストの計測（イシュー #906）

## 1. 背景

- REQ-16 の追加要件 `dpop` の受け入れ基準に「DPoP を有効にしたゲートの性能への影響を計測し、記録する
  （NFR-6）」がある。設計 `docs/design/jwt-verify-plugin.md` 9.11 節の対応表はこれを #906 に割り当てている。
- DPoP を有効にしたゲートは 1 要求ごとに、(1) アクセストークンの検証（既存のキャッシュに当たる）、
  (2) proof の署名検証（要求ごとに毎回。キャッシュしない、設計 9.1）、(3) `jti` の `ReplayStore::register`
  を行う。
- 方式は #821（`benches/reports/issue821-jwt-verify-performance.md`）に揃える。#821 は別の CPU
  （i7-13700K）で測った値なので、本レポートの数値とは直接比べない。DPoP 無効側の基準（D0）は同じ実行の中で
  測り直した。

## 2. 計測方法

### 2.1 プロセス内マイクロベンチ

`crates/plugin-jwt/examples/jwt_dpop_bench.rs`（`dpop` feature 必須）。

`jti` は窓の間は再使用を拒否されるため、同じ要求を繰り返す #821 の方式は使えない。**計測区間の外で**
`jti` が一意な proof と `RequestHead` を ops 個作り、試行ごとに新しい認証器（= 新しい `ReplayStore`）を
作る。アクセストークンは ES256 でウォームアップの 1 回だけミスさせ、以降はキャッシュヒットにする
（`cache_misses == 1`、`cache_hits == ops` を assert）。proof の方式だけを RS256 と ES256 で変える。
`iat` の窓は過去側を上限（300 秒）にして、事前生成から計測終了までの経過で外れないようにした
（窓の検査は整数比較 2 回でコストに影響しない）。

| 記号 | シナリオ |
|---|---|
| D0 | DPoP 無効の認証器・ゲートが、同じ `cnf` 付きトークンを `Bearer` で受理するキャッシュヒット（基準） |
| D1 | `dpop::verify_proof` の直接呼び出し（proof 単体の検証コスト） |
| D2 | `DpopMode::Required` の `authenticate_at`（トークンのヒット + proof 検証 + `register`） |
| D3 | `DpopMode::Required` の `JwtGate::check`（実ゲート経路。毎回 `Allow` を assert）。D3 - D0 が DPoP 無効時との差 |
| D4 | `DpopMode::AllowUnboundBearer` の認証器・ゲートが `cnf` のないトークンを `Bearer` で受理する経路と、DPoP 無効で同じトークンを渡した場合 |
| D5 | `ReplayStore::register` 単体。(a) 空から既定容量まで入れたときの 1 回あたり、(b) 満杯の後に窓が過ぎた最初の 1 回の掃除時間 |

5,000 ops x 7 試行（D5 は 131,072 件 x 7 試行）で、試行ごとの値の中央値を採る。3 回実行した。

### 2.2 HTTP 負荷（NFR-6）

`benches/jwt-nfr6-bench.sh` の opt-in `JWT_DPOP=1` と `crates/plugin-jwt/examples/jwt_gate_server.rs`
（`FANDHE_BACKEND_JWT_BENCH_DPOP=allow-unbound`）。系列 (d)「ゲート有効 + DPoP
（`AllowUnboundBearer`）+ `cnf` のない ES256 トークンを Bearer で提示」を、既存の (a) baseline・
(c) ES256 ゲート有効（DPoP なし）と同じ手順で測る。

**`DpopMode::Required` で要求ごとに proof を付ける経路は HTTP では計測できない。** 理由は次のとおり。

1. `oha` は固定ヘッダしか送れず、2 回目以降は同じ proof が `jti` の再使用検知で 401 になる
   （`verify_all_200` が必ず失敗する）。
2. 再使用検知を無効にする設定をライブラリへ足せば計測はできるが、安全性の後退になるため行わない。
3. 要求ごとに proof を変える負荷クライアントは本イシューの範囲外（8 節）。

## 3. 実行環境

| 項目 | 値 |
|---|---|
| 日時 | 2026-10-27 |
| 対象コミット | `f1a2dac`（origin/main）+ 本変更（example・スクリプト・文書のみ。`crates/plugin-jwt/src/**` は無変更） |
| CPU | Apple M4 Max |
| OS | macOS（Darwin 27.0.0 arm64） |
| rustc | 1.98.1 |
| ビルド | `--release` |
| oha | 1.14.0 |
| シェル | `/opt/homebrew/bin/bash` 5.3.9 は存在するが、スクリプトの実行は `PATH` 上の `bash`（macOS 標準 3.2）。構文・実行とも通った |
| 負荷 | `uptime` の load average 3〜4（37 セッションの共有開発機）。**専有環境ではない** |
| 専有枠 | 未使用。`benches/lib/exclusive.sh` の静穏確認は `/proc` に依存するため Darwin では使えず、`nfr6-exclusive.sh` は実行していない（#821 レポート 7 節に従い、非専有環境の参考値と明記する） |

## 4. 結果

### 4.1 マイクロベンチ（中央値、ns/op。3 回分の並び、代表値は 3 回の中央値）

proof の方式別:

| proof | D1 `verify_proof` | D2 認証器 | D3 ゲート | D3 - D0 |
|---|---|---|---|---|
| RS256 | 13,422 / 13,323 / 13,348（代表 13,348） | 14,077 / 14,063 / 14,174（代表 14,077） | 14,106 / 14,117 / 14,130（代表 14,117） | 約 13,800 |
| ES256 | 31,655 / 31,649 / 31,619（代表 31,649） | 32,487 / 32,328 / 32,269（代表 32,328） | 32,402 / 32,518 / 32,538（代表 32,518） | 約 32,200 |

DPoP 無効側の基準（同じ実行で測り直した値）:

| 経路 | ns/op |
|---|---|
| D0 `JwtGate::check`（`Bearer`、キャッシュヒット） | 約 313〜317 |
| D0 `authenticate`（同上） | 約 270〜285 |
| D4 DPoP 無効・`cnf` なしトークン・ゲート | 267 / 246 / 254（代表 254） |
| D4 `AllowUnboundBearer`・`cnf` なしトークン・ゲート | 265 / 252 / 258（代表 258） |

- RS256 proof の検証は ES256 の約 0.42 倍（ES256 は約 2.4 倍遅い）。#821 の署名検証の比（約 2.7 倍）と同じ向き。
- D2 と D3 はほぼ同じ。ゲート自体の追加コストは測定誤差内。D2 - D1 は約 0.7〜0.9 µs で、これが
  トークンのキャッシュヒット（約 0.3 µs）+ `register`（約 0.14 µs）+ ヘッダ走査・`htu` 構築などの合計に当たる。
- D4 の差（`AllowUnboundBearer` - DPoP 無効）は約 +4 ns で誤差内。DPoP を設定しても Bearer（`cnf` なし）の
  経路にはほとんどコストが乗らない。

### 4.2 `ReplayStore`（D5）

| 項目 | 値（3 回の代表） |
|---|---|
| 空から 131,072 件まで入れたときの 1 回あたり | 約 139 ns（138.5〜141.5） |
| 満杯の後、窓が過ぎた最初の 1 回の `register` が全件を掃除する時間 | **約 20.3 ms**（20.2〜20.4 ms） |

最初の 1 回の掃除は 131,072 件を `BinaryHeap` と `HashMap` から取り出す。この間 `Mutex` を持つため、
同じ `ReplayStore` を使う他の要求は約 20 ms 待つ。設計 9.12 節が「長い無通信の後の最初の `register` だけ
取り出しが多くなる」と書いていた点を実測で確認した。ただし満杯（既定容量）まで溜まる負荷は
約 2,000 rps の持続に相当し、その負荷が窓（既定 60 秒）より長く止んだ後に再開する場合に限られる。

### 4.3 HTTP 負荷（NFR-6、系列 (d)）

5 回 x 5 秒、32 接続の中央値。**非専有の参考値**。

| 系統 | RPS | p95 (s) | 対 baseline（RPS / p95） | 判定 |
|---|---|---|---|---|
| (a) baseline | 165,743 | 0.000223 | — | — |
| (b) リンクのみ（ゲート無効） | 164,240 | 0.000224 | 99.09% / 100.39% | WARN |
| (c) ゲート有効 ES256（DPoP なし） | 155,080 | 0.000234 | 93.57% / 104.95% | FAIL |
| (d) ゲート有効 + DPoP（`AllowUnboundBearer`） | 153,389 | 0.000235 | 92.55% / 105.43% | FAIL |

(d) を (c) に対して見ると RPS 98.91%、p95 100.46%。DPoP の設定を足したことによる差は約 1% で、
5 回の試行内のばらつき（(d) の RPS は 151,491〜154,843）と同程度。

## 5. NFR-6 の判定

- **Bearer 経路（系列 (d)）**: 実測できた。全パスに介入するゲートは NFR-7 に近いため、実務許容帯（95〜105%）で
  見る。(d) は対 baseline で FAIL だが、**DPoP なしの (c) も同じ環境で同様に FAIL**（93.57%）であり、
  この差は DPoP ではなくゲートそのものと、非専有・負荷生成と被試験サーバが同一ホストという環境に
  よるものである。DPoP の設定を足した分は (c) 比で約 1% に収まる。専有環境での確定は未実施で、
  FAIL を PASS に丸めない。
- **`DpopMode::Required` の proof 経路**: **HTTP 負荷では判定できない**（2.2 節の理由）。代わりに
  マイクロベンチの絶対コストを示す。1 要求あたり RS256 proof で約 13.8 µs、ES256 proof で約 32.2 µs の
  CPU 時間が加わる。baseline の 1 要求あたりの間隔は約 6 µs（165,743 rps）なので、**推定**として、
  最小サーバでワーカーが CPU で飽和する負荷のもとでは Required の RPS は baseline に対し大きく
  下がる（桁として数分の 1）。ただしこの比は推定であり、HTTP で実測した値ではない。この負担は
  設計上の必然（9.1 節: 写した proof を使えなくする効果のため、署名検証を毎回行いキャッシュしない）で、
  ライブラリの欠陥ではない。実アプリでは、ハンドラ本体（DB 等）に比べて µs 規模の署名検証が
  支配的かどうかで決まる。

## 6. 考察

- proof の署名検証が支配的である。RS256 は公開指数が小さく検証が軽い（約 13.3 µs）。ES256 は約 31.6 µs。
  fandhe-auth の既定は ES256 の proof になりうるため、DPoP を有効にするとき CPU を見積もる際は
  ES256 の値（約 32 µs/要求）を使うとよい。RS256 の proof を許可する運用では約 14 µs/要求。
- アクセストークンの検証はキャッシュヒット（約 0.3 µs）で、proof の検証の約 1/40〜1/100 に過ぎない。
  DPoP のコストの大半は proof の署名検証で、キャッシュの効きとは独立である。
- `register` の通常コストは約 140 ns で、ロック区間も短く問題にならない。
- **見直しの要否**:
  - 容量の既定（131,072）・窓の既定（60 秒）は、本計測の範囲では**変更する根拠は出なかった**。
    通常コストは小さく、満杯拒否もフェイルクローズで安全側である。
  - 一方、満杯の後の最初の `register` が約 20 ms のロックを持つ点は、レイテンシの尾（p99）に出うる。
    掃除を 1 回の `register` あたり件数上限つきにする（残りは次回以降へ繰り延べる）案は有効だが、
    `src/**` の変更が要るため本 PR では行わず、改善候補として 8 節に残す（掃除の遅延中に満杯判定が
    偽の満杯になる点などの設計検討が要る）。

## 7. 受け入れ条件との対応

| 条件 | 状態 |
|---|---|
| 1. RS256・ES256 それぞれの proof の検証コストと、DPoP 無効時との差を記録する | 達成（4.1。D1・D3 と D3 - D0） |
| 2. NFR-6 の判定、または判定できない理由を記録する | 達成。Bearer 経路は実測（4.3、非専有の参考値）、proof 経路は HTTP で判定できない理由と推定を記録（5 節） |
| 3. 計測環境と再現手順を記録する | 達成（3 節・9 節） |

## 8. 範囲外と改善候補（起票は承認後）

- 要求ごとに proof を変える HTTP 負荷クライアント。`Required` の NFR-6 を実測で判定するために要る。
- `ReplayStore` の掃除の件数上限化（6 節）。ライブラリ変更が要る。
- 専有環境（Linux の専有枠）での (c)・(d) の再計測。本計測は非専有の macOS。
- `benches/jwt-nfr6-bench.sh` の oha 呼び出しに `-w` を足した（oha 1.14 が HTTP/1 の期限時に実行中の
  要求を "aborted due to deadline" として数え、成功率検査が計測を誤って失敗にするため）。
  他の NFR-6 スクリプト（hub・webrtc・graphql 等）にも同じ事象が出うる。本 PR では jwt のみ修正し、
  水平展開は別途。

## 9. 再現手順

```bash
# ビルド
cargo build --release -p fandhe-backend-plugin-jwt --features dpop --example jwt_dpop_bench --example jwt_gate_server
cargo build --release -p fandhe-backend-core --example minimal --no-default-features

# マイクロベンチ（FANDHE_BACKEND_JWT_DPOP_BENCH_OPS / FANDHE_BACKEND_JWT_VERIFY_BENCH_TRIALS /
# FANDHE_BACKEND_JWT_DPOP_BENCH_REPLAY_FILL で調整可）
cargo run --release -p fandhe-backend-plugin-jwt --features dpop --example jwt_dpop_bench

# NFR-6（oha・jq・curl が必要）。JWT_DPOP=1 で系列 (d) を追加する
JWT_DPOP=1 bash benches/jwt-nfr6-bench.sh
# 専有枠（Linux。flock・静穏確認付き）
JWT_DPOP=1 TARGETS=jwt bash benches/nfr6-exclusive.sh
```
