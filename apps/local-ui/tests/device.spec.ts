import { test, expect, type Page } from '@playwright/test';
import { spawn, execFileSync, type ChildProcess } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { createServer } from 'node:net';

const root = resolve(import.meta.dirname, '../../..');
const binary = process.env.CIPHERWHISPER_TEST_BINARY || join(root, 'target/debug/cipherwhisper');
const env = Object.fromEntries(
  Object.entries({
    ...process.env,
    CIPHERWHISPER_PASSPHRASE: 'browser-device-integration-passphrase',
  }).filter(([key]) => !['http_proxy', 'https_proxy', 'all_proxy'].includes(key.toLowerCase())),
);
const processes: ChildProcess[] = [];
let dir: string,
  center: string,
  bob: string,
  client: string,
  relay: string,
  device: string,
  centerToken: string,
  bobToken: string,
  clientToken: string,
  card: any,
  topic: any,
  identity: any;
async function port() {
  const server = createServer();
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  const address = server.address() as { port: number };
  await new Promise<void>((r) => server.close(() => r()));
  return address.port;
}
function launch(...args: string[]) {
  const p = spawn(binary, args, { env, stdio: 'ignore' });
  processes.push(p);
}
async function request(base: string, path: string, token?: string, body?: unknown): Promise<any> {
  const response = await fetch(base + path, {
    headers: {
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
    },
    ...(body === undefined ? {} : { method: 'POST', body: JSON.stringify(body) }),
  });
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return response.json();
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
  dir = await mkdtemp(join(tmpdir(), 'cipherwhisper-device-browser-'));
  relay = `http://127.0.0.1:${await port()}`;
  center = `http://127.0.0.1:${await port()}`;
  bob = `http://127.0.0.1:${await port()}`;
  client = `http://127.0.0.1:${await port()}`;
  device = `https://127.0.0.1:${await port()}`;
  execFileSync(binary, ['tls-init', '--host', '127.0.0.1', '--out', join(dir, 'tls')], { env });
  launch('relay', '--bind', new URL(relay).host, '--database', join(dir, 'relay.sqlite'));
  await retry(() => request(relay, '/health'));
  launch(
    'serve',
    '--name',
    'Alice',
    '--data',
    join(dir, 'center'),
    '--relay',
    relay,
    '--bind',
    new URL(center).host,
    '--sync-seconds',
    '1',
    '--device-bind',
    new URL(device).host,
    '--device-tls-cert',
    join(dir, 'tls/server.pem'),
    '--device-tls-key',
    join(dir, 'tls/server-key.pem'),
    '--device-ca',
    join(dir, 'tls/ca.pem'),
    '--device-url',
    device,
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
  centerToken = (await retry(() => readFile(join(dir, 'center/admin.token'), 'utf8'))).trim();
  bobToken = (await retry(() => readFile(join(dir, 'bob/admin.token'), 'utf8'))).trim();
  identity = await retry(() => request(center, '/identity', centerToken));
  const peer = await retry(() => request(bob, '/identity', bobToken));
  await request(center, '/peers', centerToken, peer);
  await request(bob, '/peers', bobToken, identity);
  await request(bob, '/sync', bobToken, {});
  topic = await request(center, '/topics', centerToken, {
    peer_id: peer.user_id,
    title: '域内同步',
  });
  await request(center, `/topics/${topic.id}/messages`, centerToken, {
    body: '配对前的历史 $x^2$',
  });
  await retry(() => request(bob, `/topics/${topic.id}/messages`, bobToken));
  card = JSON.parse(
    execFileSync(binary, ['device-init', '--name', 'Alice Laptop', '--data', join(dir, 'client')], {
      env,
      encoding: 'utf8',
    }),
  );
});
test.afterAll(async () => {
  for (const p of processes) p.kill('SIGINT');
  await Promise.all(
    processes.map((p) =>
      p.exitCode !== null
        ? Promise.resolve()
        : new Promise((r) => {
            p.once('exit', r);
            setTimeout(() => {
              p.kill('SIGKILL');
              r(null);
            }, 5000).unref();
          }),
    ),
  );
  if (dir) await rm(dir, { recursive: true, force: true });
});
test('authorize in shared UI, download signed pairing, render client history, send and inspect rejected queue, revoke', async ({
  browser,
}) => {
  test.setTimeout(120000);
  const ca = await browser.newContext(),
    cb = await browser.newContext();
  const a = await ca.newPage(),
    c = await cb.newPage();
  const errors: string[] = [],
    external: string[] = [];
  for (const page of [a, c]) {
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('request', (r) => {
      if (![center, client].some((origin) => r.url().startsWith(origin))) external.push(r.url());
    });
  }
  await login(a, center, centerToken);
  await a.getByRole('button', { name: /域内设备管理/ }).click();
  await a.getByLabel('设备公开身份卡 JSON').fill(JSON.stringify(card));
  const download = a.waitForEvent('download');
  await a.getByRole('button', { name: '授权设备并下载配对文件' }).click();
  const path = join(dir, 'laptop.pair.json');
  await (await download).saveAs(path);
  expect(JSON.parse(await readFile(path, 'utf8')).device.id).toBe(card.id);
  await expect(a.locator('.device-records')).toContainText('Alice Laptop');
  launch(
    'connect',
    '--data',
    join(dir, 'client'),
    '--pairing',
    path,
    '--trust-domain',
    identity.user_id,
    '--bind',
    new URL(client).host,
    '--sync-seconds',
    '300',
  );
  clientToken = (await retry(() => readFile(join(dir, 'client/admin.token'), 'utf8'))).trim();
  await retry(() => request(client, '/status', clientToken));
  await request(client, '/sync', clientToken, {});
  await login(c, client, clientToken);
  await c.getByRole('button', { name: /域内同步/ }).click();
  await expect(c.locator('.message')).toHaveCount(1);
  await expect(c.locator('.message .katex')).toHaveCount(1);
  await c.getByLabel('消息正文').fill('从客户端发送 **Markdown**');
  await c.getByRole('button', { name: '发送 ↑', exact: true }).click();
  await expect(c.locator('.message')).toHaveCount(2);
  await c.getByRole('button', { name: '立即同步' }).click();
  await expect
    .poll(async () => (await request(bob, `/topics/${topic.id}/messages`, bobToken)).length)
    .toBe(2);
  await expect(c.locator('.message strong')).toContainText(['你', '你']);
  await c.getByRole('button', { name: /设备同步与队列/ }).click();
  await expect(c.getByRole('dialog', { name: '设备同步', exact: true })).toContainText(
    identity.user_id,
  );
  await expect(c.getByRole('heading', { name: '待处理请求 · 0' })).toBeVisible();
  await c.screenshot({ path: join(root, 'artifacts/device-sync.png'), fullPage: true });
  await c.getByRole('button', { name: '关闭窗口' }).click();
  // Center archives while this client's mirror is stale: it preserves the rejected text.
  await request(center, `/topics/${topic.id}`, centerToken, { title: '域内同步', archived: true });
  await c.getByLabel('消息正文').fill('被拒绝也不能丢失这段原文');
  await c.getByRole('button', { name: '发送 ↑', exact: true }).click();
  await c.getByRole('button', { name: '立即同步' }).click();
  await c.getByRole('button', { name: /设备同步与队列/ }).click();
  await expect(c.locator('.device-records')).toContainText('已拒绝');
  await expect(c.locator('.device-records pre')).toHaveText('被拒绝也不能丢失这段原文');
  await expect(c.getByRole('button', { name: '删除失败记录' })).toBeVisible();
  await a.getByRole('button', { name: '撤销设备' }).click();
  await expect(a.locator('.device-records')).toContainText('已撤销');
  const report = await request(client, '/sync', clientToken, {});
  expect(report.errors.join(' ')).toContain('401');
  expect((await request(client, `/topics/${topic.id}/messages`, clientToken)).length).toBe(3);
  expect(errors).toEqual([]);
  expect(external).toEqual([]);
  await ca.close();
  await cb.close();
});
