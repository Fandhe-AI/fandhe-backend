# REQ-9 後続 E2E 統合検証レポート — Outbox・同意ゲート実データモデル（イシュー #97）

イシュー #97「MS-6 後続 E2E 統合検証（Outbox Relay 完了待ち）」の受け入れ基準
「Outbox・同意ゲートの実データモデルで E2E 統合検証を完了する」に対する検証記録。
検証手段は `scripts/accept/hub-e2e-accept.sh`（`docs/design/outbox-consent-integration.md`
11.2 節の 4 検証項目を実装）。

## 状態: 前提待ち（未実施）

**本レポート作成時点（2026-07-18）では実測は未実施。** 実測完了とはみなさない。

- 前提となる `micro-service-hub` の Outbox Relay（MS-5、目標 2026-09-30）・同意管理
  サービス（MS-3、目標 2026-08-31）は roadmap 上の目標日に対しいずれも未到達
- `gh repo view Fandhe-AI/micro-service-hub` は
  `Could not resolve to a Repository with the name 'Fandhe-AI/micro-service-hub'.`
  を返し、現在のアクセス権では進捗を直接確認できない。これは「未完了と断定できる
  根拠」ではなく「確認不能・人間確認要」の事実であり、両者を区別して記録する
- `docs/design/outbox-consent-integration.md` 11.1 節の未決事項（`outbox` テーブルの
  実カラム定義、`consent_grants` 実スキーマ/アクセス方式、`consent_revoked` ペイロード
  形式）も未確定

このため本 PR（Phase 1）は**検証ハーネス・レポート様式・判定記録の先行整備のみ**を
スコープとし、実測（Phase 2）は着手条件成立・ユーザー承認後に別途実施する
（対応可否判定は「可」＝Phase 1、Phase 2 は別途「条件付き可」として扱う。判定記録は
`_/local-plans/97-hub-e2e-verification.md`、`_/` 配下のためコミット対象外）。

**イシュー #97 はこのレポートをもってクローズしない。** 実測（Phase 2）完了まで open
のまま維持する（`.claude/rules/feature-modification.md` 完遂判定 3 条件のうち「受け入れ
基準充足」が本 PR の時点では未充足のため）。

## 検証項目（計画対応表、11.2 節）

| 記号 | 検証項目 | 検証方法 | 期待結果 |
|------|---------|---------|---------|
| A | 越境アクセス時の 0 行（RLS フェイルクローズ） | テナント A のセッションコンテキスト（`SET LOCAL app.current_org_id`）でテナント B の `outbox` 行をパラメータクエリで参照、およびコンテキスト未設定時の同クエリ | 双方とも 0 行（PoC-6 と同型の越境遮断ケースを実データで再実行） |
| B | 同意フィルタの実データ整合 | 実 `consent_grants`（またはサービス API）に対しオプトイン済み/未設定/取り消し済みの 3 状態を用意し、期待集合（同意済みのみ抽出・未登録テナントは全除外）と照合 | 全 3 状態で期待集合と一致（オプトイン原則の実データ確認） |
| C | Outbox Relay 配送 | 検証用イベントを `enqueue`（業務トランザクション内 INSERT 相当）し、Relay によるポーリング配送（配送状態列の遷移）をタイムアウト付きで確認 | タイムアウト（既定 30 秒）以内に配送済みと確認できる |
| D | RLS ポリシー・`SET LOCAL` の適用漏れ検知 | `pg_class.relforcerowsecurity` カタログ照会で `outbox` テーブルの `FORCE ROW LEVEL SECURITY` 適用を確認 | `relforcerowsecurity = t`（10 節 A05 の申し送り解消） |

## 実行環境（本レポート作成時点、Phase 1）

| 項目 | 値 |
|------|-----|
| 作成日 | 2026-07-18 |
| 対象コミット（作業ブランチ起点、`origin/main`） | `ac438e0`（イシュー #95 マージ時点） |
| rustc | 1.96.0 (ac68faa20 2026-05-25) |
| cargo | 1.96.0 (30a34c682 2026-05-25) |
| psql | 未導入（本 worktree に不在。検証ハーネスは前提チェック段で `exit 2` を返し PASS と偽らない） |
| curl | 8系（`/usr/bin/curl`、`--version` 未記録） |
| micro-service-hub 進捗確認 | `gh repo view Fandhe-AI/micro-service-hub` → 404（Could not resolve to a Repository）。確認不能・要人間確認 |

## 判定サマリー（Phase 1 時点）

前提環境変数（`HUB_E2E_PG_URI` / `HUB_E2E_CONSENT_API` / `HUB_E2E_ORG_A` /
`HUB_E2E_ORG_B`）を意図的に未設定として `scripts/accept/hub-e2e-accept.sh` を実行し、
fail-closed の陰性確認（未成立時に PASS を偽らないこと）のみを行った。

```
$ env -u HUB_E2E_PG_URI -u HUB_E2E_CONSENT_API -u HUB_E2E_ORG_A -u HUB_E2E_ORG_B \
    bash scripts/accept/hub-e2e-accept.sh
実行前提エラー: 以下の環境変数が未設定です（HUB_E2E_PG_URI HUB_E2E_CONSENT_API HUB_E2E_ORG_A HUB_E2E_ORG_B）
前提（micro-service-hub 側 Outbox Relay・同意管理サービスの稼働、接続情報の
安全な受け渡し）が成立するまで本スクリプトは実測できません。
docs/design/outbox-consent-integration.md 11.1/11.2 節・イシュー #97 参照。
$ echo $?
2
```

| 判定 | 基準 | 詳細 |
|------|------|------|
| （未実施） | A: 越境アクセス時の 0 行 | 前提未成立のため未実測 |
| （未実施） | B: 同意フィルタの実データ整合 | 前提未成立のため未実測。加えて同意管理サービス API 契約（11.1 節）未確定のため実装済み判定ロジック自体が SKIP を返す設計（`check_consent_filter_parity`） |
| （未実施） | C: Outbox Relay 配送 | 前提未成立のため未実測 |
| （未実施） | D: RLS ポリシー・`SET LOCAL` 適用漏れ検知 | 前提未成立のため未実測 |

**この表は「未実施」を「PASS」と混同しない（fail-closed）。実測は Phase 2 で行い、
本レポートを更新する。**

## ローカル代替環境での予行について

本 worktree には `docker`（共有デーモン、他の並列実装 worktree と共有）が導入されて
いるが、共有インフラへ新規コンテナを起動すると並列実行中の他イシューの作業に影響し
うるため、本 PR では実施しなかった（グローバル状態を変更しない運用制約）。また
`psql` クライアント自体が本 worktree に未導入であり、クエリ部分単体の疎通確認も
実施していない。予行を行う場合は隔離済みの使い捨て環境（専用コンテナ・専用
worktree）で実施し、実測（Phase 2）と混同しないことを明記した上で別途記録する。

## 着手条件（Phase 2、`.claude/rules/feasibility-guardrail.md` 6 節）

1. `micro-service-hub` 側 Outbox Relay（MS-5）・同意管理サービス（MS-3）の完了確認
   （人間によるアクセス権付与または進捗情報の提供が必要）
2. `docs/design/outbox-consent-integration.md` 11.1 節未決事項（`outbox` 実カラム
   定義・`consent_grants` 実スキーマ/アクセス方式・`consent_revoked` ペイロード形式）
   の確定情報入手
3. 検証用環境の接続情報（`HUB_E2E_PG_URI` 等）の安全な受け渡し（シークレットとして
   コミットしない経路での提供）

3 点すべて成立し、かつユーザー承認を得た上で対応可否 3 軸（実施可能か・安全か・
影響範囲が許容内か）を再判定してから Phase 2 に着手する。承認は 3 軸再判定を省略
しない（`.claude/rules/feasibility-guardrail.md` の運用どおり）。

## 検証コマンド一覧（Phase 2 実施時の再現手順）

```bash
# 4 検証項目をまとめて実行（着手条件成立後）
HUB_E2E_PG_URI="postgres://user:pass@host:5432/dbname" \
HUB_E2E_CONSENT_API="https://consent.example.internal" \
HUB_E2E_ORG_A="<org-a-uuid>" \
HUB_E2E_ORG_B="<org-b-uuid>" \
bash scripts/accept/hub-e2e-accept.sh

# 前提未成立時の fail-closed 陰性確認（実行前提エラー、exit 2）
env -u HUB_E2E_PG_URI -u HUB_E2E_CONSENT_API -u HUB_E2E_ORG_A -u HUB_E2E_ORG_B \
    bash scripts/accept/hub-e2e-accept.sh

# 構文・静的検証
bash -n scripts/accept/hub-e2e-accept.sh
shellcheck scripts/accept/hub-e2e-accept.sh
```

## 関連

- 設計・未決事項: `docs/design/outbox-consent-integration.md`（1.3 節・11 節）
- 前提タスク（クローズ済み）: TASK-9.1〜9.6（#61〜#65・#89）
- 対応可否判定記録: `_/local-plans/97-hub-e2e-verification.md`（ローカル、コミット対象外）
