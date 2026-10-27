# 汎用 JWT 検証プラグイン設計（REQ-16）

イシュー #811（ツリーのルートは #807）。要件 REQ-16（汎用 JWT 検証プラグイン）・REQ-9
（hub 配線）に対応する。根拠の仕様は `docs/spec/04-requirements.md` の REQ-16・REQ-9・
除外事項（第 8・10・11 項）と `docs/spec/requests/fandhe-auth.md`。

> ドラフト。最終承認は PR レビューで行う。本書は設計判断の記録のみで、コード変更を
> 含まない。実装は Phase 2（#812〜#818）・Phase 3（#819〜#823）が担う。`jwks-refresh`
> （追加要件、ツリー #891）の設計は 8 章に記録し、実装は Phase 1（#894〜#898）が担う。
> `dpop`（追加要件、ツリー #891 の Phase 2）の設計は 9 章に記録し、実装は #901〜#907 が担う。

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
  であることを確認した（#822 でも `cargo search` と API の 404 を再確認、`crates-io-release.md` 2 節に記録）。

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
その場で 401。鍵の取り直しはアプリの非同期タスクが差し替え API で行う（`jwks-refresh` feature 有効時は
feature のタスクとトリガーでも行える。8 章）。

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
  差し替え API＋アプリ側取得。なお spec#26 で、HTTP を持たない取り直しの仕組みのみ `jwks-refresh`
  として採用した（8 章）。HTTP クライアント同梱を見直す条件は、`fandhe-auth-backend` の鍵ローテーションを運用で
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
| （横断）受け入れ検証レポート | #823 | `docs/acceptance/req16-jwt-verify.md` |

## 4. `plugin-boundary.md` 5.6 節との整合

矛盾しない点:

- 5.6.1: 同期・I/O なしで判定でき、core を変えない。
- 5.6.2: 依存グラフに入れるか否かで pay-for-what-you-use が成立する。
- 5.6.3: `GateOutcome` はクレームを運ばず、共有ハンドル経由で得る。
- 5.6.4: まず依存逆転を検討する。

#822 へ回した追記（本イシューでは `plugin-boundary.md` を編集しなかった。いずれも #822 で対応済み）:

- (a) Gate 型の 2 例目として `plugin-jwt` を加え、5.6.1 の「RS256」記述を RS256・ES256 へ更新。
- (b) plugin から plugin への依存エッジ `fandhe-backend-plugin-hub-wiring → fandhe-backend-plugin-jwt`
  を `scripts/dep-direction-check.sh` の `allowed_edge_patterns` と `dependency-graph-contract.md`
  へ追加する。現状の許可リストでは違反になるため、#820 と同時か先に入れる必要がある。
- (c) `plugin-jwt` の `拡張点対応: RequestGate` 宣言と `extension-closure-gate.sh` の通過確認。
- (d) 公開 crate 数 13 → 14 の追随（`CLAUDE.md`・`crates-io-release.md`・ルート `Cargo.toml` の
  コメント・`CHANGELOG.md` 冒頭）。
- (e) REQ-17 の CSRF ゲートとの登録順は #826 で `plugin-boundary.md` 5.6.6 節に定めた
  （CSRF を先、認証ゲートを後。5.6.5 節から相互参照）。

## 5. セキュリティ考慮（OWASP Top 10・RFC 8725）

- **A01**: 全異常をフェイルクローズで 401。鍵集合が空なら全件拒否。`iss`・`aud`・`typ` を型として
  必須にし、トークン取り違え（ID トークン・logout トークン・別 client 向け）による迂回を防ぐ。
- **A02**: 署名検証は `ring`。RS256 は `RSA_PKCS1_2048_8192_SHA256`（2048 bit 未満拒否）、
  ES256 は `ECDSA_P256_SHA256_FIXED`（R‖S 64 B）。ヘッダ `alg` を信用せず、許可リストと鍵に
  固定した方式の両方に一致させる。アクセストークンでは `jku`・`x5u`・`jwk` 等のヘッダ引数は無視し取得も利用もしない
  （DPoP proof では `jwk` が検証鍵になる点が異なる。9 章）。
  `crit` があれば拒否。公開 JWKS 中の秘密鍵成分と `oct` を拒否。キャッシュキーは SHA-256。
- **A03**: 拒否本文と `WWW-Authenticate` は固定文字列。エラー型・`Display` にトークン・`kid` を
  含めない。通知口へ渡す `kid` は長さ上限付きで信頼できない値と明記。
- **A04（DoS）**: 既存上限（トークン 8 KiB、JWKS 64 KiB・32 件）を引き継ぎ、`kid` あたり鍵 3 件、
  `aud` 配列要素数と `kid` 長にも上限を設ける。署名検証まで payload を解析しない。失敗は
  キャッシュしない。キャッシュ容量は有界。未知 `kid` 通知は最小間隔でまとめる。
- **A05**: 検証が抜けた構成を作れない構築 API。leeway は既定 0・上限 300 秒。JWK 矛盾は文書全体拒否。
- **A06**: 新しい crate・版を増やさない（`Cargo.lock` 差分で確認）。`rsa` crate
  （RUSTSEC-2023-0071）は使わない。`cargo audit`・`cargo deny` は #822・#823 で確認。
- **A08**: JWKS 差し替えは利用側が取得した文書のみ。`jwks-refresh` の取得関数にも固定 URL と
  HTTPS 必須（SSRF 対策、8.11）を契約として課す。
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

- `plugin-boundary.md`・`dependency-graph-contract.md`・`dep-direction-check.sh` の追随は #822（対応済み）。
- crate 新設と実装は #813〜#816、テストは #817、fuzz 配線は #818、hub 移行は #820、計測は #821、
  受け入れレポートは #823。
- 見直しの条件は 2.10 節を参照。`jwks-refresh` 固有のスコープ外は 8.15 節、`dpop` 固有は 9.13 節を参照。

## 8. JWKS の取り直しの仕組み（`jwks-refresh`、イシュー #893）

イシュー #893（ツリーのルートは #891、Phase 1 の親は #892）。根拠の仕様は `docs/spec/04-requirements.md` の
REQ-16「JWKS の取り直しの仕組み（追加要件）」と `docs/spec/requests/fandhe-auth.md` の「#22」節。
本章は設計の記録のみでコード変更を含まない。実装は #894〜#898 が担う。

### 8.1 範囲と前提

**結論**: opt-in feature `jwks-refresh` は、ゲートの外で `SharedKeySet` を最新に保つ非同期タスクを提供する。
HTTP の取得・クライアント・TLS は持たず、アプリが渡す非同期の取得関数に任せる（除外事項 #8・#11）。単位は
`SharedKeySet` 1 つにつきタスク 1 つ（発行者ごと）。`RequestGate::check` の同期・I/O なし契約（NFR-7）は
変えず、ゲートは未知 `kid` を従来どおりその場で 401 にする。

**理由**: 2.5 節の「鍵の取り直しはアプリの非同期タスクが行う」を、追従の速さと偽の `kid` に対する取得回数の
増え方がサービスごとに分かれないよう共通化するため。**影響先**: #894〜#898。

### 8.2 依存と feature の配線

**結論**: `crates/plugin-jwt/Cargo.toml` に `jwks-refresh = ["dep:tokio"]` を足し、
`tokio = { version = "1", default-features = false, features = ["time", "sync"], optional = true }` とする。
既定 feature には入れない。`rt`・`macros` は使わないため、crate 内で `tokio::spawn` も `tokio::select!` も
使わない（複数待機の競合は `plugin-websocket` の `race2` と同じ手書き方式）。コードは
`#[cfg(feature = "jwks-refresh")]` のモジュール（例 `refresh`）に閉じ、公開 API の doc に対象 feature を明記する。

**「無効時ゼロ」の検証可能な定義**: `plugin-jwt` は `core` 経由で `tokio`（`time`・`sync` を含む）を推移依存として
既に持つ。このため「`cargo tree -p fandhe-backend-plugin-jwt` に `tokio` が現れない」は、そのままでは満たせない
（dev-dependency の `tokio` も表示される）。次の 3 点で判定する（#894 の受け入れ条件の文言をこれに補正する）。

- (a) feature 無効時に `cargo tree -p fandhe-backend-plugin-jwt -e normal --depth 1` で `plugin-jwt` から `tokio` への
  直接の辺がない。
- (b) feature 有効時に `Cargo.lock` の `[[package]]` が増えず、統合ビルドで有効になる `tokio` の feature 集合も増えない
  （`core` が既に `time`・`sync` を有効にしている）。
- (c) feature 無効時は `cfg` によって `refresh` モジュールと関連シンボルがコンパイルされない（`cargo build`・`cargo doc` を
  feature なし・有効・全 feature の 3 構成で確認する）。

`rt` を要する `spawn()` 便宜関数は、spec の「時刻・同期の機能だけ」に反するため足さない。足す場合は別 issue で
spec と合わせて再判定する。**影響先**: #894（Cargo・`docs/dep-impact/records.md`）、#897（確認方法）。

### 8.3 取得関数の trait とエラー型

**結論**: `JwksFetcher: Send + Sync + 'static` を定め、`fn fetch(&self) -> FetchFuture<'_>` とする。
`FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<JwksResponse, FetchError>> + Send + 'a>>`。`futures`・
`async-trait` への依存は足さず dyn 互換にする（`routes::HandlerFuture`・websocket の `BoxFuture` と同方針）。
クロージャ `Fn() -> Fut` への blanket 実装は、型推論の失敗と契約の見えにくさがあるため v1 では採らず、
必要になれば非破壊で追加する。

- `JwksResponse`: 本文 `String`（UTF-8 検査は取得関数側の責務。`KeySet::from_json` が `&str` を受けるため）と
  `max_age: Option<Duration>`。`Cache-Control` の解析は取得関数の責務で、`no-store`・`no-cache`・`max-age` なしは `None`。
- `FetchError`: `#[non_exhaustive]`。文字列も攻撃者入力も持たない種別のみ（`Transport`・`Status`・`TooLarge`・`Other`）。
  `Display` は固定文言（2.4 節の `VerifyError` と同じ。OWASP A09）。
- **キャンセル契約**: 時間切れ・停止のとき、取得の future は任意の `await` 点で drop される（websocket の `on_message` と同形）。

**代替案**: `async fn` in trait は dyn 互換でないため不採用。**影響先**: #894。

### 8.4 起動・停止の API

**結論**: `JwksRefresher::new(shared: SharedKeySet, fetcher, config) -> (RefreshTask, RefreshHandle)`。`RefreshTask` は
`Future<Output = ()> + Send + 'static` で、アプリが自分のランタイムで `tokio::spawn` する（`time` ドライバ必須と doc に明記）。
`RefreshHandle` の drop または明示の `shutdown()` で停止状態と `Notify` を立て、タスクは取得中でも抜ける（取得の future は
drop）。停止の合図はハンドルと共有する `Arc` の状態で持ち、トリガー（8.6）を保持していても停止は妨げない。

- **注意**: タスクを止める・spawn しない・drop すると、定期の取得に加え **`max_stale` によるフェイルクローズも働かなくなる**。
- 取得関数の panic は、取得の future を `catch_unwind` するポーリング用ラッパ（`unsafe` なし）で包み、取得失敗として扱い
  タスクを生かす。`panic = "abort"` では効かない（2.5 節・`diagnostics-sink.md` と同じ書き方）。

**代替案**: crate 内で `spawn` する形は `rt` が要るため不採用。`async fn run(self)` のみの形は停止手段が drop に限られるため
ハンドル併用を採用。**影響先**: #894・#895。

### 8.5 設定型と値の検査

**結論**: `RefreshConfig::new(period: Duration)` で周期を必須引数にする（既定値なし。発行者の回転運用で決まる値のため）。
builder で設定するのは次の 3 つ。

| 項目 | 既定 | 備考 |
|------|------|------|
| `min_interval`（取り直しの間隔の下限） | 10 秒（`DEFAULT_UNKNOWN_KEY_MIN_INTERVAL` と同値） | `Duration` 精度。秒精度の `NotifyThrottle` とは別物 |
| `fetch_timeout`（1 回の取得の時間の上限） | 5 秒 | 取得関数側の時間切れとの多層防御 |
| `max_stale` | なし（opt-in） | 超過で鍵集合を空にする（8.9） |

検査はすべて構築時で、失敗は `RefreshConfigError`（`#[non_exhaustive]`）で返す。0 は拒否する（下限が 0 だと 8.7 の
回数の上限式が崩れる）。過大値も拒否する（周期 ≤ 24 時間、下限 ≤ 1 時間、`fetch_timeout` ≤ 60 秒、`max_stale` ≤ 30 日。
`clock_skew` の 300 秒上限と同じ流儀で、`tokio::time::Instant + Duration` のオーバーフロー panic も防ぐ）。
関係の検査として `min_interval <= period`（超えると周期の意味が失われるため丸めず拒否）と
`max_stale >= period + fetch_timeout`（通常の周期の間に空にならないよう）、
`fetch_timeout <= min_interval`（取得は直列で開始は下限ごとのため、取得が次の開始を押し出さず 8.7 の遅延式が成り立つ）を求める。必須の周期の欠落は型で防ぎ、
#894 のテストで示す。**影響先**: #894。

### 8.6 未知 `kid` の通知口への接続

**結論**: feature が `RefreshTrigger`（`Clone`）を提供し、`UnknownKeyNotifier` を実装する。`notify` は
`tokio::sync::Notify::notify_one()` を呼ぶだけ（許可 1 つに畳まれる合図で、同期・ブロックなし・I/O なし。タスク終了後は
no-op）。キューを持つ mpsc は、満杯の扱いが不要でまとめの意味論に一致する `Notify` を優先し採らない。`kid` は使わず
取得先の決定にも使わない。

**2 つの間引きが直列になる問題**: 既存の `NotifyThrottle`（秒精度、10 秒、間引いた通知は捨てる）の後ろにトリガーを置くと、
「偽の `kid` で通知 → 直後に事前公開なしで鍵が切り替わる → 新しい `kid` の通知が最大 10 秒間引かれる → さらにタスクの
下限を待つ」となり、最悪で「通知の間隔 + 下限 + 取得時間」になって「下限 + 取得時間以内」を満たせない。

**推奨**: feature ゲートした `JwtAuthenticatorBuilder::jwks_refresh_trigger(trigger)` を足す。トリガーは利用者の通知口とは
別の枠に入れ、`NotifyThrottle` を通さず未知 `kid` のたびに呼ぶ（原子操作 1 回で DoS の増幅はなく、取得回数の上限は
タスク側の下限だけで成り立つ）。利用者の通知口は従来どおり間引く。複数の `JwtAuthenticator` が同じ `SharedKeySet` を
共有しても、トリガーの clone 共有で上限は保たれる。

**代替案**: 「`unknown_key_min_interval(ZERO)` を設定すること」を doc で求める形は、誤設定できてしまうため不採用。
**影響先**: #896（`auth.rs` の cfg 付き変更）、#897（遅延の基準が本推奨を前提とする）。

**実装時の決定（#896）**:
- トリガーは `RefreshHandle::trigger()` で得る（非破壊の追加。`JwksRefresher::new` の戻り値の形は変えない）。
  `Drop` は実装せず、保持してもタスクは生きない・止まらない（停止は `RefreshHandle` のみ）。
- 合図の入口は `UnknownKeyNotifier` と `jwks_refresh_trigger` に絞り、`fire` は公開しない。ゲートは判定確定後に
  inherent の `fire()` を直接呼ぶため `UnknownKeyEvent`（`kid`）は合図の経路へ届かない。`JwksFetcher::fetch` も
  引数を持たず、取得先を `kid` から決める余地が構造的にない。
- 停止後の合図は何もしない（`is_stopped` を見てから `notify_one`）。`Notify` はキューを持たないため「満杯」状態がない。
- `unknown_key_notifier` へ登録すると `NotifyThrottle` の後ろに並ぶ。doc で `jwks_refresh_trigger` を案内する。
- タスクの挙動は変えない。取得中に届いた合図は許可として残り、「取得開始 + 下限」に 1 回だけ追加取得する。

### 8.7 合図のまとめと取得回数の上限の根拠

**結論**: 取得は常に直列（同時に 1 件）。定期・合図・再試行のどれでも、取得の開始は
`next_allowed_at = 直前の取得の開始 + 下限` 以降に限る（単一の状態）。下限の間に届いた合図は `Notify` の許可 1 つに
まとまり、`next_allowed_at` で 1 回の取得になる。

**根拠**: すべての取得の開始時刻が互いに下限 L 以上離れているので、長さ T の半開区間に入る開始は高々 `ceil(T / L)` 回。
spec の式 `ceil(T / L) + 定期の取得回数` はこれ以上なので満たす（閉区間で数えると +1 だが定期の項の範囲に収まる）。
#897 のテストは半開区間で数え、この式と一致させる。**追従の遅延**: 時刻 s に合図が届くと開始は
`max(s, 直前の開始 + L) <= s + L`、反映は `+ 取得時間`（`fetch_timeout` 以内）。これが「下限 + 取得時間」の根拠で、
8.6 の推奨形が前提になる。

**周期**: 成功時の次回は `max(L, min(period, max-age))`、失敗時は L 後に再試行（開始時刻から引き直す）。起動直後に
1 回取得する（初期の鍵集合が空でも早く埋めるため）。**影響先**: #895・#896・#897。

### 8.8 失敗のときの扱い

**結論**: 取得関数の `Err`・時間切れ（`fetch_timeout`）・`KeySet::from_json` のエラー（文書長・件数・`kid` あたり 3 件・
`NoUsableKeys`・完全同一鍵の重複を含む）のいずれでも `SharedKeySet` を変えず、再試行は下限 L の間隔（8.7 の
`next_allowed_at` で揃う）。指数バックオフは、spec が「下限の間隔」と定めているため採らない。

反映は `SharedKeySet::set` のみで、ゲートと同じ JWK の検査（`KeySet::from_json`）を通った集合だけを反映する。明示の
`{"keys":[]}` は 2.3 節で正当な空集合と定めたため、**成功として反映し全件拒否になる**（発行者の明示の意思）。`kid` の
重複を全体の失敗にしたい場合は、取得関数側で検査を足す（`fandhe-auth.md` の回答と一致）。

**観測口**: 同期の観測 trait `RefreshObserver`（既定 no-op。「ブロックしない・panic しない」契約で `Diagnostics`・
`UnknownKeyNotifier` と同形）と、`RefreshHandle` から取れる状態のスナップショット（最後の成功時刻・連続失敗回数）の
両方を設ける。イベントに秘密も攻撃者入力も載せない。**影響先**: #895。

### 8.9 `max_stale` の判定の時点

**結論**: 起点は「最後に取得に成功した時刻」で、起動直後は「タスクの開始時刻」（初期の鍵集合は開始時点で新しいとみなす。
起点を持たないと初回失敗が永久に空にできず、フェイルクローズが働かない）。タスクは `last_success + max_stale` の期限
タイマーを他の待機と競合させ、**期限に達した時点で**（次の取得の結果を待たずに）`SharedKeySet::set(KeySet::empty())` を行う。
取得失敗の直後にも同じ判定を行う。空にすると既存の `Arc::ptr_eq` の契約で検証キャッシュも無効になり、キャッシュ経由の
受理が残らない。以後に成功すれば鍵集合は戻る。未設定（既定）なら空にしない。時刻はすべて `tokio::time::Instant`
（単調時刻）で測る。**影響先**: #895。

**実装時の決定（#895）**: 「最後に取得に成功した時刻」は取得の**開始時刻**とした（完了時刻より早く、フェイルクローズ寄りで、
仮想時間のテストで決定的になる）。`RefreshStatus::last_success` も同じ時刻を指す。

### 8.10 時刻を制御するテストの方針

`tokio::time::pause` / `#[tokio::test(start_paused = true)]` を使う。dev-dependencies の `tokio` に `test-util` と `rt`
（current_thread）を足す（`rt-multi-thread` では pause が使えない）。dev 専用なので 8.2 の通常依存の基準に影響しない。
`NotifyThrottle` は `SystemTime` のため pause では進まないが、8.6 の推奨形ではトリガーの経路が通らないので問題にならない。
取得関数の偽物（呼び出し回数のカウンタ、応答・失敗・遅延を台本で返す）で次を確かめる: 周期・`max-age` が短い場合・
下限での下からの抑え、失敗 3 種での保持と L 間隔での再試行、`max_stale`、停止、大量の合図のまとめ、回数の上限式。
割り当ては #895〜#897。

### 8.11 取得関数の実装者への契約

`JwksFetcher` の doc に次の文言を書く（#894 がそのまま転記し、#898 のガイドも一致させる）。

- 取得先の URL は構築時に固定し、`kid`・リクエスト由来の値から組み立てない（SSRF 対策。`kid` は攻撃者が決められる値）。
- HTTPS を使い、証明書の検証を無効にしない。
- リダイレクトは追わないか、同じオリジンに限る。
- 本文を読む量に上限を設ける（64 KiB を目安。crate 側の `from_json` 上限は解析の防御で、受信メモリの防御にならない）。
- 取得関数自身にも時間切れを設ける（crate 側の `fetch_timeout` との多層防御）。
- 認証情報・本文をログに出さない。
- future は任意の時点で drop されうる。

### 8.12 REQ-16 `jwks-refresh` 受け入れ基準と Phase 1 issue の対応表

| REQ-16 の基準 | 主に満たす issue | 補助する issue |
|---|---|---|
| 依存ゼロ（有効時は既存 `tokio` の時刻・同期のみ、無効時は依存・コードゼロ。検証は 8.2 の (a)〜(c)） | #894 | #897（レポート）、#898（dep-impact・文書） |
| 時刻制御テスト: 定期の取得・`max-age` が短い場合・下限による抑え | #895 | #894（設定の検査） |
| 時刻制御テスト: 失敗・時間切れ・解析失敗で保持、`max_stale` で空にする | #895 | — |
| 時刻制御テスト: 未知 `kid` を契機にした取り直し・合図のまとめ・回数の上限式 | #896（単体） | #897（ゲートを通した統合） |
| 事前公開なしの切替後、「下限 + 取得時間」以内に受理し始める | #897 | #896（8.6 の推奨形） |
| （横断）設計の記録 | #893 | — |
| （横断）取得関数の実装例・CHANGELOG・文書 | #898 | — |

### 8.13 セキュリティ考慮（OWASP Top 10）

- **A01**: フェイルクローズは維持する（未知 `kid` は即 401、`max_stale` 超過で全件拒否）。失敗時に鍵を保持する既定は
  spec の決定（可用性を優先）で、利用側が `max_stale` で切り替えられる。
- **A04（DoS）**: 合図は単一許可にまとめ、取得は直列で、回数を 8.7 の式で抑える。時間切れあり。値の上限で `Instant` の
  オーバーフローを防ぐ。通知はブロックしない。
- **A05**: 周期は必須、値は構築時に検査する。
- **A06**: 新しい crate・版を増やさない（8.2 (b)）。
- **A08**: ゲートと同じ JWK の検査を通った集合だけを反映する。
- **A09**: エラーとイベントに秘密・攻撃者入力を載せない。
- **A10（SSRF）**: URL は固定し、`kid` から組み立てない（8.11）。
- 本章は docs のみで、秘密情報・鍵・`.env` を含めない。

### 8.14 対応可否判定記録（#893 分）

`.claude/rules/feasibility-guardrail.md` に従う。#894〜#898 は着手時にそれぞれ 3 軸を判定する。

#### 判定区分

可

#### 該当カテゴリと判断根拠

なし

#### 要人間判断事項

なし

#### 代替案

なし

#### 3 軸判定結果

- 実施可能か: 可。受け入れ条件（各論点の決定と理由、対応表、依存の配線方針）は本章の存在で検証できる。
- 安全か: 可。docs のみで、内容は 8.13 のとおりフェイルクローズと OWASP に沿う。
- 影響範囲が許容内か: 可。影響は `docs/design/` の追記と索引・`CLAUDE.md` の 1 行に限られ、コード・Cargo・CI は変更しない。

### 8.15 スコープ外・見直しの条件

- `hub-wiring` は `plugin-jwt` を `jwt` として再エクスポートするが、`jwks-refresh` の feature 転送は #891 ツリーの範囲外。
  利用側は `fandhe-backend-plugin-jwt` に直接 feature を指定すれば feature 統合で有効になる。転送が必要になれば別 issue で判断する。
- HTTP クライアントの同梱（別の opt-in feature）は、取得関数を書くサービスが複数になり書き方の違いが問題になった時点で
  `fandhe-auth.md` の見直し条件に従う。
- `rt` を使う `spawn` 便宜関数は追加しない（8.2）。`plugin-boundary.md` 5.6 節への追記の要否は #898 で判断する。

#### 実装時の決定（#898）

- 取得関数の実装例は `docs/guide/jwks-refresh.md` に置いた。HTTP クライアントは利用側の依存でフレームワークの依存にできず、
  example にすると dev-dependency として lock と dep-audit の対象に入るため、説明用でコンパイル検証の対象外のコードとした
  （`docs/guide/README.md` の「サンプルコードの原則」の例外。フレームワーク API の正は doc test）。
- `plugin-boundary.md` 5.6.5 節には 1 項目だけ追記した（取り直しタスクはゲートの外で動き、Gate 型の契約を変えない）。
- `plugin-hub-wiring` の doc comment にある「JWKS 取得は責務外」への案内追加は別クレートの変更のため範囲外とした。

## 9. DPoP 結合のアクセストークンの検証（`dpop`、イシュー #900）

イシュー #900（ツリーのルートは #891、Phase 2 の親は #899）。根拠の仕様は
`docs/spec/04-requirements.md` の REQ-16「DPoP 結合のアクセストークンの検証（追加要件）」と
`docs/spec/requests/fandhe-auth.md` の「#23」節。本章は設計の記録のみでコード変更を含まない。
実装は #901〜#907 が担う。以下の数値と名前は推奨案で、最終承認は PR レビューで行う
（#903・#906 の計測結果で見直す余地を残す）。

`fandhe-auth` はエージェントに DPoP 結合の資格情報を発行する。下流が資格情報の写しを使えなく
する効果つきでそれを受け付けられるよう、`plugin-jwt` に opt-in の feature `dpop`（RFC 9449）を足す。

### 9.1 範囲と前提

- **結論**: `dpop` は既定で無効の opt-in feature とする。RFC 9449 の resource server 側の検査
  （proof の検証・`cnf.jkt` との結合・`jti` の再使用検知）だけを担う。`DPoP-Nonce` とインスタンス間の
  `jti` 共有は扱わない（9.13）。
- `RequestGate::check` の同期・I/O なしの契約（NFR-7）を保つ。proof の署名は要求ごとに検証し、
  キャッシュしない。アクセストークンの検証結果のキャッシュ（2.6 節）は維持する。
- feature が有効でも、`JwtAuthenticatorBuilder` に DPoP の設定を渡さなければ従来どおり Bearer だけの
  挙動で、`cnf` は見ない。**理由**: feature 統合（別 crate が `dpop` を有効にした場合）で、opt-in
  していないゲートの挙動が変わらないようにするため。
- feature を無効にした下流は、DPoP 結合のトークンも bearer として受理する（従来の挙動）。この点は doc に注意として明記する。
- **影響先**: #904・#907。

### 9.2 依存と feature の配線

- **結論**: `dpop = []` とし `dep:` は使わない。新規依存・新しい版はない（受け入れ条件 (c)）。
  `ring`・`base64`・`serde`・`serde_json` は既に通常の依存で、次の API で足りる。
  - `ring::digest::{digest, SHA256}`: `ath`・サムプリント・`jti` のキー。
  - `ring::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED}`: proof の署名検証。
  - `base64` の `URL_SAFE_NO_PAD`、std の `Mutex`・`HashMap`・`BinaryHeap`。
- 「無効時ゼロ」の判定は 8.2 節とは異なる。optional 依存がないので依存の残留は自明に起きず、確認するのは次の 3 点。
  - `cfg(feature = "dpop")` のモジュール（例: `dpop`）と関連シンボルが無効時にコンパイルされない。
  - `Cargo.lock` の `[[package]]` が増えない。
  - feature なし・`dpop`・全 feature の 3 構成で `cargo build` と `cargo doc` が通る。
- 2.7 節の依存の解釈を引き継ぐ。
- **影響先**: #901・#904・#907。

### 9.3 方式と構築の API

- **結論**: `#[non_exhaustive] enum DpopMode { Required, AllowUnboundBearer }` とし、`Default` は実装しない。
  構築子は `DpopConfig::new(mode, origin: PublicOrigin, algs: AllowedAlgorithms)` で、3 引数すべて必須
  （既定値を持たない）。proof の許可 `alg` も 2.2 節の「検証が抜けた構成を作れない」に揃えて必須にする。
- 配線は `JwtAuthenticatorBuilder::dpop(config)`（`cfg(feature = "dpop")`）。`JwtGate` 側と `authenticate` の
  シグネチャは変えない。メソッドとパスは `RequestHead` から取る。
- **1 要求 1 回の契約**: DPoP 有効時の `authenticate` は `jti` 登録を伴うため、同一要求内で 2 回呼ぶと 2 回目は
  replay 扱いで失敗する。`authenticate` を呼ぶのはゲート 1 箇所だけにし、ゲートが得た検証済みの `Claims` を
  `GateContext` 等の後段への受け渡し経路でハンドラへ渡す。ハンドラは再検証せず受け取った値を読むだけにする
  （受け渡しの具体形は #901・#904 で決める。`authenticate` のシグネチャは変えない）。
- 方式と公開オリジンが欠けた構成を作れないことは、#904 で `compile_fail` の doc test により示す。
- **理由**: spec の「構築時に明示して選ばせ、既定は持たない」に従う。`non_exhaustive` は将来の nonce 方式の余地。
- **影響先**: #901・#904。

### 9.4 proof の解析と鍵の検査

- **入力の上限**: `DPoP` ヘッダはちょうど 1 つ（0 個も 2 個以上も拒否。名前は大文字小文字を区別しない）。
  proof 長は `MAX_TOKEN_LEN`（8192 B）と同じ値を上限とする（RSA 8192 bit の `jwk` も収まる）。
- **ヘッダ**: `typ` は `dpop+jwt`（`application/` 接頭辞の有無・ASCII 大文字小文字は 2.2 節の `at+jwt` と同じ流儀）。
  `alg` は proof の許可リスト内。`crit` があれば拒否。`jwk` は必須。`jku`・`x5u`・`x5c`・`kid` は読まない。
- **`jwk`**: `kty` は `EC`（`crv` = `P-256`、`x`・`y` は 32 B ちょうど）か `RSA`。秘密鍵の要素
  （`d`/`p`/`q`/`dp`/`dq`/`qi`/`oth`/`k`）と `oct` は拒否。鍵の種別とヘッダ `alg` が一致しなければ拒否。
  `jwk` に `alg` があればヘッダ `alg` との一致を要する。
- **RSA の最小鍵長**は 2048 bit。根拠は `RSA_PKCS1_2048_8192_SHA256` が検証時に 2048〜8192 bit を強制する点で、
  解析時にも `n` を 256〜1024 B として先に弾く。
- **実装方針**: `jwks::classify` から鍵材料の復号（秘密鍵要素の拒否・kty/crv と alg の一致・座標長・`n`/`e` の上限）を
  内部の共有関数に切り出す。JWKS 経路は `kid`・`alg` 必須で包み、proof 経路は不要で呼ぶ。`signature_ok` は
  `(Algorithm, &KeyMaterial)` を受ける形へ寄せる。内部のリファクタで公開 API は変えない。
- **順序**: 署名を検証するまでクレームは解析しない（3 章と同じ）。必須クレームは `jti`・`htm`・`htu`・`iat`・`ath`。
  `jti` は 1〜256 B。それ以外（`nonce` 等）は読まない。
- **sans-IO の公開面**（#902 の fuzz の入口）: 純関数
  `verify_proof(proof: &str, ctx: &ProofContext<'_>, now_unix: u64) -> Result<VerifiedProof, DpopError>`。
  `ProofContext` は method・正規化済みの期待 `htu`・アクセストークン・許可 `alg`・窓を持ち、
  `VerifiedProof` はサムプリント・`jti`・`iat` を持つ。
- `DpopError` は `#[non_exhaustive]`・`Copy` で文字列を持たず、`Display` は固定文言（`VerifyError` と同方針）。
- **影響先**: #901・#902。

### 9.5 公開オリジンと `htu`

- **結論**: `PublicOrigin::new(&str) -> Result<PublicOrigin, OriginError>`。
  - scheme は `https`（`http` も許すが doc で https を推奨）。
  - host は ASCII のみ（IDN は punycode で渡させる）で小文字に正規化。IPv6 は角括弧の形。
  - port は 1〜65535 で、既定ポート（443/80）は省いた正規形にする。
  - userinfo・パス（空と `/` 以外）・クエリ・フラグメントがあれば拒否。長さに上限を設ける。
  - `Host` と `X-Forwarded-*` は使わない（spec と #20 の保留に従う）。
- 期待する `htu` は「正規化済みオリジン + `RequestHead::path()`」。target が origin-form（`/` 始まり）以外
  （absolute-form・`*`）なら proof を拒否する（フェイルクローズ）。
- proof の `htu` は絶対 URI として解析し、クエリとフラグメントを除く。RFC 3986 6.2.2・6.2.3 のうち次だけ正規化する:
  scheme・host の小文字化、既定ポートの省略、空パスを `/` にする、パーセントエンコードの 16 進を大文字にそろえる。
  パーセントデコードはしない。ドットセグメントと不正な `%` は拒否。その上で同じ正規化をかけたリクエスト側とバイト一致で比べる。
- `htm` は `method()` と大文字小文字を区別して完全一致で比べる。
- **理由**: デコードと比較の差を突く迂回の余地をなくし、フェイルクローズにする。前段プロキシがパス接頭辞を剥がす構成は対象外（9.13）。
- **影響先**: #901・#904。

### 9.6 `iat` の窓と `jti` の保持

- **`iat` の窓**: 受理範囲は `now − past ≤ iat ≤ now + future`。推奨値は `past` が既定 60 秒・上限 300 秒
  （`MAX_CLOCK_SKEW` と同じ）・0 は拒否、`future` が既定 5 秒・上限 60 秒。すべて構築時に検査し
  `DpopConfigError`（`#[non_exhaustive]`）で返す。窓を短くするほど写しを使い回せる余地が減る。
- **`jti` の保持**:
  - 単位は (サムプリント, `jti`)。保持キーは SHA-256(jkt ‖ jti) の 32 B で、生の `jti` は持たない。
  - 期限は `iat + past`。期限切れは時刻順に取り除く（例: `BinaryHeap` + `HashMap`）。
  - 容量は既定（例: 131,072）と設定可能な上限（例: 4,194,304）を持つ。1 項目あたりのメモリ見積りと、
    容量の目安（ピーク時の DPoP の rps × (past + future)）を doc に載せる。
  - 期限切れを掃いても満杯なら新しい proof を拒否する（フェイルクローズ。`ReplayStoreFull` → `invalid_dpop_proof`）。
  - 1 つの `JwtAuthenticator`（clone で共有）につき 1 つ持つ。
- **ロック**: `Mutex` を短時間だけ持ち、保持中に `.await`・I/O・署名検証をしない。poison は `into_inner` で回復（`TokenCache` と同じ規律）。
- **検査の順序**: `jti` の確認と登録は、他のすべての検査（トークン検証・proof 署名・`ath`/`htm`/`htu`/`iat`・`cnf.jkt` 一致）が
  成功した**最後**に、同じロック区間で原子的に行う。署名のない入力で保持を埋められないようにするため（A04）。失敗した要求は登録しない。
- **同一要求内の再 `authenticate` は禁止**: 登録は 1 要求につきゲートの `authenticate` 1 回のみ。同じ要求で再度呼ぶと
  replay と判定されて失敗する。ハンドラが claims を必要とする場合は 9.3 節の受け渡し経路で読む（再検証しない）。
- **既知の限界の doc 文言案**: 「`jti` の一度だけの検査はゲート（`JwtAuthenticator`）ごと・プロセスごとで、複数インスタンス間では
  共有しない。写した proof を `iat` の窓の中で別インスタンスへ送る使い回しは防げない。窓を短くして余地を限ること。
  アクセストークンだけの写しは、鍵を持たない者には proof を作れないため使えない」。
- **影響先**: #903・#904。

### 9.7 サムプリントと `cnf.jkt`

- **サムプリント**: RFC 7638 に従い必須メンバーだけを辞書順に並べた空白なしの JSON（EC は `{"crv","kty","x","y"}`、
  RSA は `{"e","kty","n"}`）の SHA-256 を base64url（パディングなし）にする。値は復号済みバイト列を正規形で
  再符号化する（EC 座標は 32 B、RSA の `n`/`e` は先頭 0 を除く）。正規形でない入力は拒否。値は base64url 文字のみなので JSON の手組みでも安全。
- **`cnf`**: 解析は `cfg(feature = "dpop")` かつ **`JwtAuthenticatorBuilder` に DPoP の設定が登録されている場合だけ**行う（9.1 節に揃える。
  feature 無効時のコードはゼロ）。DPoP 設定が未登録の認証器では `cnf` を解析せず、不正な `cnf` があっても従来どおり受理する
  （`Malformed` 判定も行わない。opt-in していないゲートの挙動を変えないため）。以下の `Malformed` 判定と `jkt` なしの拒否は、
  DPoP 設定の登録時のみ適用する。`Claims` に `cnf_jkt()` を足してキャッシュのヒット時も結合を検査できるようにする。
  `cnf` がオブジェクトでない・`jkt` が文字列でない・43 文字の base64url でないものは `Malformed`。
  `cnf` があっても `jkt` がないもの（`x5t#S256` など別方式の結合）は、どちらの方式でも拒否する。検証できない結合を bearer に格下げしないため。
- 比較は完全一致。`jkt` は秘密ではないので定数時間比較は不要（その旨を doc に書く）。
- **影響先**: #901・#904。

### 9.8 ゲートでの判定の流れと方式

- `Authorization` はちょうど 1 つ。スキームの解析は `bearer_token` を拡張した内部関数で `(Scheme::{Bearer, Dpop}, token)` を返す。
  既存 `bearer_token` の公開挙動は変えない。
- 順序: (1) スキームとトークンの取り出し、(2) アクセストークン検証（キャッシュ含む）、(3) 方式とスキームによる振り分け、
  (4) proof 検証（毎回）、(5) `cnf.jkt` とサムプリントの比較、(6) `jti` 登録。
- 判定表:

| スキーム | トークンの `cnf.jkt` | `Required` | `AllowUnboundBearer` |
|---|---|---|---|
| `Bearer` | あり | 拒否（格下げ防止、RFC 9449 7.2） | 拒否 |
| `Bearer` | なし | 拒否 | 従来どおり受理 |
| `DPoP` | なし | 拒否 | 拒否 |
| `DPoP` | あり | proof の全検査を通れば受理 | 同左 |

- 手順 (6) の `jti` 登録は 1 要求につき 1 回だけ。同一要求内で `authenticate` を再実行しない（replay 扱いで失敗する）。
  ゲート通過後にハンドラが claims を読む場合は、ゲートが検証済みの `Claims` を後段へ渡す（9.3 節）。
- `VerifyError` に `cfg(feature = "dpop")` の変種 `Dpop(DpopError)` を足す。非破壊の追加で、`#[non_exhaustive]` のため
  下流の match は既存のまま 401 に倒れる。
- **影響先**: #904。

### 9.9 拒否の応答

- 状態は 401。本文は固定 JSON（`{"error":"invalid_token"}` か `{"error":"invalid_dpop_proof"}`）。
  `algs` は構築時の許可 `alg` から空白区切り・固定順で作り、リクエスト由来の値は入れない。`error_description` は付けない。
- `error` の対応:
  - トークン側（署名・`exp`・`iss` など、Bearer への格下げ、結合のないトークン、`cnf` の不正）→ `invalid_token`。
  - proof 側（`DPoP` ヘッダの欠落・重複、解析、`typ`/`alg`、`jwk`、署名、`htm`/`htu`/`iat`/`ath`、`jkt` 不一致、`jti` 再使用、保持が満杯）→ `invalid_dpop_proof`。
    `jkt` 不一致を proof 側に置くのは、提示された鍵が結合した鍵と違うという proof の問題だから。
- `WWW-Authenticate`: `Required` は `DPoP` の 1 行（資格情報なしなら `DPoP algs="ES256 RS256"`、失敗なら `DPoP error="…", algs="…"`）。
  `AllowUnboundBearer` は `DPoP` と `Bearer` の 2 行（`with_header` は重ねて足せる）。`error` は使われたスキームの行にだけ付け、
  格下げのときは `DPoP` の行に `error="invalid_token"` を付ける。
- DPoP を設定していない構成の応答（2.4 節の `Bearer` 1 行）は変えない。
- #904 では、`gate.rs` の `assert_full` が 1 行を前提にしているため、DPoP 設定経路のテストを分ける必要がある。
- **影響先**: #904・#905。

### 9.10 `plugin-hub-wiring` から使う形

- **結論**: hub-wiring に `dpop = ["fandhe-backend-plugin-jwt/dpop"]` を足して転送する。無効時は依存もコードもゼロ。
- `TenantGateConfig` に `Authenticator`（`Authenticator::from_jwt(JwtAuthenticator::<HubClaims>::builder(..).dpop(cfg).build())`）から
  作る構築子を足す（非破壊の追加）。
- DPoP を設定したときだけ `TenantGate` の 401 に 9.9 と同じ `WWW-Authenticate` を付ける。チャレンジの組み立ては
  `plugin-jwt` 側の公開補助関数（認証器と `VerifyError` から 401 の `Response` を作るもの）に寄せ、hub で二重実装しない。
- `org_id` の 403 判定は DPoP 検査の後。REQ-9 の既存応答は変えない。
- **8.15 節との違い**: `jwks-refresh` の転送は範囲外としたが、`dpop` は spec が「REQ-9 の hub 共通配線からも同じ設定で使える」ことを
  求め、hub 側にも cfg 付きコード（チャレンジ）が要るため、転送を範囲内とする。
- **影響先**: #905。

### 9.11 REQ-16 `dpop` 受け入れ基準と Phase 2 issue の対応表

| 基準の細目 | 主に満たす issue | 補助する issue |
|---|---|---|
| `typ`・`alg` の不正、秘密鍵要素を含む `jwk`、署名の不正、`ath`・`htm`・`htu` の不一致、窓外の `iat` の拒否、RS256・ES256 の受理 | #901 | #907（レポート） |
| 再使用の `jti` の拒否（保持の上限を含む） | #903 | #904 |
| `Bearer` での DPoP 結合トークン、`DPoP` ヘッダの欠落・重複、`cnf.jkt` 不一致の拒否、401 と `WWW-Authenticate: DPoP` | #904 | #905（hub 経路） |
| hub 共通配線からの利用と REQ-9 の維持 | #905 | — |
| proof 解析部の fuzz（NFR-11） | #902 | — |
| 性能への影響の計測（NFR-6） | #906 | — |
| 新規依存なし・無効時ゼロ（9.2） | #904（Cargo） | #907（dep-impact・レポート） |
| （横断）設計の記録 | #900 | — |
| （横断）CHANGELOG・受け入れレポート・文書 | #907 | — |

### 9.12 セキュリティ考慮（OWASP Top 10）

- **A01**: 両方式で Bearer への格下げを拒否する。`jkt` のない `cnf` を拒否する。全異常は 401 のフェイルクローズ。
- **A02**: proof の署名は `ring` で毎回検証する。RSA は 2048 bit 以上、EC は P-256 のみで点の検証は ring が行う。
  秘密鍵要素と `oct` は拒否。アクセストークンでは `jwk` ヘッダを無視し（5 章）、proof では `jwk` だけを検証鍵とする。
- **A03/A09**: チャレンジと本文は固定文字列か構築時設定から作った値だけ。エラー型は文字列を持たない。`jti`・トークン・`jwk` をログに出さない。
- **A04**: proof 長・`jti` 長・`htu` 長に上限を設ける。保持が満杯なら拒否し、`jti` の登録は最後に行う。ロック保持は短くする。
- **A05**: 方式に既定を持たせず、公開オリジンを必須にし、窓の値は構築時に検査する。
- **A06**: 新しい crate も版も増えない。
- **A07**: proof の再使用は窓とゲートごとの `jti` で防ぎ、インスタンス間の限界は明記する（9.6）。
- 本章は docs のみで、秘密情報・鍵・`.env` を含めない。

### 9.13 対応可否判定記録（#900 分）と、スコープ外・見直しの条件

`.claude/rules/feasibility-guardrail.md` に従う。#901〜#907 は着手時にそれぞれ 3 軸を判定する。

#### 判定区分

可

#### 該当カテゴリと判断根拠

なし

#### 要人間判断事項

なし

#### 代替案

なし

#### 3 軸判定結果

##### 実施可能か

可。受け入れ条件（各論点の決定と理由、対応表、依存の配線方針）は本章の存在で検証できる。

##### 安全か

可。docs のみで、内容は 9.12 のとおりフェイルクローズと OWASP に沿う。

##### 影響範囲が許容内か

可。影響は `docs/design/` の追記と索引・`CLAUDE.md` の 1 行に限られ、コード・Cargo・CI は変更しない。

#### スコープ外・見直しの条件

見直しの条件は `docs/spec/requests/fandhe-auth.md` の「見直しの条件」表を引用元とする。

- `DPoP-Nonce` は扱わない。
- インスタンス間で共有する `jti` の保持（利用側が渡す trait）は、ゲートの同期・I/O なしの契約と両立させることを前提に再検討する。
- パス接頭辞を剥がすプロキシ構成と、信頼するプロキシからの転送ヘッダは扱わない。
- PS256・EdDSA など proof の方式の追加は扱わない。
- `plugin-boundary.md` 5.6 節への追記と利用者向けガイドは #907 が担う。

#### 実装時の決定（#901）

9.4〜9.7 節が開いたままにした点を、`dpop` feature の純関数部分（`dpop::verify_proof`・
`jwk_thumbprint`）の実装で次のとおり決めた。

- **置き場所**: `IatWindow`・`DpopConfigError`（9.6 節）と `PublicOrigin`・`ExpectedHtu`（9.5 節）
  は #901 で `dpop` モジュールに置いた。#904 はこれらを拡張する（別の型を作らない）。
  `TokenType` に proof 用の変種は足さない（`JwtPolicy` で proof 用の種別を作れてしまうため）。
  proof の許可 `alg` は既存の `AllowedAlgorithms` を使い回す。
- **ゲート側の関心は #904**: 検証済みの `Claims` を後段へ渡す形、`Authorization: DPoP` の解析、
  `cnf.jkt` との比較、401 と `WWW-Authenticate` は #904 が決める。`verify_proof` は
  `VerifiedProof`（サムプリント・`jti`・`iat`）だけを返す。
- **`ExpectedHtu`**: `PublicOrigin::expected_htu(path)` で要求ごとに作る。`path` は `/` で始まり
  `?`・`#` を含まないこと。absolute-form・`*` は `DpopError::InvalidRequestTarget`。
  proof の `htu` は同じ規則（scheme・host の小文字化、既定ポートの省略、`%XX` の大文字化、
  クエリ・フラグメントの除去）で正規化して完全一致を比べ、解析できない値は `UriMismatch`。
- **ドットセグメント**: `.`・`..` に加えて `%2E` を含む表記（`%2e%2E` など）も拒否する。デコード
  差を突く迂回を防ぐ保守側の解釈。不正な `%`・制御文字・空白・非 ASCII・`\` も拒否する。
- **上限**: `MAX_PROOF_LEN` = 8192（`MAX_TOKEN_LEN` と同じ）、`MAX_JTI_LEN` = 256、
  `MAX_HTU_LEN` = 4096、`MAX_ORIGIN_LEN` = 512。いずれも復号・解析の前に判定する。
- **`jwk` の検査**: 秘密鍵の要素（`d`・`p`・`q`・`dp`・`dq`・`qi`・`oth`・`k`）は
  `PrivateKeyMaterial`、`kty` は `RSA`・`EC`（`crv` は `P-256`）のみ。ヘッダの `alg` または
  `jwk.alg` と食い違えば `KeyAlgorithmMismatch`。`use` があって `sig` 以外なら拒否する。
  `kid`・`key_ops`・`x5c` は読まない。RSA の `n` は 256〜1024 バイト（2048 bit 以上）、`e` は
  1〜8 バイトで、`n`・`e` の先頭が 0 の表現は拒否する（9.7 節の正規形）。EC の座標は 32 バイト固定。
  `jwk` はオブジェクトだけを受け、キーの重複は `Malformed`。
- **`typ`**: `dpop+jwt` と `application/dpop+jwt` を ASCII の大文字小文字を無視して受理する。
- **`DpopError` の変種**: 文字列を持たず `Copy`。#903 の `jti` 関連の変種（再使用・保持上限）と
  #904 の変種は `#[non_exhaustive]` のため後から足せる。
- **新規依存なし**: `Cargo.lock` に差分はない。`signature_ok` は `KeyMaterial` を受ける形にして
  JWT 検証と共有した（挙動は変えていない）。

#### 実装時の決定（#903）

9.6 節の `jti` の保持を `dpop::ReplayStore`（`crates/plugin-jwt/src/dpop/replay.rs`）で実装した。
ゲート配線（`JwtAuthenticator` への保持と 401 への対応づけ）は #904 が担う。

- **入口は `register` だけ**: `register(&VerifiedProof, now_unix)` が確認と登録を同じロック区間で
  原子的に行う。`contains` と `insert` は公開しない（TOCTOU を作らない）。`VerifiedProof` だけを
  受けるので、署名のない入力で保持を埋められない。
- **キー**: SHA-256(サムプリント ‖ `jti`) の 32 B。生の `jti` は保持しない。サムプリントは常に
  43 文字の固定長なので区切りなしの連結でも曖昧さはない。ハッシュはロックの外で計算する。
  `HashMap` は std の既定（SipHash・ランダム鍵）を使い、要求由来の値に対する HashDoS 耐性を保つ
  （`rustc-hash` は plugin-jwt に足さない。新規依存なし）。
- **期限と掃除の境界**: 期限は `iat + past`。`verify_proof` は `iat + past >= now` の間受理するので、
  掃除は `期限 < now` の項目だけを除く（`<=` にすると同じ秒に 2 回受理されうる）。掃除は
  `BinaryHeap`（期限の昇順）の先頭から行い、`map` と 1 対 1 に保つ。償却では 1 項目あたり
  O(log n) だが、長い無通信の後の最初の `register` だけ取り出しが多くなる（#906 の計測で見直す）。
- **満杯**: 掃除の後に判定するので、期限切れで埋まった偽の満杯は起こさない。満杯なら
  `DpopError::ReplayStoreFull`、再使用なら `DpopError::Replayed`。拒否した proof は登録しない。
- **容量**: 既定 `DEFAULT_REPLAY_CAPACITY` = 131,072、上限 `MAX_REPLAY_CAPACITY` = 4,194,304。
  0 と上限超は構築時に `DpopConfigError` で拒否。上限分は事前に確保しない。保持数が減っても
  確保済みの容量は縮めない（高水位）。
- **契約**: 構築時の `IatWindow` と `register` の `now_unix` は、同じ要求で `verify_proof` に渡した
  ものと揃える（#904 は 1 つの `DpopConfig` から両方を作る）。
- **ロック規律**: ロック区間は `HashMap`/`BinaryHeap` の操作だけ。非同期待機・I/O・署名検証・
  ハッシュ計算を含まず、モジュールは tokio に依存しない。poison は `TokenCache` と同じく
  `into_inner` で回復する。
- **既知の限界**: ゲートごと・プロセスごとの保持で、複数インスタンス間では共有しない（9.13 節）。

#### 実装時の決定（#904）

9.3・9.7〜9.9 節の配線を `JwtAuthenticatorBuilder::dpop(DpopConfig)` と `JwtAuthenticator` で
実装した（`crates/plugin-jwt/src/{auth,gate,verify}.rs`、設定型は `dpop/config.rs`）。
hub-wiring への転送は #905、計測は #906、CHANGELOG・ガイド・受け入れレポートは #907 が担う。

- **設定型**: `DpopMode`（`Required` / `AllowUnboundBearer`、`#[non_exhaustive]`、`Default` なし）と
  `DpopConfig::new(mode, origin, algs)`（3 引数すべて必須）。`with_iat_window`・
  `with_replay_capacity`（0 と上限超は構築時に拒否）は任意。省いた構築がコンパイルできないことと
  `DpopMode::default()` が存在しないことは `compile_fail` の doc test で固定した。`build()` は
  失敗しないまま。
- **`cnf` の取り込み**: `RawClaims.cnf` は失敗しない寛容なデシリアライザで `Absent` / `Jkt` /
  `Unusable` に分類する。DPoP を設定しない認証器と `verify()` は値を見ないため、不正な `cnf` を含む
  トークンも従来どおり受理する（9.1 節の注意）。設定した認証器は `Unusable`・43 文字の base64url で
  ない `jkt` を `Malformed`（`invalid_token`）で拒否する。キャッシュには `Claims` ごと載るので、
  ヒット時も結合の検査が働く。公開アクセサは `Claims::cnf_jkt`。
- **判定**: 9.8 節の表のとおり。`now` はトークン検証・`verify_proof`・`register` で同じ値を使う。
  サムプリントの比較は公開値同士なので定数時間比較にしない。`DPoP` ヘッダの個数は `DPoP`
  スキームを採ったときだけ検査し、`AllowUnboundBearer` の `Bearer` + 結合なしでは余分な `DPoP`
  ヘッダを無視する。
- **応答**: `JwtAuthenticator::unauthorized_response(head, err)` に集約した（`JwtGate::check` と
  #905 の hub が共有）。シグネチャは設計時の案に `head` を足した。`AllowUnboundBearer` の併記で
  `error` をどちらの行に付けるかを、使われたスキームから決めるため。`algs` は構築時に正準順
  `ES256 RS256` で 1 回だけ作る（`AllowedAlgorithms::iter` は挿入順のため）。`error` は
  `BoundTokenAsBearer`・`UnboundToken` が `invalid_token`、他の `Dpop(_)` が `invalid_dpop_proof`。
  DPoP を設定していない場合の応答は従来とバイト単位で同じ。
- **ハンドラへの受け渡し**: core には要求ごとの受け渡し領域がなく、`authenticate` を再度呼ぶと
  `jti` の 2 回目の登録で `Replayed` になる。そこで `authenticate_after_gate(_at)` を足した。
  同じ検査を行い、`jti` を**ゲート用とは別のハンドラ用の保持器**へ登録する。共有履歴の
  「登録済み」は、その要求がゲートを通った証明にならない（ゲート付きとゲートなしの経路が同じ
  認証器を共有するとき、また後続ゲートの拒否で登録だけが残るとき、共有履歴を通過の証明にすると
  再送が通ってしまう）ため、ゲートの登録を参照しない。ゲートの経路・ハンドラの経路のそれぞれで
  同じ proof は一度しか受理されず、ゲートなしの経路でも新しい proof は通常どおり検査して受理する。
  1 要求のハンドラでは 1 回だけ呼ぶ。費用は 1 要求あたり proof の署名検証が 2 回、保持器が 2 つ
  （#906 で計測）。ゲートとハンドラの間で `iat` の窓の端を越えるとハンドラ側で拒否されうる。
  DPoP 未設定の認証器では `authenticate` と同じ。
- **新規依存なし**: `Cargo.lock` に差分はない。`signature_ok` は `KeyMaterial` を受ける形にして
  JWT 検証と共有した（挙動は変えていない）。

#### 実装時の決定（#903）

9.6 節の `jti` の保持を `dpop::ReplayStore`（`crates/plugin-jwt/src/dpop/replay.rs`）で実装した。
ゲート配線（`JwtAuthenticator` への保持と 401 への対応づけ）は #904 が担う。

- **入口は `register` だけ**: `register(&VerifiedProof, now_unix)` が確認と登録を同じロック区間で
  原子的に行う。`contains` と `insert` は公開しない（TOCTOU を作らない）。`VerifiedProof` だけを
  受けるので、署名のない入力で保持を埋められない。
- **キー**: SHA-256(サムプリント ‖ `jti`) の 32 B。生の `jti` は保持しない。サムプリントは常に
  43 文字の固定長なので区切りなしの連結でも曖昧さはない。ハッシュはロックの外で計算する。
  `HashMap` は std の既定（SipHash・ランダム鍵）を使い、要求由来の値に対する HashDoS 耐性を保つ
  （`rustc-hash` は plugin-jwt に足さない。新規依存なし）。
- **期限と掃除の境界**: 期限は `iat + past`。`verify_proof` は `iat + past >= now` の間受理するので、
  掃除は `期限 < now` の項目だけを除く（`<=` にすると同じ秒に 2 回受理されうる）。掃除は
  `BinaryHeap`（期限の昇順）の先頭から行い、`map` と 1 対 1 に保つ。償却では 1 項目あたり
  O(log n) だが、長い無通信の後の最初の `register` だけ取り出しが多くなる（#906 の計測で見直す）。
- **満杯**: 掃除の後に判定するので、期限切れで埋まった偽の満杯は起こさない。満杯なら
  `DpopError::ReplayStoreFull`、再使用なら `DpopError::Replayed`。拒否した proof は登録しない。
- **容量**: 既定 `DEFAULT_REPLAY_CAPACITY` = 131,072、上限 `MAX_REPLAY_CAPACITY` = 4,194,304。
  0 と上限超は構築時に `DpopConfigError` で拒否。上限分は事前に確保しない。保持数が減っても
  確保済みの容量は縮めない（高水位）。
- **契約**: 構築時の `IatWindow` と `register` の `now_unix` は、同じ要求で `verify_proof` に渡した
  ものと揃える（#904 は 1 つの `DpopConfig` から両方を作る）。
- **ロック規律**: ロック区間は `HashMap`/`BinaryHeap` の操作だけ。非同期待機・I/O・署名検証・
  ハッシュ計算を含まず、モジュールは tokio に依存しない。poison は `TokenCache` と同じく
  `into_inner` で回復する。
- **既知の限界**: ゲートごと・プロセスごとの保持で、複数インスタンス間では共有しない（9.13 節）。

#### 実装時の決定（#904）

9.3・9.7〜9.9 節の配線を `JwtAuthenticatorBuilder::dpop(DpopConfig)` と `JwtAuthenticator` で
実装した（`crates/plugin-jwt/src/{auth,gate,verify}.rs`、設定型は `dpop/config.rs`）。
hub-wiring への転送は #905、計測は #906、CHANGELOG・ガイド・受け入れレポートは #907 が担う。

- **設定型**: `DpopMode`（`Required` / `AllowUnboundBearer`、`#[non_exhaustive]`、`Default` なし）と
  `DpopConfig::new(mode, origin, algs)`（3 引数すべて必須）。`with_iat_window`・
  `with_replay_capacity`（0 と上限超は構築時に拒否）は任意。省いた構築がコンパイルできないことと
  `DpopMode::default()` が存在しないことは `compile_fail` の doc test で固定した。`build()` は
  失敗しないまま。
- **`cnf` の取り込み**: `RawClaims.cnf` は失敗しない寛容なデシリアライザで `Absent` / `Jkt` /
  `Unusable` に分類する。DPoP を設定しない認証器と `verify()` は値を見ないため、不正な `cnf` を含む
  トークンも従来どおり受理する（9.1 節の注意）。設定した認証器は `Unusable`・43 文字の base64url で
  ない `jkt` を `Malformed`（`invalid_token`）で拒否する。キャッシュには `Claims` ごと載るので、
  ヒット時も結合の検査が働く。公開アクセサは `Claims::cnf_jkt`。
- **判定**: 9.8 節の表のとおり。`now` はトークン検証・`verify_proof`・`register` で同じ値を使う。
  サムプリントの比較は公開値同士なので定数時間比較にしない。`DPoP` ヘッダの個数は `DPoP`
  スキームを採ったときだけ検査し、`AllowUnboundBearer` の `Bearer` + 結合なしでは余分な `DPoP`
  ヘッダを無視する。
- **応答**: `JwtAuthenticator::unauthorized_response(head, err)` に集約した（`JwtGate::check` と
  #905 の hub が共有）。シグネチャは設計時の案に `head` を足した。`AllowUnboundBearer` の併記で
  `error` をどちらの行に付けるかを、使われたスキームから決めるため。`algs` は構築時に正準順
  `ES256 RS256` で 1 回だけ作る（`AllowedAlgorithms::iter` は挿入順のため）。`error` は
  `BoundTokenAsBearer`・`UnboundToken` が `invalid_token`、他の `Dpop(_)` が `invalid_dpop_proof`。
  DPoP を設定していない場合の応答は従来とバイト単位で同じ。
- **ハンドラへの受け渡し**: core には要求ごとの受け渡し領域がなく、`authenticate` を再度呼ぶと
  `jti` の 2 回目の登録で `Replayed` になる。そこで `authenticate_after_gate(_at)` を足した。
  同じ検査を行い、`register` の代わりに crate 内の非公開 `ReplayStore::claim` で
  「ゲートが登録済みで、まだ受け取られていないこと」を要求し、同じロック区間で受け取り済みに印を
  付ける（proof 1 つにつき一度だけ成功）。未登録・受け取り済みは `DpopError::NotAdmitted`
  （フェイルクローズ）。core に要求ごとの通過証明を運ぶ領域がないため、共有履歴の「登録済み」
  だけでは認証成功にせず、一度きりの受け取りにすることで、ゲートが受理した proof をゲートを通らない
  経路へ再送しても通らないようにした。`claim` は非公開で、9.6 節の
  「`contains` と `insert` を分けた公開の入口を持たない」契約を保つ。1 要求のハンドラでは 1 回だけ呼ぶ。費用は 1 要求あたり proof の
  署名検証が 2 回（#906 で計測）。ゲートとハンドラの間で `iat` の窓の端を越えるとハンドラ側で
  拒否されうる。DPoP 未設定の認証器では `authenticate` と同じ。
- **新規依存なし**: `Cargo.lock` に差分はない。`Lookup::Hit` は `Claims` が `cnf` で大きくなったため
  `Box` 化した。
