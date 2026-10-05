// rebrand.test.mjs — python 製スクリプト（rebrand_site.py / check_site.py / scaffold.py）の
// 回帰テスト（test_rebrand.py）を `node --test` の入口から実行するブリッジ。
//
// Python のテストを別コマンドにすると CI・ローカルで片方が実行されず退行を見逃すため、
// 本リポジトリの既存スキル（setup-firebase-hosting）と同じ `node --test` 1 コマンドに集約する。
// 失敗時は unittest の出力をそのまま assert メッセージに載せて原因を追えるようにする。
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const TESTS_DIR = dirname(fileURLToPath(import.meta.url))

test('python3 が利用できる', () => {
  const r = spawnSync('python3', ['--version'], { encoding: 'utf8' })
  assert.equal(r.status, 0, 'python3 が必要（setup-github-pages の前提条件）')
})

test('test_rebrand.py（rebrand・check_site・scaffold の unittest）が全件成功する', () => {
  const r = spawnSync('python3', [join(TESTS_DIR, 'test_rebrand.py')], { encoding: 'utf8' })
  assert.equal(r.status, 0, `unittest 失敗\n${r.stdout}\n${r.stderr}`)
  assert.match(r.stderr, /\nOK\n?$/, 'unittest が OK で終了していない')
})
