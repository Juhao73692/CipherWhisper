import { test, expect, type Page } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtemp, readFile, rm, mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { createServer } from 'node:net';
import { request as httpRequest } from 'node:http';
const root = resolve(import.meta.dirname, '../../..');
const binary = process.env.CIPHERWHISPER_TEST_BINARY || join(root, 'target/debug/cipherwhisper');
let dir: string,
  alice: string,
  bob: string,
  relay: string,
  aToken: string,
  bToken: string,
  bootstrapUrl: string;
const processes: ChildProcess[] = [];
async function port() {
  const server = createServer();
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  const address = server.address() as { port: number };
  await new Promise<void>((r) => server.close(() => r()));
  return address.port;
}
function launch(...args: string[]) {
  const p = spawn(binary, args, {
    env: {
      ...process.env,
      CIPHERWHISPER_PASSPHRASE: 'browser-integration-test-passphrase',
      PATH: `${join(dir, 'bin')}:${process.env.PATH}`,
      CIPHERWHISPER_OPEN_CAPTURE: join(dir, 'open-url'),
    },
    stdio: 'ignore',
  });
  processes.push(p);
}
async function request<T = any>(
  base: string,
  path: string,
  token?: string,
  body?: unknown,
): Promise<T> {
  const r = await fetch(base + path, {
    headers: {
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
    },
    ...(body === undefined ? {} : { method: 'POST', body: JSON.stringify(body) }),
  });
  if (!r.ok) throw new Error(`${path}: ${r.status}`);
  return r.json() as Promise<T>;
}
async function retry<T>(fn: () => Promise<T>): Promise<T> {
  let last;
  for (let i = 0; i < 100; i++) {
    try {
      return await fn();
    } catch (e) {
      last = e;
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  throw last;
}
async function login(page: Page, base: string, token: string) {
  await page.goto(base);
  await page.getByLabel('本机管理令牌').fill(token);
  await page.getByRole('button', { name: '解锁本机工作区' }).click();
  await expect(page.locator('.workspace')).toBeVisible();
}
test.beforeAll(async () => {
  dir = await mkdtemp(join(tmpdir(), 'cipherwhisper-browser-'));
  relay = `http://127.0.0.1:${await port()}`;
  alice = `http://127.0.0.1:${await port()}`;
  bob = `http://127.0.0.1:${await port()}`;
  await mkdir(join(dir, 'bin'));
  const opener = '#!/bin/sh\numask 077\nprintf \'%s\' \"$1\" > \"$CIPHERWHISPER_OPEN_CAPTURE\"\n';
  for (const name of ['open', 'xdg-open'])
    await writeFile(join(dir, 'bin', name), opener, { mode: 0o700 });
  launch('relay', '--bind', new URL(relay).host, '--database', join(dir, 'relay.sqlite'));
  await retry(() => request(relay, '/health'));
  launch(
    'serve',
    '--open',
    '--name',
    'Alice',
    '--data',
    join(dir, 'alice'),
    '--relay',
    relay,
    '--bind',
    new URL(alice).host,
    '--sync-seconds',
    '1',
  );
  launch(
    'serve',
    '--name',
    'Bob',
    '--data',
    join(dir, 'bob'),
    '--relay',
    relay,
    '--bind',
    new URL(bob).host,
    '--sync-seconds',
    '1',
  );
  aToken = (await retry(() => readFile(join(dir, 'alice/admin.token'), 'utf8'))).trim();
  bToken = (await retry(() => readFile(join(dir, 'bob/admin.token'), 'utf8'))).trim();
  await retry(() => request(alice, '/identity', aToken));
  await retry(() => request(bob, '/identity', bToken));
  bootstrapUrl = await retry(() => readFile(join(dir, 'open-url'), 'utf8'));
  await request(alice, '/sync', aToken, {});
  await request(bob, '/sync', bToken, {});
});
test.afterAll(async () => {
  for (const p of processes) p.kill('SIGINT');
  await Promise.all(
    processes.map((p) =>
      p.exitCode === null
        ? new Promise((r) => {
            p.once('exit', r);
            setTimeout(() => {
              p.kill('SIGKILL');
              r(null);
            }, 5000).unref();
          })
        : Promise.resolve(),
    ),
  );
  if (dir) await rm(dir, { recursive: true, force: true });
});
test('real two-endpoint chat, rendering, reply, search, topic isolation and lock', async ({
  browser,
}) => {
  const ca = await browser.newContext(),
    cb = await browser.newContext();
  const a = await ca.newPage(),
    b = await cb.newPage();
  const exceptions: string[] = [],
    external: string[] = [];
  for (const page of [a, b]) {
    page.on('pageerror', (e) => exceptions.push(e.message));
    page.on('request', (r) => {
      if (!r.url().startsWith(alice) && !r.url().startsWith(bob)) external.push(r.url());
    });
  }
  const aliceCard = await request(alice, '/identity', aToken),
    bobCard = await request(bob, '/identity', bToken);
  await a.goto(bootstrapUrl);
  await expect(a.locator('.workspace')).toBeVisible();
  expect(new URL(a.url()).hash).toBe('');
  const usedCode = new URLSearchParams(new URL(bootstrapUrl).hash.slice(1)).get('bootstrap');
  expect(bootstrapUrl).not.toContain(aToken);
  expect(
    (
      await fetch(alice + '/ui/session', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ code: usedCode }),
      })
    ).status,
  ).toBe(401);
  await a.getByRole('button', { name: '添加联系人', exact: true }).click();
  await a.getByLabel('或粘贴公开身份卡 JSON').fill(JSON.stringify(bobCard));
  await a.getByRole('button', { name: '验证并添加联系人' }).click();
  await login(b, bob, bToken);
  await b.getByRole('button', { name: '添加联系人', exact: true }).click();
  await b.getByLabel('或粘贴公开身份卡 JSON').fill(JSON.stringify(aliceCard));
  await b.getByRole('button', { name: '验证并添加联系人' }).click();
  await a.getByRole('button', { name: '新建话题', exact: true }).click();
  await a.getByLabel('话题标题').fill('数学研究');
  await a.getByRole('button', { name: '创建话题', exact: true }).click();
  const source =
    '# 相对论笔记\n\n行内 $x^2$ 与公式：\n\n$$\nE=mc^2\n$$\n\n> 思考与证明\n\n| 项目 | 状态 |\n|---|---|\n| 能量 | 已确认 |\n\n```rust\nfn main() { println!("hello"); }\n```\n\n[文档](https://example.com)\n\n![远程图](https://tracker.invalid/pixel)\n\n<script>window.pwned=true</script>';
  await a.getByLabel('消息正文').fill(source);
  await a.getByRole('button', { name: '预览', exact: true }).click();
  await expect(a.locator('.draft-preview .katex')).toHaveCount(2);
  await a.getByRole('button', { name: '发送 ↑', exact: true }).click();
  await expect(a.locator('.message')).toHaveCount(1);
  await expect(b.getByRole('button', { name: /数学研究/ })).toBeVisible();
  await b.getByRole('button', { name: /数学研究/ }).click();
  await expect(b.locator('.message')).toHaveCount(1);
  await expect(b.locator('.message .katex')).toHaveCount(2);
  await expect(b.locator('.message pre.shiki span[style]').first()).toBeVisible();
  await expect(b.locator('.message table')).toBeVisible();
  await expect(b.locator('.message script,.message img')).toHaveCount(0);
  expect(await b.evaluate(() => 'pwned' in window)).toBe(false);
  await b.getByRole('button', { name: '查看原文' }).click();
  await expect(b.locator('.source-text')).toHaveText(source);
  await b.getByRole('button', { name: '查看渲染' }).click();
  await b.getByRole('button', { name: '↳ 回复', exact: true }).click();
  await b.getByLabel('消息正文').fill('收到，继续推导。');
  await b.getByLabel('消息正文').press('Control+Enter');
  await expect(a.locator('.message')).toHaveCount(2);
  await expect(a.locator('.reply-quote')).toBeVisible();
  await a.locator('.history').evaluate((el) => {
    (el as HTMLElement).style.scrollBehavior = 'auto';
    el.scrollTop = 0;
  });
  await a.screenshot({ path: join(root, 'artifacts/chat-desktop.png'), fullPage: true });
  await a.getByRole('button', { name: '新建话题', exact: true }).click();
  await a.getByLabel('话题标题').fill('NAS 维护');
  await a.getByRole('button', { name: '创建话题', exact: true }).click();
  await expect(a.locator('.message')).toHaveCount(0);
  await a.getByLabel('消息正文').fill('不要丢失这个草稿');
  await a.getByRole('button', { name: /数学研究/ }).click();
  await expect(a.locator('.message')).toHaveCount(2);
  await a.getByRole('button', { name: /NAS 维护/ }).click();
  await expect(a.getByLabel('消息正文')).toHaveValue('不要丢失这个草稿');
  await a.getByRole('button', { name: /搜索本机消息/ }).click();
  await a.getByRole('textbox', { name: '搜索关键词' }).fill('相对论');
  await a.getByRole('button', { name: '搜索', exact: true }).click();
  await a.locator('.search-results button').click();
  await expect(a.getByRole('heading', { name: '数学研究', exact: true })).toBeVisible();
  const rename = a.getByRole('button', { name: '重命名话题' });
  await a.getByLabel('消息正文').hover();
  await expect(rename).toHaveCSS('opacity', '0');
  await a.getByRole('heading', { name: '数学研究', exact: true }).hover();
  await expect(rename).toHaveCSS('opacity', '1');
  const titleBounds = await a.getByRole('heading', { name: '数学研究', exact: true }).boundingBox();
  const editBounds = await rename.boundingBox();
  expect(editBounds!.x + editBounds!.width).toBeLessThanOrEqual(titleBounds!.x);
  await rename.click();
  await a.getByLabel('话题标题').fill('数学与物理');
  await a.getByRole('button', { name: '保存标题' }).click();
  await expect(b.getByRole('heading', { name: '数学与物理', exact: true })).toBeVisible();
  await a.getByRole('button', { name: '归档话题', exact: true }).click();
  await expect(a.getByRole('dialog')).toContainText('归档“数学与物理”？');
  await a.getByRole('button', { name: '取消', exact: true }).click();
  await expect(a.getByLabel('消息正文')).toBeVisible();
  await expect(a.locator('.archived-banner')).toHaveCount(0);
  await a.getByRole('button', { name: '归档话题', exact: true }).click();
  await a.getByRole('button', { name: '确认归档', exact: true }).click();
  await expect(a.locator('.archived-banner')).toBeVisible();
  await a.getByRole('button', { name: '恢复话题', exact: true }).last().click();
  await expect(a.getByLabel('消息正文')).toBeVisible();
  await b.setViewportSize({ width: 390, height: 844 });
  await b.locator('.history').evaluate((el) => {
    (el as HTMLElement).style.scrollBehavior = 'auto';
    el.scrollTop = 0;
  });
  await b.screenshot({ path: join(root, 'artifacts/chat-mobile.png'), fullPage: true });
  expect(await b.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(exceptions).toEqual([]);
  expect(external).toEqual([]);
  expect(await a.evaluate(() => Object.keys(localStorage))).toEqual([]);
  await a.locator('.self-card').click();
  await a.getByRole('button', { name: '锁定界面' }).click();
  await expect(a.getByLabel('本机管理令牌')).toBeVisible();
  await expect(a.locator('.message')).toHaveCount(0);
  await b.reload();
  await expect(b.getByLabel('本机管理令牌')).toBeVisible();
  await ca.close();
  await cb.close();
});
test('assets public, management authenticated and hostile origins rejected', async () => {
  const html = await fetch(alice);
  expect(html.status).toBe(200);
  expect(html.headers.get('content-security-policy')).toContain("script-src 'self'");
  expect(await html.text()).not.toContain(aToken);
  expect((await fetch(alice + '/identity')).status).toBe(401);
  expect(
    (
      await fetch(alice + '/identity', {
        headers: { Authorization: `Bearer ${aToken}`, Origin: 'https://evil.invalid' },
      })
    ).status,
  ).toBe(403);
  const hostile = await new Promise<number>((resolve) => {
    const req = httpRequest(
      alice + '/identity',
      { headers: { Authorization: `Bearer ${aToken}`, Host: 'evil.invalid' } },
      (res) => {
        res.resume();
        resolve(res.statusCode!);
      },
    );
    req.end();
  });
  expect(hostile).toBe(403);
  expect(
    (
      await fetch(alice + '/ui/session', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ code: '0'.repeat(64) }),
      })
    ).status,
  ).toBe(401);
});
