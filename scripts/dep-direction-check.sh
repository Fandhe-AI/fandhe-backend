#!/usr/bin/env bash
# 依存方向一方向性の機械検証（TASK-1.5/#14 で新設、TASK-11.1/#33 で workspace 全体へ展開、
# docs/spec/04-requirements.md REQ-1・docs/spec/05-tasks.md TASK-11.1）。
#
# `server → routes → http::*` の一方向依存（循環なし）が workspace の実態と
# 乖離していないことを 3 段で検証する:
#   1. `cargo metadata` から抽出した workspace 内 path 依存エッジをホワイトリストと
#      照合する（許可外のエッジ・循環を検出したら FAIL）
#   2. `${CRATES_DIR}`（既定 `crates`）直下の各クレートについて、エントリポイント
#      （`src/lib.rs` を優先、なければ `src/main.rs`）に統一形式の依存方向宣言
#      （`server → routes → http::*`）があることを確認する（doc とコードの乖離検知）。
#      TASK-11.1 でハードコード 3 ファイルの列挙から動的列挙へ変更し、将来クレートの
#      追加時にも宣言漏れが自動検出されるようにした（規約の継続的な機械保証）
#   3. `crates/core` / `crates/http` / `crates/routes` の `src/**/*.rs` にプラグイン固有
#      シンボル（`[Pp]lugin` を含む識別子）・`Cargo.toml` の `plugin-` 依存がないことを
#      grep で確認する（`scripts/accept/core-deps-unsafe-audit.sh` 基準 F と同一手法。
#      TASK-11.1 で `crates/core` を対象に追加）
#
# 判定不能（cargo metadata 失敗・jq 未導入・エントリポイント不在等）はフェイルクローズで
# FAIL とし、「検証していないのに PASS 扱い」を防ぐ（`.claude/rules/security.md`）。
#
# 呼び出し元: `.github/workflows/ci.yml` の `unsafe-triage` ジョブ、または人間が
# `bash scripts/dep-direction-check.sh` として直接実行する。
#
# セルフテスト: `scripts/tests/run-dep-direction-tests.sh`（fixture の正常/違反グラフで
# PASS/FAIL を固定化する）。チェック 1 は `--metadata-file`、チェック 2 は `--crates-dir`
# で実データ取得を差し替え、workspace の実状態に依存せず検証する。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${WORKSPACE_ROOT}"

METADATA_FILE=""
CRATES_DIR="crates"
while [ $# -gt 0 ]; do
    case "$1" in
        --metadata-file)
            METADATA_FILE="$2"
            shift 2
            ;;
        --crates-dir)
            CRATES_DIR="$2"
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
            #
            # `backend-framework-core:bf-plugin-webrtc`（TASK-8.1、#26）: `webrtc`
            # feature 有効時のみ、同型のパスインターセプト機構（`plugin::try_intercept`
            # 内の cfg-gated 分岐）で in-process WebRTC ハンドラ
            # （`crates/plugin-webrtc`、webrtc-rs 依存）へ配線する。上記
            # `bf-plugin-webrtc-proxy` の許可根拠（拡張点の同期 API 限定に非同期
            # 呼び出しを持ち込めない）と同一のため、個別のエッジとして明示追加する。
            #
            # 例外 2: `backend-framework-core:bf-plugin-websocket`（TASK-4.1、#22）。
            # WebSocket は「委譲判定のみ」の `UpgradeHandler`（同期 API）に加え、
            # ハンドシェイク検証・101 応答送出・フレーミング委譲という非同期処理を
            # 要する Upgrade 型プラグインの第 1 号であり、webrtc-proxy と同型の
            # 依存逆転（コア → プラグインの optional 依存）で表現する。
            # `bf-plugin-websocket` 自体は循環依存を避けるため
            # `backend-framework-core` に依存しない（下の
            # `bf-plugin-*:backend-framework-core` パターンには乗らない設計、
            # `crates/plugin-websocket/src/lib.rs` の doc・
            # `docs/design/plugin-boundary.md` 6.1 節を参照）。本エッジも他
            # プラグインへ一般化せず、新規 Upgrade 型プラグインは本リストへの
            # 明示追加とレビューを要求する。
            allowed_edge_patterns=(
                "backend-framework-core:bf-http"
                "backend-framework-core:bf-routes"
                "backend-framework-core:bf-plugin-webrtc-proxy"
                "backend-framework-core:bf-plugin-webrtc"
                "backend-framework-core:bf-plugin-websocket"
                # TASK-2.4（#21）: REQ-2「少なくとも 2 種のプラグインを feature
                # flag で着脱できる」受け入れ基準の第 2 インスタンス（パス
                # インターセプト型）。根拠は上記
                # backend-framework-core:bf-plugin-webrtc-proxy の例外コメントと
                # 同一（3 拡張点はいずれも dyn 互換性のため同期 API 限定であり、
                # パスインターセプト型プラグインの依存を既存拡張点経由の依存逆転
                # で表現できない）。`crates/plugin-graphql` の doc・
                # `docs/design/plugin-loading-tradeoffs.md` を参照。
                "backend-framework-core:bf-plugin-graphql"
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
# 2: エントリポイント（lib.rs／main.rs）依存方向宣言の存在検査
#
# `${CRATES_DIR}` 直下の各ディレクトリ（1 階層のみ。crates/http/fuzz のようなネスト
# クレートは対象外＝ workspace Cargo.toml の exclude と整合）をクレートとみなし、
# `src/lib.rs` を優先、無ければ `src/main.rs` をエントリポイントとして解決する。
# 両方欠落・宣言欠落はいずれも FAIL（フェイルクローズ）。将来クレートを追加した際も
# 本ループが自動的に対象へ含めるため、宣言の付け忘れが CI で機械検出される。
# ---------------------------------------------------------------------------
DECLARATION="server → routes → http::*"
declaration_missing=()
crate_count=0

if [ ! -d "${CRATES_DIR}" ]; then
    fail "2: エントリポイント依存方向宣言 — ${CRATES_DIR} が存在しません（判定不能）"
else
    for crate_dir in "${CRATES_DIR}"/*/; do
        [ -d "${crate_dir}" ] || continue
        crate_name="$(basename "${crate_dir}")"
        crate_count=$((crate_count + 1))

        if [ -f "${crate_dir}src/lib.rs" ]; then
            entrypoint="${crate_dir}src/lib.rs"
        elif [ -f "${crate_dir}src/main.rs" ]; then
            entrypoint="${crate_dir}src/main.rs"
        else
            declaration_missing+=("${crate_name}（src/lib.rs・src/main.rs のいずれも不在）")
            continue
        fi

        if ! grep -qF "${DECLARATION}" "${entrypoint}"; then
            declaration_missing+=("${entrypoint}")
        fi
    done

    if [ "${crate_count}" -eq 0 ]; then
        fail "2: エントリポイント依存方向宣言 — ${CRATES_DIR} 直下にクレートが 1 件も見つかりませんでした（判定不能）"
    elif [ ${#declaration_missing[@]} -eq 0 ]; then
        pass "2: エントリポイント依存方向宣言 — ${CRATES_DIR} 直下 ${crate_count} クレート全てのエントリポイントに統一形式の宣言あり"
    else
        fail "2: エントリポイント依存方向宣言 — 欠落: ${declaration_missing[*]}"
    fi
fi

# ---------------------------------------------------------------------------
# 3: core・routes・http のプラグイン固有シンボル非依存検査
#    （scripts/accept/core-deps-unsafe-audit.sh 基準 F と同一手法。TASK-11.1 で
#    crates/core を対象に追加。bf-plugin-* 自体は「plugin」を正当に含むため
#    検査対象にしない。依存方向はチェック 1 のホワイトリストが別途担保する）
#
#    例外: TASK-2.1（#18、PR #129）でチェック 1 のホワイトリストへ追加された
#    エッジ backend-framework-core:bf-plugin-webrtc-proxy（本体チェック 1
#    該当コメント・docs/design/plugin-boundary.md 6.1 節参照）に対応する実装。
#    3 拡張点（Middleware/UpgradeHandler/RequestGate、いずれも dyn 互換性のため
#    同期 API 限定）に載らない非同期アップグレード中継（WebRTC シグナリング
#    プロキシへの委譲）専用に、crates/core/src/plugin.rs へ隔離してある。
#    TASK-8.1（#26）で同一パターンを踏襲する backend-framework-core:bf-plugin-webrtc
#    エッジ（in-process WebRTC ハンドラ）も同様に追加された。
#    本チェックはこのファイル・crates/core/src/lib.rs の `mod plugin;` 宣言・
#    crates/core/src/server.rs の `webrtc_proxy`/`webrtc_config` 系シンボル・
#    crates/core/Cargo.toml の `bf-plugin-webrtc-proxy`/`bf-plugin-webrtc` 依存に
#    限り許可し、他プラグインへは一般化しない（上記 2 件以外の plugin- 依存・
#    プラグイン固有シンボルは引き続き通常どおり FAIL する）。
#
#    TASK-4.1（#22）で `backend-framework-core:bf-plugin-websocket`（チェック 1
#    該当コメント参照）を同様に許可した際、`bf_plugin_websocket` 系シンボル
#    （`crates/core/src/plugin.rs` の Upgrade シーム実装・
#    `crates/core/src/server.rs` の `websocket` 系ビルダー/フィールド）も
#    webrtc-proxy と同一方針で例外対象に加える。
# ---------------------------------------------------------------------------
webrtc_proxy_exception_file="crates/core/src/plugin.rs"
# TASK-2.4（#21）で graphql feature（bf-plugin-graphql、backend-framework-core:
# bf-plugin-graphql の許可リスト例外に対応）を同一ファイルへ追加したため、
# 例外シンボルパターンにも graphql 系識別子を含める（webrtc-proxy・webrtc・
# websocket・graphql の 4 件に限定したまま維持し、一般化はしない）。
webrtc_proxy_exception_symbol_pattern='bf_plugin_webrtc_proxy|bf_plugin_webrtc\b|webrtc_proxy|webrtc_config|bf_plugin_websocket|websocket|bf_plugin_graphql|crate::plugin::|pub\(crate\) mod plugin;'

plugin_hits_all=""
for dir in crates/core crates/http crates/routes; do
    if [ -d "${dir}/src" ]; then
        # 日本語 doc comment 中の「プラグイン」誤検出を避けるため // 行コメントは除外し、
        # 識別子パターン（Plugin/plugin を含む識別子）のみを検査する。除外パターンは
        # grep -rn の "file:line:content" 形式を踏まえ "file:line:" プレフィックス込みで書く。
        hits="$(grep -rn --include='*.rs' -E '[A-Za-z_]*[Pp]lugin' "${dir}/src" | grep -v -E '^[^:]*:[0-9]+:[[:space:]]*//' || true)"
        if [ "${dir}" = "crates/core" ] && [ -n "${hits}" ]; then
            # webrtc-proxy 専用モジュール全体、および lib.rs/server.rs の
            # webrtc_proxy 関連シンボルのみを例外として除外する（上記コメント参照）。
            hits="$(printf '%s\n' "${hits}" | grep -v -E "^${webrtc_proxy_exception_file}:" | grep -v -E "${webrtc_proxy_exception_symbol_pattern}" || true)"
        fi
        if [ -n "${hits}" ]; then
            plugin_hits_all="${plugin_hits_all}${hits}
"
        fi
    else
        fail "3: プラグイン非依存（core/routes/http） — ${dir}/src が存在しません（検証対象なし。crates/routes 新設 PR 内で検出された場合は構成漏れ）"
    fi
    if [ -f "${dir}/Cargo.toml" ]; then
        # doc comment（`#` 行）中の「plugin-」誤検出を避け、実際の依存宣言行のみを対象にする
        # （.rs 側のコメント除外と同一方針）。
        cargo_toml_hits="$(grep -n 'plugin-' "${dir}/Cargo.toml" | grep -v -E '^[0-9]+:[[:space:]]*#' || true)"
        if [ "${dir}" = "crates/core" ] && [ -n "${cargo_toml_hits}" ]; then
            # `bf-plugin-webrtc-proxy =` の依存宣言・`dep:bf-plugin-webrtc-proxy` の
            # feature 宣言、`bf-plugin-webrtc =` の依存宣言・`dep:bf-plugin-webrtc` の
            # feature 宣言（`bf-plugin-webrtc` は `bf-plugin-webrtc-proxy` の前方一致
            # 部分文字列でもあるため 1 パターンでまとめて除外できる）、
            # `bf-plugin-websocket =` の依存宣言・`dep:bf-plugin-websocket` の
            # feature 宣言、および `bf-plugin-graphql =` の依存宣言・
            # `dep:bf-plugin-graphql` の feature 宣言を許可する。
            cargo_toml_hits="$(printf '%s\n' "${cargo_toml_hits}" | grep -v -E 'bf-plugin-webrtc(-proxy)?|bf-plugin-websocket|bf-plugin-graphql' || true)"
        fi
        if [ -n "${cargo_toml_hits}" ]; then
            plugin_hits_all="${plugin_hits_all}${dir}/Cargo.toml に plugin- 依存あり: ${cargo_toml_hits}
"
        fi
    fi
done

if [ -z "${plugin_hits_all}" ]; then
    pass "3: プラグイン非依存（core/routes/http） — crates/core・crates/http・crates/routes にプラグイン固有シンボル・依存を検出せず"
else
    fail "3: プラグイン非依存（core/routes/http） — 検出: ${plugin_hits_all}"
fi

echo ""
if [ "${HAS_FAIL}" -eq 1 ]; then
    echo "=== 依存方向一方向性検証: FAIL ==="
    exit 1
else
    echo "=== 依存方向一方向性検証: PASS ==="
    exit 0
fi
