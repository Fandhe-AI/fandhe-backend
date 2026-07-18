#!/usr/bin/env bash
# イシュー #97「MS-6 後続 E2E 統合検証（Outbox Relay 完了待ち）」の受け入れ検証
# ハーネス。`docs/design/outbox-consent-integration.md` 11.2 節が定める 4 検証項目
# （越境アクセス時の 0 行・同意フィルタ実データ整合・Outbox Relay 配送・RLS 適用漏れ
# 検知）を、実 PostgreSQL・実 `micro-service-hub` サービスとの結線で検証する。
#
# 前提（micro-service-hub 側）:
#   - Outbox Relay（MS-5、目標 2026-09-30）・同意管理サービス（MS-3、目標 2026-08-31）が
#     稼働していること
#   - 本スクリプト作成時点（2026-07-18）ではいずれも未完了見込みであり、本スクリプトは
#     前提チェック段で環境変数未設定を検知して `exit 2` する（fail-closed。判定不能を
#     PASS と偽らない、.claude/rules/security.md・feasibility-guardrail.md）
#
# 接続情報はすべて環境変数でのみ受け取り、既定値・実接続文字列・トークンを本スクリプト
# 内に一切ハードコードしない（OWASP A02、.claude/rules/security.md）:
#   HUB_E2E_PG_URI          … 検証用 PostgreSQL への接続文字列（例:
#                              postgres://user:pass@host:5432/dbname）
#   HUB_E2E_CONSENT_API     … 同意管理サービスの検証用ベース URL
#   HUB_E2E_RELAY_TIMEOUT_SEC … Outbox Relay 配送待ちのタイムアウト秒数（既定 30）
#   HUB_E2E_ORG_A / HUB_E2E_ORG_B … 検証 A（越境アクセス）用のテナント ID 2 件
#
# 呼び出し元: 人間が着手条件（micro-service-hub 側完了確認・接続情報の安全な受け渡し・
# ユーザー承認）成立後に `bash scripts/accept/hub-e2e-accept.sh` として直接実行する。
# CI 常設ジョブには追加しない（外部サービス必須のため、.claude/rules/ci.md の
# self-hosted runner 常設負荷を避ける）。
#
# 判定基準（docs/design/outbox-consent-integration.md 11.2 節）:
#   A: 越境アクセス時の 0 行（RLS フェイルクローズ）
#   B: 同意フィルタの実データ整合（オプトイン原則: 未登録テナントは全除外）
#   C: Outbox Relay 配送（enqueue → ポーリング配送の到達確認）
#   D: RLS ポリシー・SET LOCAL 適用漏れ検知（pg_class.relforcerowsecurity のカタログ確認）
#
# すべてパラメータ化クエリ（psql -v 変数バインド）とし、文字列連結による SQL 組み立てを
# 禁止する（OWASP A03、docs/design/outbox-consent-integration.md 7 節のクエリ規約）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"
cd "${WORKSPACE_ROOT}"

RELAY_TIMEOUT_SEC="${HUB_E2E_RELAY_TIMEOUT_SEC:-30}"

echo "=== #97 / REQ-9 後続 E2E 統合検証（Outbox Relay 完了待ち） ==="
echo "workspace root: ${WORKSPACE_ROOT}"
echo ""

# ---------------------------------------------------------------------------
# 前提チェック: 必須環境変数・前提ツール・PostgreSQL 到達性。
# 未成立は「規約違反」ではなく「実行前提エラー」であり、feasibility-check.sh の
# exit 2 規約と同型で区別する（テスト未実施を PASS/FAIL いずれとも混同しない）。
# ---------------------------------------------------------------------------
check_prerequisites() {
    local missing=()

    [ -z "${HUB_E2E_PG_URI:-}" ] && missing+=("HUB_E2E_PG_URI")
    [ -z "${HUB_E2E_CONSENT_API:-}" ] && missing+=("HUB_E2E_CONSENT_API")
    [ -z "${HUB_E2E_ORG_A:-}" ] && missing+=("HUB_E2E_ORG_A")
    [ -z "${HUB_E2E_ORG_B:-}" ] && missing+=("HUB_E2E_ORG_B")

    if [ "${#missing[@]}" -gt 0 ]; then
        echo "実行前提エラー: 以下の環境変数が未設定です（$(printf '%s ' "${missing[@]}")）" >&2
        echo "前提（micro-service-hub 側 Outbox Relay・同意管理サービスの稼働、接続情報の" >&2
        echo "安全な受け渡し）が成立するまで本スクリプトは実測できません。" >&2
        echo "docs/design/outbox-consent-integration.md 11.1/11.2 節・イシュー #97 参照。" >&2
        exit 2
    fi

    if ! check_tool psql "PostgreSQL クライアント（apt install postgresql-client 等）"; then
        echo "実行前提エラー: psql が見つかりません（自動インストールは行わない）" >&2
        exit 2
    fi
    if ! check_tool curl "curl（同意管理サービス API 呼び出しに使用）"; then
        echo "実行前提エラー: curl が見つかりません（自動インストールは行わない）" >&2
        exit 2
    fi

    # 接続到達性のみを確認する（実データは一切出力しない）。
    if ! psql "${HUB_E2E_PG_URI}" -Atqc "SELECT 1" >/dev/null 2>/tmp/hub-e2e-accept-pg-connect.log; then
        echo "実行前提エラー: HUB_E2E_PG_URI への接続に失敗しました（詳細は" >&2
        echo "/tmp/hub-e2e-accept-pg-connect.log。接続文字列自体はログに出力しない）" >&2
        exit 2
    fi

    echo "前提チェック: OK（環境変数・psql/curl・PostgreSQL 到達性）"
}

# ---------------------------------------------------------------------------
# A: 越境アクセス時の 0 行（RLS フェイルクローズ）
#
# テナント A のセッションコンテキスト（SET LOCAL app.current_org_id）でテナント B の
# outbox 行をパラメータクエリで参照し 0 行であること、およびコンテキスト未設定時も
# 0 行（ポリシー恒偽）であることを確認する（PoC-6 同型の越境遮断テストケースの
# 実データ再実行、docs/design/outbox-consent-integration.md 6 節の 2 層設計のうち
# データ層側を検証）。
# ---------------------------------------------------------------------------
check_cross_tenant_zero_rows() {
    local out_scoped out_unscoped rows_scoped rows_unscoped

    set +e
    out_scoped="$(psql "${HUB_E2E_PG_URI}" -Atq \
        -v org_a="${HUB_E2E_ORG_A}" -v org_b="${HUB_E2E_ORG_B}" \
        -c "BEGIN; SET LOCAL app.current_org_id = :'org_a'; SELECT count(*) FROM outbox WHERE org_id = :'org_b'; ROLLBACK;" \
        2>/tmp/hub-e2e-accept-a-scoped.log)"
    local status_scoped=$?
    set -e

    if [ "${status_scoped}" -ne 0 ]; then
        record_fail "A: 越境アクセス時の 0 行（RLS フェイルクローズ）" "テナント A コンテキストでの越境クエリ自体が失敗し測定不能: $(tail -5 /tmp/hub-e2e-accept-a-scoped.log | tr '\n' ' ')"
        return
    fi
    rows_scoped="$(printf '%s' "${out_scoped}" | tail -1 | tr -d '[:space:]')"

    set +e
    out_unscoped="$(psql "${HUB_E2E_PG_URI}" -Atq \
        -v org_b="${HUB_E2E_ORG_B}" \
        -c "BEGIN; SELECT count(*) FROM outbox WHERE org_id = :'org_b'; ROLLBACK;" \
        2>/tmp/hub-e2e-accept-a-unscoped.log)"
    local status_unscoped=$?
    set -e

    if [ "${status_unscoped}" -ne 0 ]; then
        record_fail "A: 越境アクセス時の 0 行（RLS フェイルクローズ）" "コンテキスト未設定時のクエリ自体が失敗し測定不能: $(tail -5 /tmp/hub-e2e-accept-a-unscoped.log | tr '\n' ' ')"
        return
    fi
    rows_unscoped="$(printf '%s' "${out_unscoped}" | tail -1 | tr -d '[:space:]')"

    if [ "${rows_scoped}" = "0" ] && [ "${rows_unscoped}" = "0" ]; then
        record_pass "A: 越境アクセス時の 0 行（RLS フェイルクローズ）" "テナント A コンテキストでのテナント B 行参照 0 件・コンテキスト未設定時も 0 件（ポリシー恒偽）"
    else
        record_fail "A: 越境アクセス時の 0 行（RLS フェイルクローズ）" "越境行数=${rows_scoped}（期待 0）・未設定時行数=${rows_unscoped}（期待 0）。RLS ポリシーまたは SET LOCAL 適用漏れの可能性（D も参照）"
    fi
}

# ---------------------------------------------------------------------------
# B: 同意フィルタの実データ整合
#
# 実同意管理サービス API に対しオプトイン済み/未設定/取り消し済みの 3 状態を想定した
# 判定結果を取得し、期待集合（同意済みのみ抽出・未登録テナントは全除外＝オプトイン
# 原則）と一致することを確認する（docs/design/outbox-consent-integration.md 5.2 節・
# 8 節）。実ペイロードは合成ダミーデータのみを用い、レスポンス本文はログに残さない。
# ---------------------------------------------------------------------------
check_consent_filter_parity() {
    local out status
    set +e
    out="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 "${HUB_E2E_CONSENT_API}/health" 2>/tmp/hub-e2e-accept-consent.log)"
    status=$?
    set -e

    if [ "${status}" -ne 0 ] || [ "${out}" != "200" ]; then
        record_fail "B: 同意フィルタの実データ整合" "同意管理サービス到達性チェック（GET \${HUB_E2E_CONSENT_API}/health）が失敗（http=${out:-なし}）。実データでの 3 状態判定は未実施"
        return
    fi

    # NOTE: 同意管理サービスの実 API 契約（grant/revoke のエンドポイント形式・
    # レスポンス schema）は micro-service-hub 側 REQ-2 の確定待ち
    # （docs/design/outbox-consent-integration.md 11.1 節）。契約確定後、この関数に
    # 3 状態（オプトイン済み/未設定/取り消し済み）の実クエリと期待値照合を実装する。
    record_skip "B: 同意フィルタの実データ整合" "同意管理サービスは到達可能だが、grant/revoke API の実契約が micro-service-hub 側 REQ-2 未確定のため 3 状態判定は未実装（11.1 節）"
}

# ---------------------------------------------------------------------------
# C: Outbox Relay 配送
#
# 検証用イベントを enqueue し、Relay によるポーリング配送（配送状態列の遷移）を
# タイムアウト付きで確認する。投入イベント数は最小固定件数（1 件）とし、無制限
# ポーリング・大量 enqueue を行わない（DoS 対策、docs/design/
# outbox-consent-integration.md 10 節）。
# ---------------------------------------------------------------------------
check_outbox_relay_delivery() {
    local event_id out status
    event_id="e2e-accept-$(date +%s)"

    set +e
    out="$(psql "${HUB_E2E_PG_URI}" -Atq \
        -v org_a="${HUB_E2E_ORG_A}" -v event_id="${event_id}" \
        -c "INSERT INTO outbox (id, org_id, event_type, payload) VALUES (:'event_id', :'org_a', 'e2e-accept-check', '{}'::jsonb);" \
        2>/tmp/hub-e2e-accept-c-enqueue.log)"
    status=$?
    set -e

    if [ "${status}" -ne 0 ]; then
        record_fail "C: Outbox Relay 配送" "enqueue（INSERT）自体が失敗し測定不能: $(tail -5 /tmp/hub-e2e-accept-c-enqueue.log | tr '\n' ' ')。outbox テーブルの実カラム定義（配送状態列名）が micro-service-hub 側で未確定の場合はここで失敗する（11.1 節）"
        return
    fi

    local elapsed=0
    local delivered=""
    while [ "${elapsed}" -lt "${RELAY_TIMEOUT_SEC}" ]; do
        set +e
        delivered="$(psql "${HUB_E2E_PG_URI}" -Atq \
            -v event_id="${event_id}" \
            -c "SELECT delivered_at IS NOT NULL FROM outbox WHERE id = :'event_id';" \
            2>/tmp/hub-e2e-accept-c-poll.log)"
        set -e
        if [ "${delivered}" = "t" ]; then
            record_pass "C: Outbox Relay 配送" "enqueue から ${elapsed}s 以内に配送済み（delivered_at 設定確認、イベント ${event_id}）"
            return
        fi
        sleep 2
        elapsed=$((elapsed + 2))
    done

    record_fail "C: Outbox Relay 配送" "${RELAY_TIMEOUT_SEC}s 以内に配送確認できず（イベント ${event_id}）。Relay 未稼働または配送状態列名が実装と不一致の可能性（11.1 節、delivered_at は想定カラム名でありmicro-service-hub 側確定待ち）"
}

# ---------------------------------------------------------------------------
# D: RLS ポリシー・SET LOCAL 適用漏れ検知（設計書 10 節 A05 の申し送り解消）
#
# outbox テーブルに FORCE ROW LEVEL SECURITY が適用されていることをカタログ
# （pg_class.relforcerowsecurity）で機械確認する。
# ---------------------------------------------------------------------------
check_rls_force_applied() {
    local out status
    set +e
    out="$(psql "${HUB_E2E_PG_URI}" -Atq \
        -c "SELECT relforcerowsecurity FROM pg_class WHERE relname = 'outbox';" \
        2>/tmp/hub-e2e-accept-d.log)"
    status=$?
    set -e

    if [ "${status}" -ne 0 ]; then
        record_fail "D: RLS ポリシー・SET LOCAL 適用漏れ検知" "カタログ照会自体が失敗し測定不能: $(tail -5 /tmp/hub-e2e-accept-d.log | tr '\n' ' ')"
        return
    fi
    if [ -z "${out}" ]; then
        record_fail "D: RLS ポリシー・SET LOCAL 適用漏れ検知" "outbox テーブルが見つからず判定不能（pg_class に該当行なし）"
        return
    fi

    if [ "${out}" = "t" ]; then
        record_pass "D: RLS ポリシー・SET LOCAL 適用漏れ検知" "outbox テーブルに FORCE ROW LEVEL SECURITY が適用済み（relforcerowsecurity=t）"
    else
        record_fail "D: RLS ポリシー・SET LOCAL 適用漏れ検知" "outbox テーブルに FORCE ROW LEVEL SECURITY が未適用（relforcerowsecurity=${out}）。テーブル所有者がポリシーを迂回できる設定ミス（A05）の可能性"
    fi
}

check_prerequisites
check_cross_tenant_zero_rows
check_consent_filter_parity
check_outbox_relay_delivery
check_rls_force_applied

print_summary "REQ-9 後続 E2E 統合検証、#97"
exit "$(summary_exit_code)"
