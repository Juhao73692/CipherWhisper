import { test, expect, type Page } from '@playwright/test';
import { spawn, execFileSync, type ChildProcess } from 'node:child_process';
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { createServer } from 'node:net';

const root = resolve(import.meta.dirname, '../../..');
const binary = process.env.CIPHERWHISPER_TEST_BINARY || join(root, 'target/debug/cipherwhisper');
const password = 'ui-test-vault-passphrase';
type Instance = {
  process: ChildProcess;
  data: string;
  url: string;
  launchUrl: string;
  capture: string;
};
let directory: string;
const instances: Instance[] = [];

async function port() {
  const server = createServer();
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  const result = (server.address() as { port: number }).port;
  await new Promise<void>((r) => server.close(() => r()));
  return result;
}
async function start(name: string): Promise<Instance> {
  const data = join(directory, name),
    capture = join(directory, `${name}-url`);
  await rm(capture, { force: true });
  const process = spawn(binary, ['ui', '--data', data, '--bind', '127.0.0.1:0'], {
    env: {
      ...globalThis.process.env,
      PATH: `${join(directory, 'bin')}:${globalThis.process.env.PATH}`,
      CIPHERWHISPER_CAPTURE_URL: capture,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  process.stdout!.on('data', (chunk) => {
    output += chunk.toString();
  });
  process.stderr!.on('data', (chunk) => {
    output += chunk.toString();
  });
  for (let i = 0; i < 200; i++) {
    if (process.exitCode !== null) throw new Error(output);
    try {
      const launchUrl = (await readFile(capture, 'utf8')).trim();
      if (launchUrl) {
        const instance = { process, data, url: new URL(launchUrl).origin, launchUrl, capture };
        instances.push(instance);
        return instance;
      }
    } catch {
      /* Wait for the mock browser launcher to receive its one-time URL. */
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  process.kill('SIGINT');
  throw new Error(`UI launcher not ready: ${output}`);
}
async function stop(instance: Instance) {
  if (instance.process.exitCode !== null) return;
  const exited = new Promise((r) => instance.process.once('exit', r));
  instance.process.kill('SIGINT');
  await exited;
}
async function setupCenter(
  page: Page,
  instance: Instance,
  name: string,
  peerPort: number,
  devicePort?: number,
) {
  await page.goto(instance.launchUrl);
  await expect(page.getByRole('heading', { name: '创建你的工作区' })).toBeVisible();
  await page.getByLabel('你的名称', { exact: true }).fill(name);
  await page.getByLabel('工作区口令', { exact: true }).fill(password);
  await page.getByLabel('再次输入口令').fill(password);
  await page.getByLabel('这台电脑的连接地址').fill('localhost');
  await page.getByLabel('聊天连接端口').fill(String(peerPort));
  if (devicePort) {
    await page.getByLabel('允许自己的其他设备连接').check();
    await page.getByLabel('设备连接端口').fill(String(devicePort));
  }
  await page.getByRole('button', { name: '创建并继续' }).click();
  await expect(page.locator('.workspace')).toBeVisible();
}
async function identity(page: Page) {
  await page.locator('.self-card').click();
  const download = page.waitForEvent('download');
  await page.getByRole('button', { name: '下载身份卡' }).click();
  const file = await download;
  const card = JSON.parse(await readFile((await file.path())!, 'utf8'));
  await page.getByRole('button', { name: '关闭窗口' }).click();
  return card;
}
async function add(page: Page, card: unknown) {
  await page.getByRole('button', { name: '添加联系人', exact: true }).click();
  await page.getByLabel('或粘贴公开身份卡 JSON').fill(JSON.stringify(card));
  await page.getByRole('button', { name: '验证并添加联系人' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
}
async function passwordLogin(page: Page, url: string) {
  await page.goto(url);
  const about = page.locator('.unlock-brand').getByRole('link', { name: 'About', exact: true });
  await page.getByLabel('工作区口令', { exact: true }).hover();
  await expect(about).toHaveCSS('opacity', '0');
  await page.locator('.unlock-brand .brand-title').hover();
  await expect(about).toHaveCSS('opacity', '1');
  const titleBounds = await page.locator('.unlock-brand .brand-title').boundingBox();
  const aboutBounds = await about.boundingBox();
  expect(aboutBounds!.x).toBeGreaterThanOrEqual(titleBounds!.x + titleBounds!.width);
  await about.focus();
  await expect(about).toHaveCSS('opacity', '1');
  await page.screenshot({ path: join(root, 'artifacts/ui-password-about.png'), fullPage: true });
  await page.getByLabel('工作区口令', { exact: true }).fill(password);
  await page.getByRole('button', { name: '解锁本机工作区' }).click();
  await expect(page.locator('.workspace')).toBeVisible();
}
test.beforeAll(async () => {
  directory = await mkdtemp(join(tmpdir(), 'cipherwhisper-ui-'));
  await mkdir(join(directory, 'bin'));
  // Intercept opening the browser, while exercising the real one-time bootstrap flow.
  const executable = globalThis.process.platform === 'darwin' ? 'open' : 'xdg-open';
  await writeFile(
    join(directory, 'bin', executable),
    '#!/bin/sh\nprintf "%s" "$1" > "$CIPHERWHISPER_CAPTURE_URL"\n',
  );
  await chmod(join(directory, 'bin', executable), 0o700);
});
test.afterAll(async () => {
  for (const instance of instances) await stop(instance);
  await rm(directory, { force: true, recursive: true });
});

test('UI creates TLS centers, imports peers, chats, rolls back a bad port, renews certificates and resumes history', async ({
  browser,
}) => {
  test.setTimeout(120000);
  const alice = await start('alice'),
    bob = await start('bob');
  const ca = await browser.newContext(),
    cb = await browser.newContext();
  const a = await ca.newPage(),
    b = await cb.newPage();
  const errors: string[] = [],
    external: string[] = [];
  for (const page of [a, b]) {
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('request', (request) => {
      if (![alice.url, bob.url].some((origin) => request.url().startsWith(origin)))
        external.push(request.url());
    });
  }
  const ap = await port(),
    bp = await port();
  await a.goto(alice.launchUrl);
  await expect(a.getByRole('heading', { name: '创建你的工作区' })).toBeVisible();
  const build = await (await fetch(`${alice.url}/ui/build`)).json();
  expect(build.number).toMatch(/^\d{3,}$/);
  expect(build.commit).toMatch(/^[a-f0-9]{40}$/);
  expect(typeof build.dirty).toBe('boolean');
  const commitCount = Number(
    execFileSync('git', ['rev-list', '--count', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim(),
  );
  expect(build.number).toBe(String(commitCount + Number(build.dirty)).padStart(3, '0'));
  expect(Number.isFinite(Date.parse(build.builtAt))).toBe(true);
  await expect(a.locator('.unlock-brand .build-short')).toHaveText(`build ${build.number}`);
  await a.screenshot({ path: join(root, 'artifacts/ui-setup.png'), fullPage: true });
  // Reopen with the captured, already exchanged URL must not consume a second bootstrap.
  await a.getByLabel('你的名称', { exact: true }).fill('Alice');
  await a.getByLabel('工作区口令', { exact: true }).fill(password);
  await a.getByLabel('再次输入口令').fill('different-password');
  await a.getByRole('button', { name: '创建并继续' }).click();
  await expect(a.getByRole('alert')).toContainText('不一致');
  await a.getByLabel('再次输入口令').fill(password);
  await a.getByLabel('这台电脑的连接地址').fill('localhost');
  await a.getByLabel('聊天连接端口').fill(String(ap));
  await a.getByRole('button', { name: '创建并继续' }).click();
  await expect(a.locator('.workspace')).toBeVisible();
  await expect(a.locator('.brand .build-short')).toHaveCount(build.debug ? 1 : 0);
  if (build.debug)
    await expect(a.locator('.brand .build-short')).toHaveText(`build ${build.number}`);
  const controls = await a.locator('.sidebar-system > *').allTextContents();
  expect(controls[0]).toContain('本机为可信域中心');
  expect(controls[1]).toContain('连接与证书设置');
  expect(controls[2]).toContain('域内设备管理');
  await expect(a.getByRole('button', { name: '用户设置', exact: true })).toBeVisible();
  await expect(a.locator('.session-actions button')).toHaveText(['退出', '关闭']);
  await setupCenter(b, bob, 'Bob', bp);
  const acard = await identity(a),
    bcard = await identity(b);
  expect(acard.endpoint).toBe(`https://localhost:${ap}`);
  expect(acard.ca_pem).toContain('BEGIN CERTIFICATE');
  await add(a, bcard);
  await add(b, acard);
  await a.getByRole('button', { name: 'Bob', exact: true }).click();
  await a.getByRole('button', { name: '测试 P2P 连接' }).click();
  await expect(a.getByRole('dialog').getByRole('status')).toContainText('成功');
  await a.getByRole('button', { name: '关闭窗口' }).click();
  await a.getByRole('button', { name: '新建话题', exact: true }).click();
  await a.getByLabel('话题标题').fill('UI 配置后的加密聊天');
  await a.getByRole('button', { name: '创建话题', exact: true }).click();
  await a.getByLabel('消息正文').fill('自动证书配置成功。');
  await a.getByRole('button', { name: '发送 ↑', exact: true }).click();
  await b.getByRole('button', { name: /UI 配置后的加密聊天/ }).click();
  await expect(b.locator('.message')).toContainText('自动证书配置成功。');
  await expect(a.locator('.domain-note')).toContainText('本机为可信域中心');
  await expect(a.locator('.sync-box')).toContainText('就绪');
  for (const text of [
    '本机历史已持久化',
    'MARKDOWN · LaTeX · CODE',
    '独立话题 · 消息以 Markdown 源文传输',
    '自动刷新 · 每 5 秒',
  ]) {
    await expect(a.getByText(text, { exact: true })).toHaveCount(0);
  }
  await expect(a.getByLabel('消息正文')).toHaveAttribute('placeholder', '写下你的想法…');
  await expect(a.locator('.sidebar-footer').getByText('About', { exact: true })).toHaveCount(0);
  const about = a.locator('.brand').getByRole('link', { name: 'About', exact: true });
  await a.getByLabel('消息正文').hover();
  await expect(about).toHaveCSS('opacity', '0');
  await a.locator('.brand-title').hover();
  await expect(about).toHaveCSS('opacity', '1');
  const logoBounds = await a.locator('.brand-title').boundingBox();
  const aboutBounds = await about.boundingBox();
  const sidebarBounds = await a.locator('.peer-panel').boundingBox();
  expect(aboutBounds!.x).toBeGreaterThanOrEqual(logoBounds!.x + logoBounds!.width);
  expect(aboutBounds!.x + aboutBounds!.width).toBeLessThanOrEqual(
    sidebarBounds!.x + sidebarBounds!.width,
  );
  await about.focus();
  await expect(about).toHaveCSS('opacity', '1');
  await a.screenshot({ path: join(root, 'artifacts/ui-logo-about.png'), fullPage: true });
  await about.click();
  await expect(a.getByRole('heading', { name: 'About', exact: true })).toBeVisible();
  await expect(a.locator('.about-page')).toContainText('本机为可信域中心');
  await expect(a.locator('.about-page')).toContainText('SQLite');
  await expect(a.locator('.about-project')).toContainText('Cipher + Whisper');
  await expect(a.locator('.about-project')).toContainText('项目目前处于开发阶段');
  await expect(a.locator('.build-full')).toContainText(
    `build ${build.number} (${build.commit}${build.dirty ? '-dirty' : ''}) at `,
  );
  await a.screenshot({ path: join(root, 'artifacts/ui-about.png'), fullPage: true });
  await a.getByRole('button', { name: '返回', exact: false }).click();
  await expect(a.locator('.message')).toContainText('自动证书配置成功。');
  await a.screenshot({ path: join(root, 'artifacts/ui-layout.png'), fullPage: true });
  const saved = JSON.parse(await readFile(join(alice.data, 'ui-config.json'), 'utf8'));
  expect(JSON.stringify(saved)).not.toContain(password);
  expect(
    (await stat(join(alice.data, 'tls', saved.certificate, 'server-key.pem'))).mode & 0o777,
  ).toBe(0o600);
  await a.getByRole('button', { name: '连接与证书设置' }).click();
  await a.getByLabel('聊天连接端口').fill(String(bp));
  await a.getByRole('button', { name: '保存并应用设置' }).click();
  await expect(a.getByRole('dialog').getByRole('alert')).toContainText('已恢复原配置');
  expect(
    JSON.parse(await readFile(join(alice.data, 'ui-config.json'), 'utf8')).network.peerPort,
  ).toBe(ap);
  await a.getByLabel('聊天连接端口').fill(String(ap));
  await a.getByLabel('重新生成连接证书').check();
  await a.getByRole('button', { name: '保存并应用设置' }).click();
  await expect(a.getByRole('dialog').getByRole('status')).toContainText('设置已生效');
  await a.screenshot({ path: join(root, 'artifacts/ui-settings.png'), fullPage: true });
  const renewed = JSON.parse(await readFile(join(alice.data, 'ui-config.json'), 'utf8'));
  expect(renewed.certificate).not.toBe(saved.certificate);
  await a.getByRole('button', { name: '关闭窗口' }).click();
  const newerCard = await identity(a);
  expect(newerCard.identity.user_id).toBe(acard.identity.user_id);
  await add(b, newerCard);
  await stop(alice);
  const restarted = await start('alice');
  // Public bootstrap is not used here: password-only unlock works after a full restart.
  await passwordLogin(a, restarted.url);
  await a.getByRole('button', { name: /UI 配置后的加密聊天/ }).click();
  await expect(a.locator('.message')).toContainText('自动证书配置成功。');
  const finalCard = await identity(a);
  expect(finalCard.identity.user_id).toBe(acard.identity.user_id);
  await a.getByRole('button', { name: '关闭工作区', exact: true }).click();
  await expect(a.getByLabel('工作区口令', { exact: true })).toBeVisible();
  await passwordLogin(a, restarted.url);
  await a.getByRole('button', { name: '退出软件', exact: true }).click();
  await expect(a.getByRole('status')).toContainText('软件已退出');
  expect(errors).toEqual([]);
  expect(external.filter((url) => !url.startsWith(restarted.url))).toEqual([]);
  await ca.close();
  await cb.close();
});

test('paused message has a red resend button, stays paused across refresh and resends as a new message', async ({
  browser,
}) => {
  test.setTimeout(120000);
  const alice = await start('retry-alice'),
    bob = await start('retry-bob');
  const ca = await browser.newContext(),
    cb = await browser.newContext();
  const a = await ca.newPage(),
    b = await cb.newPage();
  await setupCenter(a, alice, 'Alice', await port());
  await setupCenter(b, bob, 'Bob', await port());
  await add(a, await identity(b));
  await add(b, await identity(a));
  await a.getByRole('button', { name: '新建话题', exact: true }).click();
  await a.getByLabel('话题标题').fill('周末计划');
  await a.getByRole('button', { name: '创建话题', exact: true }).click();
  await a.getByLabel('消息正文').fill('准备聊天');
  await a.getByRole('button', { name: '发送 ↑', exact: true }).click();
  await b.getByRole('button', { name: /周末计划/ }).click();
  await expect(b.locator('.message')).toContainText('准备聊天');
  await stop(bob);
  const body = '周六一起散步？';
  await a.getByLabel('消息正文').fill(body);
  await a.getByRole('button', { name: '发送 ↑', exact: true }).click();
  const oldBubble = a.locator('.message').filter({ hasText: body });
  const oldId = await oldBubble.getAttribute('id');
  // Fast-forward only the failure counter, then let the real worker perform failure 10.
  execFileSync('python3', [
    '-c',
    'import sqlite3,sys; db=sqlite3.connect(sys.argv[1]); db.execute("UPDATE outbox SET attempts=9,next_attempt=0 WHERE message_id=(SELECT id FROM messages WHERE body=?)", (sys.argv[2],)); db.commit()',
    join(alice.data, 'domain.sqlite'),
    body,
  ]);
  const resend = oldBubble.getByRole('button', { name: '重新发送消息', exact: true });
  await expect(resend).toBeVisible();
  await expect(resend).toHaveCSS('color', 'rgb(189, 52, 52)');
  await expect(oldBubble).toContainText('发送失败');
  await a.getByRole('button', { name: '立即同步', exact: true }).click();
  const attempts = () =>
    Number(
      execFileSync(
        'python3',
        [
          '-c',
          'import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute("SELECT attempts FROM outbox WHERE message_id=?", (sys.argv[2],)).fetchone()[0])',
          join(alice.data, 'domain.sqlite'),
          oldId!.slice('message-'.length),
        ],
        { encoding: 'utf8' },
      ).trim(),
    );
  expect(attempts()).toBe(10);
  await a.screenshot({ path: join(root, 'artifacts/chat-retry-paused.png'), fullPage: true });
  // Restart both endpoints: pause persists while the new message gets a fresh ID.
  await stop(alice);
  const restartedA = await start('retry-alice'),
    restartedB = await start('retry-bob');
  await passwordLogin(a, restartedA.url);
  await passwordLogin(b, restartedB.url);
  await a.getByRole('button', { name: /周末计划/ }).click();
  await expect(resend).toBeVisible();
  await resend.click();
  await expect(a.locator('.message').filter({ hasText: body })).toHaveCount(2);
  const ids = await a
    .locator('.message')
    .filter({ hasText: body })
    .evaluateAll((els) => els.map((el) => el.id));
  expect(ids[0]).toBe(oldId);
  expect(ids[1]).not.toBe(oldId);
  await b.getByRole('button', { name: /周末计划/ }).click();
  const received = b.locator('.message').filter({ hasText: body });
  await expect(received).toHaveCount(1);
  await expect(received).toHaveAttribute('id', ids[1]);
  expect(attempts()).toBe(10);
  await ca.close();
  await cb.close();
});

test('UI device identity, authorization, trusted pairing, independent history and revocation', async ({
  browser,
}) => {
  test.setTimeout(120000);
  const center = await start('center'),
    device = await start('device');
  const ca = await browser.newContext(),
    cd = await browser.newContext();
  const a = await ca.newPage(),
    d = await cd.newPage();
  await setupCenter(a, center, 'My center', await port(), await port());
  const card = await identity(a);
  await d.goto(device.launchUrl);
  await d.getByRole('radio', { name: /连接自己的中心/ }).check();
  await d.getByLabel('设备名称', { exact: true }).fill('Laptop');
  await d.getByLabel('工作区口令', { exact: true }).fill(password);
  await d.getByLabel('再次输入口令').fill(password);
  await d.getByRole('button', { name: '创建并继续' }).click();
  await expect(d.getByRole('button', { name: '下载设备身份卡' })).toBeVisible();
  const downloading = d.waitForEvent('download');
  await d.getByRole('button', { name: '下载设备身份卡' }).click();
  const deviceFile = await downloading;
  const deviceCard = await readFile((await deviceFile.path())!, 'utf8');
  await a.getByRole('button', { name: '域内设备管理' }).click();
  await a.getByLabel('设备公开身份卡 JSON').fill(deviceCard);
  const pairingDownload = a.waitForEvent('download');
  await a.getByRole('button', { name: '授权设备并下载配对文件' }).click();
  const pairing = await pairingDownload;
  await d.getByLabel('或粘贴配对文件内容').fill(await readFile((await pairing.path())!, 'utf8'));
  await d.getByLabel('已核对的完整中心身份指纹').fill('td_wrong');
  await d.getByRole('button', { name: '验证配对并连接' }).click();
  await expect(d.getByRole('alert')).toContainText('指纹不匹配');
  await d.getByLabel('已核对的完整中心身份指纹').fill(card.identity.user_id);
  await d.getByRole('button', { name: '验证配对并连接' }).click();
  await expect(d.locator('.workspace')).toBeVisible();
  await d.getByRole('button', { name: '设备同步与队列' }).click();
  await expect(d.getByRole('dialog')).toContainText(`https://localhost:`);
  await d.getByRole('button', { name: '关闭窗口' }).click();
  await d.reload();
  await passwordLogin(d, device.url);
  await expect(d.locator('.self-card')).toContainText('My center');
  const revoke = a.getByRole('button', { name: '撤销设备', exact: true });
  await a.getByRole('heading', { name: '域内设备管理', exact: true }).hover();
  await expect(revoke).toHaveCSS('opacity', '0');
  await a.locator('.device-record').hover();
  await expect(revoke).toHaveCSS('opacity', '1');
  await revoke.click();
  await expect(a.getByRole('dialog')).toContainText('已撤销');
  await ca.close();
  await cd.close();
});

test('launcher protects settings, bootstrap replay and same-origin requests', async ({
  request,
}) => {
  const instance = await start('guards');
  expect((await request.get(`${instance.url}/launcher`)).status()).toBe(401);
  expect((await request.post(`${instance.url}/launcher/network`, { data: {} })).status()).toBe(401);
  expect(
    (
      await request.post(`${instance.url}/ui/unlock`, {
        headers: { Origin: 'http://evil.example' },
        data: { passphrase: password },
      })
    ).status(),
  ).toBe(403);
  expect(
    (
      await request.get(`${instance.url}/ui/launcher`, { headers: { Host: 'evil.example' } })
    ).status(),
  ).toBe(403);
  const code = new URLSearchParams(new URL(instance.launchUrl).hash.slice(1)).get('bootstrap');
  expect((await request.post(`${instance.url}/ui/session`, { data: { code } })).status()).toBe(200);
  expect((await request.post(`${instance.url}/ui/session`, { data: { code } })).status()).toBe(401);
});
