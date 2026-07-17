#!/usr/bin/env bash
# pay-for-what-you-use（.claude/rules/pay-for-what-you-use.md）を機械的に PASS/FAIL
# 判定するゲートスクリプト（TASK-2.2、#19、docs/spec/05-tasks.md）。
#
# 前提タスク TASK-2.1（#18）で `webrtc-proxy` feature（`optional = true` + `dep:` 構文、
# crates/core/Cargo.toml）によるプラグイン境界第 1 号が確立されたが、検証は
# `docs/design/plugin-boundary.md` 6 節の手動コマンド表に留まっていた。本スクリプトは
# 「プラグイン feature 無効時に当該プラグインの依存クレート・`unsafe`・コードが 0 件で
# 載らないこと」を次の 5 段で機械検証する:
#
#   (a) `cargo metadata` から `backend-framework-core` の `[features]` を動的列挙し、
#       `dep:bf-plugin-*` を含む feature を「プラグイン feature」として抽出する
#       （feature 増加時にスクリプト変更不要。`dep-audit.sh`・`dep-direction-check.sh`
#       と同方針）。0 件はスクリプト自体の腐敗を疑い FAIL（フェイルクローズ）
#   (b) `cargo tree`: 無効構成（--no-default-features）に全プラグインクレートが
#       出現しないこと（依存 0 件の直接検証）。各 feature 単独有効化（ポジティブ
#       コントロール）で当該クレートが出現し、他プラグインは混入しないこと
#       （配線切れ・列挙腐敗の検知）
#   (c) `cargo geiger`: 無効構成の依存グラフにプラグインクレートが含まれないこと
#       （依存グラフに載らなければ unsafe も載らない、unsafe 0 件の検証）
#   (d) リリースバイナリサイズ: `crates/core/examples/minimal` を無効構成／
#       `--all-features` の 2 構成でビルドし、無効構成 <= 有効構成であること。
#       補強としてシンボル表（nm）に `bf_plugin` 由来シンボルが出現しないことを検証
#       （コード 0 件の直接検証。nm 不在時はこの補強のみ SKIP、サイズ比較は維持）
#   (e) 全構成ビルド: 無効構成・feature 単独構成ごと・`--all-features` がすべて成功すること
#
# 判定不能（前提ツール欠如・cargo 実行失敗・列挙 0 件等）はフェイルクローズで FAIL とし、
# 「検証していないのに PASS 扱い」を防ぐ（.claude/rules/security.md）。
#
# 呼び出し元: `.github/workflows/ci.yml` の `pay-for-what-you-use` ジョブ、または人間が
# `bash scripts/pay-for-what-you-use-check.sh` として直接実行する。
#
# 情報提示（ゲートではない）用途の `scripts/dep-impact.sh` とは役割を分けて併存させる
# （dep-impact.sh は記録台帳向けの計測値出力、本スクリプトは PASS/FAIL 判定）。
#
# セルフテスト: `scripts/tests/run-pay-for-what-you-use-tests.sh`（ネットワーク・cargo
# ビルド不要）。(a) は `--metadata-file`、(b) は `--tree-negative-file`・
# `--tree-positive-dir`、(c) は `--geiger-packages-file`、(d) のサイズ比較・シンボル検査
# ロジックは `--size-negative`/`--size-positive`/`--symbols-file` で実データ取得を
# 差し替え、workspace の実状態・cargo ビルドに依存せず判定ロジックを検証する。
# (e) は cargo ビルドそのものが検証対象のため fixture 化せず、本スクリプトの通常実行
# （CI・人間によるローカル実行）でのみ検証される。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${WORKSPACE_ROOT}"

CORE_PACKAGE="backend-framework-core"
CORE_MANIFEST="${WORKSPACE_ROOT}/crates/core/Cargo.toml"
TARGET_DIR="target/pay-for-what-you-use-check"

METADATA_FILE=""
TREE_NEGATIVE_FILE=""
TREE_POSITIVE_DIR=""
GEIGER_PACKAGES_FILE=""
SIZE_NEGATIVE=""
SIZE_POSITIVE=""
SYMBOLS_FILE=""
SKIP_BUILD_STEPS=0

while [ $# -gt 0 ]; do
    case "$1" in
        --metadata-file)
            METADATA_FILE="$2"
            shift 2
            ;;
        --tree-negative-file)
            TREE_NEGATIVE_FILE="$2"
            shift 2
            ;;
        --tree-positive-dir)
            TREE_POSITIVE_DIR="$2"
            shift 2
            ;;
        --geiger-packages-file)
            GEIGER_PACKAGES_FILE="$2"
            shift 2
            ;;
        --size-negative)
            SIZE_NEGATIVE="$2"
            shift 2
            ;;
        --size-positive)
            SIZE_POSITIVE="$2"
            shift 2
            ;;
        --symbols-file)
            SYMBOLS_FILE="$2"
            shift 2
            ;;
        --skip-build-steps)
            # (d)/(e) はいずれも cargo ビルドを要し、セルフテスト（ネットワーク・
            # cargo ビルド不要方針）からは実行できない。--size-* / --symbols-file
            # 注入時のみ判定ロジックを検証し、実ビルドを伴う工程はスキップする。
            SKIP_BUILD_STEPS=1
            shift
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

for cmd in jq cargo; do
    if ! command -v "${cmd}" >/dev/null 2>&1; then
        echo "エラー: ${cmd} が見つかりません。導入してから再実行してください。" >&2
        exit 1
    fi
done

# =============================================================================
# (a) プラグイン feature の動的列挙
# =============================================================================
if [ -n "${METADATA_FILE}" ]; then
    if [ ! -f "${METADATA_FILE}" ]; then
        fail "a: プラグイン feature 列挙 — --metadata-file が指す ${METADATA_FILE} が存在しません"
        metadata_json=""
    else
        metadata_json="$(cat "${METADATA_FILE}")"
    fi
else
    metadata_json="$(cargo metadata --no-deps --format-version 1 2>/tmp/pfwu-check-metadata.log || true)"
    if [ -z "${metadata_json}" ]; then
        fail "a: プラグイン feature 列挙 — cargo metadata の実行に失敗しました（/tmp/pfwu-check-metadata.log 参照）"
    fi
fi

plugin_entries=()
if [ -n "${metadata_json}" ]; then
    # feature 名 → `dep:bf-plugin-<name>` を持つ feature のみ抽出し、
    # 「feature\tクレート名」の TSV を組み立てる（plugin-boundary.md 2 節の
    # 命名規約: クレート名 bf-plugin-<name> から接頭辞を除いた <name> が feature 名）。
    plugin_features_tsv="$(printf '%s' "${metadata_json}" | jq -r --arg pkg "${CORE_PACKAGE}" '
        .packages[] | select(.name == $pkg) | .features
        | to_entries[] as $e
        | ($e.value[] | select(startswith("dep:"))) as $dep
        | select($dep | startswith("dep:bf-plugin-"))
        | "\($e.key)\t\($dep | sub("^dep:";""))"
    ' 2>/tmp/pfwu-check-jq.log || true)"

    if [ -s /tmp/pfwu-check-jq.log ]; then
        fail "a: プラグイン feature 列挙 — jq による feature 抽出に失敗しました（/tmp/pfwu-check-jq.log 参照）"
    elif [ -z "${plugin_features_tsv}" ]; then
        # 現時点で webrtc-proxy が必ず 1 件存在する。0 件は列挙ロジックの腐敗を
        # 疑い判定不能として FAIL 扱いにする（フェイルクローズ）。
        fail "a: プラグイン feature 列挙 — ${CORE_PACKAGE} に dep:bf-plugin-* を含む feature が 1 件も見つかりませんでした（判定不能）"
    else
        naming_violation=""
        while IFS=$'\t' read -r feature crate; do
            [ -z "${feature}" ] && continue
            expected_feature="${crate#bf-plugin-}"
            if [ "${feature}" != "${expected_feature}" ]; then
                naming_violation="${naming_violation}${feature} (期待: ${expected_feature}, クレート: ${crate}); "
                continue
            fi
            plugin_entries+=("${feature}:${crate}")
        done <<<"${plugin_features_tsv}"

        if [ -n "${naming_violation}" ]; then
            fail "a: プラグイン feature 列挙 — feature 命名規約（docs/design/plugin-boundary.md 2 節）違反: ${naming_violation}"
        else
            pass "a: プラグイン feature 列挙 — $(printf '%s ' "${plugin_entries[@]}")"
        fi
    fi
fi

# =============================================================================
# (b) cargo tree 検証（依存 0 件）
# =============================================================================
if [ "${#plugin_entries[@]}" -eq 0 ]; then
    fail "b: cargo tree 検証 — プラグイン feature が列挙できていないため実行できません（(a) 参照）"
else
    # --- 無効構成: いずれのプラグインクレートも出現しないこと ---
    if [ -n "${TREE_NEGATIVE_FILE}" ]; then
        if [ ! -f "${TREE_NEGATIVE_FILE}" ]; then
            fail "b: cargo tree 検証（無効構成） — --tree-negative-file が指す ${TREE_NEGATIVE_FILE} が存在しません"
            tree_negative=""
        else
            tree_negative="$(cat "${TREE_NEGATIVE_FILE}")"
        fi
    else
        tree_negative="$(cargo tree -p "${CORE_PACKAGE}" -e normal --no-default-features --prefix none 2>/tmp/pfwu-check-tree-negative.log || true)"
        if [ -z "${tree_negative}" ]; then
            fail "b: cargo tree 検証（無効構成） — cargo tree の実行に失敗しました（/tmp/pfwu-check-tree-negative.log 参照）"
        fi
    fi

    if [ -n "${tree_negative}" ]; then
        leaked=()
        for entry in "${plugin_entries[@]}"; do
            crate="${entry#*:}"
            if printf '%s' "${tree_negative}" | grep -qE "(^|[[:space:]])${crate} v"; then
                leaked+=("${crate}")
            fi
        done
        if [ "${#leaked[@]}" -gt 0 ]; then
            fail "b: cargo tree 検証（無効構成） — 無効構成にもかかわらず出現したクレート: ${leaked[*]}"
        else
            pass "b: cargo tree 検証（無効構成） — 全プラグインクレートが依存グラフから 0 件"
        fi
    fi

    # --- 有効構成（ポジティブコントロール）: 対象クレートが出現し、他プラグインは混入しないこと ---
    for entry in "${plugin_entries[@]}"; do
        feature="${entry%%:*}"
        crate="${entry#*:}"

        if [ -n "${TREE_POSITIVE_DIR}" ]; then
            positive_file="${TREE_POSITIVE_DIR}/${feature}.txt"
            if [ ! -f "${positive_file}" ]; then
                fail "b: cargo tree 検証（有効構成 ${feature}） — ${positive_file} が存在しません"
                continue
            fi
            tree_positive="$(cat "${positive_file}")"
        else
            tree_positive="$(cargo tree -p "${CORE_PACKAGE}" -e normal --no-default-features --features "${feature}" --prefix none 2>/tmp/pfwu-check-tree-positive.log || true)"
            if [ -z "${tree_positive}" ]; then
                fail "b: cargo tree 検証（有効構成 ${feature}） — cargo tree の実行に失敗しました（/tmp/pfwu-check-tree-positive.log 参照）"
                continue
            fi
        fi

        if ! printf '%s' "${tree_positive}" | grep -qE "(^|[[:space:]])${crate} v"; then
            fail "b: cargo tree 検証（有効構成 ${feature}） — feature 有効時にも ${crate} が出現しません（配線切れの疑い）"
            continue
        fi

        other_leaked=()
        for other in "${plugin_entries[@]}"; do
            other_crate="${other#*:}"
            [ "${other_crate}" = "${crate}" ] && continue
            if printf '%s' "${tree_positive}" | grep -qE "(^|[[:space:]])${other_crate} v"; then
                other_leaked+=("${other_crate}")
            fi
        done
        if [ "${#other_leaked[@]}" -gt 0 ]; then
            fail "b: cargo tree 検証（有効構成 ${feature}） — 他プラグインクレートが混入: ${other_leaked[*]}"
        else
            pass "b: cargo tree 検証（有効構成 ${feature}） — ${crate} のみが出現し他プラグインの混入なし"
        fi
    done
fi

# =============================================================================
# (c) cargo geiger 検証（unsafe 0 件。依存グラフに載らなければ unsafe も載らない）
# =============================================================================
if [ "${#plugin_entries[@]}" -eq 0 ]; then
    fail "c: cargo geiger 検証 — プラグイン feature が列挙できていないため実行できません（(a) 参照）"
else
    # geiger_packages が「取得できず空」のまま (c) の判定（leaked チェック）へ
    # フォールスルーすると、空リストに対する走査は必ず leaked=0 件となり黙って
    # PASS 相当（未実行のまま無検証で終了）してしまう。取得経路のどこかで既に
    # fail() 済みかどうかを geiger_step_failed で追跡し、未報告のまま空になった
    # 場合は下（*）で明示的にフェイルクローズする（Bugbot 指摘、PR #134/#19）。
    geiger_step_failed=0
    # /tmp 配下の一時ログは固定パスだと、共有 self-hosted ランナー上で同時実行中の
    # 別ジョブが同じファイルを truncate し、jq/geiger の失敗内容が握り潰されて
    # geiger_packages が意図せず空になる恐れがある（Bugbot 指摘）。job/PID ごとに
    # 一意なパスにして競合を防ぐ。
    geiger_log="/tmp/pfwu-check-geiger.${GITHUB_RUN_ID:-local}.$$.log"
    geiger_jq_log="/tmp/pfwu-check-geiger-jq.${GITHUB_RUN_ID:-local}.$$.log"

    if [ -n "${GEIGER_PACKAGES_FILE}" ]; then
        if [ ! -f "${GEIGER_PACKAGES_FILE}" ]; then
            fail "c: cargo geiger 検証 — --geiger-packages-file が指す ${GEIGER_PACKAGES_FILE} が存在しません"
            geiger_step_failed=1
            geiger_packages=""
        else
            geiger_packages="$(cat "${GEIGER_PACKAGES_FILE}")"
        fi
    else
        if ! command -v cargo-geiger >/dev/null 2>&1; then
            fail "c: cargo geiger 検証 — cargo-geiger が見つかりません。導入: cargo install --locked cargo-geiger@0.13.0"
            geiger_step_failed=1
            geiger_packages=""
        else
            # cargo-geiger はビルドを伴い CI ランナー環境（レジストリ通信・並行ジョブに
            # よるキャッシュ競合等）に起因する一過性の失敗実績があるため、判定を FAIL に
            # 倒す前に 1 回だけ再試行する（fail-closed の後退ではなく、決定的な失敗と
            # 一過性の失敗を区別するための最小限のノイズ低減。2 回とも失敗した場合のみ
            # 下記ログ出力を経て FAIL 判定する）。
            # cargo-geiger は --target-dir オプションを持たないため、(d) の release
            # ビルド同様の隔離は CARGO_TARGET_DIR 環境変数で行う。self-hosted ランナー
            # では CARGO_TARGET_DIR がジョブ間で共有される構成になっており、並行実行中の
            # 他ブランチ（本 PR には存在しない crate を含む）のビルド成果物・増分メタ
            # データを cargo-geiger が誤って再利用しようとして
            # `Io(Os { code: 2, kind: NotFound, ... })` で失敗する事例を実機で確認した
            # （PR #134/#19 CI、診断ログ出力により特定）。専用ディレクトリに隔離することで
            # 他ジョブの状態に左右されない決定的な実行にする。
            geiger_json=""
            for geiger_attempt in 1 2; do
                geiger_json="$(CARGO_TARGET_DIR="${TARGET_DIR}-geiger" cargo geiger --manifest-path "${CORE_MANIFEST}" --no-default-features --output-format Json -q 2>"${geiger_log}" || true)"
                if [ -n "${geiger_json}" ]; then
                    break
                fi
                echo "[geiger] 試行 ${geiger_attempt}/2 が失敗しました" >&2
            done
            if [ -z "${geiger_json}" ]; then
                fail "c: cargo geiger 検証 — cargo geiger の実行に失敗しました（${geiger_log} 参照）。cargo-geiger はビルドを伴い壊れやすい実績があるため FAIL として扱う"
                geiger_step_failed=1
                # /tmp 配下のログは CI ランナー上でジョブ終了後に消え、GitHub Actions の
                # ログにも残らない（アーティファクトとして保存していないため）。原因調査を
                # 次回実行で即座に行えるよう、stdout（CI ログに残る）へも同内容を出力する
                # （PR #134/#19 CI 失敗時に一次ログを参照できず原因特定が滞った反省）。
                if [ -f "${geiger_log}" ]; then
                    echo "----- cargo geiger stderr（${geiger_log}） -----"
                    sed 's/^/[geiger] /' "${geiger_log}" || true
                    echo "----- ここまで -----"
                fi
                geiger_packages=""
            else
                geiger_packages="$(printf '%s' "${geiger_json}" | jq -r '.packages[].package.id.name' 2>"${geiger_jq_log}" || true)"
                if [ -s "${geiger_jq_log}" ]; then
                    fail "c: cargo geiger 検証 — geiger JSON 出力の解析に失敗しました（${geiger_jq_log} 参照）"
                    geiger_step_failed=1
                    echo "----- jq stderr（${geiger_jq_log}） -----"
                    sed 's/^/[geiger-jq] /' "${geiger_jq_log}" || true
                    echo "----- ここまで -----"
                    geiger_packages=""
                fi
            fi
        fi
    fi

    if [ -n "${geiger_packages}" ]; then
        leaked=()
        for entry in "${plugin_entries[@]}"; do
            crate="${entry#*:}"
            if printf '%s\n' "${geiger_packages}" | grep -qxF "${crate}"; then
                leaked+=("${crate}")
            fi
        done
        if [ "${#leaked[@]}" -gt 0 ]; then
            fail "c: cargo geiger 検証 — 無効構成の依存グラフに出現したプラグインクレート（unsafe 計上対象になり得る）: ${leaked[*]}"
        else
            pass "c: cargo geiger 検証 — 無効構成の依存グラフにプラグインクレートは 0 件（unsafe 計上対象なし）"
        fi
    elif [ "${geiger_step_failed}" -eq 0 ]; then
        # (*) 取得経路のどこでも fail() が呼ばれていないのに geiger_packages が空
        # （cargo-geiger/jq が「成功」しつつ空文字列を返した、または
        # --geiger-packages-file の指すファイルが空だった等）。unsafe グラフを
        # 実際には検証していないため、PASS/SKIP ではなく明示的に FAIL とする
        # （fail-closed、.claude/rules/security.md）。
        fail "c: cargo geiger 検証 — geiger_packages が空のため判定不能です（cargo-geiger/jq が空リストを返したか --geiger-packages-file の内容が空。unsafe グラフを検証できていないため PASS/SKIP とせず FAIL とする）"
    fi
fi

# self-hosted ランナーのディスク容量枯渇対策（PR #146/#29 CI 実測: bf-plugin-webrtc の
# 全構成ビルド (e) 中に `No space left on device` で FAIL）。(c) の geiger 用
# target-dir（cargo-geiger 自体のビルド成果物）は判定に必要な geiger_packages を
# 既に取得済みで以降の工程では不要になるため、後続の重いビルド工程（(d)(e)）が
# 使う disk 容量を最小化する目的で直ちに削除する。他ジョブと共有しない専用ディレクトリ
# （TARGET_DIR-geiger、上記コメント参照）のため、削除しても他ジョブの成果物には影響しない。
rm -rf "${TARGET_DIR}-geiger"
echo "[disk] (c) 後の空き容量:" >&2
df -h "${WORKSPACE_ROOT}/target" >&2 2>/dev/null || df -h >&2 || true

# =============================================================================
# (d) バイナリサイズ計測（コード 0 件）
# =============================================================================
size_negative="${SIZE_NEGATIVE}"
size_positive="${SIZE_POSITIVE}"
symbols_content=""

if [ -n "${SYMBOLS_FILE}" ]; then
    if [ ! -f "${SYMBOLS_FILE}" ]; then
        fail "d: バイナリサイズ計測 — --symbols-file が指す ${SYMBOLS_FILE} が存在しません"
    else
        symbols_content="$(cat "${SYMBOLS_FILE}")"
    fi
fi

if [ "${SKIP_BUILD_STEPS}" -eq 0 ] && [ -z "${SIZE_NEGATIVE}" ] && [ -z "${SIZE_POSITIVE}" ]; then
    negative_bin="${TARGET_DIR}/release/examples/minimal"
    positive_bin="${TARGET_DIR}-all/release/examples/minimal"

    echo "==> cargo build --release --example minimal（無効構成）" >&2
    if cargo build --release -p "${CORE_PACKAGE}" --example minimal --no-default-features --target-dir "${TARGET_DIR}" 2>/tmp/pfwu-check-build-negative.log; then
        size_negative="$(stat -c '%s' "${negative_bin}" 2>/dev/null || stat -f '%z' "${negative_bin}")"
    else
        fail "d: バイナリサイズ計測（無効構成ビルド） — cargo build に失敗しました（/tmp/pfwu-check-build-negative.log 参照）"
        # /tmp 配下のログは CI ランナー上でジョブ終了後に消え、GitHub Actions の
        # ログにも残らない（geiger と同じ理由。Bugbot レビュー PR #134/#19 指摘対応）。
        # 原因調査を次回実行で即座に行えるよう、stdout（CI ログに残る）へも出力する。
        if [ -f /tmp/pfwu-check-build-negative.log ]; then
            echo "----- cargo build stderr（無効構成, /tmp/pfwu-check-build-negative.log） -----"
            sed 's/^/[build-negative] /' /tmp/pfwu-check-build-negative.log || true
            echo "----- ここまで -----"
        fi
    fi

    echo "==> cargo build --release --example minimal（--all-features）" >&2
    if cargo build --release -p "${CORE_PACKAGE}" --example minimal --all-features --target-dir "${TARGET_DIR}-all" 2>/tmp/pfwu-check-build-positive.log; then
        size_positive="$(stat -c '%s' "${positive_bin}" 2>/dev/null || stat -f '%z' "${positive_bin}")"
    else
        fail "d: バイナリサイズ計測（有効構成ビルド） — cargo build に失敗しました（/tmp/pfwu-check-build-positive.log 参照）"
        if [ -f /tmp/pfwu-check-build-positive.log ]; then
            echo "----- cargo build stderr（--all-features, /tmp/pfwu-check-build-positive.log） -----"
            sed 's/^/[build-positive] /' /tmp/pfwu-check-build-positive.log || true
            echo "----- ここまで -----"
        fi
    fi

    if [ -z "${SYMBOLS_FILE}" ]; then
        if command -v nm >/dev/null 2>&1 && [ -f "${negative_bin}" ]; then
            symbols_content="$(nm "${negative_bin}" 2>/dev/null || true)"
        fi
    fi
fi

if [ -n "${size_negative}" ] && [ -n "${size_positive}" ]; then
    diff=$((size_positive - size_negative))
    if [ "${size_negative}" -le "${size_positive}" ]; then
        pass "d: バイナリサイズ計測 — 無効構成 ${size_negative} bytes <= 有効構成 ${size_positive} bytes（差分 ${diff} bytes）"
    else
        fail "d: バイナリサイズ計測 — 無効構成 ${size_negative} bytes が有効構成 ${size_positive} bytes を上回りました"
    fi
elif [ "${SKIP_BUILD_STEPS}" -eq 1 ]; then
    : # セルフテストでビルドを意図的にスキップした場合はサイズ比較を評価しない
else
    fail "d: バイナリサイズ計測 — サイズを取得できませんでした（上記ビルド失敗の詳細を参照）"
fi

if [ -n "${symbols_content}" ]; then
    symbol_leaked=()
    for entry in "${plugin_entries[@]}"; do
        crate="${entry#*:}"
        crate_symbol_prefix="$(printf '%s' "${crate}" | tr '-' '_')"
        if printf '%s' "${symbols_content}" | grep -qF "${crate_symbol_prefix}"; then
            symbol_leaked+=("${crate}")
        fi
    done
    if [ "${#symbol_leaked[@]}" -gt 0 ]; then
        fail "d: シンボル表検証 — 無効構成バイナリにプラグイン由来シンボルが検出されました: ${symbol_leaked[*]}"
    else
        pass "d: シンボル表検証 — 無効構成バイナリにプラグイン由来シンボルなし"
    fi
elif [ "${SKIP_BUILD_STEPS}" -eq 0 ] && [ -z "${SYMBOLS_FILE}" ]; then
    echo "[SKIP] d: シンボル表検証 — nm が利用できないか対象バイナリが存在しないため SKIP（サイズ比較ゲートは維持）" >&2
fi

# (c) の geiger 用 target-dir と同じ理由（disk 容量枯渇対策、PR #146/#29 CI 実測）で、
# (d) の release ビルド 2 本（無効構成／--all-features）に使った target-dir も
# サイズ・シンボル計測を終えた時点で不要になる。最も重い (e)（feature 単独構成 ×N +
# --all-features の debug ビルドを 1 つの target-dir に積み上げる）の直前に解放し、
# ピーク時のディスク使用量を抑える。--skip-build-steps 時はこれらのディレクトリを
# 作成していないため rm -rf は no-op（存在しないパスの削除は -f で無害）。
rm -rf "${TARGET_DIR}" "${TARGET_DIR}-all"
echo "[disk] (d) 後・(e) 開始前の空き容量:" >&2
df -h "${WORKSPACE_ROOT}/target" >&2 2>/dev/null || df -h >&2 || true

# =============================================================================
# (e) 全構成ビルド検証
# =============================================================================
if [ "${SKIP_BUILD_STEPS}" -eq 1 ]; then
    : # セルフテストでは cargo ビルドを伴う工程を実行しない
elif [ "${#plugin_entries[@]}" -eq 0 ]; then
    fail "e: 全構成ビルド検証 — プラグイン feature が列挙できていないため実行できません（(a) 参照）"
else
    build_failed=()
    # (c)(d) と同じ理由で、self-hosted ランナー上の共有 target ディレクトリを
    # そのまま使うとジョブ間の並行ビルドが成果物・増分メタデータを汚染し、この
    # ステップだけがフレークする（Bugbot レビュー PR #134/#19 指摘対応）。
    # --target-dir で専用ディレクトリに隔離し、他ジョブの状態に左右されない
    # 決定的な実行にする。
    build_target_dir_e="${TARGET_DIR}-e"
    build_failed_logs=()

    if ! cargo build -p "${CORE_PACKAGE}" --no-default-features --target-dir "${build_target_dir_e}" 2>/tmp/pfwu-check-build-e-negative.log; then
        build_failed+=("--no-default-features")
        build_failed_logs+=("/tmp/pfwu-check-build-e-negative.log")
    fi

    for entry in "${plugin_entries[@]}"; do
        feature="${entry%%:*}"
        if ! cargo build -p "${CORE_PACKAGE}" --no-default-features --features "${feature}" --target-dir "${build_target_dir_e}" 2>"/tmp/pfwu-check-build-e-${feature}.log"; then
            build_failed+=("--features ${feature}")
            build_failed_logs+=("/tmp/pfwu-check-build-e-${feature}.log")
        fi
    done

    if ! cargo build -p "${CORE_PACKAGE}" --all-features --target-dir "${build_target_dir_e}" 2>/tmp/pfwu-check-build-e-all.log; then
        build_failed+=("--all-features")
        build_failed_logs+=("/tmp/pfwu-check-build-e-all.log")
    fi

    if [ "${#build_failed[@]}" -gt 0 ]; then
        fail "e: 全構成ビルド検証 — ビルド失敗構成: ${build_failed[*]}（/tmp/pfwu-check-build-e-*.log 参照）"
        # ディスク容量枯渇（PR #146/#29 CI 実測: `No space left on device`）が疑われる
        # 失敗かどうかを次回調査で即座に切り分けられるよう、失敗時点の空き容量を
        # 出力する（(c)(d) 後の cleanup 実施済みでも枯渇する場合、(e) 単体のビルド量が
        # 原因と判断できる。上記 cleanup コメント参照）。
        echo "[disk] (e) 失敗時点の空き容量:" >&2
        df -h "${WORKSPACE_ROOT}/target" >&2 2>/dev/null || df -h >&2 || true
        # /tmp 配下のログは CI ランナー上でジョブ終了後に消え、GitHub Actions の
        # ログにも残らない（(d) と同じ理由）。原因調査を次回実行で即座に行えるよう、
        # stdout（CI ログに残る）へも失敗構成それぞれのログ全文を出力する。今回失敗
        # した構成のログのみを対象にする（glob だと成功構成や前回実行の残存ログまで
        # 巻き込み、self-hosted ランナーで /tmp が永続する場合に誤解を招くため）。
        for log_file in "${build_failed_logs[@]}"; do
            [ -f "${log_file}" ] || continue
            echo "----- cargo build stderr（${log_file}） -----"
            sed "s/^/[build-e:$(basename "${log_file}")] /" "${log_file}" || true
            echo "----- ここまで -----"
        done
    else
        pass "e: 全構成ビルド検証 — 無効構成・feature 単独構成・--all-features すべて成功"
    fi
fi

echo ""
if [ "${HAS_FAIL}" -eq 1 ]; then
    echo "=== pay-for-what-you-use 検証: FAIL ==="
    exit 1
else
    echo "=== pay-for-what-you-use 検証: PASS ==="
    exit 0
fi
