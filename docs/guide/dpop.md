# DPoP 結合トークンの検証（dpop）

`fandhe-backend-plugin-jwt` の opt-in feature `dpop` は、RFC 9449 の DPoP（Demonstrating
Proof of Possession）で結合されたアクセストークンを検証します。アクセストークンが盗まれても、
対応する秘密鍵を持たない者は使えなくなります。

## 概要

DPoP を設定したゲートが行うこと:

- `Authorization: DPoP <token>` と `DPoP` ヘッダ（proof）を読み、アクセストークンの検証に加えて
  proof の署名、`htm`・`htu`・`ath`・`iat`・`jti`、トークンの `cnf.jkt` との一致を毎回検査する
- DPoP で結合されたトークンを `Bearer` で提示された場合は拒否する（格下げ拒否）
- 拒否は 401 と `WWW-Authenticate: DPoP`（`error` は `invalid_token` か `invalid_dpop_proof`、
  `algs` に許可する proof の `alg`）で返す
- 同じ proof の再使用を、`iat` の窓の間だけ `jti` を保持して拒否する

しないこと:

- **`DPoP-Nonce` は扱いません。**
- **`jti` の保持は複数インスタンス間で共有しません。** 下の「既知の限界」を読んでください。

## 有効化

```toml
[dependencies]
fandhe-backend-plugin-jwt = { version = "...", features = ["dpop"] }
# hub 共通配線から使うとき
fandhe-backend-plugin-hub-wiring = { version = "...", features = ["dpop"] }
```

無効のままなら依存もコードも増えません。有効にしても新しい依存は増えません（暗号は
既存の `ring` のみ）。crates.io 公開版 0.6.0 で利用できます。

> **注意: DPoP を設定していないゲートは、結合トークンも bearer として受理します。**
> feature を有効にしただけでは検査は始まりません。DPoP を強制したい下流のゲートは、
> 必ず `JwtAuthenticatorBuilder::dpop` で設定してください。

## 設定

`DpopConfig::new(mode, origin, algs)` の 3 引数はすべて必須で、既定値はありません。
設定漏れが「検証しない」へ静かに倒れないようにするためです。

| 項目 | 内容 |
|------|------|
| `DpopMode::Required` | すべての要求で DPoP を必須にする。`cnf.jkt` のないトークンは `Bearer` でも `DPoP` でも拒否 |
| `DpopMode::AllowUnboundBearer` | `cnf.jkt` のないトークンに限り `Bearer` を受理する移行期間用。結合トークンの `Bearer` 提示は拒否 |
| `PublicOrigin` | 公開オリジン（`scheme://host[:port]`）。proof の `htu` と突き合わせる。本番は https にする |
| `AllowedAlgorithms` | 許可する proof の `alg`（RS256・ES256） |
| `IatWindow` | `iat` の許容幅。既定は過去 60 秒・未来 5 秒、上限は過去 300 秒・未来 60 秒 |
| `with_replay_capacity` | `jti` の保持数の上限。既定 131,072、上限 4,194,304 |

- `PublicOrigin` は設定で与える値です。要求の `Host` や `X-Forwarded-*` から導出しません。
  パス接頭辞を剥がす前段プロキシの構成は対象外です
- 容量の目安は「ピーク時の DPoP の rps × (過去側 + 未来側の秒数)」です。既定の窓なら 131,072 で
  約 2,000 rps に相当します。1 項目あたりのメモリは `fandhe_backend_plugin_jwt::dpop::ReplayStore`
  の doc を参照してください。満杯になると新しい proof を拒否します（フェイルクローズ）
- 窓を広げたり容量を大きくしたりするほど、再使用を許す余地と保持するメモリが増えます。
  必要以上に広げないでください

## 配線

> **このコードは説明用で、コンパイル検証の対象外です。** フレームワーク側の API の呼び出し方は
> rustdoc の doc test（`JwtAuthenticatorBuilder::dpop`、`TenantGateConfig::from_authenticator`）が
> 「正」です。

plugin-jwt を直接使う場合:

```rust,ignore
use fandhe_backend_plugin_jwt::dpop::{DpopConfig, DpopMode, IatWindow, PublicOrigin};
use fandhe_backend_plugin_jwt::{Algorithm, AllowedAlgorithms, JwtAuthenticator, JwtGate, NoExtraClaims};

let cfg = DpopConfig::new(
    DpopMode::Required,
    PublicOrigin::new("https://api.example")?, // 設定で固定する。Host ヘッダからは作らない
    AllowedAlgorithms::only(Algorithm::Es256),
)
.with_iat_window(IatWindow::new(Duration::from_secs(30), Duration::from_secs(5))?)
.with_replay_capacity(65_536)?;

let auth = JwtAuthenticator::<NoExtraClaims>::builder(keys, policy)
    .dpop(cfg)
    .build();
let gate = JwtGate::new(auth); // Server::gate(gate) で登録する
```

hub 共通配線を使う場合は、DPoP を設定した認証器を `Authenticator::from_jwt` で包んで渡します。

```rust,ignore
let auth = JwtAuthenticator::<HubClaims>::builder(keys, policy).dpop(cfg).build();
let config = TenantGateConfig::from_authenticator(Authenticator::from_jwt(auth));
```

テナント境界の `org_id` による 403 は、DPoP の検査が通った後に判定されます。DPoP の拒否は
401 のまま変わりません。

## ハンドラで検証結果を読む

1 要求につき `authenticate` は 1 回だけ呼ばれます。ハンドラは `authenticate` をもう一度呼ばず、
ゲートが渡した結果を読みます。

- plugin-jwt: `JwtGate::verified(head)`
- hub: `TenantGate::verified(head)`

同じ proof で再検証すると `jti` が再使用扱いになり、`Replayed` で失敗します。ゲートを通って
いない要求では `NotAdmitted` が返ります。同じ型のゲートを二重に登録した場合は、上書きせず 500 で
拒否します。

## 拒否の応答

| 状況 | 応答 |
|------|------|
| 資格情報なし | 401、`WWW-Authenticate: DPoP algs="..."`（`AllowUnboundBearer` のときは `Bearer` の行も加わる） |
| アクセストークンが不正 | 401、使われたスキームの行に `error="invalid_token"` |
| proof が不正（欠落・重複・署名・`htm`・`htu`・`ath`・`iat`・再使用・`cnf.jkt` 不一致） | 401、`error="invalid_dpop_proof"` |
| 結合トークンを `Bearer` で提示 | 401 |

本文は固定の JSON で、トークン・proof・`jwk` の値は含みません。

## 既知の限界と運用上の注意

- **`jti` の保持はゲートごと・プロセスごとです。複数インスタンス間では共有しません。**
  写し取った proof を `iat` の窓の中で別のインスタンスへ送る使い回しは防げません。窓を短くして
  余地を限ってください。アクセストークンだけの写しは、秘密鍵を持たない者には proof を作れない
  ため使えません
- `DPoP-Nonce` は扱いません
- feature が無効、または DPoP を設定していない下流は、結合トークンも bearer として受理します。
  feature の統合で plugin-jwt の `dpop` だけが有効になり、hub の `dpop` が無効のままだと、
  チャレンジが付かないことがあります（拒否は 401 のまま）
- 満杯になってから最初の `register` は、期限切れの項目をまとめて掃除するため約 20 ms ロックを
  持ちます（`benches/reports/issue906-jwt-dpop-gate-performance.md`）
- トークン・proof・`jwk`・`jti` をログに出さないでください。本クレートのエラー型の `Debug` も
  これらを含みません
- DPoP を有効にしたときの追加コストは、proof の署名検証が支配的です（RS256 で約 13.8 µs、
  ES256 で約 32.2 µs。計測条件は上記レポート）

## 関連

- rustdoc: `fandhe_backend_plugin_jwt::dpop`（`verify_proof`・`DpopConfig`・`ReplayStore`）と
  `JwtAuthenticatorBuilder::dpop`
- 設計の詳細: `docs/design/jwt-verify-plugin.md` 9 章
- 受け入れ結果: `docs/acceptance/req16-jwt-verify.md`
