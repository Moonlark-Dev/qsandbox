/**
 * A small, dependency-free Markdown renderer for QQ Markdown messages.
 *
 * The input is escaped before any tag is produced, and the only tags that can
 * appear in the output are the ones built here. Raw HTML in a bot message is
 * therefore shown as text instead of being executed.
 */

const ESCAPES: Record<string, string> = {
  '&': '&amp;',
  '<': '&lt;',
  '>': '&gt;',
  '"': '&quot;',
  "'": '&#39;',
}

export function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (char) => ESCAPES[char] ?? char)
}

export interface MarkdownOptions {
  /** Resolve `<qqbot-at-user id="…">`'s openid to a group nickname. */
  atUser?: (id: string) => string | undefined
}

// QQ text-interaction tags (official `trans/text-chain.html`).  They are part
// of the Markdown body, so they are rendered as the client would render them:
// an `@某人` chip for mentions and a clickable command chip for `/指令`.
// Keep this grammar in sync with `crates/mqs/src/text_tags.rs`: the server
// parses the same tags to decide which command a click pushes, and both sides
// must agree on how many command tags exist (the GUI only sends the index).
const QQ_TAG_RE =
  /<qqbot-(at-user|at-everyone|cmd-enter|cmd-input)((?:\s+[A-Za-z][A-Za-z0-9-]*\s*=\s*(?:"[^"]*"|'[^']*'))*)\s*\/?>/g
const QQ_ATTR_RE = /([A-Za-z][A-Za-z0-9-]*)\s*=\s*(?:"([^"]*)"|'([^']*)')/g

function tagAttributes(source: string): Record<string, string> {
  const attrs: Record<string, string> = {}
  let match: RegExpExecArray | null
  QQ_ATTR_RE.lastIndex = 0
  while ((match = QQ_ATTR_RE.exec(source))) {
    // Duplicate attribute names: the last one wins, like the Rust parser.
    attrs[match[1].toLowerCase()] = match[2] ?? match[3] ?? ''
  }
  return attrs
}

// The official docs require `text`/`show` to be urlencoded before embedding.
function decodeTagValue(raw: string): string {
  if (!raw) return ''
  try {
    return decodeURIComponent(raw)
  } catch {
    return raw
  }
}

function renderQqTag(
  name: string,
  rawAttributes: string,
  options: MarkdownOptions,
): string {
  const attrs = tagAttributes(rawAttributes)
  if (name === 'at-everyone') {
    return `<span class="qq-at qq-at-all" title="@全体成员">@全体成员</span>`
  }
  if (name === 'at-user') {
    const id = decodeTagValue(attrs.id || '')
    if (!id) return ''
    const resolved = options.atUser?.(id)?.trim()
    const label = resolved || '成员'
    return `<span class="qq-at" data-qq-at="${escapeHtml(id)}" title="${escapeHtml(resolved ? `@${resolved}` : id)}">@${escapeHtml(label)}</span>`
  }
  const text = decodeTagValue(attrs.text || '')
  if (!text) return ''
  if (name === 'cmd-enter') {
    return `<button type="button" class="qq-cmd" data-qq-cmd="enter" title="${escapeHtml(`点击直接发送：${text}`)}">${escapeHtml(text)}</button>`
  }
  const show = decodeTagValue(attrs.show || '') || text
  const reference = (attrs.reference || '').trim().toLowerCase() === 'true'
  const hint = reference ? '点击填入输入框并引用本条消息' : '点击填入输入框'
  return `<button type="button" class="qq-cmd" data-qq-cmd="input" data-qq-reference="${reference}" title="${escapeHtml(`${hint}：${text}`)}">${escapeHtml(show)}</button>`
}

function safeUrl(raw: string): string | null {
  // `raw` has already been HTML-escaped, so turn &amp; back into & before
  // validating and writing it into the href/src attribute.
  const url = raw.replace(/&amp;/g, '&').trim()
  if (/^(https?:\/\/|mailto:)/i.test(url)) return url
  if (url.startsWith('/') && !url.startsWith('//')) return url
  return null
}

function renderInline(source: string, options: MarkdownOptions): string {
  // Code spans, images and links are pulled out as opaque tokens first, so
  // emphasis markers inside a URL or a code span cannot corrupt the markup.
  const tokens: string[] = []
  const keep = (html: string): string => {
    tokens.push(html)
    return `\u0000${tokens.length - 1}\u0000`
  }

  let text = source.replace(/`([^`\n]+)`/g, (_, inner: string) =>
    keep(`<code>${escapeHtml(inner)}</code>`),
  )
  // Text-interaction tags are pulled out before escaping, so `<qqbot-…>` is
  // never shown verbatim; anything unrecognised stays plain text.
  text = text.replace(QQ_TAG_RE, (match, name: string, attrs: string) => {
    const html = renderQqTag(name, attrs, options)
    return html ? keep(html) : match
  })
  text = escapeHtml(text)

  text = text.replace(/!\[([^\]]*)\]\(([^)\s]+)\)/g, (match, alt: string, url: string) => {
    const safe = safeUrl(url)
    if (!safe) return match
    return keep(`<img class="md-image" src="${escapeHtml(safe)}" alt="${alt}" loading="lazy" />`)
  })
  text = text.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (match, label: string, url: string) => {
    const safe = safeUrl(url)
    if (!safe) return match
    return keep(
      `<a href="${escapeHtml(safe)}" target="_blank" rel="noopener noreferrer">${label}</a>`,
    )
  })

  text = text.replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>')
  text = text.replace(/__([^_\n]+)__/g, '<strong>$1</strong>')
  text = text.replace(/(^|[^*])\*([^*\n]+)\*(?!\*)/g, '$1<em>$2</em>')
  text = text.replace(/(^|[^_])_([^_\n]+)_(?!_)/g, '$1<em>$2</em>')
  text = text.replace(/~~([^~\n]+)~~/g, '<del>$1</del>')

  return text.replace(/\u0000(\d+)\u0000/g, (_, index: string) => tokens[Number(index)] ?? '')
}

function isTableSeparator(line: string): boolean {
  return /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)+\|?\s*$/.test(line)
}

function splitRow(row: string): string[] {
  return row
    .trim()
    .replace(/^\|/, '')
    .replace(/\|$/, '')
    .split('|')
    .map((cell) => cell.trim())
}

function startsBlock(lines: string[], index: number): boolean {
  const line = lines[index] ?? ''
  if (!line.trim()) return true
  if (/^```/.test(line)) return true
  if (/^#{1,6}\s+/.test(line)) return true
  if (/^\s*([-*_])(?:\s*\1){2,}\s*$/.test(line)) return true
  if (/^>\s?/.test(line)) return true
  if (/^\s*([-+*]|\d+[.)])\s+/.test(line)) return true
  if (line.includes('|') && index + 1 < lines.length && isTableSeparator(lines[index + 1])) return true
  return false
}

function renderBlocks(source: string, options: MarkdownOptions): string {
  const lines = source.replace(/\r\n?/g, '\n').split('\n')
  const blocks: string[] = []
  let index = 0

  while (index < lines.length) {
    const line = lines[index]
    if (!line.trim()) {
      index++
      continue
    }

    const fence = line.match(/^```\s*([\w+-]*)\s*$/)
    if (fence) {
      const body: string[] = []
      index++
      while (index < lines.length && !/^```\s*$/.test(lines[index])) body.push(lines[index++])
      if (index < lines.length) index++
      const language = fence[1] ? ` class="language-${escapeHtml(fence[1])}"` : ''
      blocks.push(`<pre><code${language}>${escapeHtml(body.join('\n'))}</code></pre>`)
      continue
    }

    const heading = line.match(/^(#{1,6})\s+(.*)$/)
    if (heading) {
      const level = heading[1].length
      blocks.push(`<h${level}>${renderInline(heading[2], options)}</h${level}>`)
      index++
      continue
    }

    if (/^\s*([-*_])(?:\s*\1){2,}\s*$/.test(line)) {
      blocks.push('<hr />')
      index++
      continue
    }

    if (/^>\s?/.test(line)) {
      const quote: string[] = []
      while (index < lines.length && /^>\s?/.test(lines[index])) {
        quote.push(lines[index++].replace(/^>\s?/, ''))
      }
      blocks.push(`<blockquote>${renderBlocks(quote.join('\n'), options)}</blockquote>`)
      continue
    }

    const list = line.match(/^(\s*)([-+*]|\d+[.)])\s+(.*)$/)
    if (list) {
      const ordered = /^\d/.test(list[2])
      const items: string[] = []
      while (index < lines.length) {
        const item = lines[index].match(/^(\s*)([-+*]|\d+[.)])\s+(.*)$/)
        if (!item || /^\d/.test(item[2]) !== ordered) break
        items.push(`<li>${renderInline(item[3], options)}</li>`)
        index++
      }
      const tag = ordered ? 'ol' : 'ul'
      blocks.push(`<${tag}>${items.join('')}</${tag}>`)
      continue
    }

    if (line.includes('|') && index + 1 < lines.length && isTableSeparator(lines[index + 1])) {
      const header = splitRow(line)
      index += 2
      const rows: string[][] = []
      while (index < lines.length && lines[index].trim() && lines[index].includes('|')) {
        rows.push(splitRow(lines[index++]))
      }
      const head = header
        .map((cell) => `<th>${renderInline(cell, options)}</th>`)
        .join('')
      const body = rows
        .map((row) => `<tr>${header.map((_, cell) => `<td>${renderInline(row[cell] ?? '', options)}</td>`).join('')}</tr>`)
        .join('')
      blocks.push(`<table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`)
      continue
    }

    const paragraph: string[] = []
    while (index < lines.length && !startsBlock(lines, index)) paragraph.push(lines[index++])
    blocks.push(`<p>${renderInline(paragraph.join('\n'), options).replace(/\n/g, '<br />')}</p>`)
  }

  return blocks.join('\n')
}

/** Render Markdown to safe HTML. */
export function renderMarkdown(source: string, options: MarkdownOptions = {}): string {
  if (!source || !source.trim()) return ''
  return renderBlocks(source, options)
}

/** Flatten QQ text-interaction tags for one-line previews. */
function plainQqTag(source: string, options: MarkdownOptions): string {
  return source
    .replace(
      /<qqbot-at-user\s+id\s*=\s*(?:"([^"]*)"|'([^']*)')\s*\/?>/g,
      (match, doubleQuoted: string, singleQuoted: string) => {
        const id = decodeTagValue(doubleQuoted || singleQuoted || '')
        if (!id) return match
        return `@${options.atUser?.(id)?.trim() || '成员'}`
      },
    )
    .replace(/<qqbot-at-everyone\s*\/?>/g, '@全体成员')
    .replace(/<qqbot-cmd-(?:enter|input)\b([^>]*)\/?>/g, (match, rawAttributes: string) => {
      const attrs = tagAttributes(rawAttributes || '')
      return decodeTagValue(attrs.show || '') || decodeTagValue(attrs.text || '') || match
    })
}

/** Flatten Markdown for one-line previews (conversation list, tooltips). */
export function markdownPlain(source: string, options: MarkdownOptions = {}): string {
  if (!source) return ''
  return plainQqTag(source, options)
    .replace(/\r\n?/g, '\n')
    .replace(/```[\w+-]*\n?([\s\S]*?)```/g, (_, body: string) => body.trim())
    .replace(/!\[([^\]]*)\]\([^)\s]+\)/g, (_, alt: string) => alt || '[图片]')
    .replace(/\[([^\]]+)\]\([^)\s]+\)/g, '$1')
    .replace(/`([^`]+)`/g, '$1')
    .replace(/^#{1,6}\s+/gm, '')
    .replace(/^>\s?/gm, '')
    .replace(/^\s*([-+*]|\d+[.)])\s+/gm, '')
    .replace(/[*_~]{1,3}([^*_~\n]+)[*_~]{1,3}/g, '$1')
    .replace(/\|/g, ' ')
    .replace(/\n{2,}/g, ' · ')
    .replace(/\n/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}
