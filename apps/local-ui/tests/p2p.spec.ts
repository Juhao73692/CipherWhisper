import { test, expect, type Page } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { createServer } from 'node:net';
const root = resolve(import.meta.dirname, '../../..');
const binary = process.env.CIPHERWHISPER_TEST_BINARY || join(root, 'target/debug/cipherwhisper');
let launcher: ChildProcess, alice: string, bob: string, aToken: string, bToken: string;
async function port() {
  const server = createServer();
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  const address = server.address() as { port: number };
  await new Promise<void>((r) => server.close(() => r()));
  return address.port;
}
async function login(page: Page, base: string, token: string) {
  await page.goto(base);
  await page.getByLabel('本机管理令牌').fill(token);
  await page.getByRole('button', { name: '解锁本机工作区' }).click();
  await expect(page.locator('.workspace')).toBeVisible();
}
test.beforeAll(async () => {
  const a = await port(),
    b = await port(),
    ap = await port(),
    bp = await port();
  alice = `http://127.0.0.1:${a}`;
  bob = `http://127.0.0.1:${b}`;
  let output = '';
  launcher = spawn(
    binary,
    [
      'local-test',
      '--alice-port',
      String(a),
      '--bob-port',
      String(b),
      '--alice-peer-port',
      String(ap),
      '--bob-peer-port',
      String(bp),
    ],
    { stdio: ['ignore', 'pipe', 'pipe'] },
  );
  launcher.stdout!.on('data', (chunk) => {
    output += chunk.toString();
  });
  launcher.stderr!.on('data', (chunk) => {
    output += chunk.toString();
  });
  let dir = '';
  for (let i = 0; i < 200; i++) {
    const match = output.match(/Temporary test data: (.+?)\. Ctrl-C/);
    if (match) {
      dir = match[1];
      break;
    }
    if (launcher.exitCode !== null) throw new Error(output);
    await new Promise((r) => setTimeout(r, 100));
  }
  if (!dir) throw new Error(`launcher not ready: ${output}`);
  expect(output).toContain('no relay');
  aToken = (await readFile(join(dir, 'Alice/admin.token'), 'utf8')).trim();
  bToken = (await readFile(join(dir, 'Bob/admin.token'), 'utf8')).trim();
});
test.afterAll(async () => {
  if (!launcher || launcher.exitCode !== null) return;
  const exited = new Promise((r) => launcher.once('exit', r));
  launcher.kill('SIGINT');
  await exited;
});
test('two-instance launcher, signed profile export/import, mutual connection and real E2EE UI chat', async ({
  browser,
}) => {
  const ca = await browser.newContext(),
    cb = await browser.newContext();
  const a = await ca.newPage(),
    b = await cb.newPage();
  const errors: string[] = [],
    external: string[] = [];
  for (const page of [a, b]) {
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('request', (r) => {
      if (![alice, bob].some((origin) => r.url().startsWith(origin))) external.push(r.url());
    });
  }
  await login(a, alice, aToken);
  await login(b, bob, bToken);
  await a.locator('.self-card').click();
  const download = a.waitForEvent('download');
  await a.getByRole('button', { name: '下载身份卡' }).click();
  const file = await download;
  expect(file.suggestedFilename()).toBe('Alice.peer.json');
  const profile = JSON.parse(await readFile((await file.path())!, 'utf8'));
  expect(profile.endpoint).toContain('http://127.0.0.1:');
  await a.getByRole('button', { name: '关闭窗口' }).click();
  await b.getByRole('button', { name: '添加联系人', exact: true }).click();
  await b.getByLabel('或粘贴公开身份卡 JSON').fill(JSON.stringify(profile));
  await b.getByRole('button', { name: '验证并添加联系人' }).click();
  await a.getByRole('button', { name: /本机 P2P 测试/ }).click();
  const body = '# 直接 P2P\n\n$x^2$\n\n```rust\nfn main() {}\n```';
  await a.getByLabel('消息正文').fill(body);
  await a.getByRole('button', { name: '发送 ↑', exact: true }).click();
  await expect(b.getByRole('button', { name: /本机 P2P 测试/ })).toBeVisible();
  await b.getByRole('button', { name: /本机 P2P 测试/ }).click();
  await expect(b.locator('.message')).toHaveCount(1);
  await expect(b.locator('.message .katex')).toHaveCount(1);
  await expect(b.locator('.message pre.shiki')).toBeVisible();
  await b.getByRole('button', { name: '查看原文' }).click();
  await expect(b.locator('.source-text')).toHaveText(body);
  await b.getByRole('button', { name: '↳ 回复', exact: true }).click();
  await b.getByLabel('消息正文').fill('已经直接连接，双向发送成功。');
  await b.getByLabel('消息正文').press('Control+Enter');
  await expect(a.locator('.message')).toHaveCount(2);
  await expect(a.locator('.reply-quote')).toBeVisible();
  await a.getByRole('button', { name: 'Bob', exact: true }).click();
  const checked = a.waitForResponse((r) => r.url().endsWith('/check'));
  await a.getByRole('button', { name: '测试 P2P 连接' }).click();
  expect((await checked).status()).toBe(200);
  await a.getByRole('button', { name: '关闭窗口' }).click();
  await b.screenshot({ path: join(root, 'artifacts/p2p-chat.png'), fullPage: true });
  expect(errors).toEqual([]);
  expect(external).toEqual([]);
  await ca.close();
  await cb.close();
});
