# 汎用 JWT 検証プラグイン設計（REQ-16）

イシュー #811（ツリーのルートは #807）。要件 REQ-16（汎用 JWT 検証プラグイン）・REQ-9
（hub 配線）に対応する。根拠の仕様は `docs/spec/04-requirements.md` の REQ-16・REQ-9・
除外事項（第 8・10・11 項）と `docs/spec/requests/fandhe-auth.md`。

> ドラフト。最終承認は PR レビューで行う。本書は設計判断の記録のみで、コード変更を
> 含まない。実装は Phase 2（#812〜#818）・Phase 3（#819〜#823）が担う。

## 1. 背景・目的

REQ-16 は、下流サービスが認証基盤（`fandhe-auth-backend`）の発行するアクセストークン
（JWT）を検証するための、hub 固有の配線（REQ-9）から独立した汎用プラグインを求める。
現行の検証は `crates/plugin-hub-wiring`（`jwt.rs`・`jwks.rs`・`auth.rs`・`gate.rs`）に
埋め込まれており、次の制約がある。

- RS256 のみ対応。`iss`・`aud`・`typ`・`nbf` を検証しない。
- `org_id` 必須の hub 固有クレーム型（`Claims`・`TokenError::MissingOrgId`）に縛られる。
- `kid` 重複を文書全体の拒否（`JwksError::DuplicateKeyId`）としており、鍵ローテーション中の
  新旧同一 `kid` 併存を表現できない。
- `kty != RSA` や RS256 以外の `alg` の鍵を黙って読み飛ばす。

本書は 10 の論点に結論と理由を固定し、後続 issue が判断を迷わないようにする。
決定済みの入力（再決定しない）: hub-wiring の `TenantGateConfig` にポリシーを必須で渡す、
移行は 0.5.0 の破壊的変更として出す。

現行実装の事実（調査時点）:

- 依存は `ring 0.17`・`base64 0.22`・`serde`・`serde_json`。いずれも `Cargo.lock` 上で
  workspace に既存（`ring` は `plugin-webrtc` 経由でも入る）。
- JWKS 上限: 文書 64 KiB・鍵 32 件・`n` 1024 B・`e` 8 B。トークン上限 8192 B。
  署名検証後に payload を解析し、`exp <= now` を拒否する。
- 検証結果キャッシュ: キーはトークンの SHA-256、容量 1024 の FIFO（期限切れを先に掃く）、
  鍵集合の `Arc::ptr_eq` で無効化、成功のみ保持、ヒット時に `exp` を再判定する。
- `RequestGate::check` は登録順評価で最初の Reject が勝つ（AND 結合）。複数ゲートでの
  発行者 OR 結合はできない。
- `scripts/dep-direction-check.sh` の `allowed_edge_patterns` は `plugin-*:core/http/routes`
  のみ許可し、plugin から plugin への依存は現状違反になる。
- 名前の可用性: 2026-10-15 に crates.io API で `fandhe-backend-plugin-jwt` が 404（未登録）
  であることを確認した。

## 2. 決定事項

### 2.1 crate 名と配置

**結論**: `crates/plugin-jwt`、パッケージ名 `fandhe-backend-plugin-jwt`。Gate 型の依存逆転
パターン（`plugin-boundary.md` 5.6 節）をとり、利用側が依存に加えて `Server::gate(...)` で
登録する。core には feature も `plugin_jwt` 再エクスポートも足さない。`lib.rs` 冒頭に
`拡張点対応: RequestGate` を宣言する（`dependency-graph-contract.md` の規約、実装は #813）。

**理由**: 本 crate は `RequestGate` 実装のため `fandhe-backend-core` に依存する。core から
optional 依存と再エクスポート（#421・#435 の方式）を張るとパッケージ循環になる。#435 の
方式は「core に登録口と feature を持つ設定登録型」向けで、Gate 型の「core を変えない」原則
（5.6.1 節）と合わない。名前は未登録で、予約は行わず初回の実 publish で確保する
（`crates-io-release.md` 2 節は空クレートでの事前予約を禁じる）。

**代替案**: core への feature 配線（循環のため不可）。**影響先**: #813。

### 2.2 ポリシー型

**結論**: `JwtPolicy` の構築に必要な値を、コンストラクタの必須引数（または型状態ビルダー）
にする。省略すると構築できず、検証の抜けた構成を型の上で表現できない。必須は次の 3 つ。

- `Issuer`: 空文字を拒否し完全一致で比較する。
- `Audience`: 1 個以上。トークンの `aud` が文字列でも配列でも、どれか 1 つの一致で受理する。
- 許可 `Algorithm` 集合: 空は構築エラー。`#[non_exhaustive] enum { Rs256, Es256 }` で、
  `none`・HS 系は型として表現できない。

その他:

- `typ` は `#[non_exhaustive] enum TokenType { AccessToken }`（v1 は `at+jwt` のみ）。
  RFC 9068 4 節に従い `at+jwt` と `application/at+jwt` を media type として ASCII 大文字小文字
  無視で受理する。それ以外（`JWT`、ID トークン、`logout+jwt`）と `typ` 欠落は拒否する。
- 時刻ずれ許容（leeway）は既定 0 秒。`.clock_skew(Duration)` で指定でき上限 300 秒、超過は
  構築エラー。`exp` 必須（欠落は拒否）。`nbf` は任意で、あれば `now + leeway >= nbf` を要求。
  `iat` は解析・公開するが判定に使わない。
- 発行者ごとの `alg` 許可リスト: v1 は 1 ポリシー（1 検証器・1 ゲート）を 1 発行者に束ねる。
  鍵選択は署名検証前に行い、payload の `iss` は署名検証後にしか読めないため、順序を
  「鍵集合は発行者に束縛 → 署名検証 → payload の `iss` がポリシーと一致」に固定し、
  署名前の未検証 `iss` で分岐しない。
- 複数発行者は v1 の対象外（ゲートは AND 結合で OR にならない）。必要になれば、1 検証器内に
  「発行者 → (ポリシー, 鍵集合)」を持つ形で再検討する（2.10 節）。
- 必須項目が欠けた構成を作れないことは、#813 で `compile_fail` doc test 等で示す。

**理由**: RFC 8725 の 3.1（`alg` 検証）・3.8（`iss`）・3.9（`aud`）・3.11（明示的型付け）・
3.12（種類の異なる JWT を排他的規則で検証）と RFC 9068 4 節。fandhe-auth の発行者は 1 つ。
**代替案**: 省略可能な設定＋既定値（検証抜けを許すため不採用）。**影響先**: #813・#815。

### 2.3 鍵集合

**結論**: JWK の各エントリを 3 つに分類する。

- (a) 文書全体を拒否する: 採用対象になりうるエントリ（`kty`・`crv` から判断して RSA、または
  EC の P-256 のもの。以下「採用候補」）での `kid` 欠落・`alg` 欠落／`kty`・
  `crv` と `alg` の不一致（`RSA` で `ES256`、`EC` で `RS256`、`ES256` で `crv != P-256`）／
  `alg: none`・HS 系・`kty: oct`／秘密鍵成分（`d` 等）を含むもの／符号化不正・長さ上限超過／
  EC の `x`・`y` が 32 バイトちょうどでないもの／件数上限超過／完全同一鍵（同じ `kid`・`alg`・
  鍵値）の重複。
- (b) 読み飛ばす: `use` があって `sig` でないもの／形式は正しいが未対応の方式（`RS384`・
  `PS256`・`EdDSA`・`ES384` など `alg` と `kty` が矛盾しないもの）。採用候補でないエントリ
  （`OKP`/Ed25519、EC の P-384 など）は `kid`・`alg` が無くても文書全体を拒否せず読み飛ばす
  （発行者が新方式の鍵を追加しても全下流が止まらないようにするため）。読み飛ばし件数は
  `KeySet::skipped()` 等で参照可能にし、黙って捨てない。
- (c) 採用する: `RSA` + `RS256`（既存の `n`・`e` 上限と先頭 0 除去を引き継ぐ）、
  `EC` + `P-256` + `ES256`（`0x04‖x‖y` の非圧縮点として ring へ渡す）。

現行の「`kty != RSA` を黙って読み飛ばす」扱いは (a) と (b) に分けて置き換える。

同一 `kid` に複数の鍵（新旧）を許す。1 `kid` あたり上限 3（前・現・次）で、超過は文書全体を
拒否する。全体の `MAX_KEYS` 32 は維持。`DuplicateKeyId` は廃止する（#820 の CHANGELOG に
記載）。検証順は次のとおり。

1. ヘッダ `alg` がポリシーの許可リストにあるか。
2. `kid` に一致する鍵があるか（無ければ未知 `kid`）。
3. 候補は、ヘッダ `alg` と一致する方式に固定された鍵（無ければ `KeyAlgorithmMismatch`）。
4. 候補を JWKS 文書の順に試し、最初に成功した鍵で受理する。
5. 全て失敗すれば `InvalidSignature`。

差し替え API は現行 `SharedJwks` 相当（`Arc<RwLock<Arc<KeySet>>>`、読み取りでロックを短く
保つ、poison は `into_inner` で回復）を引き継ぐ。文書に鍵があったが採用 0 件の場合は構築
エラー（`NoUsableKeys`）とし、`{"keys":[]}` を明示した場合のみ空集合を許す（全件拒否、
フェイルクローズ）。

**理由**: 発行者が EdDSA 鍵を足しただけで全下流の JWKS 読み込みが止まる可用性障害を避ける。
一方、矛盾する鍵と危険な鍵は攻撃・設定ミスの兆候なのでフェイルクローズで拒否する。
`kid` あたり上限は、署名不正トークン大量送信時（失敗はキャッシュしない）の検証回数増加を抑える。
**影響先**: #814・#815・#820。

### 2.4 公開 API

**結論**:

- 検証関数: `verify::<E>(token, &KeySet, &JwtPolicy, now_unix) -> Result<Claims<E>, VerifyError>`。
- `Claims<E>` は登録クレーム（`iss`、`sub`（Option）、`aud`、`exp`、`nbf`・`iat`・`jti`・
  `client_id`・`scope` は Option）を持つ。追加クレームは利用側定義の型
  `E: DeserializeOwned + Clone + Send + Sync + 'static`（既定は `()` 相当）として同じ payload
  から取り出す。
- hub 固有の `org_id` は汎用型に入れない。hub 側で `HubClaims { org_id: Option<String> }` を
  定義し、署名・クレーム検証後に hub 側で 403 を判定する。
- 生 payload の保持はしない（キャッシュ 1 エントリが最大 8 KiB × 1024 になりメモリが増える）。
- エラー型は `#[non_exhaustive]`・`Copy`・文字列を持たない enum。変種: `MissingToken`・
  `TooLong`・`Malformed`・`AlgorithmNotAllowed`・`MissingKeyId`・`UnknownKeyId`・
  `KeyAlgorithmMismatch`・`InvalidSignature`・`CriticalHeader`・`InvalidType`・
  `IssuerMismatch`・`AudienceMismatch`・`Expired`・`NotYetValid`・`MissingClaim`・
  `InvalidExtraClaims`。`Display` は固定文言でトークン内容・`kid` を含めない。
- ハンドラがクレームを得る口は、現行 `Authenticator` と同形の共有ハンドル（例:
  `JwtAuthenticator<E>`、ゲート設定から clone して取得）。`GateOutcome` はクレームを運ばない
  （5.6.3 節）。
- ゲート `JwtGate` の拒否は 401、本文は固定 JSON。`WWW-Authenticate` は RFC 6750 3 節に従い、
  トークン無しは `Bearer`、不正は `Bearer error="invalid_token"`（`error_description` は付けない）。

**理由**: 型で受けることで hub の `org_id` を汎用型から追い出せる。エラーに秘密や攻撃者入力を
載せない（OWASP A09）。**影響先**: #815・#816。

### 2.5 未知 `kid` の通知口

**結論**: 同期 trait `UnknownKeyNotifier: Send + Sync { fn notify(&self, event: &UnknownKeyEvent<'_>) }`
を設け、クロージャにも実装する（`diagnostics-sink.md` の `Diagnostics` と同形）。`tokio` 等の
チャネルには依存しない（利用側が実装内で `try_send` すればよい）。契約は「ブロックしない・
I/O しない・panic しない」で、panic 境界は diagnostics-sink 5 節に倣い `catch_unwind` で包む
多層防御を推奨する（`panic = "abort"` では効かないことを明記）。

`kid` は攻撃者制御値なので、(i) ヘッダ `kid` に長さ上限（例 256 B）を設け超過は `Malformed`
で拒否し通知しない、(ii) イベントでは信頼できない値と明記する、(iii) 本 crate 側で
`AtomicU64` による最小通知間隔（既定 10 秒、設定可）で通知をまとめる。未知 `kid` のトークンは
その場で 401。鍵の取り直しはアプリの非同期タスクが差し替え API で行う。

**理由**: ゲートの同期・I/O なし契約（NFR-7、`RequestGate` の doc）を守り、通知の嵐が DoS を
増幅しないようにする。**影響先**: #814・#816。

### 2.6 署名検証結果のキャッシュ

**結論**: `auth.rs` の方式を引き継ぐ。キーはトークンの SHA-256（生トークンは保持しない）、
容量 1024・期限切れを先に掃いて FIFO 追い出し、鍵集合の `Arc::ptr_eq` で無効化、成功のみ保持、
ヒットごとに `exp`・`nbf` を leeway 付きで再判定する。キャッシュは 1 ポリシーに束ね、ポリシーは
構築後に変更不可。ES256 検証は RS256 より重い（`fandhe-auth.md` の注記）ため、効果は #821 で
中央値として計測した（`crates/plugin-jwt/examples/jwt_verify_bench.rs`。結果は
`benches/reports/issue821-jwt-verify-performance.md`）。
**影響先**: #816・#821。

### 2.7 依存の解釈

**結論**: 「新規の外部依存を追加しない（`ring` のみ）」は、**workspace の `Cargo.lock` に新しい
crate・新しい版を増やさない**ことと解釈する。暗号は `ring` のみ。`base64 0.22`・`serde`
（derive）・`serde_json` は hub-wiring と同版を共有する。ルートの `[workspace.dependencies]` に
追記し各 crate から `{ workspace = true }` で参照する（#813）。検証は `Cargo.lock` の差分で
`[[package]]` が増えていないことと、`cargo tree` の前後比較で行う。Gate 型での「feature 無効時」は
「本 crate を依存に加えない構成」と読み替える。

**理由**: `fandhe-auth.md` の該当節は暗号ライブラリに限って「新規依存なし」と書いている。
JSON・base64 の自前実装は攻撃表面を増やす（`plugin-hub-wiring/Cargo.toml` の既存判断と同じ）。
この解釈の妥当性は本 PR のレビューで確認する点であり、承認済みではない。**影響先**: #813・#822。

### 2.8 fuzz の置き場所

**結論**: 独立 fuzz crate `crates/plugin-jwt/fuzz` を新設する。`crates/http/fuzz` と同形
（`[workspace]` 空テーブル、`publish = false`、ルート `exclude` に追加）。#818 で
`scripts/fuzz.sh`（現状 `FUZZ_DIR` 固定）を複数 `FUZZ_DIR` 対応に直し、CI `fuzz-smoke` の
キャッシュと `ci-cache-slim-target.sh` の対象パスを広げる。対象は 3 つ。

- (i) JWKS 解析: 任意文字列を `KeySet::from_json` に渡す。
- (ii) 署名検証前の経路: 任意トークン（分割・base64・ヘッダ JSON・`alg`・`kid`・`crit`）。
- (iii) 署名検証後のクレーム解析: 入力をヘッダと payload に分け、ハーネス内で固定テスト鍵を
  使い RS256 で署名する（PKCS#1 v1.5 は決定的）。公開 API のみでクレーム検証に到達でき、
  fuzz 専用の公開 API や feature は足さない。

**理由**: `crates/http/fuzz` から依存すると http の fuzz ビルドに `ring` と plugin 依存が入り
責務が混ざる。どちらも `exclude` するため pay-for-what-you-use の差は出ず、決め手は責務分離と
#507 の standalone 隔離の前例。`cargo geiger` 等で偽陽性にならないことは #818 で確認した（ルート workspace の `cargo metadata` に fuzz crate が現れないこと、`scripts/pay-for-what-you-use-check.sh` が PASS すること。手順と結果は `docs/design/fuzzing.md` の #818 の節を参照）。
**影響先**: #818。

### 2.9 hub-wiring の移行

**結論**（入力: ポリシーを `TenantGateConfig` に必須で渡す、0.5.0）:

- `jwt::verify_token`・`jwt::Claims`・`jwt::TokenError`・`jwks::{JwksKeySet, SharedJwks, JwksError}`
  の独自実装は削除する。
- `pub use fandhe_backend_plugin_jwt as jwt;` のように crate 全体を再エクスポートし、利用者の
  依存を 1 つで済ませる（#421 の whole-crate 再エクスポート方針と揃える）。
- `auth::Authenticator` は `JwtAuthenticator<HubClaims>` の別名か薄いラッパーにし、`org_id`
  欠落の 403 は hub 側のエラー型へ移す。`bearer_token` は汎用 crate へ移し hub は再エクスポート。
- `#[deprecated]` の互換層は設けない。旧 `verify_token(token, keys, now)` はポリシーを受けず、
  残すと「検証が抜けた経路」を公開し続けるため。
- 移行手順は #820 で `CHANGELOG.md` の `[Unreleased]` に書く。`versioning-policy.md` の 0.x 期
  規則に従い `y` を上げる。

**理由**: REQ-16 受け入れ基準 5 と「検証が抜けた構成を作れない」原則。**影響先**: #820・#822。

### 2.10 見直しの条件

- **JWKS の HTTP 取得（`jwks-remote`）**は本ツリー（Phase 2・3）の範囲外。v1 は通知口＋
  差し替え API＋アプリ側取得。見直す条件は、`fandhe-auth-backend` の鍵ローテーションを運用で
  始める段階で、複数の下流サービスが同じ取得コードを書くようになったとき。形は `plugin-jwt` の
  opt-in feature（既定無効、無効時は依存ゼロ）。必須要件: 設定固定 URL のみ（SSRF 対策）、
  HTTPS 必須、応答サイズ上限（64 KiB）、タイムアウト、最小再取得間隔、取得失敗時は旧鍵を保持。
  除外事項第 11 項と `fandhe-auth.md` の見直し条件表に整合させる。
- その他: 複数発行者、`EdDSA`（RFC 9864）・`PS256` 等の方式追加、`typ` の別プロファイル。

## 3. REQ-16 受け入れ基準と issue の対応表

| REQ-16 受け入れ基準 | 主に満たす issue | 補助する issue |
|---|---|---|
| 1. 各種拒否（iss・aud・typ・nbf・exp・未知 kid・許可外 alg・種別不一致・none・HS） | #817（網羅テスト） | #813（ポリシー）、#814（鍵集合）、#815（検証） |
| 2. RS256・ES256 の受理と回転中の新旧鍵 | #817 | #814、#815 |
| 3. ゲートが同期・I/O なし（NFR-7） | #816 | #823（レポート） |
| 4. 新規依存なし・未使用時 0 件 | #822（`cargo tree` 0 件の記録） | #813（`cargo tree` 前後比較） |
| 5. REQ-9 の移行と既存基準の維持 | #820 | #822 |
| 6. JWT 解析部の fuzz（NFR-11） | #818 | — |
| （横断）性能・キャッシュの効果 | #821 | — |
| （横断）受け入れ検証レポート | #823 | — |

## 4. `plugin-boundary.md` 5.6 節との整合

矛盾しない点:

- 5.6.1: 同期・I/O なしで判定でき、core を変えない。
- 5.6.2: 依存グラフに入れるか否かで pay-for-what-you-use が成立する。
- 5.6.3: `GateOutcome` はクレームを運ばず、共有ハンドル経由で得る。
- 5.6.4: まず依存逆転を検討する。

#822 へ回す追記（本イシューでは `plugin-boundary.md` を編集しない）:

- (a) Gate 型の 2 例目として `plugin-jwt` を加え、5.6.1 の「RS256」記述を RS256・ES256 へ更新。
- (b) plugin から plugin への依存エッジ `fandhe-backend-plugin-hub-wiring → fandhe-backend-plugin-jwt`
  を `scripts/dep-direction-check.sh` の `allowed_edge_patterns` と `dependency-graph-contract.md`
  へ追加する。現状の許可リストでは違反になるため、#820 と同時か先に入れる必要がある。
- (c) `plugin-jwt` の `拡張点対応: RequestGate` 宣言と `extension-closure-gate.sh` の通過確認。
- (d) 公開 crate 数 13 → 14 の追随（`CLAUDE.md`・`crates-io-release.md`・ルート `Cargo.toml` の
  コメント・`CHANGELOG.md` 冒頭）。
- (e) REQ-17 の CSRF ゲートとの登録順は #826 が定める、と相互参照する。

## 5. セキュリティ考慮（OWASP Top 10・RFC 8725）

- **A01**: 全異常をフェイルクローズで 401。鍵集合が空なら全件拒否。`iss`・`aud`・`typ` を型として
  必須にし、トークン取り違え（ID トークン・logout トークン・別 client 向け）による迂回を防ぐ。
- **A02**: 署名検証は `ring`。RS256 は `RSA_PKCS1_2048_8192_SHA256`（2048 bit 未満拒否）、
  ES256 は `ECDSA_P256_SHA256_FIXED`（R‖S 64 B）。ヘッダ `alg` を信用せず、許可リストと鍵に
  固定した方式の両方に一致させる。`jku`・`x5u`・`jwk` 等のヘッダ引数は無視し取得も利用もしない。
  `crit` があれば拒否。公開 JWKS 中の秘密鍵成分と `oct` を拒否。キャッシュキーは SHA-256。
- **A03**: 拒否本文と `WWW-Authenticate` は固定文字列。エラー型・`Display` にトークン・`kid` を
  含めない。通知口へ渡す `kid` は長さ上限付きで信頼できない値と明記。
- **A04（DoS）**: 既存上限（トークン 8 KiB、JWKS 64 KiB・32 件）を引き継ぎ、`kid` あたり鍵 3 件、
  `aud` 配列要素数と `kid` 長にも上限を設ける。署名検証まで payload を解析しない。失敗は
  キャッシュしない。キャッシュ容量は有界。未知 `kid` 通知は最小間隔でまとめる。
- **A05**: 検証が抜けた構成を作れない構築 API。leeway は既定 0・上限 300 秒。JWK 矛盾は文書全体拒否。
- **A06**: 新しい crate・版を増やさない（`Cargo.lock` 差分で確認）。`rsa` crate
  （RUSTSEC-2023-0071）は使わない。`cargo audit`・`cargo deny` は #822・#823 で確認。
- **A08**: JWKS 差し替えは利用側が取得した文書のみ。将来 `jwks-remote` を入れる場合は固定 URL と
  HTTPS 必須（SSRF 対策）。
- **A09**: エラーは秘密を含まない種別のみ。ログに生トークンを出さない。
- 本 PR は docs のみで、秘密情報・鍵・`.env` を含めない。fixture 鍵は #817 でテスト専用と明記する。

## 6. 対応可否判定記録

`.claude/rules/feasibility-guardrail.md` に従う、本イシュー（#811、設計文書の追加）の判定記録。
Phase 2・3 の各 issue は着手時にそれぞれ 3 軸を判定する。

### 判定区分

可

### 該当カテゴリと判断根拠

なし

### 要人間判断事項

なし

### 代替案

なし

### 3 軸判定結果

#### 実施可能か

可。受け入れ基準 4 件（10 論点の結論と理由、基準と issue の対応表、5.6 節との整合、3 軸判定記録）は
本書の 2・3・4・6 章の存在で検証できる。

#### 安全か

可。docs のみの変更で、内容はフェイルクローズと RFC 8725 に沿い、OWASP Top 10 の観点を 5 章に記録した。

#### 影響範囲が許容内か

可。影響は `docs/design/` の新規 1 件と索引・`CLAUDE.md` の追記に限られ、コード・Cargo・CI は変更しない。

## 7. スコープ外・見直しの条件

- `plugin-boundary.md`・`dependency-graph-contract.md`・`dep-direction-check.sh` の追随は #822。
- crate 新設と実装は #813〜#816、テストは #817、fuzz 配線は #818、hub 移行は #820、計測は #821、
  受け入れレポートは #823。
- 見直しの条件は 2.10 節を参照。
