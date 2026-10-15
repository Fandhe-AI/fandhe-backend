# テスト専用鍵フィクスチャ

ここの鍵はすべて **テスト専用** です。**本番環境で絶対に使用しないでください。**
リポジトリに平文でコミットされており、秘匿性はありません。`tests/` ごと crates.io の
公開物に同梱されても問題ない使い捨て鍵で、`src/` や非テストのビルド成果物には入りません
（統合テストの `include_bytes!` からのみ参照）。実在の発行者鍵・`.env`・トークンは
コミットしないこと。

形式はすべて PKCS#8 v1 DER（ring の `from_pkcs8` が受理する形。EC は ECPrivateKey に
公開鍵を含み、曲線は named curve）。対応する公開鍵（JWK の `n`/`e`、`x`/`y`）は
テストコード内で `ring::signature::KeyPair::public_key()` から導出し、二重管理しません。

| ファイル | 用途 | 使用テスト |
|---|---|---|
| `test-rsa-2048.pk8` | RS256 の署名鍵（回転の「旧」） | `verify.rs`、`req16_acceptance.rs` |
| `test-rsa-2048-rotated.pk8` | RS256 回転の「新」鍵 | `req16_acceptance.rs` |
| `test-ec-p256.pk8` | ES256 の署名鍵（回転の「旧」） | `req16_acceptance.rs` |
| `test-ec-p256-rotated.pk8` | ES256 回転の「新」鍵 | `req16_acceptance.rs` |

RSA の 2 本は `plugin-hub-wiring` の同名フィクスチャと同一バイト列の複製です。

## 生成手順

```sh
# ES256（P-256）
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -pkeyopt ec_param_enc:named_curve \
  | openssl pkcs8 -topk8 -nocrypt -outform DER -out test-ec-p256.pk8

# RS256（RSA 2048bit）
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -pkeyopt rsa_keygen_pubexp:65537 \
  | openssl pkcs8 -topk8 -nocrypt -outform DER -out test-rsa-2048.pk8
```
