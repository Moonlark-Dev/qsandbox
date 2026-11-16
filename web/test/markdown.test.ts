/**
 * Regression tests for the dependency-free QQ Markdown renderer.
 *
 * Run with `npm test` (Node 22 type stripping, no build step) so the fidelity
 * details that only exist in the GUI, such as the client's `\(…\)` inline
 * formula delimiter, stay covered.
 */
import assert from 'node:assert/strict'
import test from 'node:test'

import { markdownPlain, renderMarkdown } from '../src/markdown.ts'

test('renders an escaped parenthesis pair as an inline formula', () => {
  // Moonlark-Dev/Moonlark#1556: QQ shows `回声洞 —— \(1234\)` as a formula.
  const html = renderMarkdown('回声洞 —— \\(1234\\)')
  assert.match(html, /class="qq-math"/)
  assert.match(html, />1234</)
  assert.ok(!html.includes('—— \\(1234\\)'), 'the delimiters must not leak into the visible text')
  // The TeX source stays available in the tooltip instead.
  assert.match(html, /title="公式：\\\(1234\\\)"/)
})

test('leaves ordinary parentheses as text', () => {
  assert.equal(renderMarkdown('回声洞 —— (1234)'), '<p>回声洞 —— (1234)</p>')
})

test('does not treat code spans as formulas', () => {
  assert.equal(renderMarkdown('示例 `\\(123\\)` 结束'), '<p>示例 <code>\\(123\\)</code> 结束</p>')
})

test('leaves an unclosed delimiter as text', () => {
  assert.equal(renderMarkdown('未闭合 \\(123 保持原样'), '<p>未闭合 \\(123 保持原样</p>')
})

test('escapes HTML inside the formula source', () => {
  const html = renderMarkdown('\\(<b>\\)')
  assert.match(html, /&lt;b&gt;/)
  assert.ok(!html.includes('<b>'))
})

test('flattens formulas for one-line previews', () => {
  assert.equal(markdownPlain('回声洞 —— \\(1234\\)'), '回声洞 —— 1234')
})
