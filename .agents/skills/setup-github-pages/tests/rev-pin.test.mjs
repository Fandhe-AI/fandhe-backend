// rev-pin.test.mjs — 上流 fandhe-frontend の commit 固定（FF_REV）と workflow の固定方針の回帰テスト。
//
// 生成器（fandhe-frontend の docs-site）は crates.io に公開されておらず、`FF_REV` の commit を
// 固定 rev の匿名 `cargo install --git`（--locked）でインストールして使う。FF_REV が未固定・不正な値・箇所ごとの不一致になると、
// 上流の任意コミット（乗っ取り・破壊的変更を含む）をビルドして公開物に混ぜる経路になる。
// 唯一の定義元は templates/docs-site-gen/FF_REV（対象リポジトリでは tools/docs-site-gen/FF_REV）。
// pages.yml・build-local.sh は値を直書きせずこのファイルを読む設計のため、ここでは次を検証する。
//   1. FF_REV が 40 桁の小文字 hex 1 行であること
//   2. スキル内のドキュメント・スクリプト・テンプレートに現れる 40 桁 hex（`uses:` 行の
//      action SHA を除く）がすべて FF_REV と一致すること（更新漏れの検出）
//   3. build-local.sh が FF_REV ファイルを読み、使用前に ^[0-9a-f]{40}$ で検証し、固定 URL・固定 rev でだけ取得すること
//   4. pages.yml が FF_REV を直書きせず、cache キーが FF_REV・build-local.sh のハッシュと rustc の commit-hash で構成されること
//   5. pages.yml の action が SHA 固定（Fandhe-AI/actions@latest のみ例外）、run: に ${{ }} が無いこと、
//      deploy 呼び出しの必須設定（runner-label 等）が揃っていること
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const SKILL_DIR = join(dirname(fileURLToPath(import.meta.url)), '..')
const read = (rel) => readFileSync(join(SKILL_DIR, rel), 'utf8')
const FF_REV_PATH = 'templates/docs-site-gen/FF_REV'
const HEX40 = /^[0-9a-f]{40}$/

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) {
      if (name === 'fixtures' || name === '__pycache__') continue
      walk(p, out)
    } else out.push(p)
  }
  return out
}

const ffRev = read(FF_REV_PATH).trim()

test('FF_REV は 40 桁の小文字 hex 1 行', () => {
  assert.match(ffRev, HEX40)
  assert.equal(read(FF_REV_PATH).trim().split('\n').length, 1)
})

test('スキル内に現れる 40 桁 hex は（action SHA を除き）すべて FF_REV と一致する', () => {
  const targets = walk(SKILL_DIR).filter(
    (p) => !p.endsWith('rev-pin.test.mjs') && relative(SKILL_DIR, p) !== FF_REV_PATH
  )
  let seen = 0
  for (const p of targets) {
    for (const line of readFileSync(p, 'utf8').split('\n')) {
      if (/\buses:\s/.test(line)) continue
      for (const m of line.matchAll(/(?<![0-9a-f])[0-9a-f]{40}(?![0-9a-f])/g)) {
        seen += 1
        assert.equal(m[0], ffRev, `${relative(SKILL_DIR, p)} の ${m[0]} が FF_REV と不一致`)
      }
    }
  }
  assert.ok(seen >= 1, 'SKILL.md に現在の FF_REV が 1 件も記載されていない（更新手順の記載漏れ）')
})

test('build-local.sh は FF_REV ファイルを読み、使用前に 40 桁 hex で検証する', () => {
  const sh = read('scripts/build-local.sh')
  assert.match(sh, /FF_REV="\$\(tr -d '\[:space:\]' < "\$\{SCRIPT_DIR\}\/FF_REV"\)"/)
  const readIdx = sh.indexOf('FF_REV="$(')
  const checkIdx = sh.indexOf('^[0-9a-f]{40}$')
  const useIdx = sh.indexOf('--rev "${FF_REV}"')
  assert.ok(readIdx >= 0 && checkIdx > readIdx && useIdx > checkIdx, '読込 → 検証 → 使用の順になっていない')
  assert.doesNotMatch(sh, /(?<![0-9a-f])[0-9a-f]{40}(?![0-9a-f])/, 'build-local.sh に commit SHA が直書きされている')
})

test('build-local.sh は固定 URL・固定 rev・--locked の匿名 cargo install と --no-page-sections で生成する', () => {
  const sh = read('scripts/build-local.sh')
  const code = sh.split('\n').filter((l) => !/^\s*#/.test(l)).join('\n')
  const url = code.match(/^FF_URL="([^"]*)"$/m)
  assert.ok(url, 'FF_URL の定義が無い')
  assert.equal(url[1], 'https://github.com/Fandhe-AI/fandhe-frontend')
  assert.ok(!url[1].includes('${'), 'FF_URL が可変値を含む')
  assert.ok(
    code.includes('cargo install --git "${FF_URL}" --rev "${FF_REV}" --locked --root "${INSTALL_ROOT}" fandhe-frontend-docs-site'),
    'cargo install の引数が仕様と異なる',
  )
  assert.ok(code.includes('--no-page-sections'), '--no-page-sections が無い')
  assert.match(code, /GIT_TERMINAL_PROMPT=0 cargo install/)
  assert.doesNotMatch(code, /submodule|--recurse|_ff|cargo build|cargo metadata|docs-site-gen\/target\/release|git (init|fetch|clone)/,
    '旧経路（_ff・wrapper build・registry 検査）の残骸がある')
  assert.doesNotMatch(code, /\beval\b/)
  assert.doesNotMatch(code, /templates\/docs-site-gen|Cargo\.toml|main\.rs/, 'wrapper を参照している')
})

test('build-local.sh の docs-site 実行行はすべて --no-page-sections を持つ（予約パス検査の代わりにショーケース混入を防ぐ）', () => {
  const sh = read('scripts/build-local.sh')
  const calls = sh.split('\n').filter((l) => /^\s*"\$\{INSTALL_ROOT\}\/bin\/docs-site"\s/.test(l))
  assert.equal(calls.length, 1, `docs-site の実行行が 1 行でない: ${calls.length}`)
  for (const l of calls) assert.ok(/\s--no-page-sections(\s|$)/.test(l), `--no-page-sections の無い実行行: ${l}`)
})

test('build-local.sh のライセンス取得は固定 URL・https 限定・リダイレクト非追従・時間とサイズ上限付き', () => {
  const sh = read('scripts/build-local.sh')
  const code = sh.split('\n').filter((l) => !/^\s*#/.test(l)).join('\n')
  const m = code.match(/license_url="([^"]*)"/)
  assert.ok(m, 'license_url の定義が無い')
  assert.equal(m[1], 'https://raw.githubusercontent.com/Fandhe-AI/fandhe-frontend/${FF_REV}/LICENSE-MIT')
  assert.equal((m[1].match(/\$\{/g) || []).length, 1, '${FF_REV} 以外の可変部分がある')
  const curl = code.match(/curl [\s\S]*?"\$\{license_url\}"/)[0]
  for (const opt of ["--proto '=https'", "--proto-redir '=https'", '--max-time', '--max-filesize', '--fail']) {
    assert.ok(curl.includes(opt), `curl に ${opt} が無い`)
  }
  assert.doesNotMatch(curl, /\s-[a-zA-Z]*L|--location/, 'curl がリダイレクトを追従する')
})

test('pages.yml は FF_REV を直書きせず、cache キーに FF_REV・build-local.sh のハッシュを含む', () => {
  const y = read('templates/pages.yml')
  assert.doesNotMatch(y, /FF_REV\s*[:=]/, 'workflow に FF_REV の定義が重複している')
  const hf = y.match(/hashFiles\(([^)]*)\)/)
  assert.ok(hf, 'hashFiles が無い')
  const args = [...hf[1].matchAll(/'([^']+)'/g)].map((m) => m[1])
  assert.deepEqual(args, ['tools/docs-site-gen/FF_REV', 'tools/docs-site-gen/build-local.sh'])
  const key = y.match(/key: .*/)[0]
  assert.doesNotMatch(key, /Cargo\.toml|main\.rs/, 'ビルドに使わないファイルが cache キーに入っている')
  assert.match(y, /steps\.rustc\.outputs\.hash/)
  assert.match(y, /rustc -vV \| sed -n 's\/\^commit-hash: \/\/p'/)
})

test('pages.yml の cache は cargo install の出力先だけを対象とし、完全一致でのみ復元する', () => {
  const y = read('templates/pages.yml')
  const sh = read('scripts/build-local.sh')
  const root = sh.match(/^INSTALL_ROOT="\$\{SCRIPT_DIR\}\/([^"]+)"/m)
  assert.ok(root, 'build-local.sh に INSTALL_ROOT の定義が無い')
  assert.equal(y.match(/^\s*path: (tools\/docs-site-gen\/\S+)$/m)[1], `tools/docs-site-gen/${root[1]}`)
  // 存在しないパスを hashFiles に渡すと黙って空文字になり、FF_REV を変えてもキーが変わらなくなる
  const scaffold = read('scripts/scaffold.py')
  const dests = [...scaffold.matchAll(/^\s*\("[^"]+", "([^"]+)"/gm)].map((m) => m[1])
  const hf = y.match(/hashFiles\(([^)]*)\)/)[1]
  for (const m of hf.matchAll(/'([^']+)'/g)) assert.ok(dests.includes(m[1]), `hashFiles の ${m[1]} が scaffold の配置先に無い`)
  assert.doesNotMatch(y, /restore-keys/, 'restore-keys は別の FF_REV の復元を許す')
  assert.doesNotMatch(y, /cache-hit/, '省略判定は build-local.sh に置く')
  const build = y.slice(y.indexOf('name: "build: install'))
  assert.doesNotMatch(build.split('- name: Upload')[0], /^\s*if:/m, 'build ステップに if: がある')
  const order = ['id: rustc', 'actions/cache@', 'build-local.sh --out'].map((k) => y.indexOf(k))
  assert.ok(order.every((n) => n >= 0) && order[0] < order[1] && order[1] < order[2], 'rustc → cache → build の順でない')
  for (const m of y.matchAll(/^\s*- name: (.*)$/gm)) assert.doesNotMatch(m[1], /rebrand|wrapper|fetch/i, `旧工程名: ${m[1]}`)
})

test('build-local.sh の install 省略判定は台帳と FF_REV を照合し、cargo install より前にある', () => {
  const code = read('scripts/build-local.sh').split('\n').filter((l) => !/^\s*#/.test(l)).join('\n')
  const guard = code.indexOf('guard_install_tree || exit 2')
  const judge = code.indexOf('"(git+%s?rev=%s#%s)"')
  const install = code.indexOf('cargo install --git')
  assert.ok(guard >= 0 && judge > guard && install > judge, '省略判定の位置が不正')
  // 台帳は対象パッケージのエントリ単位で照合する（grep -Fq の部分一致では別エントリの記載でも通る）
  assert.doesNotMatch(code.slice(guard, install), /grep -Fq/)
  assert.match(code.slice(guard, install), /tomllib/)
  assert.match(code.slice(guard, install), /fandhe-frontend-docs-site/)
})

test('pages.yml の action は SHA 固定（Fandhe-AI/actions の reusable のみ @latest）', () => {
  const y = read('templates/pages.yml')
  const uses = [...y.matchAll(/^\s*(?:-\s+)?uses:\s+(\S+)/gm)].map((m) => m[1])
  assert.ok(uses.length >= 5)
  const unpinned = uses.filter((u) => !/@[0-9a-f]{40}$/.test(u))
  assert.deepEqual(unpinned, ['Fandhe-AI/actions/.github/workflows/pages-deploy.yml@latest'])
})

test('pages.yml の run: ブロックに ${{ }} を直接埋め込んでいない（env 経由の原則）', () => {
  const lines = read('templates/pages.yml').split('\n')
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(/^(\s*)(?:-\s+)?run:\s*(.*)$/)
    if (!m) continue
    assert.ok(!m[2].includes('${{'), `line ${i + 1}: run に式が直書きされている`)
    if (m[2] === '|' || m[2] === '>') {
      const indent = m[1].length
      for (let j = i + 1; j < lines.length; j++) {
        if (lines[j].trim() === '') continue
        if (lines[j].search(/\S/) <= indent) break
        assert.ok(!lines[j].includes('${{'), `line ${j + 1}: run ブロックに式が直書きされている`)
      }
    }
  }
})

test('pages.yml の平文スカラー値に `: ` を含めない（YAML パースエラーの回帰防止）', () => {
  // `run: echo "…commit-hash: …"` のように平文スカラー内へ `: ` が入ると
  // "mapping values are not allowed" で workflow 全体が無効になる（実測で発生）。
  // 引用符・ブロックスカラー（| >）・フロー記法の値は対象外。
  const lines = read('templates/pages.yml').split('\n')
  lines.forEach((line, i) => {
    const m = line.match(/^\s*(?:-\s+)?[A-Za-z0-9_.-]+:\s+([^"'|>\[{\s].*)$/)
    if (!m) return
    const value = m[1].replace(/\s+#.*$/, '')
    assert.ok(!value.includes(': '), `line ${i + 1}: 平文スカラーに ": " を含む（ブロックスカラーにする）: ${line.trim()}`)
  })
})

test('pages.yml の必須設定（runner-label・permissions・concurrency・artifact）', () => {
  const y = read('templates/pages.yml')
  assert.match(y, /runner-label:\s*ubuntu-latest/, 'runner-label 省略は deploy が永久 pending になる')
  assert.match(y, /persist-credentials:\s*false/)
  assert.match(y, /include-hidden-files:\s*true/)
  assert.match(y, /if-no-files-found:\s*error/)
  assert.match(y, /name:\s*pages-dist/)
  assert.match(y, /dist-dir:\s*"\."/)
  assert.match(y, /cancel-in-progress:\s*false/)
  assert.match(y, /^permissions:\n\s+contents:\s*read$/m)
  assert.match(y, /pages:\s*write/)
  assert.match(y, /id-token:\s*write/)
  assert.match(y, /^\s+- "rust-toolchain\.toml"$/m, 'rust-toolchain.toml はビルド入力のため paths に必要')
  assert.match(y, /^\s+- "tools\/docs-site-gen\/\*\*"$/m)
  assert.match(y, /branches:\s*\["__SGP_DEFAULT_BRANCH__"\]/)
})

test('scaffold.py が配置するテンプレート・スクリプトがすべて実在する', () => {
  const py = read('scripts/scaffold.py')
  const srcs = [...py.matchAll(/^\s*\("((?:templates|scripts)\/[^"]+)",/gm)].map((m) => m[1])
  // 配置物は 8 件。wrapper（Cargo.toml・src/main.rs）・brand.toml・置換スクリプトは配置しない（旧構成の削除候補として案内するだけ）
  assert.deepEqual(srcs.sort(), [
    'scripts/_common.py',
    'scripts/build-local.sh',
    'scripts/check_site.py',
    'templates/docs-site-gen/FF_REV',
    'templates/index.md',
    'templates/nav.toml',
    'templates/pages.yml',
    'templates/rust-toolchain.toml',
  ])
  for (const s of srcs) assert.doesNotThrow(() => statSync(join(SKILL_DIR, s)), `${s} が存在しない`)
  for (const gone of ['templates/brand.toml', 'templates/docs-site-gen/Cargo.toml', 'templates/docs-site-gen/src/main.rs']) {
    assert.throws(() => statSync(join(SKILL_DIR, gone)), `${gone} は削除済みのはず`)
  }
  const filesBlock = py.slice(py.indexOf('FILES = ['), py.indexOf('\n]\n', py.indexOf('FILES = [')))
  assert.doesNotMatch(filesBlock, /Cargo\.toml|main\.rs|brand\.toml/)
})

test('SKILL.md の frontmatter（name・model・user-invocable・description 長）', () => {
  const md = read('SKILL.md')
  const fm = md.match(/^---\n([\s\S]*?)\n---\n/)
  assert.ok(fm, 'frontmatter が無い')
  assert.match(fm[1], /^name: setup-github-pages$/m)
  assert.match(fm[1], /^model: sonnet$/m)
  assert.match(fm[1], /^user-invocable: true$/m)
  const desc = fm[1].match(/^description:\s*(.+)$/m)
  assert.ok(desc, 'description が 1 行で書かれていない')
  assert.ok(desc[1].length <= 1536, 'description が上限超過')
})

test('SKILL.md は新規構築と更新の両方を案内し、更新の発火語・モード判定・非破壊の方針を含む', () => {
  const md = read('SKILL.md')
  const desc = md.match(/^description:\s*(.+)$/m)[1]
  for (const w of ['Rust 製 SSG', 'Markdown 管理', 'Actions 自動デプロイ', 'ブランド設定', 'Pages サイトを更新して', 'デザインを最新にして', 'GitHub Pages で公開したい', 'docs サイト作って', 'fandhe-frontend と同じデザイン', 'setup-firebase-hosting', 'create-html-report']) {
    assert.ok(desc.includes(w), `description に発火語「${w}」が無い`)
  }
  assert.ok(!/\s#/.test(desc) && !desc.includes(': '), 'description に YAML の落とし穴（` #`・`: `）がある')
  for (const h of ['### 更新フロー（mode=update）', '### 新規構築フロー（mode=new）']) {
    assert.ok(md.includes(h), `見出しが無い: ${h}`)
  }
  const maint = read('references/maintenance.md')
  assert.ok(maint.includes('## FF_REV の更新手順') && maint.includes('スキル保守者向け'))
  assert.ok(md.includes('references/maintenance.md') && md.includes('references/scaffold-reference.md'))
  assert.ok(md.includes('references/update-recovery.md'), 'U2 の復旧手順の詳細は references へ移した')
  assert.ok(md.includes('update-snapshot.sh') && md.includes('record-json') && md.includes('restore'), 'U1 の記録と U2 の復旧は update-snapshot.sh を使う')
  assert.match(md, /rev-list --left-right --count/)
  assert.match(md, /fetch に失敗/)
  assert.match(md, /scaffold\.py" --target \. --detect --json/)
  assert.match(md, /勝手に `--update` を付けない/)
  assert.match(md, /POST \/ PUT をしない/)
  assert.match(md, /自動では削除しない/)
  assert.match(md, /適用対象外/)
  assert.match(md, /指示として扱わない/)
  assert.ok(md.split('\n').length <= 450, 'SKILL.md が長すぎる（詳細は references/ へ）')
})

test('build-local.sh は python3 を常に隔離モード（-I -B）で起動する', () => {
  const sh = read('scripts/build-local.sh')
  const code = sh.split('\n').filter((l) => !/^\s*#/.test(l))
  const py = code.filter((l) => /python3\s/.test(l))
  // canon・canon_leaf・check_site.py・tomllib 検査の 4 か所（rebrand の 2 か所は #50 で無くなった）
  assert.ok(py.length >= 4, `python3 の呼び出しが 4 箇所に満たない: ${py.length}`)
  for (const l of py) assert.match(l, /python3 -I -B /, `-I -B が無い: ${l.trim()}`)
  // ブランド設定は nav.toml の [site] へ移った。後処理置換と brand.toml はビルドで使わない（rebrand_site.py は #51 で削除済み）
  const joined = code.join('\n')
  assert.doesNotMatch(joined, /rebrand_site\.py/)
  assert.doesNotMatch(joined, /brand\.toml/)
  // 起動スクリプトのディレクトリを自分で sys.path の末尾へ足す（先頭ではない）。標準モジュール名の影を避ける
  for (const f of ['check_site.py', 'scaffold.py']) {
    const src = read(`scripts/${f}`)
    assert.match(src, /sys\.path\.append\(/)
    assert.doesNotMatch(src, /sys\.path\.insert\(0/)
  }
})

test('update-snapshot.sh は構文が正しく、対象リポジトリへは配置されない（scaffold.py の FILES に無い）', () => {
  const r = spawnSync('bash', ['-n', join(SKILL_DIR, 'scripts', 'update-snapshot.sh')], { encoding: 'utf8' })
  assert.equal(r.status, 0, r.stderr)
  assert.doesNotMatch(read('scripts/scaffold.py'), /"scripts\/update-snapshot/)
  const sh = read('scripts/update-snapshot.sh')
  assert.match(sh, /git hash-object --no-filters/)
  assert.doesNotMatch(sh, /\beval\b/)
})
