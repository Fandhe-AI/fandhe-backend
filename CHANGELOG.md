# Changelog

公開対象 15 クレート（`fandhe-backend-http` / `fandhe-backend-routes` / `fandhe-backend-core` /
`fandhe-backend-plugin-*` 12 種。0.5.0 で `fandhe-backend-plugin-jwt` と `fandhe-backend-plugin-csrf` が加わり、0.4.2 以前は 13 クレート）は lockstep（同一バージョン一斉更新）で運用する
（詳細は [`docs/design/crates-io-release.md`](docs/design/crates-io-release.md) 7 節）。
恒久非公開クレート（`axum-ref` / `ws-load-client` / `docs-site`）はこの一覧に含めない。

## [Unreleased]

### Added

- `fandhe-backend-plugin-websocket`: `WsOpenContext::extensions()` / `WsConnContext::extensions()` を
  追加しました（イシュー [#934](https://github.com/Fandhe-AI/fandhe-backend/issues/934)、
  実 TCP の統合テストと文書は [#935](https://github.com/Fandhe-AI/fandhe-backend/issues/935)）。
  `RequestGate` が入れた検証結果を `on_open`・`on_message_with_ctx`・`on_close` から読み取り専用で
  参照できます（ハンドシェイクのフックは従来どおり `WsHandshakeContext::head().extensions()`）。
  `Debug` は値を出しません。ゲートが拒否した upgrade は 101 にもハンドラにも届きません。
  **限界**: 値はセッションの間ずっと保持され、JWT の `exp` はハンドシェイク時点でしか検証しません。
  コアの `Server::max_connection_lifetime` は upgrade 後の委譲セッションには効きません。緩和策は
  ガイド `docs/guide/ws-gate-extensions.md` を参照してください。新規依存・`unsafe` はなく、追加のみで
  既存の型は壊れません。crates.io 公開版 0.6.0 には含まれません。

## [0.6.0] - 2026-10-27

公開対象 15 クレートを lockstep バンプ（`docs/design/crates-io-release.md` 7.8 節）。
`fandhe-backend-core` の `RequestGate::check` の引数追加（下の Changed 先頭の **BREAKING** 項目）を
含むため minor を上げます。
実 publish（`v0.6.0` タグ push → verify → dry-run → GitHub Environments
`crates-io-release` の required reviewers 承認 → `cargo publish --workspace`）は
2026-10-27 に実施完了（release run 40473625824、15 クレートすべてが crates.io
インデックスへ反映されたことを確認済み。詳細は
[`docs/design/crates-io-release.md`](docs/design/crates-io-release.md) 7.8 節・8 節チェックリスト参照）。

### Added

- `fandhe-backend-http`: リクエスト単位の型付き拡張領域 `extensions::RequestExtensions` と
  `RequestHead::extensions()` / `extensions_mut()` を追加しました（イシュー [#924](https://github.com/Fandhe-AI/fandhe-backend/issues/924)）。
  `RequestGate` が得た検証結果（検証済み Claims 等）を、そのゲートを通過した同じリクエストの
  ハンドラ・`Interceptor` だけが `head.extensions().get::<T>()` で読めます。値はリクエストと同じ
  寿命で、keep-alive の後続リクエスト・別接続・ゲート未登録・`Reject` 後には届きません。同じ型の
  二重登録は上書きされず `Err` を返します。`Debug` は件数だけを出し、`PartialEq` の比較には含めません。
  新規依存・`unsafe` はなく、使わない構成の追加コストはありません。設計は
  `docs/design/gate-request-extensions.md`。追加のみで型は壊れません（`RequestHead` の新フィールドは
  非公開）。

- `fandhe-backend-plugin-jwt`: opt-in の feature `dpop` を追加しました（RFC 9449 の DPoP で結合された
  アクセストークンの検証。イシュー [#901](https://github.com/Fandhe-AI/fandhe-backend/issues/901)〜
  [#906](https://github.com/Fandhe-AI/fandhe-backend/issues/906)・
  [#907](https://github.com/Fandhe-AI/fandhe-backend/issues/907)、親
  [#899](https://github.com/Fandhe-AI/fandhe-backend/issues/899)、ルート
  [#891](https://github.com/Fandhe-AI/fandhe-backend/issues/891)）。既定は無効で、新規依存はありません
  （暗号は既存の `ring`。無効時はコードもバイナリ増もゼロ）。`dpop` モジュール（`verify_proof`・
  `ProofContext`・`VerifiedProof`・`DpopError`・`DpopConfig`・`DpopMode`（`Required` /
  `AllowUnboundBearer`、既定値なし）・`DpopConfigError`・`IatWindow`・`PublicOrigin`・`ExpectedHtu`・
  `OriginError`・`ReplayStore`・`JwkThumbprint`・`jwk_thumbprint`）と、
  `JwtAuthenticatorBuilder::dpop`・`JwtAuthenticator::dpop_mode`・`JwtAuthenticator::unauthorized_response`・
  `Claims::cnf_jkt`・`VerifyError::Dpop`（`#[non_exhaustive]` への追加）が対象です。拒否は 401 と
  `WWW-Authenticate: DPoP`（`invalid_token` / `invalid_dpop_proof`、`algs`）で返し、DPoP で結合された
  トークンの `Bearer` 提示は拒否します。**`jti` の保持はゲートごと・プロセスごとで、複数インスタンス間では
  共有しません**（写した proof を窓の中で別インスタンスへ送る使い回しは防げないため、窓を短くして余地を
  限ります）。**feature が無効、または DPoP を設定していないゲートは、結合トークンも bearer として
  受理します**。DPoP を強制したい下流は必ず設定してください。設定例は `docs/guide/dpop.md`、設計は
  `docs/design/jwt-verify-plugin.md` 9 章。crates.io 公開版 0.5.0 には未収録です。追加のみで型は
  壊れません。

- `fandhe-backend-plugin-jwt`: ゲートの検証結果をハンドラへ渡す `GateVerified<E>`・`NotAdmitted`・
  `JwtGate::verified` を追加しました（feature によらない。イシュー
  [#904](https://github.com/Fandhe-AI/fandhe-backend/issues/904)）。同じ型のゲートを二重に登録した
  ときは、上書きせず 500 で拒否します。追加のみで型は壊れません。

- `fandhe-backend-plugin-hub-wiring`: opt-in の feature `dpop`（`fandhe-backend-plugin-jwt/dpop` の
  転送のみ、新規依存なし）を追加しました（イシュー
  [#905](https://github.com/Fandhe-AI/fandhe-backend/issues/905)）。あわせて feature によらない
  `TenantGateConfig::from_authenticator`・`TenantVerified`・`NotAdmitted`・`TenantGate::verified` を
  追加しました。有効にしただけでは挙動は変わらず、DPoP を設定した認証器を
  `TenantGateConfig::from_authenticator` に渡したときだけ DPoP の検証と `WWW-Authenticate: DPoP` が
  働きます。0.5.0 には未収録で、追加のみで型は壊れません。

- `fandhe-backend-plugin-jwt`: opt-in の feature `jwks-refresh` を追加しました（イシュー
  [#898](https://github.com/Fandhe-AI/fandhe-backend/issues/898)、親
  [#892](https://github.com/Fandhe-AI/fandhe-backend/issues/892)、ルート
  [#891](https://github.com/Fandhe-AI/fandhe-backend/issues/891)）。ゲートの外で `SharedKeySet` を
  最新に保つ取り直しタスク（`JwksRefresher::new` が返す `RefreshTask` をアプリが `tokio::spawn` し、
  `RefreshHandle` で停止と状態参照）、取得関数の trait `JwksFetcher`（`JwksResponse`・`FetchError`・
  `FetchFuture`）、設定型 `RefreshConfig`（周期は必須。最小間隔・時間切れ・`max_stale`）、観測口
  `RefreshObserver`・`RefreshEvent`・`RefreshStatus`、未知 `kid` を契機とする合図 `RefreshTrigger` と
  `JwtAuthenticatorBuilder::jwks_refresh_trigger` が対象です。HTTP クライアントと TLS は持たず、
  取得関数をアプリが渡します（実装例は `docs/guide/jwks-refresh.md`）。既定では無効で、有効時に
  加わるのは workspace に既にある `tokio`（`time`・`sync`）への optional 依存だけです。追加のみで
  型は壊れません。

- `fandhe-backend-plugin-webrtc`: `WebRtcConfig::with_max_data_channels_per_connection` と
  `max_data_channels_per_connection()` を追加しました（イシュー
  [#877](https://github.com/Fandhe-AI/fandhe-backend/issues/877)、親
  [#883](https://github.com/Fandhe-AI/fandhe-backend/issues/883)、関連
  [#871](https://github.com/Fandhe-AI/fandhe-backend/issues/871)）。1 接続あたりに同時に開けるデータ
  チャネル数の上限で、超過分は即 close します（接続は維持）。`0` は無制限です。既定値の変更は
  下の Changed を参照してください。API の追加のみで型は壊れません。

- `fandhe-backend-plugin-webrtc`: `WebRtcConfig::with_data_channel_send_buffer_limit` と
  `data_channel_send_buffer_limit()` を追加しました（イシュー
  [#874](https://github.com/Fandhe-AI/fandhe-backend/issues/874)、親
  [#871](https://github.com/Fandhe-AI/fandhe-backend/issues/871)）。データチャネル 1 本あたりの
  送信バッファ上限（バイト）を設定し、`PeerConnection` の生成時に webrtc へ配線します。
  N 本のチャネルを持つ接続は最大 N × 上限を保持しうります。`0` は無制限です。既定値の変更は
  下の Changed を参照してください。API の追加のみで型は壊れません。

- `fandhe-backend-plugin-webrtc`: `WebRtcConfig::with_udp_bind_addrs` / `udp_bind_addrs` を
  追加しました（ICE が使う UDP バインドアドレス。既定は `["0.0.0.0:0"]`）。追加のみで
  breaking change ではありません。

### Changed

- **BREAKING** `fandhe-backend-core`: `RequestGate::check` に第 3 引数
  `extensions: &mut RequestExtensions` を追加しました（イシュー [#924](https://github.com/Fandhe-AI/fandhe-backend/issues/924)）。ゲートが得た検証結果を
  同じリクエストのハンドラへ渡すための経路で、コアは全ゲートが `Allow` を返したときだけ、その
  リクエスト用の器を `RequestHead` へ移します。`GateOutcome` と `GateContext` は変わりません。
  必須メソッドの引数追加のため、非推奨期間を挟まずに壊れます（二重メソッドを残さない方針は
  `GateContext` を足した #486 と同じ）。
  - 移行（値を渡さない既存ゲート）: `impl RequestGate` の `check` に第 3 引数
    `_extensions: &mut RequestExtensions` を足すだけで、挙動は変わりません。
    `RequestExtensions` は `fandhe_backend_core::extension` と crate ルートから使えます。
  - 移行（`check` を直接呼ぶテスト）: `gate.check(&head, &ctx, &mut RequestExtensions::new())`
    のように空の器を渡します。
  - 値を渡したいゲート: `extensions.insert(value)` で入れ、`Err`（同じ型を先のゲートが入れ済み）
    のときは拒否を返します。ハンドラは `head.extensions().get::<T>()` で読みます。
  - 影響した workspace 内の実装: `fandhe-backend-plugin-csrf` の `CsrfGate`、
    `fandhe-backend-plugin-hub-wiring` の `TenantGate`、`fandhe-backend-plugin-jwt` の `JwtGate`
    （シグネチャの追随のみで挙動は変わりません）。

- `fandhe-backend-plugin-webrtc`: **1 接続あたりのデータチャネル数の既定が、従来の無制限から
  16 本になりました**（送信バッファ上限はチャネルごとに効くため、チャネル数が無制限だと 1 接続の
  メモリが青天井になる）。同時に開いている本数が上限を超えるチャネルは即 close され、接続と既存
  チャネルは維持されます。従来の無制限に戻すには `with_max_data_channels_per_connection(0)` を指定して
  ください（型は壊れない追加ですが、17 本以上を使う既存利用者は挙動が変わります）。
  イシュー [#877](https://github.com/Fandhe-AI/fandhe-backend/issues/877)、親
  [#883](https://github.com/Fandhe-AI/fandhe-backend/issues/883)。

- `fandhe-backend-plugin-webrtc`: **データチャネルの送信バッファ上限の既定が、従来の無制限から
  1 チャネルあたり 1 MiB になりました**（遅い相手によるメモリ枯渇の抑止）。上限到達時は
  `DataChannel::send` が空きを待つバックプレッシャで、エラーや切断は起きません。従来の無制限に
  戻すには `with_data_channel_send_buffer_limit(0)` を指定してください（型は壊れない追加ですが、
  既定の挙動が変わります。1 MiB を超える単一メッセージもバッファが空なら送信されます）。
  イシュー [#874](https://github.com/Fandhe-AI/fandhe-backend/issues/874)、
  [#876](https://github.com/Fandhe-AI/fandhe-backend/issues/876)、親
  [#871](https://github.com/Fandhe-AI/fandhe-backend/issues/871)。

- `fandhe-backend-plugin-webrtc`: 内部の `webrtc`（webrtc-rs）を 0.17 系から 0.21 系へ
  移行しました（イシュー [#870](https://github.com/Fandhe-AI/fandhe-backend/issues/870)）。
  `webrtc` の型は公開 API に出ておらず、`POST /rtc/offer` のワイヤ契約（JSON）も変わりません。
  API が全面的に異なるため、イベント処理は `PeerConnectionEventHandler` 実装 1 つ
  （`SignalingHandler`）に集約しました。0.21 は UDP バインドアドレスが必須のため、
  **`WebRtcConfig` の既定で `0.0.0.0:0` にバインドします。0.17 系の既定は IPv4 と IPv6 の両方の
  ICE 候補を集めていたので、既定では IPv4 のみに狭まります**（IPv6 の無い環境での挙動を全環境では
  確認していないため、既定値自体は IPv6 を含めない保守的な選択としています）。従来の挙動に戻すには
  `with_udp_bind_addrs(vec!["0.0.0.0:0".to_owned(), "[::]:0".to_owned()])` を明示してください
  （バインドできなかったアドレスは webrtc 側でスキップされ、1 つでもバインドできれば接続は生成できます）。
  アドレスは `SocketAddr`（`ip:port`）として解釈できる必要があり、**ホスト名は受け付けません**
  （ブロッキングの名前解決がドライバ上で走るため）。空のリスト、または解釈できない要素を含む設定は
  既定値へ戻らず、`POST /rtc/offer` が枠の予約前に `500`
  （`{"error":"peer_connection_init_failed"}`）で拒否します（フェイルクローズ）。
  0.21 は `close()` せずに drop するとドライバと UDP ソケットが残るため、接続の枠を除去する全経路
  （シグナリングの失敗・タイムアウト、呼び出し元 Future の drop、終端状態の検知、drain）で
  `close()` も行うようにしました。`PeerConnection` の生成（`build()`）はキャンセルされない所有タスクで
  完了まで走らせ、生成中にタイムアウトや呼び出し元のキャンセルが起きても、生成タスクが pc を `close()`
  して枠を解放します（生成中の枠は同時接続数の上限に数え続けるため、残留するドライバは上限を超えません）。
  `close_active_peers` / `drain_for_shutdown` は、待ち時間の上限を超えても `close()` を
  中断せず切り離したタスクで完了まで続けます。内部の `RTCPeerConnection` 表記の doc は
  `PeerConnection` に改めています。

### Security

- `fandhe-backend-plugin-webrtc`: `webrtc` 0.17 系が `dtls` 経由で引いていた `crypto-bigint 0.5.5` の
  脆弱性 RUSTSEC-2026-0387 を、`webrtc` 0.21 系への移行で解消しました（0.21 系は `crypto-bigint` を
  依存グラフに含みません。イシュー
  [#870](https://github.com/Fandhe-AI/fandhe-backend/issues/870)、方針は
  [`docs/design/webrtc-rs-version-strategy.md`](docs/design/webrtc-rs-version-strategy.md)）。

## [0.5.0] - 2026-10-16

公開対象 15 クレートを lockstep バンプ（`docs/design/crates-io-release.md` 7.7 節）。
`fandhe-backend-plugin-jwt` と `fandhe-backend-plugin-csrf` はこの版で初公開します。
実 publish（`v0.5.0` タグ push → verify → dry-run → GitHub Environments
`crates-io-release` の required reviewers 承認 → `cargo publish --workspace`）は
2026-10-16 に実施完了（release run 38904246045、15 クレートすべてが crates.io
インデックスへ反映されたことを確認済み。詳細は
[`docs/design/crates-io-release.md`](docs/design/crates-io-release.md) 7.7 節・8 節チェックリスト参照）。

### BREAKING CHANGES

- `fandhe-backend-http`: `SetCookie` に cookie 接頭辞（`__Host-` / `__Secure-`、
  RFC 6265bis）の構築時検証を追加しました（イシュー #810）。
  - `SetCookie::new` は `__Host-` / `__Secure-` で始まる名前（ASCII 大文字小文字無視）を
    `CookieError::ReservedPrefix` で拒否します。移行手順: 接頭辞付き名前は
    `SetCookie::host_prefixed`（`Secure` と `Path=/` を自動付与）/
    `SetCookie::secure_prefixed`（`Secure` を自動付与）で構築してください。
  - `__Host-` cookie の `SetCookie::path` は `/` 以外を `CookieError::HostPrefixPath` で拒否します。
  - `CookieError` を `#[non_exhaustive]` にし、`ReservedPrefix` / `PrefixMismatch` /
    `HostPrefixPath` を追加しました。移行手順: `CookieError` を網羅的に `match` している
    場合は `_ =>` アームを追加してください。

- `fandhe-backend-plugin-hub-wiring`: JWT 検証部を汎用 JWT 検証クレート
  `fandhe-backend-plugin-jwt` へ移しました（イシュー #820、REQ-16 受け入れ基準 5、
  設計は [`docs/design/jwt-verify-plugin.md`](docs/design/jwt-verify-plugin.md) 2.9 節）。
  検証ポリシー（発行者・受信者・許可アルゴリズム・トークン種別）を
  `TenantGateConfig` の構築に必須で渡す形になり、ポリシーなしの構成は型として作れません。
  旧公開パスの互換層は置きません。
  - 移行手順（ポリシーの渡し方）:
    ```rust
    use fandhe_backend_plugin_hub_wiring::jwt::{
        Algorithm, AllowedAlgorithms, Audiences, Issuer, JwtPolicy, TokenType,
    };
    let policy = JwtPolicy::new(
        Issuer::new("https://issuer.example")?,
        Audiences::single("hub-api")?,
        AllowedAlgorithms::only(Algorithm::Rs256),
        TokenType::AccessToken,
    );
    // 旧: TenantGateConfig::from_jwks_json(&jwks_json)?
    let config = TenantGateConfig::from_jwks_json(&jwks_json, policy)?;
    // 旧: TenantGateConfig::new(shared_jwks)  ->  new(shared_key_set, policy)
    ```
  - 公開パスの対応: `jwks::{JwksKeySet, SharedJwks, JwksError}` は
    `jwt::{KeySet, SharedKeySet, JwksError}`、`TokenError` は `AuthError`
    （`Jwt(VerifyError)` と `MissingOrgId` の 2 変種）、`Claims` は `jwt::Claims`
    （`TenantClaims::claims()` で参照）へ移りました。`jwt::verify_token` は削除しました
    （`jwt::verify` はポリシーと現在時刻を取ります）。`Authenticator::new` は
    `(SharedKeySet, JwtPolicy)` を取る公開コンストラクタになり、`authenticate` は
    `TenantClaims`（`org_id()` / `claims()`）を返します。
  - トークンに必要なクレーム: ヘッダ `typ` は `at+jwt`（`application/at+jwt` も可、
    大文字小文字は区別しない）が必須です。payload は `iss`（ポリシーと完全一致）・
    `aud`（ポリシーのいずれかと一致、文字列でも配列でも可）・`exp` が必須で、`org_id` も
    引き続き必須です。`nbf` があれば検証し、`crit` ヘッダがあれば拒否します。`kid` は
    256 バイトまでです。ES256 はポリシーの許可リストに入れたときだけ受理します。
  - JWKS の扱い: 同じ `kid` の鍵を 3 件まで許すようにし（ローテーション用）、
    `DuplicateKeyId` は廃止しました。採用候補（RSA / EC P-256）の鍵に `kid` や `alg`
    がない場合、`kty` と `alg` が合わない場合、秘密鍵成分・`oct`・`alg: none` / HS 系が
    ある場合は文書ごと拒否します。**`alg` を持たない RSA JWK は旧実装では採用されて
    いましたが、新実装では文書ごと拒否されます。** 発行元の JWKS に `"alg": "RS256"` を
    付けてください。未対応の方式（EdDSA 等）は読み飛ばします。
  - 401 / 403 の判定・応答本文は変わりません（`org_id` の欠落・空は 403、それ以外の
    検証失敗は 401）。
  - リリース順の注意: `fandhe-backend-plugin-jwt` の暫定の `publish = false` は
    #822（公開準備）で解除済みです。`fandhe-backend-plugin-hub-wiring` が依存するため、
    0.5.0 で 2 クレートを同時に初公開しました。

### Changed

- `fandhe-backend-plugin-compression` / `fandhe-backend-plugin-cors`: `Vary: Accept-Encoding` /
  `Vary: Origin` の付与を `fandhe-backend-http` の `Response::with_vary` へ移しました
  （イシュー #830）。2 行目以降の `Vary` 行にあるトークンと `Vary: *` を考慮するため、
  行が重複せず、`Vary: *` の応答には足さなくなりました。公開 API は変わらず、破壊的変更では
  ありません。

### Added

- `fandhe-backend-http`: 同一オリジンに限るリダイレクト先の検証済み型
  `location::SameOriginLocation`（と sans-IO 検証関数 `validate_same_origin_location`、
  `#[non_exhaustive]` のエラー型 `SameOriginLocationError`）と、POST 後の 303 を組み立てる
  `Response::see_other` を追加しました（イシュー #828）。空・`/` 始まりでない値・`//`・`/\`・
  `\`・制御文字（HTAB 含む）を構築時に拒否し、percent デコード・正規化は行いません
  （`%2F` / `%5C` は不透明なバイトとして受理）。既存の `Response::redirect` の挙動は
  変わりません。fuzz target `same_origin_location` を追加しました。
- `fandhe-backend-http`: `Response::with_vary` と `Response::with_no_store` を追加しました
  （イシュー #829）。`with_vary` はすべての `Vary` 行と `Vary: *` を考慮し、大文字小文字を
  区別せず重複なくトークンを新しい行として足します。`with_no_store` は `Cache-Control:
  no-store` を何度呼んでも 1 つだけ付けます。`Set-Cookie` を含む応答へは自動では付けず、
  doc で `with_no_store` の併用を推奨します。
- `fandhe-backend-plugin-csrf`: CSRF 検査ゲートのクレートを新設しました（REQ-17、0.5.0 で
  初公開）。`CsrfGate`（`RequestGate` 実装）を `CsrfGate::builder()` /
  `CsrfGateBuilder`（`allow_origin`・`exempt_path`・`build`）で構築します。`GET`・`HEAD`・
  `OPTIONS` 以外の要求は、`Sec-Fetch-Site` が `same-origin` / `none` のときだけ許可し、
  このヘッダが無いときは `Origin` が許可リストとバイト完全一致のときだけ許可します。
  両ヘッダとも無い要求、`Origin: null`、重複ヘッダは拒否します。除外パスは完全一致のみです。
  拒否は本文なし・理由なしの `403` で、ヘッダ値を反映しません。構築時の不備は
  `CsrfConfigError` で返し、上限定数 `MAX_ALLOWED_ORIGINS` / `MAX_EXEMPT_PATHS` /
  `MAX_ENTRY_LEN` を公開します。外部依存はなく、Gate 型のため core の feature は持たず、
  `Server::gate` で登録します。Cookie 認証の画面では CSRF ゲートを認証ゲートより先に登録する
  ことを推奨します（設計は
  [`docs/design/plugin-boundary.md`](docs/design/plugin-boundary.md) 5.6.6 節）。
- `fandhe-backend-plugin-jwt`: 汎用 JWT 検証クレートを新設しました（REQ-16、0.5.0 で
  初公開）。RS256・ES256 の検証（`JwtPolicy` で `iss`・`aud`・`typ`・許可 `alg` を必須とし、
  `none`・HS 系は型として表現不能）、JWKS 鍵集合（`KeySet` / `SharedKeySet`）、
  `verify`、検証結果キャッシュ付き `JwtAuthenticator`、`RequestGate` 実装 `JwtGate`、
  未知 `kid` の通知口 `UnknownKeyNotifier` を提供します。Gate 型のため core の feature は
  なく、`Server::gate` で登録します（設計は
  [`docs/design/jwt-verify-plugin.md`](docs/design/jwt-verify-plugin.md)）。
- `fandhe-backend-http`: `SetCookie::host_prefixed` / `SetCookie::secure_prefixed` を追加しました。
- `fandhe-backend-http`: セッション Cookie 用に `SetCookie::session` / `session_with` / `session_removal` /
  `session_removal_with` と `SessionSameSite`（`Lax` / `Strict` のみ）、`CookieError::SessionSameSiteNone` を
  追加しました（イシュー #827）。`HttpOnly`・`Secure`・`SameSite=Lax`・`Path=/` を既定で付与し、削除用は
  同じ属性に `Max-Age=0` を加えます。`__Host-` 名とも組み合わせられます（必須ではありません）。
- `fandhe-backend-http`: `FormError` に `IntoResponse` と `From<FormError> for HttpError` を追加し、
  上限超過（`BodyTooLong`・`TooManyPairs`）を 413、UTF-8 不正・percent-decode 失敗
  （`InvalidUtf8Body`・`Decode`）を 400 の固定メッセージ応答に対応づけました。あわせて上限を
  引数で渡す `form::parse_form_with_limit` と `form::FormLimits` を追加しました（イシュー #831）。
  既定値（8 KiB・256 組）と `parse_form` の挙動は変わりません。
- `fandhe-backend-plugin-hub-wiring`: `jwt`（`fandhe-backend-plugin-jwt` 全体の再エクスポート）、
  `AuthError` / `TenantClaims` / `HubClaims`、`Authenticator::from_jwt` を追加しました。

### Removed

- `fandhe-backend-plugin-hub-wiring`: 独自の JWT / JWKS 検証実装（`jwt.rs`・`jwks.rs`・
  検証結果キャッシュ）を削除し、`fandhe-backend-plugin-jwt` への依存に置き換えました
  （上記 BREAKING CHANGES 参照）。

## [0.4.2] - 2026-09-28

公開対象 13 クレートを lockstep バンプ（`docs/design/crates-io-release.md` 7.6 節）。
実 publish（`v0.4.2` タグ push → verify → dry-run → GitHub Environments
`crates-io-release` の required reviewers 承認 → `cargo publish --workspace`）は
2026-09-28 に実施完了（release run 36455715959、13 クレートすべてが crates.io
インデックスへ反映されたことを確認済み。詳細は
[`docs/design/crates-io-release.md`](docs/design/crates-io-release.md) 7.6 節・8 節チェックリスト参照）。

BREAKING CHANGE はありません（後方互換な API 追加・非破壊修正のみ。バージョン方針は
`docs/design/ws-connection-context-and-close.md` 7 節）。

### Added

- `fandhe-backend-core`: ライブラリ内部の実行時診断（accept 失敗・
  TCP_NODELAY 設定失敗・graceful shutdown / rebind の grace 超過強制クローズの
  4 箇所）を、利用側で差し替え可能な診断シンクへ移行しました。新規 trait
  `Diagnostics`・イベント型 `DiagnosticEvent`（`#[non_exhaustive]`）・既定
  シンク `StderrDiagnostics` を追加し、`Server::diagnostics(sink)` で登録
  できます。未登録時（既定）は `StderrDiagnostics` が使われ、現行の
  `eprintln!` 出力と文言・接頭辞・出力先（stderr）が完全互換です。クロージャを
  そのまま登録できる blanket impl があるため、
  `Server::new().diagnostics(|_event: &DiagnosticEvent<'_>| {})` の 1 行で
  出力を抑止したり、`tracing::warn!` 等へ転送する独自シンクを書いたりできます
  （引数の型注釈は必須で、`|_| {}` のみでは HRTB が絡み型推論に失敗しコンパイル
  が通りません。新規外部依存の追加なし、`tracing` feature とは無関係。
  設計は
  [`docs/design/diagnostics-sink.md`](docs/design/diagnostics-sink.md)、
  イシュー [#720](https://github.com/Fandhe-AI/fandhe-backend/issues/720)）。
  挙動変更として、最終 graceful shutdown・rebind 旧世代 drain の grace 超過
  強制クローズ 2 箇所は、通知（診断シンクへの `report` 呼び出し）を強制
  クローズの完了**後**に行うよう順序を入れ替えています（利用側シンクの異常・
  遅延がフェイルクローズを妨げないため。既定シンクでは出力順序が反転しても
  外部から観測可能な違いはほぼありません）。
- `fandhe-backend-core`: `Middleware` に新規メソッド `on_response_with_status`
  を追加しました。レスポンス送出後の観測フックからクライアントへ**実際に
  送出したステータスコード**（`Interceptor::map_response` 等のレスポンス
  改変シーム適用後の最終値）を参照できます。既定実装は `on_response` へ
  委譲するため、`on_response` だけを実装した既存コードは変更なしにそのまま
  動作します（コアは新フックのみを呼び出す非破壊追加。既存の必須メソッド
  `on_response` にも既定実装（no-op）を付与しました）。ステータス付き
  アクセスログ（`GET /path 200 3ms` 等）が必要な実装は本メソッドを override
  してください（イシュー
  [#721](https://github.com/Fandhe-AI/fandhe-backend/issues/721)）。
- `fandhe-backend-routes`: 独立に組み立てた複数の `Router` を 1 つに合成する
  `Router::merge` を追加しました。重複するルート登録（静的ルートの
  `(method, path)` 完全一致、パラメータルートの method 一致 + セグメント形状
  等価、両方への `fallback`/`fallback_with`・`options_fallback` 登録）は
  黙って上書きせず `RouterMergeError` でフェイルクローズに検出します
  （意図しないハンドラの差し替え防止、OWASP A01）。合成後のパラメータルートの
  登録順は self が先・other が後（`Vec::extend`）。衝突しない部分的な重なりは
  既存の優先順位（静的 → パラメータ、パラメータは登録順）がそのまま適用され、
  `route`/`route_param` 単体の「同一 `Router` 内での再登録」の既存意味論
  （`route` は後勝ち、`route_param` は登録順の先勝ち）は変わりません
  （イシュー [#722](https://github.com/Fandhe-AI/fandhe-backend/issues/722)）。
- `fandhe-backend-plugin-websocket`: ハンドシェイクの受理判定フック
  `WebSocketConfig::with_handshake_check`（新規 trait `WsHandshakeCheck`・
  コンテキスト型 `WsHandshakeContext`）を追加しました。RFC 6455 検証を通過した
  upgrade 要求について 101 応答の送出前に一度だけ同期で評価し、`{name}` パス
  パラメータ（`with_path_pattern` 由来）・リクエストヘッダ・接続元アドレス
  （`handle_upgrade_with_peer_addr` 由来、イシュー
  [#728](https://github.com/Fandhe-AI/fandhe-backend/issues/728)）を参照できます。
  拒否する場合はフックが返す `Response` を送出し upgrade しません（1xx/2xx・
  304・`0..=99`/`600` 以上のステータスはフェイルクローズに `400 Bad Request`
  へ正規化します）。`RequestGate`
  拡張点はパスパラメータを持たないため、それを代替する
  `plugin-websocket` 内蔵の拒否経路として設計しました（コア拡張点は増やさない。
  `WsError` への variant 追加は破壊的変更になるため行わず、戻り値は拒否時も
  `Ok(())`）。未登録時（既定）は挙動が変わりません（後方互換追加。設計は
  `docs/design/ws-connection-context-and-close.md` 16 節、イシュー
  [#716](https://github.com/Fandhe-AI/fandhe-backend/issues/716)）。
- `fandhe-backend-plugin-websocket`: 接続コンテキスト（`WsOpenContext` /
  `WsConnContext`）から、接続元アドレス・主要リクエストヘッダ（`Host` /
  `Origin` / `User-Agent`）・query 文字列を参照できるようにしました。両型へ
  `host()` / `origin()` / `user_agent()` / `query()` アクセサを追加し、
  `WsConnContext` には新たに `peer_addr()` も追加しています（`WsOpenContext::
  peer_addr()` と同じ値を返す）。固定 5 項目（`peer_addr`/`host`/`origin`/
  `user_agent`/`query`）のみを保持する許可リスト方式で、任意のヘッダを
  取得する汎用アクセサは設けていません。ヘッダ値は 1024 バイト、query は
  2048 バイトを上限とし、**超過した値は切り詰めずに `None`** にします
  （フェイルクローズ。切り詰めると `Host`/`Origin` の許可リスト照合が
  部分一致でバイパスされうるため）。2 つのコンテキストは同一の内部値を
  `Arc` 共有するため、値の抽出は 1 接続あたり 1 回だけです。新しい値は
  いずれもクライアントの申告値（`peer_addr` を除く）または機密混入の懸念が
  あるため `Debug` 出力には含めません（`WsOpenContext`/`WsConnContext`
  いずれも既存の `finish_non_exhaustive()` を維持）。非破壊追加（既存の
  `on_open`/`on_message_with_ctx`/`on_close` 実装は無変更で動作します）。
  設計は `docs/design/ws-connection-context-and-close.md` 16 節、イシュー
  [#717](https://github.com/Fandhe-AI/fandhe-backend/issues/717)。
- `fandhe-backend-plugin-websocket`: `on_open` から接続元の実 peer address を
  参照できるようにしました。新関数 `handle_upgrade_with_peer_addr`
  （`handle_upgrade` の 5 引数に `peer_addr: Option<std::net::SocketAddr>` を
  追加した非破壊追加）と `WsOpenContext::peer_addr()` アクセサを追加し、
  コア（`fandhe-backend-core`）は accept したソケットの実 peer address
  （`RequestGate::check` へ渡す `GateContext::peer_addr` と同一由来、イシュー
  [#486](https://github.com/Fandhe-AI/fandhe-backend/issues/486)）を Upgrade
  委譲経路へもそのまま渡すようになりました。既存 `handle_upgrade`（5 引数）は
  `peer_addr: None` で本関数へ委譲する薄いラッパーとして残り、公開シグネチャは
  無変更です。`tokio::io::duplex` 等の非ソケット経路、または `peer_addr` を
  渡さない旧 API からの呼び出しでは常に `None`（`GateContext::peer_addr` と
  同型のフェイルクローズ契約）。接続元アドレスは偽装できない値ですが PII に
  近い情報のため、`WsOpenContext` の `Debug` 出力には含めません
  （設計は `docs/design/ws-connection-context-and-close.md` 15 節、イシュー
  [#728](https://github.com/Fandhe-AI/fandhe-backend/issues/728)）。
- `fandhe-backend-plugin-websocket`: サーバー起点の Ping と Pong 期限による死活監視
  `WebSocketConfig::with_ping_interval(interval, pong_timeout)`（既定は無効、
  `interval`/`pong_timeout` に `Duration::ZERO` を指定すると `PingIntervalError`
  で構築時に拒否）を追加しました。有効化すると `interval` ごとにサーバーから
  `Message::Ping` を送出し、送出時点から `pong_timeout` 以内にクライアントの
  Pong が届かない接続を `CloseReason::PongTimeout`（非破壊追加の新規 variant）
  で切断します。`idle_timeout` はクライアントからの受信でのみリセットされ
  サーバー起点の送出ではリセットしないため、サーバー起点 push を受けている
  だけの受信専用クライアント（例: CDP 互換サーバー相手の Playwright/Puppeteer）
  は `idle_timeout` だけでは死活監視できず、本設定が必要になります。
  期限切れの判定は受信待ちで読めるフレームがなくなった時点で行うため、
  期限を過ぎても届いているフレームは先に読んで処理し、その中の Pong も
  有効として扱います（Pong を返さない対向も、フレームが途切れず届いている
  間は切断しません）。1 回の送出（Ping・Reply・
  outbound push 等）がその開始時刻から `pong_timeout` を超えてブロックした
  場合も同じ `CloseReason::PongTimeout` で終了します。
  `without_ping_interval()` で明示的に無効へ戻せます。既存の設定・挙動は
  無変更のまま動作します（後方互換。設計は本ファイル・
  `docs/api/plugin-config-api.md`、イシュー
  [#713](https://github.com/Fandhe-AI/fandhe-backend/issues/713)、親
  [#712](https://github.com/Fandhe-AI/fandhe-backend/issues/712)）。
- `fandhe-backend-plugin-websocket`: `on_message` 処理中に接続 ID・接続コンテキスト
  を参照できるようにしました。プロセス内で一意な接続識別子 `WsConnId`、接続単位の
  コンテキスト `WsConnContext`（`conn_id()` / `sender()` / `param()` / `params()`）、
  `WsOpenContext::conn_id` を追加し、`WsMessageHandler` に provided メソッド
  `on_message_with_ctx`（既定実装は既存の `on_message` へ委譲）を追加しました。
  既存の `on_message` のみを実装したハンドラは無変更のままコンパイル・動作します
  （後方互換。設計は `docs/design/ws-connection-context-and-close.md`、イシュー
  [#704](https://github.com/Fandhe-AI/fandhe-backend/issues/704)、親
  [#702](https://github.com/Fandhe-AI/fandhe-backend/issues/702)）。
- `fandhe-backend-plugin-websocket`: セッション終了理由の型
  `handler::CloseReason`（`ClientClose` / `Eof` / `IdleTimeout` / `Cancelled` /
  `HandlerClose` / `MessageTooLarge` / `Failed(FailureKind)`）と
  `handler::FailureKind`（`Io` / `Protocol` / `Handler`）を追加し、
  セッション内部（`run_session_inner`）が全終了経路ごとに終了理由を算出する
  ようにしました。いずれも `#[non_exhaustive]` で、クライアント入力の
  payload は保持しません（設計は
  `docs/design/ws-connection-context-and-close.md` 4 節、イシュー
  [#726](https://github.com/Fandhe-AI/fandhe-backend/issues/726)、親
  [#705](https://github.com/Fandhe-AI/fandhe-backend/issues/705)）
- `fandhe-backend-plugin-websocket`: 切断通知フック `WsMessageHandler::
  on_close(&self, ctx: &WsConnContext, reason: CloseReason)`（既定 no-op）を
  追加しました。`on_open` が呼ばれた接続についてのみ、終了経路を問わず
  セッション終了時にちょうど 1 回呼ばれます（ハンドシェイク検証失敗・101
  送出前キャンセルでは `on_open` 同様呼ばれない、フェイルクローズの対称性）。
  既存の `on_message`/`on_open` のみを実装したハンドラは無変更のまま
  コンパイル・動作します（後方互換。設計は
  `docs/design/ws-connection-context-and-close.md` 4 節・9 節、イシュー
  [#729](https://github.com/Fandhe-AI/fandhe-backend/issues/729)、親
  [#705](https://github.com/Fandhe-AI/fandhe-backend/issues/705)）
- `fandhe-backend-plugin-websocket`: `WsSender::closed()`（切断まで待つ非同期
  メソッド）と `WsSender::is_closed()`（同期判定）を追加しました。`on_close`
  を使わない最小限の代替として、セッションの切断を待つ・判定する手段を
  提供します。clone した `WsSender` でも同じ時点で切断を観測します。
  ハンドラ本体（`on_message`/`on_message_with_ctx`）の中で `closed()` を
  インライン `await` すると自己デッドロックするため、`on_open` 等から
  `tokio::spawn` した別タスクでのみ使う契約です（イシュー
  [#727](https://github.com/Fandhe-AI/fandhe-backend/issues/727)、親
  [#705](https://github.com/Fandhe-AI/fandhe-backend/issues/705)）
- `fandhe-backend-plugin-websocket`: `WsSender::close(code, reason)` を
  追加しました。`on_message` の戻り値 `WsOutcome::Close` とは異なり、
  `on_open` から `spawn` したタスク等、ハンドラの外からも任意のタイミングで
  サーバー起点の Close ハンドシェイクを開始できます。close 呼び出し前に
  `send` が `Ok` を返した push メッセージは Close フレームより先に送出される
  順序保証を持ちます（内部の送信キューへ単一の bounded mpsc として直列に流す
  ことで FIFO 特性のみで保証。`close_grace` 超過・世代キャンセル・idle
  timeout・送出失敗で打ち切られた場合は、残りの push と Close フレームを
  送らずに終了します）。close 後の `send`/`WsSender::is_closed()` は他の
  終了経路と同様の挙動を返します。close code の RFC 6455 7.4 節・IANA 登録に
  基づく検証（`1000..=1003`・`1007..=1009`・`1011..=1014`・`3000..=4999` のみ
  許可。`1010` はクライアント専用のため拒否）・reason の
  123 バイト上限検証に
  失敗した場合、または既に close 済み・セッション終了済みの場合は
  `WsCloseError`（`InvalidCode` / `ReasonTooLong` / `Closed`、
  `#[non_exhaustive]`）を返します。終了理由 `CloseReason` に variant
  `SenderClose` を追加しました（非破壊。設計は
  `docs/design/ws-connection-context-and-close.md`、イシュー
  [#710](https://github.com/Fandhe-AI/fandhe-backend/issues/710)、親
  [#708](https://github.com/Fandhe-AI/fandhe-backend/issues/708)）
- `fandhe-backend-plugin-websocket`: `WebSocketConfig::with_outbound_capacity`
  で送信キュー（`WsSender`）の容量を接続ごとに指定できるようにしました
  （既定は変わらず 8）。0 は `OutboundCapacityError::Zero`、上限
  `MAX_OUTBOUND_CAPACITY`（4096）を超える値は `OutboundCapacityError::TooLarge`
  を返し、`tokio::sync::mpsc::channel` がライブラリ境界で panic する余地を
  構築時に排除します。あわせて `WsSender::try_send` を追加しました。
  `WsSender::send` とは異なり満杯でも `.await` で待たず、送信キューが
  「満杯（`WsTrySendError::Full`）」か「セッション終了・close 確定済み
  （`WsTrySendError::Closed`）」かを区別して即座に返します。いずれの
  variant も送れなかった `WsMessage` をそのまま保持し、呼び出し側が
  再試行・破棄を選べます（`into_inner()`/`is_full()`/`is_closed()`）。
  順序保証・push の消化契約は `WsSender::send` と同一です（イシュー
  [#709](https://github.com/Fandhe-AI/fandhe-backend/issues/709)、親
  [#708](https://github.com/Fandhe-AI/fandhe-backend/issues/708)）
- `fandhe-backend-plugin-websocket`: クレートルートに型エイリアス `BoxFuture`
  （`Pin<Box<dyn Future<Output = T> + Send + 'a>>`）を公開しました。
  `WsMessageHandler` の実装で `futures-util` へ直接依存せずに戻り値の型を
  書けます。`futures_util::future::BoxFuture` と同一の型のため、既存の
  書き方のハンドラも無変更のままコンパイル・動作します（イシュー
  [#723](https://github.com/Fandhe-AI/fandhe-backend/issues/723)、PR
  [#745](https://github.com/Fandhe-AI/fandhe-backend/pull/745)）

### Fixed

- `fandhe-backend-plugin-websocket`: 受信メッセージ/フレームが
  `WebSocketConfig::max_message_size` / `max_frame_size` を超えたとき、
  Close フレームを送らずに接続を drop していた不具合を修正しました。
  現在は RFC 6455 7.4.1 節の close code 1009（Message Too Big）・固定
  reason `"message too big"` を送出してから閉じます（利用側は理由のない
  異常切断ではなく 1009 を観測できます）。frame サイズ上限超過時は
  送信途中の巨大 payload がソケットに残りうるため、Close 送出後に生
  ストリームの半閉鎖（FIN）+ 有界な読み捨て（`close_grace` で有界化、
  固定長バッファで蓄積しない）を行い、カーネルの RST で Close 1009 が
  失われるのを避けます。上限超過メッセージがハンドラへ到達しない
  既存の DoS 対策・サーバー側の戻り値
  （`Err(WsError::Protocol(tungstenite::Error::Capacity(_)))`）は
  変わりません（イシュー
  [#719](https://github.com/Fandhe-AI/fandhe-backend/issues/719)）
- `fandhe-backend-plugin-websocket`: `WsMessageHandler::on_message`（および
  `on_message_with_ctx`）実行中に `WsSender::send` を送信キュー容量
  （`DEFAULT_OUTBOUND_CAPACITY = 8`）を超える回数呼ぶとデッドロックする不具合を
  修正しました。ハンドラ実行中も outbound を cancel（最優先）→ (ハンドラ完了 |
  outbound 到着) の順で消化する内側 race ループを追加し、ハンドラ完了直後・
  返信送出前に既に到着済みの push を排出するようにしました（送出順序の保証・
  不定契約は `crates/plugin-websocket/src/session.rs` モジュール doc
  「ハンドラ実行中の送信キュー消化」節を参照。イシュー
  [#706](https://github.com/Fandhe-AI/fandhe-backend/issues/706)）
- `fandhe-backend-plugin-websocket`: ハンドラが `WsOutcome::Close` を返したとき、
  それまでに `WsSender::send` で送信キューへ積まれていた push を送らずに
  Close していた不具合を修正しました。現在は Close 処理の開始時点で送信キューを
  封鎖し、キュー済みの push を Close フレームより先に送出します（以後の
  `WsSender::send` は `Err` を返します）。flush と Close の送出は全体で
  `WebSocketConfig::close_grace` を上限とし、超過・世代キャンセル・送出失敗で
  打ち切られた場合は残りの push と Close フレームを送らずに終了します
  （二次 DoS 対策。イシュー
  [#711](https://github.com/Fandhe-AI/fandhe-backend/issues/711)、PR
  [#735](https://github.com/Fandhe-AI/fandhe-backend/pull/735)）
- `fandhe-backend-plugin-websocket`: ハンドラが `Err` を返したとき、送信キューを
  封鎖し、`close_grace` を上限にキュー済みの push を送出してから終了するように
  しました（従来は排出せずに即時終了していました）。封鎖前に確定した
  `WsSender::close` があれば、その Close を送り、`on_close` の理由は
  `Failed(Handler)` ではなく `SenderClose` になります。封鎖と同時に、待機中・
  以後の `send`/`close` は `Err` を返します。cancel・idle timeout・クライアント
  Close・EOF・受信/送信エラーの経路でも、受信側を手放した後に `send`/`close` が
  `Ok` を返す（値が黙って捨てられる）ことはなくなりました（これらの経路は
  従来どおりキュー済みの push を送出しません）。`WsOutcome::Reply`
  を返した場合の排出は送信キュー容量回までに制限します。また、`WsSender::close`
  を呼ぶと、呼び出しを確定前に取り下げない限り、クライアントが受信を止めていても
  （送信キューが満杯で close がまだ確定できない場合も）、要求の観測（通常は
  呼び出しの直後）から `close_grace`
  以内に Close ハンドシェイクを終えるか接続を打ち切ります。その間に世代
  キャンセル・idle timeout が先に発火した場合は、その経路の契約（発火時点から
  `close_grace`）に従います（close を要求していない間の push の送出には従来どおり
  期限を設けません）。ハンドラの戻り値は、セッションが
  その送出直前に送信キューと同じロックで close の確定を判定する時点より前に
  `WsSender::close` が確定していれば、ハンドラの実行中か完了後の送信キュー排出中
  かを問わず破棄します（判定より後に確定した場合、`Reply` は Close より先に
  送出されえます）。イシュー
  [#710](https://github.com/Fandhe-AI/fandhe-backend/issues/710)

## [0.4.1] - 2026-09-26

公開対象 13 クレートを lockstep バンプ（`docs/design/crates-io-release.md` 7.5 節）。
実 publish（`v0.4.1` タグ push → verify → dry-run → GitHub Environments
`crates-io-release` の required reviewers 承認 → `cargo publish --workspace`）は
2026-09-26 に実施完了（release run 36232494978、13 クレートすべてが crates.io
インデックスへ反映されたことを確認済み。詳細は
[`docs/design/crates-io-release.md`](docs/design/crates-io-release.md) 7.5 節・8 節チェックリスト参照）。

BREAKING CHANGE はありません（後方互換な API 追加とセキュリティ修正のみ）。

### Added

- `fandhe-backend-plugin-websocket`: サーバー起点で任意タイミングにメッセージを
  push できる送信ハンドル `WsSender` を追加しました。`run_session` の受信ループへ
  bounded mpsc の outbound チャネルを合流させ、cancel（最優先）→
  （クライアント受信 or アイドル期限）→ outbound（push）の優先順で処理します
  （イシュー [#670](https://github.com/Fandhe-AI/fandhe-backend/issues/670)、PR
  [#682](https://github.com/Fandhe-AI/fandhe-backend/pull/682)）
- `fandhe-backend-plugin-websocket`: `WsMessageHandler::on_open`（既定 no-op）と
  `WsOpenContext` を追加し、101 応答送出成功後に `WsSender` をハンドラへ渡す経路を
  確立しました（ハンドシェイク失敗・101 送出前キャンセルでは呼ばれないフェイル
  クローズ契約。チャネル容量は `handler::DEFAULT_OUTBOUND_CAPACITY = 8` 固定、
  イシュー [#671](https://github.com/Fandhe-AI/fandhe-backend/issues/671)、PR
  [#684](https://github.com/Fandhe-AI/fandhe-backend/pull/684)）
- `fandhe-backend-plugin-websocket`: `WebSocketConfig::with_path_pattern` +
  `pattern::PathPattern` により `{name}` パスパラメータ付きパターン登録・
  マッチングに対応しました（CDP 互換サーバーの `/devtools/page/{id}` 等、イシュー
  [#675](https://github.com/Fandhe-AI/fandhe-backend/issues/675)、PR
  [#683](https://github.com/Fandhe-AI/fandhe-backend/pull/683)）
- `fandhe-backend-plugin-websocket`: `WsOpenContext::param` / `params`
  （非デコード契約、`Debug` 出力には含めない）で、`with_path_pattern` が抽出した
  パスパラメータを `on_open` ハンドラから読めるようにしました。完全一致パスのみの
  設定では従来どおりパラメータは空です（イシュー
  [#676](https://github.com/Fandhe-AI/fandhe-backend/issues/676)、PR
  [#687](https://github.com/Fandhe-AI/fandhe-backend/pull/687)）

上記 4 件に伴い、`fandhe-backend-plugin-websocket` の `tokio` feature へ
`sync`（bounded mpsc 用）を追加しました（`io-util` / `time` / `sync` の 3 つ、
新規クレート増分なし）。

### Security

- `fandhe-backend-plugin-static`: URL セグメントの字句検証で `:` を拒否し、
  Windows のドライブ相対パスへの迂回と NTFS ADS 構文を遮断しました
  （`is_safe_segment` / `is_safe_segment_relaxed`）。あわせて `canonicalize` 後の
  実パスを root 配下で再検証する `resolved_path_is_safe` を追加し、8.3 短縮
  ファイル名エイリアスによるドットファイル保護のバイパスも防ぎます。二層防御が
  3 OS（`ubuntu-latest` / `macos-latest` / `windows-latest`）で一貫して機能する
  ことをテストで確認済みです（本番コードは元々 `cfg(unix)` 非依存。イシュー
  [#680](https://github.com/Fandhe-AI/fandhe-backend/issues/680)、PR
  [#689](https://github.com/Fandhe-AI/fandhe-backend/pull/689)）

## [0.4.0] - 2026-08-13

公開対象 13 クレートを lockstep バンプ（`docs/design/crates-io-release.md` 7.4 節）。
実 publish（`v0.4.0` タグ push → verify → dry-run → GitHub Environments
`crates-io-release` の required reviewers 承認 → `cargo publish --workspace`）は
2026-08-13 に実施完了（release run 31681376502、13 クレートすべてが crates.io
インデックスへ反映されたことを確認済み。詳細は
[`docs/design/crates-io-release.md`](docs/design/crates-io-release.md) 7.4 節・8 節チェックリスト参照）。

### BREAKING CHANGES

1. **`fandhe-backend-http`**: `RequestHead` の `method` / `target` フィールドを
   `pub String` から非公開へ変更し、`RequestHead::method()` /
   `RequestHead::target()` の `&str` を返すアクセサ経由でのみ取得する契約に
   しました（イシュー
   [#591](https://github.com/Fandhe-AI/fandhe-backend/issues/591)、性能改善
   ツリー [#579](https://github.com/Fandhe-AI/fandhe-backend/issues/579)
   Phase 3。設計は
   [`docs/design/zero-copy-request-head.md`](docs/design/zero-copy-request-head.md)）。
   内部表現をヘッド部 1 個の所有バッファ（`Box<str>`）+ 各フィールドを
   `Range<usize>` として保持する方式へ変更し、1 リクエストあたりの追加
   ヒープアロケーションをヘッダ本数 N に依存しない定数個（`buf` 1 +
   `headers` 1）へ削減しています。`version` は `Copy` 型で alloc 削減の
   動機がないため引き続き `pub` のまま維持します。
   - **移行手順**: `head.method` / `head.target` によるフィールド直接
     アクセスは全てコンパイルエラーになります。`head.method()` /
     `head.target()`（いずれも `&str` を返す）へ書き換えてください。
     `String` を要する箇所（所有権が必要な場合等）は `head.method()
     .to_owned()` のように明示的に変換してください。値の意味・検証内容
     （RFC 9110 tchar トークン検証・制御文字非許可等）に変更はありません。
   - **互換性方針**: 本変更は `fandhe-backend-http` の 0.x 系における
     破壊的変更として次回 lockstep バンプ（マイナーバージョン、
     `docs/design/crates-io-release.md` 7 節）で 13 クレート一斉に反映
     します。crates.io 公開済みバージョンからの更新時は上記移行手順に
     従ってビルドエラーを解消してください。
   - **拡張点 trait への影響なし**（イシュー
     [#592](https://github.com/Fandhe-AI/fandhe-backend/issues/592)）: 4 拡張点
     trait（`Middleware` / `UpgradeHandler` / `RequestGate` / `Interceptor`）
     および `Handler::handle` / `Handler::handle_streaming` は、いずれも
     引数が `&RequestHead`（共有参照）のままでシグネチャ変更はありません。
     これら trait を実装するプラグイン・利用者コードは、実装本体内で
     `head.method` / `head.target` を直接参照している箇所のみ
     `head.method()` / `head.target()` へ書き換えれば移行が完了します
     （workspace 内 core/routes/plugin-\* クレートの追随は
     [#602](https://github.com/Fandhe-AI/fandhe-backend/pull/602) で先行実施
     済み。詳細な棚卸し結果は
     [`docs/design/zero-copy-request-head.md`](docs/design/zero-copy-request-head.md)
     の「#592 実施記録」節を参照）。

### Docs

- **`fandhe-backend-core`**: `BoundServer::run_until`（最終 graceful shutdown）
  および `RebindHandle::rebind`（旧世代 drain）における idle keep-alive 接続
  の扱いをリファレンスへ明記しました（イシュー
  [#518](https://github.com/Fandhe-AI/fandhe-backend/issues/518)）。挙動の
  変更はなく、#313 実装当初から一貫していた既存挙動を**公開契約**として
  明文化したものです：drain 開始を理由に idle keep-alive 接続を即座には
  閉じない・grace 期限内に処理が完了する範囲で後続リクエストを受理・完走し
  `Connection: close` を伴って応答する（grace 超過時は強制クローズが優先し
  処理途中でも abort されうる）・Upgrade リクエストは 503 で拒否される・
  接続は grace + ε 以内に必ず閉じる、の 4 点。詳細・判断根拠は
  [`docs/design/graceful-shutdown.md`](docs/design/graceful-shutdown.md) 7.2 節、
  rebind 経由の同一契約は
  [`docs/design/rebind.md`](docs/design/rebind.md) 5.6 節、利用者向け解説は
  [`docs/guide/graceful-shutdown.md`](docs/guide/graceful-shutdown.md) を参照してください。

## [0.3.0] - 2026-08-05

公開対象 13 クレートを lockstep バンプ（`docs/design/crates-io-release.md` 7.3 節）。
実 publish（`v0.3.0` タグ push → verify → dry-run → GitHub Environments
`crates-io-release` の required reviewers 承認 → `cargo publish --workspace`）は
2026-08-05 に実施完了（release run 31012481870、13 クレートすべてが crates.io
インデックスへ反映されたことを確認済み。詳細は
[`docs/design/crates-io-release.md`](docs/design/crates-io-release.md) 7.3 節・8 節チェックリスト参照）。

### BREAKING CHANGES

1. **`fandhe-backend-core`**: `RequestGate::check` のシグネチャへ `ctx: &GateContext`
   引数を追加しました（イシュー
   [#486](https://github.com/Fandhe-AI/fandhe-backend/issues/486)）。
   `GateContext::peer_addr()` で accept したソケットの実 peer address を
   `RequestGate` 実装から参照できるようになります（`X-Forwarded-For` 等の
   偽装可能なヘッダとは別物で、実 TCP 接続の peer を運びます）。
   - **移行手順**: 既存の `fn check(&self, head: &RequestHead) -> GateOutcome` を
     `fn check(&self, head: &RequestHead, _ctx: &GateContext) -> GateOutcome`
     へ書き換えるだけで従来どおり動作します。peer address に基づく判定
     （CIDR 照合等）を追加したい実装は `ctx.peer_addr()` を参照してください。
     `tokio::io::duplex` 等の非ソケット経路（`handle_connection`）では常に
     `None` になるため、peer addr を判定に必須とする実装は `None` の場合に
     必ず `GateOutcome::Reject` を返してください（フェイルクローズ、
     `GateContext` の doc を参照）。実 peer address を注入したい呼び出し元は
     新設の `handle_connection_with_peer_addr` を使ってください。
2. **`fandhe-backend-plugin-websocket`**: `handle_upgrade` のシグネチャへ
   キャンセル `Future` 引数 `cancel: C where C: Future<Output = ()>` を
   追加しました（イシュー
   [#492](https://github.com/Fandhe-AI/fandhe-backend/issues/492)、設計は
   [#490](https://github.com/Fandhe-AI/fandhe-backend/issues/490)・
   `docs/design/ws-cancellation-propagation.md`）。コア（`fandhe-backend-core`）
   の `UpgradeHandler` 経由の呼び出しは自動的に世代キャンセルシグナルを渡す
   ため影響を受けません。`handle_upgrade` を直接呼び出しているコード
   （テスト等）が対象です。
   - **移行手順**: キャンセル不要な呼び出しは第 5 引数に
     `std::future::pending::<()>()` を渡してください（従来と同じ「キャンセル
     されない」挙動になります）。キャンセル発火時は、ハンドシェイク開始前
     なら 101 応答を送出せず終了、セッション確立後なら正常な Close
     ハンドシェイク（close code 1001 Going Away・固定 reason）を送出し、
     応答（または EOF）を最大 `WebSocketConfig::close_grace`（既定 10 秒、
     イシュー [#500](https://github.com/Fandhe-AI/fandhe-backend/issues/500)
     でビルダー設定可能化）待ってから終了します。

### Changed

- `fandhe-backend-plugin-websocket`: `handle_upgrade` のキャンセル `Future`
  （イシュー [#492](https://github.com/Fandhe-AI/fandhe-backend/issues/492)）
  の適用範囲を、受信待ちだけでなくユーザーハンドラ（`WsMessageHandler::
  on_message`）実行中・`WsOutcome::Reply`/`WsOutcome::Close` の送出中にも
  拡大し、即座に打ち切って正常な Close ハンドシェイク（close code 1001
  Going Away）へ分岐するようにしました（イシュー
  [#499](https://github.com/Fandhe-AI/fandhe-backend/issues/499)、設計は
  `docs/design/ws-cancellation-propagation.md` 10 節）。シグネチャ変更は
  ありません。
  - **移行時の注意（BREAKING CHANGE ではないが挙動変更）**: `on_message` が
    返す `Future` は shutdown・rebind 世代 drain の発火時に**任意の
    `await` 点で drop されうる**契約へ変更されました。中断されると困る
    副作用（完了保証が必要な書き込み等）を伴うハンドラ実装は、当該処理を
    `tokio::spawn` でセッションから切り離してください
    （`WsMessageHandler::on_message` の doc を参照）。

### Added

- `fandhe-backend-plugin-websocket`: `WebSocketConfig::with_close_grace` を
  追加し、キャンセル発火時・アイドルタイムアウト発火時の両経路が共有する
  Close handshake ドレイン猶予（既定 10 秒）を利用者が設定可能にしました
  （イシュー [#500](https://github.com/Fandhe-AI/fandhe-backend/issues/500)）。
  `Duration::ZERO`（即終端）・既定より大きい値も受け付けます（クランプなし。
  DoS 観点の考慮は `WebSocketConfig::close_grace` の doc を参照）。無効化
  （無期限待ち）を許す `Option<Duration>` にはせず、常に有界な `Duration`
  として設計しています（フェイルクローズ）
- `fandhe-backend-core`: 稼働中の `BoundServer` へ listener 差し替え（rebind）を
  指示できる `BoundServer::rebind_handle` / `RebindHandle::rebind` を追加
  （イシュー [#485](https://github.com/Fandhe-AI/fandhe-backend/issues/485)）。
  `rebind()` は呼び出し側で新規 `TcpListener` を bind してから accept ループへ
  差し替えを依頼する構造のため、bind 失敗時は旧 listener・in-flight 接続に
  一切影響しない（fail-closed）。差し替え直前までの「旧世代」接続は
  `Server::shutdown_grace_period` を上限に完走を待ち、超過分は強制クローズする
  （`run_until` 自体はブロックしない、詳細は
  `docs/design/rebind.md` を参照）。`rebind_handle` を一度も呼ばなければ
  `mpsc` チャネルは生成されず、既存の `run()` / `run_until` の挙動は不変
  （pay-for-what-you-use）
- `fandhe-backend-plugin-compression`: `CompressionConfigBuilder::blocking_threshold`
  （既定 64 KiB）を追加し、`apply_compression` を `plan_compression` /
  `compress_body` / `attach_compressed` の 3 関数へ分割公開（イシュー
  [#468](https://github.com/Fandhe-AI/fandhe-backend/issues/468)）
- `fandhe-backend-core`: `UpgradeHandler` 経由で WebSocket へ委譲された接続へ、
  最終 graceful shutdown（#313）・rebind 世代 drain（#485/#488）双方の世代
  キャンセルシグナルを配線しました（イシュー
  [#491](https://github.com/Fandhe-AI/fandhe-backend/issues/491)、設計は
  [#490](https://github.com/Fandhe-AI/fandhe-backend/issues/490)・
  `docs/design/ws-cancellation-propagation.md`）。世代ごとの
  `tokio::sync::watch` チャネルを drain 開始時（最終 shutdown は
  `shutdown_flag` を立てた直後、rebind は背景 drain タスクの冒頭）に 1 回
  発火し、委譲済み WS 専用タスクへ伝播します。新規依存・tokio feature 追加は
  ゼロ（コアの既存 `sync` feature 内で完結、`plugin-websocket` への委譲境界は
  キャンセル `Future` として渡し `sync` feature を要求しない）。
- `fandhe-backend-plugin-websocket`: `handle_upgrade` がキャンセル `Future`
  発火時に正常な Close ハンドシェイク（close code 1001 Going Away → 応答を
  最大 10 秒待つ有界ドレイン）で切断するようになりました（イシュー
  [#492](https://github.com/Fandhe-AI/fandhe-backend/issues/492)）。#491 が
  導入した中間実装（キャンセル発火時に `Future` を drop する TCP
  ハードクローズ）を置き換え、WS プロトコルレベルで正常な切断を行います
  （breaking change、上記 BREAKING CHANGES 節を参照）。両経路（最終
  shutdown・rebind 世代 drain）の end-to-end 検証は、Close に応答しない
  居座りクライアントの有界終端・rebind 反復での permit 単調消費なしを
  含めてイシュー [#493](https://github.com/Fandhe-AI/fandhe-backend/issues/493)
  の統合テスト（`crates/core/tests/ws_cancellation.rs`）が担保します
  （`BoundServer::run_until` / `RebindHandle::rebind` の doc「既知の限界」
  から、解消済みの WS 除外記述も同イシューで更新済み）。
- `fandhe-backend-plugin-webrtc`: `close_active_peers` / `drain_for_shutdown`
  （`drain` モジュール）を追加しました。`WebRtcConfig::registry` 上のアクティブな
  `RTCPeerConnection` を、1 接続あたり有界タイムアウトで明示的に `close()`
  します。`drain_for_shutdown` は加えて `WebRtcConfig::begin_terminal_drain`
  を呼び、以降のシグナリング成功をレジストリへ登録させずフェイルクローズで
  拒否します（イシュー
  [#498](https://github.com/Fandhe-AI/fandhe-backend/issues/498)）。追加のみで
  breaking change ではありません
- `fandhe-backend-core`: WS 委譲タスクの世代キャンセル機構（#489〜#497）を WS
  以外の長時間委譲プラグインへ水平展開する第 1 弾として、`SessionDrain`
  （`webrtc` feature ゲート、`GenerationCancel`/`UpgradeCancel` とは独立した
  新シーム）を追加しました。`Server::webrtc` 登録済みの `WebRtcConfig` へ、
  最終 graceful shutdown（#313）は `crate::plugin::SessionDrain::fire(true)`
  （`drain_for_shutdown` 相当）、rebind 世代 drain（#485/#488）は
  `fire(false)`（`close_active_peers` 相当）を発火します（イシュー
  [#498](https://github.com/Fandhe-AI/fandhe-backend/issues/498)、設計は
  `docs/design/ws-cancellation-propagation.md` 10 節）。棚卸しの結果、他の
  プラグイン（graphql / openapi / cors / compression / static / tracing /
  webrtc-proxy）は展開不要と判断しています（同 10.1 節）。新規依存・tokio
  feature 追加はゼロ

### Changed

- `fandhe-backend-core`: `finalize_response`（非公開シーム）が body 長
  `blocking_threshold` 以上の gzip 圧縮を `tokio::task::spawn_blocking` へ
  切り離すようになりました。接続タスクの tokio ワーカスレッドが巨大応答の
  圧縮で長時間占有され他タスクのテールレイテンシへ波及する問題への対処
  （しきい値未満は従来どおりインライン実行、応答は不変。実測根拠は
  `benches/reports/issue468-compression-blocking.md`・`docs/design/
  plugin-boundary.md` 5.10.7 節、イシュー
  [#468](https://github.com/Fandhe-AI/fandhe-backend/issues/468)）

### Fixed

- `fandhe-backend-core`: `rebind()`（イシュー
  [#485](https://github.com/Fandhe-AI/fandhe-backend/issues/485)）で listener を
  差し替える際、旧 listener の accept backlog に滞留していた「3-way handshake
  完了済みだが未 `accept()`」の接続が RST を受け取っていた事象を緩和しました。
  差し替え直前に非ブロッキング・有界（`REBIND_BACKLOG_DRAIN_LIMIT` 件・
  `max_connections` の permit ゲート範囲内）に旧 backlog を drain してサーブする
  ようになります。permit 枯渇時の滞留分は引き続き RST（`docs/design/rebind.md`
  7 節、イシュー
  [#501](https://github.com/Fandhe-AI/fandhe-backend/issues/501)）

## [0.2.0] - 2026-08-01

イシュー [#437](https://github.com/Fandhe-AI/fandhe-backend/issues/437) で公開対象
13 クレートを lockstep バンプし、`v0.2.0` タグ push → verify → dry-run →
GitHub Environments `crates-io-release` の承認を経て `release.yml` から
crates.io へ公開済み（[`docs/design/crates-io-release.md`](docs/design/crates-io-release.md)
6 節・7.1 節）。

### BREAKING CHANGES

1. **`fandhe-backend-core`**: `GateOutcome::Reject` を `{ status: u16, body: Vec<u8> }` から、
   構築時検証済みの `Response`（`fandhe-backend-http`）を運ぶ `{ response: Response }` へ
   変更しました（イシュー [#424](https://github.com/Fandhe-AI/fandhe-backend/issues/424)、
   PR [#431](https://github.com/Fandhe-AI/fandhe-backend/pull/431)）。
   - **移行手順**: ヘッダ不要の拒否応答は `GateOutcome::reject(status, body)` ヘルパへ
     書き換えてください（従来と同じワイヤ出力になります）。ヘッダ付き拒否応答
     （例: レート制限の `429 + Retry-After`）は `Response` を構築し
     `GateOutcome::Reject { response }` を返してください。拒否応答が必ず `Response`
     構築時のヘッダ検証（CR/LF/NUL 拒否・予約ヘッダ拒否）を通るようになり、
     `RequestGate` 実装からのヘッダインジェクションを型レベルで防ぐセキュリティ強化を
     兼ねています。
2. **`fandhe-backend-plugin-static`**: 非 `#[non_exhaustive]` の公開 enum
   `StaticConfigError` へ `InvalidMimeMapping` バリアントを追加しました（イシュー
   [#423](https://github.com/Fandhe-AI/fandhe-backend/issues/423)、PR
   [#430](https://github.com/Fandhe-AI/fandhe-backend/pull/430)）。
   - **移行手順**: `StaticConfigError` を網羅 `match` しているコードに
     `InvalidMimeMapping` の腕（または `_` フォールバック）を追加してください。

### Added

- `fandhe-backend-core`: 3 拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）で
  表現できないリダイレクト・レスポンス改変向けの拡張点 `Interceptor` を追加
  （`intercept` / `map_response` の 2 フック、`Server::interceptor` で複数登録可）
  （イシュー [#420](https://github.com/Fandhe-AI/fandhe-backend/issues/420)、PR
  [#428](https://github.com/Fandhe-AI/fandhe-backend/pull/428)）
- `fandhe-backend-core`: `Interceptor::map_response` を `Handler::handle_streaming` の
  ストリーミング応答ヘッドにも適用（PR
  [#443](https://github.com/Fandhe-AI/fandhe-backend/pull/443)）
- `fandhe-backend-core`: `static` / `compression` feature 有効時に設定型
  `StaticFilesConfig` / `CompressionConfig` を `plugin_static` / `plugin_compression`
  として再エクスポート（プラグインクレートへの直接依存を追加不要に）（イシュー
  [#421](https://github.com/Fandhe-AI/fandhe-backend/issues/421)、PR
  [#429](https://github.com/Fandhe-AI/fandhe-backend/pull/429)）
- `fandhe-backend-plugin-static`: `StaticFilesConfigBuilder::mime(ext, content_type)` で
  内蔵 MIME テーブルにない拡張子を利用者が追加登録可能に。内蔵テーブルへ
  `.webmanifest` 等の PWA/SSG 頻出拡張子も追加（イシュー
  [#423](https://github.com/Fandhe-AI/fandhe-backend/issues/423)、PR
  [#430](https://github.com/Fandhe-AI/fandhe-backend/pull/430)）
- `fandhe-backend-plugin-static`: `StaticFilesConfigBuilder::fallthrough_on_miss`
  （既定 `false`）で未ヒット GET を下流 `Handler` へフォールスルー可能に（mount `/` +
  動的エンドポイント共存構成、イシュー
  [#419](https://github.com/Fandhe-AI/fandhe-backend/issues/419)、PR
  [#427](https://github.com/Fandhe-AI/fandhe-backend/pull/427)）
- `fandhe-backend-plugin-compression`: `CompressionConfigBuilder::compress_streaming`
  （既定 `false`、opt-in）で `Handler::handle_streaming` の chunked ストリーミング
  応答へチャンク単位の gzip 圧縮を適用可能に。body 全体をバッファリングしない
  専用エンコーダ（`StreamingGzipEncoder`）+ コア側の第 5 のシーム
  （`prepare_streaming_compression`）で、既存のバックプレッシャ・応答完全性契約を
  壊さずに接続（HTTP/1.1 chunked 経路限定、HTTP/1.0 は対象外。イシュー
  [#461](https://github.com/Fandhe-AI/fandhe-backend/issues/461)）

### Fixed

- `fandhe-backend-plugin-static`: 末尾スラッシュ 1 個（`<mount>/dir/`）のディレクトリ
  URL で `index.html` を解決するよう修正（連続スラッシュ拒否は維持、イシュー
  [#418](https://github.com/Fandhe-AI/fandhe-backend/issues/418)、PR
  [#426](https://github.com/Fandhe-AI/fandhe-backend/pull/426)）

## [0.1.0] - 2026-07-21

公開対象 13 クレートの初回 publish。

