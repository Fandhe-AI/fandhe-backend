# テスト専用鍵フィクスチャ

`test-rsa-2048.pk8` / `test-rsa-2048-rotated.pk8` は `tests/tenant_gate.rs`（RS256 +
JWKS 連携の E2E テスト、鍵ローテーションシナリオ）専用の RSA 2048bit 秘密鍵
（PKCS#8 DER、`openssl genpkey` で生成）です。

**本番環境で絶対に使用しないでください。** リポジトリに平文でコミットされており、
秘匿性はありません。

生成コマンド:

```sh
openssl genpkey -algorithm RSA \
    -pkeyopt rsa_keygen_bits:2048 \
    -pkeyopt rsa_keygen_pubexp:65537 | \
  openssl pkcs8 -topk8 -nocrypt -outform DER -out test-rsa-2048.pk8
```

対応する JWKS（公開鍵の `n`/`e`）はテストコード内で
`ring::signature::KeyPair::public_key()` から動的導出し、フィクスチャの二重管理を
避けています（公開鍵材料をここに別途コミットしません）。
