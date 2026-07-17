#!/usr/bin/env bash
# 依存方向一方向性の機械検証（TASK-1.5、#14、docs/spec/04-requirements.md REQ-1）。
#
# `server → routes → http::*` の一方向依存（循環なし）が workspace の実態と
# 乖離していないことを 3 段で検証する:
#   1. `cargo metadata` から抽出した workspace 内 path 依存エッジをホワイトリストと
#      照合する（許可外のエッジ・循環を検出したら FAIL）
#   2. core / routes / http の各 `src/lib.rs` に統一形式の依存方向宣言
#      （`server → routes → http::*`）があることを確認する（doc とコードの乖離検知）
#   3. `crates/http` / `crates/routes` の `src/**/*.rs` にプラグイン固有シンボル
#      （`[Pp]lugin` を含む識別子）・`Cargo.toml` の `plugin-` 依存がないことを
#      grep で確認する（`scripts/accept/core-deps-unsafe-audit.sh` 基準 F と同一手法）
#
# 判定不能（cargo metadata 失敗・jq 未導入等）はフェイルクローズで FAIL とし、
# 「検証していないのに PASS 扱い」を防ぐ（`.claude/rules/security.md`）。
#
# 呼び出し元: `.github/workflows/ci.yml` の `unsafe-triage` ジョブ、または人間が
# `bash scripts/dep-direction-check.sh` として直接実行する。
#
# セルフテスト: `scripts/tests/run-dep-direction-tests.sh`（fixture の正常/違反グラフで
# PASS/FAIL を固定化する）。fixture は `--metadata-file` で `cargo metadata` 呼び出しを
# 差し替えて workspace の実状態に依存せず検証する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${WORKSPACE_ROOT}"

METADATA_FILE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --metadata-file)
            METADATA_FILE="$2"
            shift 2
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

HAS_FAIL=0

fail() {
    echo "[FAIL] $1" >&2
    HAS_FAIL=1
}

pass() {
    echo "[PASS] $1"
}

if ! command -v jq >/dev/null 2>&1; then
    fail "1: 依存エッジホワイトリスト照合 — jq が未導入のため判定不能（導入: OS のパッケージマネージャ、例 apt install jq）"
else
    if [ -n "${METADATA_FILE}" ]; then
        if [ ! -f "${METADATA_FILE}" ]; then
            fail "1: 依存エッジホワイトリスト照合 — --metadata-file が指す ${METADATA_FILE} が存在しません"
            metadata_json=""
        else
            metadata_json="$(cat "${METADATA_FILE}")"
        fi
    else
        metadata_json="$(cargo metadata --no-deps --format-version 1 2>/tmp/dep-direction-check-metadata.log || true)"
        if [ -z "${metadata_json}" ]; then
            fail "1: 依存エッジホワイトリスト照合 — cargo metadata の実行に失敗しました（/tmp/dep-direction-check-metadata.log 参照）"
        fi
    fi

    if [ -n "${metadata_json}" ]; then
        # kind=null（normal dependencies のみ）に限定する。dev-dependencies（kind=dev）・
        # build-dependencies（kind=build）は実行時依存方向に影響しないため対象外とする
        # （テスト専用依存の混入を許容する設計判断、本スクリプト doc 参照）。
        edges_tsv="$(printf '%s' "${metadata_json}" | jq -r '
            .packages[] | .name as $from |
            .dependencies[] |
            select(.path != null and (.kind == null)) |
            "\($from)\t\(.name)"
        ' 2>/tmp/dep-direction-check-jq.log || true)"

        if [ -z "${edges_tsv}" ] && [ ! -s /tmp/dep-direction-check-jq.log ]; then
            # workspace 内 path 依存が 1 件もない状態は現状ありえない（core が最低でも
            # bf-http に依存する）。jq エラーではなく本当に 0 件だった場合も、依存方向
            # グラフを検証できていないため判定不能として FAIL 扱いにする（フェイルクローズ）。
            fail "1: 依存エッジホワイトリスト照合 — path 依存エッジが 0 件でした（測定不能）"
        elif [ -s /tmp/dep-direction-check-jq.log ]; then
            fail "1: 依存エッジホワイトリスト照合 — jq によるエッジ抽出に失敗しました（/tmp/dep-direction-check-jq.log 参照）"
        else
            # 許可リスト（from パターン:to パターン）。fnmatch 相当の shell パターンで判定する。
            # server → routes → http::* の一方向を基本とし、逆方向（http/routes → 上位層）・
            # コアからのプラグイン依存を原則禁止として機械的に排除する。将来クレートを
            # 追加する際は本リストの明示更新を要求する（ホワイトリスト方式の意図:
            # 未知のエッジはすべて拒否）。
            #
            # 例外: `backend-framework-core:bf-plugin-webrtc-proxy`（TASK-2.1、#18）。
            # `docs/spec/04-requirements.md` REQ-2 は「プラグインは `[features]` の
            # `dep:` 構文で feature 無効時に依存自体を未解決のまま除外する」
            # コンパイル時プラグイン機構を明示的に要求しており、パスインターセプト型
            # プラグイン（非同期の上流中継を伴う WebRTC シグナリングプロキシ等）は
            # 3 拡張点（`Middleware`/`UpgradeHandler`/`RequestGate`、いずれも dyn
            # 互換性のため同期 API に限定、`crates/core/src/extension.rs` 冒頭 doc）
            # に非同期呼び出しを持ち込めないため、既存拡張点経由の依存逆転（プラグイン
            # 側のみが core に依存する形）では表現できない。`crates/core/Cargo.toml`
            # の `optional = true` + `dep:` 構文により feature 無効時は本エッジ自体が
            # 未解決のまま消え、依存・コード・`unsafe` を一切バイナリに含まない
            # （pay-for-what-you-use、.claude/rules/pay-for-what-you-use.md）ことを
            # `docs/design/plugin-boundary.md` で検証済み。本エッジは他プラグインへ
            # 一般化せず（`bf-plugin-*` ワイルドカードにしない）、新規プラグインが
            # 同パターンを踏襲する際は本リストへの明示追加とレビューを要求する。
            allowed_edge_patterns=(
                "backend-framework-core:bf-http"
                "backend-framework-core:bf-routes"
                "backend-framework-core:bf-plugin-webrtc-proxy"
                "bf-routes:bf-http"
                "bf-plugin-*:bf-http"
                "bf-plugin-*:bf-routes"
                "bf-plugin-*:backend-framework-core"
            )

            violating_edges=()
            while IFS=$'\t' read -r from to; do
                [ -z "${from}" ] && continue
                allowed=0
                for pattern in "${allowed_edge_patterns[@]}"; do
                    from_pat="${pattern%%:*}"
                    to_pat="${pattern##*:}"
                    # shellcheck disable=SC2053  # パターンマッチとして意図的に未クォート展開
                    if [[ "${from}" == ${from_pat} && "${to}" == ${to_pat} ]]; then
                        allowed=1
                        break
                    fi
                done
                if [ "${allowed}" -eq 0 ]; then
                    violating_edges+=("${from} -> ${to}")
                fi
            done <<<"${edges_tsv}"

            # 循環依存検出（DFS）。許可リスト自体が有向非巡回グラフを意図した設計だが、
            # 許可リストのパターン記述ミスによる誤許可（例: 双方向パターンの重複登録）を
            # 独立に検知する多層防御として、エッジ抽出結果に対し別途 DFS を回す。
            declare -A adj
            while IFS=$'\t' read -r from to; do
                [ -z "${from}" ] && continue
                adj["${from}"]="${adj[${from}]:-} ${to}"
            done <<<"${edges_tsv}"

            declare -A visiting
            declare -A visited
            cycle_found=0
            dfs_has_cycle() {
                local node="$1"
                visiting["${node}"]=1
                local neighbor
                for neighbor in ${adj[${node}]:-}; do
                    if [ "${visiting[${neighbor}]:-0}" = "1" ]; then
                        return 0
                    fi
                    if [ "${visited[${neighbor}]:-0}" != "1" ]; then
                        if dfs_has_cycle "${neighbor}"; then
                            return 0
                        fi
                    fi
                done
                visiting["${node}"]=0
                visited["${node}"]=1
                return 1
            }
            mapfile -t all_nodes < <(printf '%s\n' "${edges_tsv}" | cut -f1 | sort -u)
            for node in "${all_nodes[@]}"; do
                if [ "${visited[${node}]:-0}" != "1" ]; then
                    if dfs_has_cycle "${node}"; then
                        cycle_found=1
                        break
                    fi
                fi
            done

            if [ "${cycle_found}" -eq 1 ]; then
                fail "1: 依存エッジホワイトリスト照合 — workspace 内クレート間の依存に循環が検出されました"
            elif [ "${#violating_edges[@]}" -gt 0 ]; then
                fail "1: 依存エッジホワイトリスト照合 — 許可リスト外のエッジ: ${violating_edges[*]}"
            else
                pass "1: 依存エッジホワイトリスト照合 — 循環なし・全エッジが許可リストに合致（$(echo "${edges_tsv}" | tr '\n' ';' | sed 's/\t/->/g')）"
            fi
        fi
    fi
fi

# ---------------------------------------------------------------------------
# 2: lib.rs 依存方向宣言の存在検査
# ---------------------------------------------------------------------------
DECLARATION="server → routes → http::*"
declaration_missing=()
for lib in crates/core/src/lib.rs crates/routes/src/lib.rs crates/http/src/lib.rs; do
    if [ ! -f "${lib}" ]; then
        declaration_missing+=("${lib}（ファイル不在）")
        continue
    fi
    if ! grep -qF "${DECLARATION}" "${lib}"; then
        declaration_missing+=("${lib}")
    fi
done

if [ ${#declaration_missing[@]} -eq 0 ]; then
    pass "2: lib.rs 依存方向宣言 — core/routes/http すべての src/lib.rs に統一形式の宣言あり"
else
    fail "2: lib.rs 依存方向宣言 — 欠落: ${declaration_missing[*]}"
fi

# ---------------------------------------------------------------------------
# 3: routes・http のプラグイン固有シンボル非依存検査
#    （scripts/accept/core-deps-unsafe-audit.sh 基準 F と同一手法）
# ---------------------------------------------------------------------------
plugin_hits_all=""
for dir in crates/http crates/routes; do
    if [ -d "${dir}/src" ]; then
        # 日本語 doc comment 中の「プラグイン」誤検出を避けるため // 行コメントは除外し、
        # 識別子パターン（Plugin/plugin を含む識別子）のみを検査する。除外パターンは
        # grep -rn の "file:line:content" 形式を踏まえ "file:line:" プレフィックス込みで書く。
        hits="$(grep -rn --include='*.rs' -E '[A-Za-z_]*[Pp]lugin' "${dir}/src" | grep -v -E '^[^:]*:[0-9]+:[[:space:]]*//' || true)"
        if [ -n "${hits}" ]; then
            plugin_hits_all="${plugin_hits_all}${hits}
"
        fi
    else
        fail "3: プラグイン非依存（routes/http） — ${dir}/src が存在しません（検証対象なし。crates/routes 新設 PR 内で検出された場合は構成漏れ）"
    fi
    if [ -f "${dir}/Cargo.toml" ] && grep -q 'plugin-' "${dir}/Cargo.toml"; then
        plugin_hits_all="${plugin_hits_all}${dir}/Cargo.toml に plugin- 依存あり
"
    fi
done

if [ -z "${plugin_hits_all}" ]; then
    pass "3: プラグイン非依存（routes/http） — crates/http・crates/routes にプラグイン固有シンボル・依存を検出せず"
else
    fail "3: プラグイン非依存（routes/http） — 検出: ${plugin_hits_all}"
fi

echo ""
if [ "${HAS_FAIL}" -eq 1 ]; then
    echo "=== 依存方向一方向性検証: FAIL ==="
    exit 1
else
    echo "=== 依存方向一方向性検証: PASS ==="
    exit 0
fi
