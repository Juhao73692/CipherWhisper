import MarkdownIt from 'markdown-it';
import katex from 'katex';
import DOMPurify from 'dompurify';
import { createHighlighterCore, type HighlighterCore } from 'shiki/core';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';
import githubLight from 'shiki/themes/github-light.mjs';
import javascript from 'shiki/langs/javascript.mjs';
import typescript from 'shiki/langs/typescript.mjs';
import rust from 'shiki/langs/rust.mjs';
import python from 'shiki/langs/python.mjs';
import json from 'shiki/langs/json.mjs';
import bash from 'shiki/langs/bash.mjs';
import sql from 'shiki/langs/sql.mjs';
import html from 'shiki/langs/html.mjs';
import css from 'shiki/langs/css.mjs';
import yaml from 'shiki/langs/yaml.mjs';
import go from 'shiki/langs/go.mjs';
import cpp from 'shiki/langs/cpp.mjs';
let highlighter: HighlighterCore | undefined;
let pending: Promise<void> | undefined;
const cache = new Map<string, string>();
export function initializeRenderer(): Promise<void> {
  return (pending ??= createHighlighterCore({
    themes: [githubLight],
    langs: [javascript, typescript, rust, python, json, bash, sql, html, css, yaml, go, cpp],
    engine: createJavaScriptRegexEngine(),
  }).then((value) => {
    highlighter = value;
    cache.clear();
  }));
}
const md = new MarkdownIt({ html: false, linkify: true, typographer: false, breaks: true });
function math(source: string, displayMode: boolean) {
  return katex.renderToString(source, {
    displayMode,
    throwOnError: false,
    trust: false,
    strict: 'warn',
    maxExpand: 500,
    maxSize: 10,
    output: 'htmlAndMathml',
    macros: {},
  });
}
// Rules operate on Markdown tokens: code fences/inline code never pass through math parsing.
md.inline.ruler.after('escape', 'math_inline', (state, silent) => {
  const start = state.pos;
  if (
    state.src[start] !== '$' ||
    state.src[start + 1] === '$' ||
    /\s/.test(state.src[start + 1] || ' ')
  )
    return false;
  let end = start + 1;
  while ((end = state.src.indexOf('$', end)) !== -1) {
    let escapes = 0;
    for (let i = end - 1; state.src[i] === '\\'; i--) escapes++;
    if (!(escapes % 2)) break;
    end++;
  }
  if (end < 0 || /\s/.test(state.src[end - 1]) || /\d/.test(state.src[end + 1] || '')) return false;
  if (!silent) {
    const token = state.push('math_inline', 'math', 0);
    token.content = state.src.slice(start + 1, end);
  }
  state.pos = end + 1;
  return true;
});
md.block.ruler.before(
  'fence',
  'math_block',
  (state, startLine, endLine, silent) => {
    const start = state.bMarks[startLine] + state.tShift[startLine];
    if (
      state.sCount[startLine] - state.blkIndent >= 4 ||
      state.src.slice(start, start + 2) !== '$$'
    )
      return false;
    let line = startLine;
    let first = state.src.slice(start + 2, state.eMarks[line]);
    let content = '';
    let found = false;
    const closing = first.indexOf('$$');
    if (closing >= 0 && !first.slice(closing + 2).trim()) {
      content = first.slice(0, closing);
      found = true;
    } else {
      content = first;
      while (++line < endLine) {
        const text = state.src.slice(state.bMarks[line] + state.tShift[line], state.eMarks[line]);
        const end = text.indexOf('$$');
        if (end >= 0 && !text.slice(end + 2).trim()) {
          content += '\n' + text.slice(0, end);
          found = true;
          break;
        }
        content += '\n' + text;
      }
    }
    if (!found) return false;
    if (silent) return true;
    state.line = line + 1;
    const token = state.push('math_block', 'math', 0);
    token.block = true;
    token.content = content.trim();
    token.map = [startLine, line + 1];
    return true;
  },
  { alt: ['paragraph', 'reference', 'blockquote', 'list'] },
);
md.renderer.rules.math_inline = (tokens, i) => math(tokens[i].content, false);
md.renderer.rules.math_block = (tokens, i) =>
  `<div class="math-block">${math(tokens[i].content, true)}</div>\n`;
md.renderer.rules.fence = (tokens, i) => {
  const code = tokens[i].content;
  const language = tokens[i].info.trim().split(/\s+/)[0].toLowerCase();
  const aliases: Record<string, string> = {
    js: 'javascript',
    ts: 'typescript',
    py: 'python',
    sh: 'bash',
    shell: 'bash',
    yml: 'yaml',
    'c++': 'cpp',
  };
  const lang = aliases[language] || language;
  if (highlighter?.getLoadedLanguages().includes(lang)) {
    try {
      return highlighter.codeToHtml(code, { lang, theme: 'github-light' }) + '\n';
    } catch {
      /* Escaped fallback for unsupported grammar input. */
    }
  }
  return `<pre><code>${md.utils.escapeHtml(code)}</code></pre>\n`;
};
// Remote images would expose a reader's IP. Render their label without fetching the URL.
md.renderer.rules.image = (tokens, i) =>
  `<span class="image-placeholder">[图片：${md.utils.escapeHtml(tokens[i].content || '未加载')}]</span>`;
DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  if (node.tagName === 'A') {
    const href = node.getAttribute('href') || '';
    if (!/^(https?:\/\/|mailto:|#)/i.test(href)) node.removeAttribute('href');
    node.setAttribute('target', '_blank');
    node.setAttribute('rel', 'noopener noreferrer');
  }
});
export function renderMarkdown(source: string): string {
  if (cache.has(source)) return cache.get(source)!;
  const result = DOMPurify.sanitize(md.render(source), {
    USE_PROFILES: { html: true, svg: true, mathMl: true },
    FORBID_TAGS: [
      'img',
      'iframe',
      'object',
      'embed',
      'form',
      'input',
      'button',
      'style',
      'video',
      'audio',
    ],
    FORBID_ATTR: ['src', 'srcset', 'id', 'name'],
    ADD_ATTR: ['target'],
  });
  if (cache.size >= 128) cache.delete(cache.keys().next().value!);
  cache.set(source, result);
  return result;
}
