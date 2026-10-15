# テスト専用鍵フィクスチャ

`test-rsa-2048.pk8` は `tests/verify.rs`（RS256 正常系）専用の RSA 2048bit 秘密鍵
（PKCS#8 DER）です。`plugin-hub-wiring` の同名フィクスチャと同一バイト列の複製です。

**本番環境で絶対に使用しないでください。** リポジトリに平文でコミットされており、
秘匿性はありません。対応する公開鍵（`n`/`e`）はテストコード内で
`ring::signature::KeyPair::public_key()` から動的導出します。
