// rev-pin.test.mjs — 上流 fandhe-frontend の commit 固定（FF_REV）と workflow の固定方針の回帰テスト。
//
// 生成器（fandhe-frontend の docs-site）は crates.io に公開されておらず、`FF_REV` の commit を
// shallow fetch して path 依存でビルドする。FF_REV が未固定・不正な値・箇所ごとの不一致になると、
// 上流の任意コミット（乗っ取り・破壊的変更を含む）をビルドして公開物に混ぜる経路になる。
// 唯一の定義元は templates/docs-site-gen/FF_REV（対象リポジトリでは tools/docs-site-gen/FF_REV）。
// pages.yml・build-local.sh は値を直書きせずこのファイルを読む設計のため、ここでは次を検証する。
//   1. FF_REV が 40 桁の小文字 hex 1 行であること
//   2. スキル内のドキュメント・スクリプト・テンプレートに現れる 40 桁 hex（`uses:` 行の
//      action SHA を除く）がすべて FF_REV と一致すること（更新漏れの検出）
//   3. build-local.sh が FF_REV ファイルを読み、使用前に ^[0-9a-f]{40}$ で検証していること
//   4. pages.yml が FF_REV を直書きせず、cache キーに FF_REV ファイルのハッシュを含むこと
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
  const useIdx = sh.indexOf('fetch -q --depth 1 origin "${FF_REV}"')
  assert.ok(readIdx >= 0 && checkIdx > readIdx && useIdx > checkIdx, '読込 → 検証 → 使用の順になっていない')
  assert.doesNotMatch(sh, /(?<![0-9a-f])[0-9a-f]{40}(?![0-9a-f])/, 'build-local.sh に commit SHA が直書きされている')
  assert.match(sh, /git -C "\$\{FF_DIR\}" fetch -q --depth 1/)
  const code = sh.split('\n').filter((l) => !/^\s*#/.test(l)).join('\n')
  assert.doesNotMatch(code, /submodule|--recurse/, 'submodule を取る操作が混入している')
})

test('build-local.sh は wrapper build 前に cargo metadata で registry 依存 0 件を検査する', () => {
  const sh = read('scripts/build-local.sh')
  const meta = sh.indexOf('cargo metadata --format-version 1')
  const build = sh.indexOf('cargo build --release')
  assert.ok(meta >= 0 && build > meta, 'cargo metadata による依存検査が build より前に無い')
  assert.match(sh, /p\.get\("source"\) is not None/)
})

test('pages.yml は FF_REV を直書きせず、cache キーに FF_REV ファイルのハッシュを含む', () => {
  const y = read('templates/pages.yml')
  assert.doesNotMatch(y, /FF_REV\s*[:=]/, 'workflow に FF_REV の定義が重複している')
  assert.match(y, /hashFiles\('tools\/docs-site-gen\/FF_REV'/)
  assert.match(y, /steps\.rustc\.outputs\.hash/)
  assert.match(y, /rustc -vV \| sed -n 's\/\^commit-hash: \/\/p'/)
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
  assert.ok(srcs.length >= 10)
  for (const s of srcs) assert.doesNotThrow(() => statSync(join(SKILL_DIR, s)), `${s} が存在しない`)
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
  for (const w of ['Rust 製 SSG', 'Markdown 管理', 'Actions 自動デプロイ', 'ブランド置換', 'Pages サイトを更新して', 'デザインを最新にして', 'GitHub Pages で公開したい', 'docs サイト作って', 'fandhe-frontend と同じデザイン', 'setup-firebase-hosting', 'create-html-report']) {
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
  assert.ok(py.length >= 6, `python3 の呼び出しが 6 箇所に満たない: ${py.length}`)
  for (const l of py) assert.match(l, /python3 -I -B /, `-I -B が無い: ${l.trim()}`)
  // 起動スクリプトのディレクトリを自分で sys.path の末尾へ足す（先頭ではない）。標準モジュール名の影を避ける
  for (const f of ['check_site.py', 'rebrand_site.py', 'scaffold.py']) {
    const src = read(`scripts/${f}`)
    assert.match(src, /sys\.path\.append\(/)
    assert.doesNotMatch(src, /sys\.path\.insert\(0/)
  }
})

test('wrapper の Cargo.toml は build = false（対象リポジトリの build.rs を自動実行しない）で、registry 依存を持たない', () => {
  const toml = read('templates/docs-site-gen/Cargo.toml')
  assert.match(toml, /^build = false$/m)
  const deps = toml.split('[dependencies]')[1].split('[workspace]')[0]
  for (const l of deps.split('\n').filter((x) => x.includes('='))) {
    assert.match(l, /path = "/, `path 依存以外が混入している: ${l}`)
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
