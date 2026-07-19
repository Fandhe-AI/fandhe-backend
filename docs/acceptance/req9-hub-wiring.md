# REQ-9 受け入れ検証レポート — hub 共通配線（TASK-9.5、#65）

> 注記: 本レポートは 2026-07 の crate・import 一括改名（#202）以前の実測記録であり、
> 旧クレート名（`backend-framework-core` / `bf-http` / `bf-routes` / `bf-plugin-*` 等）
> 表記のまま保持している。実測値本文は改変しない（`docs/design/framework-naming.md` 7 節）。
> 旧環境変数名 `BF_*` 表記のまま保持（#203）。

`docs/spec/05-tasks.md` TASK-9.5「hub 共通配線受け入れテスト」の受け入れ基準を
`scripts/accept/hub-wiring-accept.sh` で検証した結果。TASK-9.1（#61、TenantGate 初版）・
TASK-9.2（#62、RS256 + JWKS 化）・TASK-9.3（#63、検証結果キャッシュ）・TASK-9.4（#64、
性能最適化）は前提タスクとしてマージ済み（本レポートはそれらの実装を前提とし、
`crates/plugin-hub-wiring/src/**` の production コードは本タスクで変更していない）。

## 受け入れ基準（計画対応表）

| 記号 | 受け入れ基準 | 検証方法 |
|------|------|------|
| A | 越境クエリ 100% 遮断・JWT 欠落/不正時のフェイルクローズ | `cargo test -p bf-plugin-hub-wiring --test hub_acceptance` |
| B | 配線コード削減率が実質 100%（PoC-6 手書き実装 3 エンドポイント・207 行比） | `scripts/accept/lib/hub-wiring-loc.sh`（マーカー区間 LOC 集計・削減率評価・ハンドラ領域の手書き配線シンボル不在確認） |
| C | 依存方向・pay-for-what-you-use（プラグイン → コアの一方向依存を維持） | `cargo tree -p backend-framework-core` に `bf-plugin-hub-wiring` が現れないこと |
| D | NFR-6（無関係パスへの RPS・p95 影響が誤差範囲内） | `benches/hub-nfr6-bench.sh`（`oha` empirical 計測） |

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-18 |
| 対象コミット（作業ブランチ起点、`origin/main`） | `01b7f1c49eae1e1f99471cb77152b3cb41519e75` |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| oha | 1.15.0 |
| 備考 | NFR-6 計測は専有環境（PoC-2 同等）ではなく、並列 issue 実装ワークフロー下の
  共有 worktree 環境で実施した（`benches/reports/task-9.5-hub-wiring-performance.md` の
  環境注記を参照） |

## 判定サマリー（`scripts/accept/hub-wiring-accept.sh`）

| 判定 | 基準 | 詳細 |
|------|------|------|
| PASS | A: 越境遮断・フェイルクローズ受け入れテスト | `cargo test -p bf-plugin-hub-wiring --test hub_acceptance` 全件 PASS（16 件） |
| PASS | B: 配線コード削減率 | マーカー区間 6 行（PoC-6 基準 207 行比 削減率 97.1%、閾値 90% 以上） |
| PASS | B補足: ハンドラ領域の手書き配線シンボル不在 | `verify_token` / `RsaKeyPair` / `JwksKeySet` / `SharedJwks::new` / `TenantGateConfig::(new\|from_jwks_json)` いずれも `build_router` 内に出現なし |
| PASS | C: 依存逆転型プラグインの維持 | `cargo tree -p backend-framework-core` に `bf-plugin-hub-wiring` が現れない |
| WARN | D: NFR-6 無関係パス影響（実務許容帯内・狭義帯外） | RPS 比 98.58〜99.71% / p95 比 100.01〜101.52%（2 回実行、詳細下記。Cursor Bugbot review 4727552092 指摘1対応でリンクコスト専用最小 example `hub_link_only.rs` へ切り替え後の数値） |

**終了コード: 0（FAIL なし、PASS / WARN のみ）**

## 基準 A（越境遮断・フェイルクローズ）の詳細

`crates/plugin-hub-wiring/tests/hub_acceptance.rs`（16 テスト）が、PoC-6 相当の実データ
入りマルチテナントハンドラ（`GET /items`・`GET /items/{id}`・`POST /items`）を構えた
ダミー hub サービス構成で以下を固定する:

- **越境クエリ 100% 遮断**: org-1 トークンで org-2 の item（id 3, 4）全件へアクセスし
  1 件も漏れず 404（データ層フェイルクローズ）になること
  （`cross_tenant_get_by_id_is_blocked_for_all_foreign_ids`）。一覧・新規作成データでも
  同様に境界が保たれること（`list_endpoint_returns_only_own_tenant_rows`・
  `post_creates_item_scoped_to_caller_org_and_stays_tenant_isolated`）
- **フェイルクローズ**: JWT 欠落・空 Bearer・改竄署名・期限切れ・`alg=none`・HS256
  ダウングレード・未知 `kid`・oversized トークンはすべて 401、`org_id` 欠落/空は 403。
  いずれのケースも到達カウンタ（`Arc<AtomicU64>`）が 0 のままであることを直接検証し、
  「`RequestGate` が拒否した場合はハンドラへ到達すらしない」ことを証跡化する
- **鍵ローテーション**: `SharedJwks::set` によるローテーション後、旧鍵トークンが拒否され
  新鍵トークンが許可されること（再起動なし、`key_rotation_via_shared_jwks_rejects_old_key_tokens_without_restart`）
- **検証結果キャッシュの共有**: TASK-9.3（#63）の `Authenticator` 共有パターンが実データ
  ハンドラでも機能し、ゲート（1 ミス）→ ハンドラ（1 ヒット）の順になること
  （`handler_reuses_gate_verification_via_shared_authenticator_with_real_data`）

`bf_routes::Router` が起動時登録の完全一致のみをサポートする制約
（`crates/routes/src/lib.rs` doc）のため、単件取得は既知 4 件の ID を個別ルートとして
列挙登録する設計とした（`examples/hub_service_demo.rs` と `tests/hub_acceptance.rs` の
両方で同一パターン）。

## 基準 B（配線コード削減率）の詳細

`crates/plugin-hub-wiring/examples/hub_service_demo.rs` の `// --- wiring:begin --- 〜
// --- wiring:end ---` マーカー区間（空行・行コメント除く実 LOC）:

```rust
let config = TenantGateConfig::from_jwks_json(&jwks_json).expect("valid demo jwks");
let authenticator = config.authenticator();
let mut server = Server::new();
if env::var("BF_HUB_GATE").as_deref() != Ok("off") {
    server = server.gate(TenantGate::new(config));
}
```

6 行（`BF_HUB_GATE=off` の NFR-6 計測用分岐を含む。分岐を除けば実質 3 行）。PoC-6
手書き実装（3 エンドポイント・207 行、JWT 検証・`org_id` 抽出・スコープ強制を各
エンドポイントで手書き）との比較で削減率 97.1%。ハンドラ本体（`build_router`）は
`Authenticator::authenticate`（本クレート提供 API）のみを呼び、JWT 検証・JWKS パース・
署名検証コードを一切書いていないことを `scripts/accept/lib/hub-wiring-loc.sh` の
`detect_handwritten_auth_in_handlers` が grep ベースで確認する。

## 基準 D（NFR-6）の詳細と判断

初期実装は `hub_service_demo.rs` に `GET /` ルートを登録しておらず、無関係パス計測が
実際には「ベースライン 200」対「hub 404（未登録ルート）」という異なる応答形状を
比較する不備を含んでいた（advisor レビューで指摘、是正済み）。`crates/core/examples/
minimal.rs` と同一形状の `GET /` ハンドラを追加してから再計測している。

### 是正2（Cursor Bugbot review 4727552092 指摘1、PR #163）

上記是正後も、比較対象に `hub_service_demo.rs`（PoC-6 相当のマルチテナント
`/items` 系ハンドラ・シードストア・`Authenticator` を持つ実データ入り example）を
使い続けていたため、`webrtc-nfr6-bench.sh` / `graphql-nfr6-bench.sh` が使う
「`GET /` のみを持つ最小 example」パターンから外れ、**アプリケーション層の
オーバーヘッド（マルチルート登録・ハンドラクロージャの `Arc`/`Clone` キャプチャ量等）
がリンクコストの計測値へ混入していた**（Cursor Bugbot 指摘）。

是正: `crates/plugin-hub-wiring/examples/hub_link_only.rs`（`examples/minimal.rs` と
同一の `GET /` のみを持ち、`BF_HUB_GATE=off` 未設定時のみ空 JWKS 構成の `TenantGate`
を登録する最小 example）を新設し、`benches/hub-nfr6-bench.sh` の比較対象を
`hub_service_demo` からこちらへ切り替えた。`hub_service_demo` は実データ・実トークンを
要する opt-in コスト参考値の手動計測（下記「参考値」節）専用として引き続き使う。

`benches/hub-nfr6-bench.sh`（`oha` による empirical 計測。計測用バイナリ:
`crates/core/examples/minimal.rs` = ベースライン、
`crates/plugin-hub-wiring/examples/hub_link_only.rs`（`BF_HUB_GATE=off`）=
`bf-plugin-hub-wiring` リンク済み・`TenantGate` 未登録・アプリ層オーバーヘッドなし）を
是正後 2 回実行した結果:

| 実行 | RPS 比（hub / baseline） | p95 比（hub / baseline） |
|------|------|------|
| 1 回目（RUNS=5, DURATION=5s, CONNECTIONS=32） | 99.71% | 100.01% |
| 2 回目（RUNS=5, DURATION=5s, CONNECTIONS=32） | 98.58% | 101.52% |

詳細な生ログは `benches/reports/task-9.5-hub-wiring-performance.md` を参照。

**判断**: NFR-6 の実務許容帯 [95%, 105%] には 2 回とも収まる（狭義帯 100.3〜100.8% は
外れるため `scripts/accept/hub-wiring-accept.sh` の判定は WARN）。是正前に記録していた
FAIL（RPS 比 55.76〜85.08%）は、`hub_service_demo` のアプリ層オーバーヘッドが混入した
測定不備によるものであり、`bf-plugin-hub-wiring` を単にリンクしただけの真のコストは
実務許容帯に収まることを、アプリ層を排除した本計測で直接確認した。是正前レポートが
記録していた「同一バイナリでもポート使用履歴・タイミングで RPS が最大 4.4 倍変動する」
という環境ノイズの存在自体は別途確認済みの事実であり、本是正後の数値もその環境ノイズの
影響を受け得る点は変わらないため、専有環境での確定的な再計測は
`benches/reports/task-9.5-hub-wiring-performance.md`「専有環境での再現手順」節を参照。

**この結果を hub-wiring 配線の設計不備として扱わない理由**: `TenantGate`
（`RequestGate` 拡張点）は `Server::gate` 未登録時（`BF_HUB_GATE=off`）は一切コアの
リクエストパスに関与しない設計であり、本計測が測っているのは「クレートをリンクした
だけ（実行時にゲート登録なし）」の影響である。JWKS/署名検証の実処理コストは
TASK-9.3（#63）で既に別途計測・最適化済み（`benches/reports/task-9.3-jwt-cache-performance.md`）。

## 参考値: opt-in コスト

`benches/reports/task-9.5-hub-wiring-performance.md` の「参考値」節を参照。ゲート
有効 + 有効トークン時の `/items` 系スループットは本タスクでは自動計測しておらず
「未計測」（手動計測手順を同レポートに記載）。

## BLOCKED / フォローアップ

- **NFR-6 の専有環境再計測**: 是正2（`hub_link_only.rs` へのリンクコスト分離）後は
  実務許容帯 [95%, 105%] に収まっているが、狭義帯 100.3〜100.8% には届いておらず、
  本環境が専有環境（PoC-2 同等）ではないため環境ノイズの影響を完全には排除できない。
  `benches/nfr6-exclusive.sh`（#178、flock 相互排他 + 静穏確認、
  `docs/design/nfr6-exclusive-measurement.md`）を整備したが、本イシュー実装時点も
  並列 issue 実装ワークフロー実行中で静穏確認が成立せず、hub 対象自体の専有環境
  確定再計測は実施できなかった（`benches/reports/task-9.5-hub-wiring-performance.md`
  追補節）。専有環境での確定的な再計測は host が真に静穏な期間にフォローアップとして
  実施する（[[out-of-scope-tracking]]）。上記 WARN 判定は維持する
- **opt-in コスト（ゲート有効時の実測）**: 本タスクでは自動計測していない。TASK-9.3
  の署名検証・キャッシュコスト計測（マイクロベンチ）とは別に、E2E スループットとして
  計測する場合は性能最適化の深掘りに該当し、別課題として扱う

## 検証コマンド一覧（再現手順）

```bash
# A・B・C・D をまとめて実行
bash scripts/accept/hub-wiring-accept.sh

# 判定ロジックのオフライン・セルフテスト（cargo 非依存）
bash scripts/tests/run-hub-wiring-accept-tests.sh

# 越境遮断・フェイルクローズ受け入れテスト単体
cargo test -p bf-plugin-hub-wiring --test hub_acceptance

# NFR-6 計測用バイナリのビルド（D の前提）
cargo build --release -p backend-framework-core --example minimal --no-default-features
cargo build --release -p bf-plugin-hub-wiring --example hub_link_only

# 依存インパクトの個別確認
cargo tree -p backend-framework-core | grep -c bf-plugin-hub-wiring  # 0

# 動作確認（example 単体）
cargo run --release -p bf-plugin-hub-wiring --example hub_service_demo
# 起動時に表示される curl コマンド例（有効トークン付き）をそのまま使う
```
