# JWKS の取り直し（jwks-refresh）

`fandhe-backend-plugin-jwt` の opt-in feature `jwks-refresh` は、ゲートの外で
`SharedKeySet` を最新に保つ非同期タスクを提供します。JWT の署名鍵（JWKS）を発行元が
ローテーションしても、サーバを再起動せずに追従できます。

## 概要

取り直しタスクが行うこと:

- 一定の周期で JWKS を取得し、`SharedKeySet` を差し替える（定期取得）
- 知らない `kid` のトークンが来たときに、最小間隔の下限を守って取り直す
- 取得に失敗したら現行の鍵を保持し続ける。`max_stale`（opt-in）を超えて更新できない
  場合は鍵集合を空にし、全トークンを拒否する（フェイルクローズ）

しないこと:

- **HTTP クライアントも TLS も持ちません。** JWKS を実際に取得する関数（`JwksFetcher`）は
  アプリが書いて渡します。どの HTTP クライアントを使うかは利用側の選択です。

## 有効化

```toml
[dependencies]
fandhe-backend-plugin-jwt = { version = "...", features = ["jwks-refresh"] }
```

無効のままなら依存もコードも増えません。有効にすると、workspace に既にある `tokio`
（`time`・`sync`）への optional 依存が加わるだけで、新しい crate は増えません。
crates.io 公開版 0.5.0 には未収録で、次の公開版から使えます。

## 取得関数の契約

`JwksFetcher::fetch` を実装するときは次の 7 項目を守ってください。取得関数が攻撃表面に
なるため、特に 1〜4 は省略できません。

1. 取得先の URL は構築時に固定し、`kid` やリクエスト由来の値から組み立てない。
   `kid` は攻撃者が決められる値で、組み立てると SSRF になる（`fetch` が引数を持たないのも
   このため）
2. HTTPS を使い、証明書の検証を無効にしない
3. リダイレクトは追わないか、同じオリジンに限る
4. 本文を読む量に上限を設ける（目安 64 KiB）。`KeySet::from_json` 側の上限は解析時の
   防御で、受信中のメモリは守らない
5. 取得関数自身にも時間切れを設ける（crate 側の `fetch_timeout` との多層防御）
6. 認証情報・本文をログに出さない
7. 返す future は任意の `await` 点で drop されうる（時間切れ・停止のとき）

あわせて、結果の返し方は次のとおりです。

| 状況 | 返す値 |
|------|--------|
| `Cache-Control: max-age=N` | `JwksResponse::new(body, Some(N 秒))` |
| `no-store`・`no-cache`・`max-age` なし | `JwksResponse::new(body, None)` |
| 接続・送信の失敗、時間切れ | `FetchError::Transport` |
| 2xx 以外の応答 | `FetchError::Status` |
| 本文が上限を超えた | `FetchError::TooLarge` |
| UTF-8 不正などその他 | `FetchError::Other` |

`FetchError` は固定文言で、本文や URL を含みません。取得関数は panic しないでください
（`panic = "abort"` の構成では panic の保護が効きません）。

## 実装例

> **このコードは説明用で、コンパイル検証の対象外です。** HTTP クライアント（ここでは
> `reqwest`）は利用側の依存であり、本フレームワークは依存しないため、リポジトリの
> example や doc test には置けません。hyper など他のクライアントでも同じ契約で書けます。
> フレームワーク側の API の呼び出し方は rustdoc の doc test（`refresh` モジュール）が
> 「正」です。クライアントの版の選定と脆弱性監査は利用側の責務です。

```rust,ignore
use std::time::Duration;

use fandhe_backend_plugin_jwt::{FetchError, FetchFuture, JwksFetcher, JwksResponse};

/// 本文の読み取り上限（契約 4）。
const MAX_BODY: usize = 64 * 1024;

pub struct HttpJwksFetcher {
    client: reqwest::Client,
    url: reqwest::Url, // 構築時に固定。kid・リクエスト値からは作らない（契約 1）
}

impl HttpJwksFetcher {
    pub fn new(url: &str) -> Result<Self, String> {
        let url = reqwest::Url::parse(url).map_err(|_| "不正な URL".to_string())?;
        if url.scheme() != "https" {
            return Err("https のみ許可".to_string()); // 契約 2
        }
        let client = reqwest::Client::builder()
            .https_only(true) // 契約 2（証明書検証は無効化しない）
            .redirect(reqwest::redirect::Policy::none()) // 契約 3
            .timeout(Duration::from_secs(4)) // 契約 5。RefreshConfig の fetch_timeout 以下にする
            .build()
            .map_err(|_| "クライアント構築に失敗".to_string())?;
        Ok(Self { client, url })
    }
}

impl JwksFetcher for HttpJwksFetcher {
    fn fetch(&self) -> FetchFuture<'_> {
        Box::pin(async move {
            // 契約 7: どの await で drop されてもよいよう、状態を持ち越さない
            let mut resp = self
                .client
                .get(self.url.clone())
                .send()
                .await
                .map_err(|_| FetchError::Transport)?;
            if !resp.status().is_success() {
                return Err(FetchError::Status);
            }
            // 宣言された長さが上限超過なら読み始めない（契約 4）
            if resp.content_length().is_some_and(|n| n > MAX_BODY as u64) {
                return Err(FetchError::TooLarge);
            }
            let max_age = resp
                .headers()
                .get(reqwest::header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_max_age);
            // bytes() で全量を読まず、累積が上限を超えたら打ち切る（契約 4）
            let mut body = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(|_| FetchError::Transport)? {
                if body.len() + chunk.len() > MAX_BODY {
                    return Err(FetchError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            let body = String::from_utf8(body).map_err(|_| FetchError::Other)?;
            // エラー値にもログにも URL・本文を含めない（契約 6）
            Ok(JwksResponse::new(body, max_age))
        })
    }
}

/// `Cache-Control` の簡易解析。`no-store`・`no-cache`・指定なしは `None`。
fn parse_max_age(v: &str) -> Option<Duration> {
    let mut max_age = None;
    for d in v.split(',').map(|d| d.trim().to_ascii_lowercase()) {
        if d == "no-store" || d == "no-cache" {
            return None;
        }
        if let Some(n) = d.strip_prefix("max-age=") {
            max_age = n.parse::<u64>().ok().map(Duration::from_secs);
        }
    }
    max_age
}
```

## 配線

```rust,ignore
use std::time::Duration;

use fandhe_backend_plugin_jwt::{
    JwksRefresher, JwtAuthenticator, JwtGate, NoExtraClaims, RefreshConfig, SharedKeySet,
};

// 起動直後は空の鍵集合から始め、最初の取得で埋める
let keys = SharedKeySet::new(/* 初期の KeySet */);

// 周期は必須。min_interval の既定は 10 秒、fetch_timeout の既定は 5 秒、max_stale は opt-in
let cfg = RefreshConfig::builder(Duration::from_secs(300))
    .max_stale(Duration::from_secs(3600))
    .build()?;

let fetcher = HttpJwksFetcher::new("https://issuer.example/.well-known/jwks.json")?;
let (task, handle) = JwksRefresher::new(keys.clone(), fetcher, cfg);
tokio::spawn(task); // time ドライバ付きのランタイムで spawn する

let auth = JwtAuthenticator::<NoExtraClaims>::builder(keys, policy)
    .jwks_refresh_trigger(handle.trigger()) // 未知 kid を契機に取り直しを合図する
    .build();
let gate = JwtGate::new(auth); // Server::gate(gate) で登録する
```

設定は構築時に検査されます（`fetch_timeout <= min_interval`、`min_interval <= period`
など。違反は `RefreshConfigError`）。`SharedKeySet` の初期値や `policy` の作り方は
`fandhe_backend_plugin_jwt` のクレート doc を参照してください。

## 運用上の注意

- タスクは **spawn しないと動きません**。spawn しない・途中で drop すると、定期取得だけでなく
  `max_stale` によるフェイルクローズも働きません。time ドライバ付き（`enable_time`）の
  ランタイムで spawn してください
- `RefreshHandle` を drop するとタスクが止まります。サーバの寿命の間は保持してください。
  明示的に止めるには `handle.shutdown()` を使います
- 取り直しの合図は `unknown_key_notifier` ではなく `jwks_refresh_trigger` に登録します。
  通知の間引きを通らず、「最小間隔 + 取得時間」以内の追従が成り立ちます
- 未知の `kid` のトークンはその場で 401 のままで、取り直しの完了は待ちません
- 発行元が `{"keys":[]}` を返した場合は成功として反映され、全トークンが拒否されます
- 観測は `RefreshObserver` と `handle.status()` で行います。観測口はブロックせず panic も
  しない契約で、件数と種別だけを渡し、本文や URL は含みません
- 反映されるのは `KeySet::from_json` の検査を通った集合だけです。検査に落ちた応答は失敗
  として扱われ、現行の鍵が残ります

## 関連

- rustdoc: `fandhe_backend_plugin_jwt::refresh`（`JwksFetcher`・`RefreshConfig`・
  `JwksRefresher` の doc test）
- 設計の詳細: `docs/design/jwt-verify-plugin.md` 8 章
