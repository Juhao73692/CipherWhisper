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
  // A native window hidden to the tray must not mark incoming messages as read.
  await expect
    .poll(
      async () =>
        (
          await (
            await fetch(`${bob}/unread`, { headers: { Authorization: `Bearer ${bToken}` } })
          ).json()
        ).length,
    )
    .toBe(0);
  await b.evaluate(() => {
    window.__cipherwhisperActive = false;
    window.dispatchEvent(new Event('cipherwhisper:visibility'));
  });
  await a.getByLabel('消息正文').fill('窗口隐藏时仍收到消息，重新打开后再标为已读。');
  await a.getByRole('button', { name: '发送 ↑', exact: true }).click();
  await expect
    .poll(
      async () =>
        (
          await (
            await fetch(`${bob}/unread`, { headers: { Authorization: `Bearer ${bToken}` } })
          ).json()
        ).length,
    )
    .toBe(1);
  await b.waitForTimeout(1000);
  expect(
    (
      await (
        await fetch(`${bob}/unread`, { headers: { Authorization: `Bearer ${bToken}` } })
      ).json()
    ).length,
  ).toBe(1);
  await b.evaluate(() => {
    window.__cipherwhisperActive = true;
    window.dispatchEvent(new Event('cipherwhisper:visibility'));
  });
  await expect(b.locator('.message')).toHaveCount(3);
  await expect
    .poll(
      async () =>
        (
          await (
            await fetch(`${bob}/unread`, { headers: { Authorization: `Bearer ${bToken}` } })
          ).json()
        ).length,
    )
    .toBe(0);
  await b.screenshot({ path: join(root, 'artifacts/p2p-chat.png'), fullPage: true });
  expect(errors).toEqual([]);
  expect(external).toEqual([]);
  await ca.close();
  await cb.close();
});

test('paged history, durable drafts, unread divider, edits, consented files and inert control text', async ({
  browser,
}) => {
  test.setTimeout(150000);
  async function api(base: string, path: string, token: string, body?: unknown) {
    const r = await fetch(base + path, {
      headers: {
        Authorization: `Bearer ${token}`,
        ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      ...(body === undefined ? {} : { method: 'POST', body: JSON.stringify(body) }),
    });
    if (!r.ok) throw new Error(`${path}: ${r.status} ${await r.text()}`);
    return r.json();
  }
  const ac = await browser.newContext(),
    bc = await browser.newContext();
  const a = await ac.newPage(),
    b = await bc.newPage();
  const bobCard = await api(bob, '/identity', bToken);
  const topic = await api(alice, '/topics', aToken, {
    peer_id: bobCard.user_id,
    title: '聊天功能验收',
  });
  for (let i = 0; i < 110; i++)
    await api(alice, `/topics/${topic.id}/messages`, aToken, {
      body: `历史 ${i}\n\n${'测试分页与阅读位置。'.repeat(12)}`,
    });
  await expect
    .poll(async () => (await api(bob, `/topics/${topic.id}/messages`, bToken)).length)
    .toBe(110);
  await login(a, alice, aToken);
  await login(b, bob, bToken);
  await a.getByRole('button', { name: /聊天功能验收/ }).click();
  await expect(a.locator('.message')).toHaveCount(50);
  await a.getByRole('button', { name: '加载更早的消息' }).click();
  await expect(a.locator('.message')).toHaveCount(100);
  await a.getByRole('button', { name: '加载更早的消息' }).click();
  await expect(a.locator('.message')).toHaveCount(110);
  await b.getByRole('button', { name: /聊天功能验收/ }).click();
  await b.getByRole('button', { name: '跳到第一条未读消息' }).click();
  await expect(b.locator('.unread-divider')).toBeVisible();
  await b.getByRole('button', { name: '回到最新消息 ↓' }).click();
  await a.getByLabel('消息正文').fill('刷新后恢复的草稿');
  await expect(a.locator('.draft-count')).toContainText('草稿已保存');
  await login(a, alice, aToken);
  await a.getByRole('button', { name: /聊天功能验收/ }).click();
  await expect(a.getByLabel('消息正文')).toHaveValue('刷新后恢复的草稿');
  const original = (await api(alice, `/topics/${topic.id}/messages`, aToken)).at(-1);
  const own = a.locator(`#message-${original.id}`);
  await own.getByRole('button', { name: '编辑', exact: true }).click();
  await own.getByLabel('编辑消息正文').fill('修改后的正文');
  await own.getByRole('button', { name: '保存修改' }).click();
  await expect(b.locator(`#message-${original.id}`)).toContainText('修改后的正文');
  await own.getByRole('button', { name: '撤回', exact: true }).click();
  await own.getByRole('button', { name: '确认撤回' }).click();
  await expect(b.locator(`#message-${original.id}`)).toContainText('这条消息已撤回');
  expect((await api(bob, `/topics/${topic.id}/messages`, bToken)).length).toBe(110);
  await a.locator('.topic-options summary').click();
  await a.getByLabel('话题标签', { exact: true }).fill('工作, 验收');
  await a.getByLabel('话题状态', { exact: true }).selectOption('resolved');
  await expect(a.getByLabel('话题标签', { exact: true })).toHaveValue('工作, 验收');
  await a.getByRole('button', { name: '保存话题设置' }).click();
  await a.getByRole('button', { name: '置顶', exact: true }).click();
  await expect
    .poll(async () => (await api(bob, '/topics', bToken)).find((t: any) => t.id === topic.id))
    .toMatchObject({ pinned: true, tags: ['工作', '验收'], status: 'resolved' });
  await a.locator('.topic-options summary').click();
  await api(bob, `/topics/${topic.id}/special`, bToken, {
    version: 1,
    kind: 'topic.meta',
    data: { pinned: true, tags: ['交接'], status: 'active' },
  });
  await expect(a.locator('.topic-tags')).toContainText('交接');
  await a.getByRole('button', { name: '取消置顶', exact: true }).click();
  await expect
    .poll(async () => {
      const t = (await api(bob, '/topics', bToken)).find((t: any) => t.id === topic.id);
      return { pinned: !!t.pinned, tags: t.tags, status: t.status };
    })
    .toMatchObject({ pinned: false, tags: ['交接'], status: 'active' });
  const bytes = Buffer.from('文件内容需要同意后才会发送。'.repeat(16000));
  await a
    .getByLabel('发送文件', { exact: true })
    .setInputFiles({ name: '附件.txt', mimeType: 'text/plain', buffer: bytes });
  await expect(b.locator('.file-card')).toContainText('等待接收方确认');
  const offer = (await api(bob, `/topics/${topic.id}/page`, bToken)).items.find((m: any) => m.file);
  expect(offer.file.received).toBe(0);
  expect(
    (
      await fetch(`${bob}/files/${offer.id}/download`, {
        headers: { Authorization: `Bearer ${bToken}` },
      })
    ).status,
  ).toBe(400);
  const injectedAccept = await api(bob, `/topics/${topic.id}/messages`, bToken, {
    body:
      'cipherwhisper.special\n' +
      JSON.stringify({
        version: 1,
        kind: 'file.accept',
        data: { fileId: offer.file.fileId, offerId: offer.id },
      }),
  });
  await expect
    .poll(async () =>
      (await api(alice, `/topics/${topic.id}/messages`, aToken)).some(
        (m: any) => m.id === injectedAccept.id,
      ),
    )
    .toBe(true);
  const stillOffered = (await api(alice, `/topics/${topic.id}/page`, aToken)).items.find(
    (m: any) => m.id === offer.id,
  );
  expect(stillOffered.file.state).toBe('offered');
  expect(stillOffered.file.acceptId).toBeNull();
  await expect(b.locator('.file-card')).toContainText('等待接收方确认');
  await b.getByRole('button', { name: '接收文件', exact: true }).click();
  await expect(b.locator('.file-card')).toContainText('传输完成，校验通过');
  const downloaded = b.waitForEvent('download');
  await b.getByRole('button', { name: '下载文件', exact: true }).click();
  const file = await downloaded;
  expect(await readFile((await file.path())!)).toEqual(bytes);
  expect((await api(bob, `/topics/${topic.id}/messages`, bToken)).length).toBe(112);
  const raw =
    'cipherwhisper.special\n' +
    JSON.stringify({
      version: 99,
      kind: 'future.feature',
      data: { text: '<script>window.pwned=true</script>' },
    });
  const literal = await api(alice, `/topics/${topic.id}/messages`, aToken, { body: raw });
  const literalMessage = b.locator(`#message-${literal.id}`);
  await expect(literalMessage.locator('.markdown')).toContainText('future.feature');
  await expect(literalMessage.locator('.unknown-message')).toHaveCount(0);
  expect(await b.evaluate(() => (window as any).pwned)).toBeUndefined();
  await literalMessage.getByRole('button', { name: '查看原文' }).click();
  await expect(literalMessage.locator('.source-text')).toHaveText(raw);
  await b.locator('.history').hover();
  await b.mouse.wheel(0, -100000);
  await expect.poll(() => b.locator('.history').evaluate((el) => el.scrollTop)).toBe(0);
  const top = 0;
  await api(alice, `/topics/${topic.id}/messages`, aToken, { body: '滚动时到达的新消息' });
  await expect(b.getByRole('button', { name: /有 1 条新消息/ })).toBeVisible();
  expect(await b.locator('.history').evaluate((el) => el.scrollTop)).toBe(top);
  await b.getByRole('button', { name: /有 1 条新消息/ }).click();
  await expect(b.locator('.message').last()).toContainText('滚动时到达的新消息');
  await expect(b.locator('.message').last()).toBeInViewport();
  await b.screenshot({ path: join(root, 'artifacts/chat-features.png'), fullPage: true });
  await ac.close();
  await bc.close();
});
