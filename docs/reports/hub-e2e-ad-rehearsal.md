# イシュー #243 作業項目4 — E2E ハーネス検証項目 A・D のローカル PostgreSQL 予行

## 位置づけ（必読）

- **これは Phase 2 実測ではない。** `docs/acceptance/req9-hub-e2e.md`（イシュー #97 の
  正式受け入れレポート）が記録する Phase 2 実測（`micro-service-hub` 側 Outbox
  Relay・同意管理サービスとの実結線検証）とは別物であり、本レポートをもって #97 の
  受け入れ基準充足とはみなさない。`docs/acceptance/req9-hub-e2e.md` は本予行を理由に
  一切書き換えていない。
- 本予行は `scripts/accept/hub-e2e-accept.sh` が定める検証項目 A（越境アクセス時の
  0 行）・D（`pg_class.relforcerowsecurity` による RLS 適用確認）について、**使い捨ての
  ローカル PostgreSQL コンテナ**上でハーネスと同型の SQL が意図どおり動くこと・
  想定スキーマで RLS が実際に機能することを確認する目的のみで実施した。B（同意
  フィルタの実データ整合）・C（Outbox Relay 配送）は対象外のまま SKIP とした
  （micro-service-hub 側サービスが存在しないローカル環境では検証不能なため）。
- `docs/design/outbox-consent-integration.md` §11.1 は `outbox` テーブルの実カラム
  定義（配送状態列の名称・型等）を **micro-service-hub 側確定待ちの未決事項**として
  明記している。本予行で用いたスキーマは PoC-6 型（`id, org_id, event_type,
  payload`）を踏襲した**想定スキーマ**であり、本番の `micro-service-hub` 実スキーマ
  との整合は一切保証されない。

## 実施日・環境

| 項目 | 値 |
|------|-----|
| 実施日 | 2026-07-19 |
| コンテナランタイム | Docker（Docker Engine 29.5.3、Client/Server 確認済み。podman は本環境未導入） |
| PostgreSQL イメージ | `postgres:16`（16.14, Debian 16.14-1.pgdg13+1） |
| コンテナ名 | `fandhe-hub-e2e-rehearsal-20260719`（既存の稼働中コンテナ群 `supabase_*` とは
  独立、命名衝突・ポート衝突なしを事前確認） |
| 公開ポート | ホスト `55432` → コンテナ `5432`（`ss -ltn` で事前に空きを確認。
  既存の supabase コンテナ群は `54323` 系列を使用） |
| psql クライアント | ホスト側に未導入のため、`docker exec` でコンテナ内蔵の
  `psql (PostgreSQL) 16.14` を使用（`scripts/accept/hub-e2e-accept.sh` が要求する
  ホスト側 psql 前提とは異なる代替手段。「4. 実施手順」参照） |
| ハーネススクリプトの直接実行可否 | 不可。`check_prerequisites` がホスト PATH 上の
  `psql`（未導入）と 4 環境変数（`HUB_E2E_PG_URI` 等）を必須とするため `exit 2` で
  即終了する。加えて B（`check_consent_filter_parity`）は同意管理サービス未起動時
  `record_fail` を返す実装であり、B/C を意図的に対象外とする本予行の趣旨と
  合わないため、A/D 相当の SQL を手動抽出して個別実行した |

## 実施手順（要約）

1. `which docker podman` → `docker` のみ利用可能（`docker version` / `docker ps` で
   デーモン到達性も確認）。podman は未導入。
2. `docker run -d --name fandhe-hub-e2e-rehearsal-20260719 -e POSTGRES_PASSWORD=... \
   -e POSTGRES_DB=hub_e2e_rehearsal -p 55432:5432 postgres:16` で使い捨てコンテナを起動。
3. コンテナ内で PoC-6 型スキーマ（下記）を作成し、`app_user`（`NOSUPERUSER
   NOBYPASSRLS`）ロールでハーネス相当の A/D クエリを実行。
4. 検証後、`docker rm -f -v fandhe-hub-e2e-rehearsal-20260719` でコンテナ・匿名
   ボリュームを削除。`docker ps -a` / `docker volume ls` で残存なしを確認。

### スキーマ定義（想定・§11.1 未決）

```sql
CREATE TABLE outbox (
    id text PRIMARY KEY,
    org_id text NOT NULL,
    event_type text NOT NULL,
    payload jsonb NOT NULL
);

-- 検証用シードデータ。越境クエリが「テーブルが空だから 0 行」という自明な結果に
-- ならないよう、org-a/org-b 双方に事前データを投入してから RLS を有効化した。
INSERT INTO outbox (id, org_id, event_type, payload) VALUES
    ('seed-b-1', 'org-b', 'seed-check', '{}'::jsonb),
    ('seed-b-2', 'org-b', 'seed-check', '{}'::jsonb),
    ('seed-a-1', 'org-a', 'seed-check', '{}'::jsonb);

ALTER TABLE outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE outbox FORCE ROW LEVEL SECURITY;

-- current_setting の第2引数 true（missing_ok）で、SET LOCAL 未設定時に例外ではなく
-- NULL を返させ、NULL = org_id が常に unknown（偽扱い）となるフェイルクローズを
-- 実現する（docs/design/outbox-consent-integration.md 6 節）。
CREATE POLICY outbox_tenant_isolation ON outbox
    USING (org_id = current_setting('app.current_org_id', true));
```

### 検証用ロール（重要な発見）

当初 `postgres`（イメージ既定のスーパーユーザ）で A のクエリを実行したところ、
`FORCE ROW LEVEL SECURITY` を設定していても越境行が **2 行返り FAIL 相当**となった。
PostgreSQL の仕様上、**スーパーユーザは `FORCE ROW LEVEL SECURITY` の有無に関わらず
常に RLS をバイパスする**ため、ハーネスが想定する RLS 実運用（アプリ接続ロールは
非スーパーユーザ）を模擬するには、スーパーユーザでの検証は不適切と判断した。

これを踏まえ、`NOSUPERUSER NOBYPASSRLS` の `app_user` ロール（`outbox` への
`SELECT`/`INSERT` 権限のみ付与、テーブル所有者ではない）を作成し、以降の A 検証は
すべて `app_user` で実行した。

## 検証 SQL（`scripts/accept/hub-e2e-accept.sh` の `check_cross_tenant_zero_rows` /
`check_rls_force_applied` と同型）

```sql
-- A: テナント A コンテキストでの越境クエリ（テナント B 行を参照）
BEGIN;
SET LOCAL app.current_org_id = 'org-a';
SELECT count(*) FROM outbox WHERE org_id = 'org-b';
ROLLBACK;

-- A: コンテキスト未設定時の同クエリ
BEGIN;
SELECT count(*) FROM outbox WHERE org_id = 'org-b';
ROLLBACK;

-- 陽性対照1: テナント B コンテキストで自テナント行を参照（0 行にならないことの確認）
BEGIN;
SET LOCAL app.current_org_id = 'org-b';
SELECT count(*) FROM outbox WHERE org_id = 'org-b';
ROLLBACK;

-- 陽性対照2: テナント A コンテキストで自テナント行を参照
BEGIN;
SET LOCAL app.current_org_id = 'org-a';
SELECT count(*) FROM outbox WHERE org_id = 'org-a';
ROLLBACK;

-- D: RLS 適用カタログ確認
SELECT relforcerowsecurity FROM pg_class WHERE relname = 'outbox';
```

## 結果

| 検証 | クエリ | 実測 | 期待 | 判定 |
|------|--------|------|------|------|
| A（スコープあり） | `app_user`・`org-a` コンテキスト下でテナント B 行を参照 | `0` | `0` | 一致 |
| A（コンテキスト未設定） | `app_user`・コンテキスト未設定でテナント B 行を参照 | `0` | `0` | 一致 |
| 陽性対照1 | `app_user`・`org-b` コンテキスト下でテナント B 自身の行を参照 | `2` | `>0` | 一致（RLS が空テーブルではなく実際にフィルタしていることの確認） |
| 陽性対照2 | `app_user`・`org-a` コンテキスト下でテナント A 自身の行を参照 | `1` | `>0` | 一致 |
| D | `pg_class.relforcerowsecurity`（`outbox`） | `t` | `t` | 一致 |

**A: 越境アクセス 0 行 — 予行では成功（`app_user` ロール使用時。スーパーユーザ
`postgres` では RLS がバイパスされ 2 行返り FAIL 相当だった点は上記「検証用ロール」節
に記録）。**

**D: `relforcerowsecurity = t` — 予行では成功。**

## 後始末

```
$ docker rm -f -v fandhe-hub-e2e-rehearsal-20260719
fandhe-hub-e2e-rehearsal-20260719
$ docker ps -a --filter "name=fandhe-hub-e2e-rehearsal"
CONTAINER ID   IMAGE     COMMAND   CREATED   STATUS    PORTS     NAMES
$ docker volume ls | grep -i rehearsal
（該当なし）
```

コンテナ・匿名ボリュームともに削除済みを確認した。他の並列稼働コンテナ
（`supabase_*` 系列）には影響していない。

## Phase 2 実測との差異・限界（本予行が保証しないこと）

- 本予行のスキーマは PoC-6 型を踏襲した**想定**であり、`micro-service-hub` 側の実
  カラム定義（§11.1 未決）とは異なる可能性がある。配送状態列（`delivered_at` 等）を
  含んでおらず、C（Outbox Relay 配送）の検証は行っていない。
- `app.current_org_id` へバインドする値は文字列リテラル固定（`'org-a'` /
  `'org-b'`）であり、ハーネス本体のようにパラメータクエリ変数（`psql -v` +
  `:'var'` 構文）を接続文字列経由で外部から与える経路までは再現していない
  （検証中に `docker exec` 経由の `-c` オプションでは変数展開が機能しない事象が
  発生したため、ヒアドキュメント経由の複数行スクリプトに切り替えて実行した。
  SQL 自体はハーネスと同型）。
- 同意管理サービス・Outbox Relay が存在しないため B・C は本予行でも対象外
  （SKIP のまま）。
- 本予行は `scripts/accept/hub-e2e-accept.sh` を接続情報付きでそのまま実行した
  ものではなく、A/D 相当の SQL を手動抽出して実行した記録である。

## 関連

- 検証ハーネス: `scripts/accept/hub-e2e-accept.sh`
- 未決事項一覧: `docs/design/outbox-consent-integration.md` §11.1
- Phase 2 正式受け入れレポート（本予行では書き換えていない）: `docs/acceptance/req9-hub-e2e.md`
- 元イシュー: #243（作業項目4）、親イシュー #97（REQ-9 後続 E2E 統合検証）
