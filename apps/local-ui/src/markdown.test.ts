import { beforeAll, describe, expect, it } from 'vitest';
import { initializeRenderer, renderMarkdown } from './markdown';
beforeAll(async () => {
  await initializeRenderer();
});
function dom(source: string) {
  const el = document.createElement('div');
  el.innerHTML = renderMarkdown(source);
  return el;
}
describe('untrusted message rendering', () => {
  it('renders Markdown, tables, quotes, lists, links and highlighted Rust', () => {
    const el = dom(
      '# 标题\n\n> 引用\n\n- 项目\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n[链接](https://example.com)\n\n```rust\nfn main() { println!("hello"); }\n```',
    );
    expect(el.querySelector('h1')?.textContent).toBe('标题');
    expect(el.querySelector('blockquote')).not.toBeNull();
    expect(el.querySelector('li')).not.toBeNull();
    expect(el.querySelector('table')).not.toBeNull();
    expect(el.querySelector('pre.shiki span[style]')).not.toBeNull();
    expect(el.querySelector('a')?.getAttribute('rel')).toBe('noopener noreferrer');
  });
  it('renders inline and block math without interpreting code as math', () => {
    const el = dom('$x^2$\n\n$$\nE=mc^2\n$$\n\n`$raw$`\n\n```text\n$$ untouched $$\n```');
    expect(el.querySelectorAll('.katex')).toHaveLength(2);
    expect(el.querySelector('.math-block math')).not.toBeNull();
    expect(el.querySelector('p code')?.textContent).toBe('$raw$');
    expect(el.querySelector('pre code')?.textContent).toBe('$$ untouched $$\n');
  });
  it('rejects raw HTML, unsafe links and resource-loading math', () => {
    const el = dom(
      '<script>alert(1)</script>\n<img src="https://tracker.example/x" onerror="alert(1)">\n\n[x](javascript:alert(1))\n\n$\\href{javascript:alert(1)}{x}$\n\n$\\includegraphics{https://tracker.example/x}$\n\n![图](https://tracker.example/pixel)',
    );
    expect(el.querySelector('script,img,iframe,object')).toBeNull();
    expect(el.querySelector('[onerror],[onclick]')).toBeNull();
    expect(
      [...el.querySelectorAll('[href]')].every(
        (n) => !/javascript:|data:/i.test(n.getAttribute('href')!),
      ),
    ).toBe(true);
    expect(el.textContent).toContain('<script>');
    expect(el.textContent).toContain('[图片：图]');
  });
  it('escapes unsupported code and preserves source text', () => {
    const source = '```unknown-language\n<img onerror="evil()"> $x$\n```';
    const original = source;
    const el = dom(source);
    expect(el.querySelector('pre code')?.textContent).toBe('<img onerror="evil()"> $x$\n');
    expect(el.querySelector('img')).toBeNull();
    expect(source).toBe(original);
  });
  it('isolates macros between messages and tolerates invalid math', () => {
    dom('$\\gdef\\foo{secret}\\foo$');
    expect(dom('$\\foo$').textContent).not.toContain('secret');
    expect(() => dom('$\\invalid{$')).not.toThrow();
  });
});
