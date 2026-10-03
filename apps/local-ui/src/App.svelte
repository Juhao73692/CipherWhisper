<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Markdown from './Markdown.svelte';
  import { api, setToken } from './api';
  import type {
    Card,
    Topic,
    Message,
    Status,
    Outbox,
    Report,
    DeviceStatus,
    Pending,
    Pairing,
  } from './types';
  let unlocked = $state(false),
    busy = $state(false),
    sending = $state(false),
    refreshing = $state(false);
  let tokenInput = $state(''),
    error = $state(''),
    notice = $state('');
  let self = $state<Card | null>(null),
    peers = $state<Card[]>([]),
    topics = $state<Topic[]>([]),
    messages = $state<Message[]>([]);
  let peerId = $state(''),
    topicId = $state(''),
    draft = $state(''),
    reply = $state<Message | null>(null),
    preview = $state(false);
  let drafts: Record<string, { body: string; reply: Message | null }> = {};
  let status = $state<Status | null>(null),
    outbox = $state<Outbox[]>([]),
    connectionError = $state('');
  let modal = $state<
    'peer' | 'topic' | 'rename' | 'identity' | 'peerIdentity' | 'search' | 'devices' | null
  >(null);
  let cardText = $state(''),
    topicTitle = $state(''),
    query = $state(''),
    results = $state<Message[]>([]);
  let searchBusy = $state(false),
    includeArchived = $state(false),
    sourceIds = $state<Set<string>>(new Set());
  let deviceRecords = $state<DeviceStatus[]>([]),
    deviceEnabled = $state(false),
    deviceText = $state(''),
    pendingOps = $state<Pending[]>([]);
  let clientMode = $derived(status?.mode === 'client');
  let directMode = $derived(status?.transport === 'direct');
  let peerRoutes = $state<{ identity: Card; endpoint: string }[]>([]);
  let pane = $state<'peers' | 'topics' | 'conversation'>('peers');
  let historyElement = $state<HTMLDivElement>(),
    editor = $state<HTMLTextAreaElement>();
  let dialogElement = $state<HTMLDivElement>();
  let priorFocus: HTMLElement | null = null;
  $effect(() => {
    if (modal) {
      priorFocus = document.activeElement as HTMLElement;
      void tick().then(() =>
        dialogElement
          ?.querySelector<HTMLElement>('input:not([type=file]),textarea,button')
          ?.focus(),
      );
    } else if (priorFocus) {
      priorFocus.focus();
      priorFocus = null;
    }
  });
  function modalKeys(event: KeyboardEvent) {
    if (event.key === 'Escape' && !busy) {
      modal = null;
      return;
    }
    if (event.key !== 'Tab') return;
    const elements = [
      ...(dialogElement?.querySelectorAll<HTMLElement>(
        'button:not(:disabled),input:not(:disabled),textarea:not(:disabled),[tabindex="0"]',
      ) || []),
    ];
    const first = elements[0],
      last = elements.at(-1);
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first?.focus();
    }
  }
  let activePeer = $derived(peers.find((p) => p.user_id === peerId));
  let activeTopic = $derived(topics.find((t) => t.id === topicId));
  let visibleTopics = $derived(
    topics.filter((t) => t.peerId === peerId && (includeArchived || !t.archived)),
  );
  let draftsBytes = $derived(new TextEncoder().encode(draft).length);
  let issue = $derived(
    connectionError ||
      status?.lastSync?.errors[0] ||
      outbox.find((o) => o.lastError)?.lastError ||
      '',
  );
  const delivery: Record<string, string> = {
    queued: '等待发送',
    sent: '密文已接收',
    delivered: '对方已接收',
    received: '已接收',
    failed: '中心拒绝',
  };
  const date = (n: number) =>
    new Date(n * 1000).toLocaleString('zh-CN', {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  const short = (id: string) => id.slice(3, 11) + '…' + id.slice(-6);
  const fail = (e: unknown) => {
    error = e instanceof Error ? e.message : String(e);
  };
  function lock() {
    setToken('');
    unlocked = false;
    self = null;
    peers = [];
    topics = [];
    messages = [];
    outbox = [];
    status = null;
    peerId = '';
    topicId = '';
    draft = '';
    reply = null;
    drafts = {};
    results = [];
    cardText = '';
    query = '';
    modal = null;
    notice = '';
    connectionError = '';
    deviceRecords = [];
    peerRoutes = [];
    deviceEnabled = false;
    deviceText = '';
    pendingOps = [];
  }
  async function unlock(token: string) {
    if (busy) return;
    busy = true;
    error = '';
    setToken(token.trim());
    try {
      self = await api<Card>('/identity');
      unlocked = true;
      tokenInput = '';
      await refresh();
    } catch (e) {
      setToken('');
      fail(e);
    } finally {
      busy = false;
    }
  }
  async function refresh() {
    if (!unlocked || refreshing) return;
    refreshing = true;
    const chosen = topicId,
      bottom = historyElement
        ? historyElement.scrollHeight - historyElement.scrollTop - historyElement.clientHeight < 100
        : true;
    try {
      const [p, t, s, o] = await Promise.all([
        api<Card[]>('/peers'),
        api<Topic[]>('/topics'),
        api<Status>('/status'),
        api<Outbox[]>('/outbox'),
      ]);
      const routes =
        s.transport === 'direct'
          ? await api<{ identity: Card; endpoint: string }[]>('/p2p/peers')
          : [];
      if (!unlocked) return;
      peers = p;
      topics = t;
      status = s;
      peerRoutes = routes;
      outbox = o;
      if (modal === 'devices') await loadDevices();
      connectionError = '';
      if (!peerId && p.length) {
        peerId = p[0].user_id;
        pane = 'topics';
      }
      if (chosen) {
        const history = await api<Message[]>(`/topics/${encodeURIComponent(chosen)}/messages`);
        if (unlocked && topicId === chosen) {
          messages = history;
          if (bottom) await scrollBottom();
        }
      }
    } catch (e) {
      connectionError = e instanceof Error ? e.message : String(e);
    } finally {
      refreshing = false;
    }
  }
  function saveDraft() {
    if (topicId) drafts[topicId] = { body: draft, reply };
  }
  function selectPeer(id: string) {
    saveDraft();
    peerId = id;
    topicId = '';
    messages = [];
    draft = '';
    reply = null;
    pane = 'topics';
    error = '';
  }
  async function selectTopic(id: string) {
    saveDraft();
    topicId = id;
    const topic = topics.find((t) => t.id === id);
    if (topic) peerId = topic.peerId;
    draft = drafts[id]?.body || '';
    reply = drafts[id]?.reply || null;
    messages = [];
    preview = false;
    pane = 'conversation';
    error = '';
    try {
      const history = await api<Message[]>(`/topics/${encodeURIComponent(id)}/messages`);
      if (unlocked && topicId === id) {
        messages = history;
        await scrollBottom();
      }
    } catch (e) {
      fail(e);
    }
  }
  async function scrollBottom() {
    await tick();
    if (historyElement) historyElement.scrollTop = historyElement.scrollHeight;
  }
  async function send() {
    if (!draft.trim() || !activeTopic || sending || draftsBytes > 65536) return;
    const id = topicId,
      body = draft,
      replyTo = reply?.id;
    sending = true;
    error = '';
    try {
      const message = await api<Message>(`/topics/${encodeURIComponent(id)}/messages`, {
        body,
        ...(replyTo ? { reply_to: replyTo } : {}),
      });
      if (!unlocked) return;
      // Preserve text typed/switched while the request was in flight.
      if (topicId === id) {
        if (draft === body) {
          draft = '';
          reply = null;
        }
        if (!messages.some((m) => m.id === message.id)) messages = [...messages, message];
        preview = false;
        await scrollBottom();
      } else if (drafts[id]?.body === body) delete drafts[id];
      await refresh();
    } catch (e) {
      fail(e);
    } finally {
      sending = false;
    }
  }
  async function sync() {
    if (busy) return;
    busy = true;
    error = '';
    notice = '';
    try {
      const r = await api<Report>('/sync', {});
      if (unlocked) {
        status = { ...status, protocol: 1, lastSync: r };
        notice = r.errors.length ? '部分任务仍在队列中重试' : '同步完成';
        await refresh();
      }
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }
  function openModal(value: typeof modal) {
    modal = value;
    error = '';
    notice = '';
    if (value === 'peer') cardText = '';
    if (value === 'topic') topicTitle = '';
    if (value === 'rename') topicTitle = activeTopic?.title || '';
    if (value === 'devices') {
      deviceText = '';
      void loadDevices();
    }
  }
  async function importPeer() {
    if (busy) return;
    busy = true;
    error = '';
    try {
      const imported = JSON.parse(cardText) as Card | { identity: Card };
      const card = 'identity' in imported ? imported.identity : imported;
      if ('identity' in imported) {
        if (!directMode) throw new Error('请在使用直接 P2P 的中心导入连接卡。');
        await api('/p2p/peers', imported);
      } else {
        if (directMode)
          throw new Error('请导入对方下载的 .peer.json 连接卡，其中需要包含 P2P 地址。');
        await api('/peers', card);
      }
      await refresh();
      selectPeer(card.user_id);
      modal = null;
      notice = '联系人已添加；请通过可信渠道核对完整身份指纹。';
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }
  async function readCard(event: Event) {
    const file = (event.target as HTMLInputElement).files?.[0];
    if (!file) return;
    if (file.size > 300 * 1024) {
      error = '身份卡文件过大';
      return;
    }
    cardText = await file.text();
  }
  async function createTopic() {
    if (busy || !topicTitle.trim()) return;
    busy = true;
    error = '';
    try {
      const t = await api<Topic>('/topics', { peer_id: peerId, title: topicTitle.trim() });
      topics = [t, ...topics];
      modal = null;
      await selectTopic(t.id);
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }
  async function updateTopic(archive: boolean) {
    if (!activeTopic || busy) return;
    busy = true;
    error = '';
    try {
      const updated = await api<Topic>(`/topics/${encodeURIComponent(activeTopic.id)}`, {
        title: modal === 'rename' ? topicTitle.trim() : activeTopic.title,
        archived: archive,
      });
      topics = topics.map((t) => (t.id === updated.id ? updated : t));
      modal = null;
      notice = archive ? '话题已归档' : '话题已更新';
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }
  async function search() {
    if (!query.trim() || searchBusy) return;
    searchBusy = true;
    error = '';
    try {
      const r = await api<Message[]>(`/search?q=${encodeURIComponent(query.trim())}`);
      if (unlocked && modal === 'search') results = r;
    } catch (e) {
      fail(e);
    } finally {
      searchBusy = false;
    }
  }
  async function goResult(message: Message) {
    modal = null;
    await selectTopic(message.topicId);
    await tick();
    document.getElementById(`message-${message.id}`)?.scrollIntoView({ block: 'center' });
  }
  function showSource(id: string) {
    const next = new Set(sourceIds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    sourceIds = next;
  }

  async function loadDevices() {
    try {
      if (clientMode) {
        pendingOps = await api<Pending[]>('/device-pending');
      } else {
        const result = await api<{ enabled: boolean; devices: DeviceStatus[] }>('/devices');
        deviceEnabled = result.enabled;
        deviceRecords = result.devices;
      }
    } catch (e) {
      fail(e);
    }
  }
  function exportJSON(value: unknown, name: string) {
    const url = URL.createObjectURL(
      new Blob([JSON.stringify(value, null, 2) + '\n'], { type: 'application/json' }),
    );
    const link = document.createElement('a');
    link.href = url;
    link.download = name;
    link.click();
    URL.revokeObjectURL(url);
  }
  async function enrollDevice() {
    if (busy) return;
    busy = true;
    error = '';
    try {
      const pairing = await api<Pairing>('/devices', JSON.parse(deviceText));
      if (unlocked) {
        exportJSON(pairing, `${pairing.device.id}.pair.json`);
        deviceText = '';
        notice = '设备已授权，配对文件已下载。请核对中心身份指纹，再在客户端导入。';
        await loadDevices();
      }
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }
  async function readDevice(event: Event) {
    const file = (event.target as HTMLInputElement).files?.[0];
    if (!file) return;
    if (file.size > 16384) {
      error = '设备身份卡文件过大';
      return;
    }
    deviceText = await file.text();
  }
  async function revokeDevice(id: string) {
    if (busy) return;
    busy = true;
    error = '';
    try {
      await api(`/devices/${encodeURIComponent(id)}/revoke`, {});
      await loadDevices();
      notice = '设备已撤销，将不再获得新数据；已有本地副本无法远程删除。';
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }
  async function discardOperation(id: string) {
    if (busy) return;
    busy = true;
    error = '';
    try {
      await api(`/device-pending/${encodeURIComponent(id)}/discard`, {});
      await loadDevices();
      await refresh();
      notice = '已删除失败请求。';
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }

  async function downloadCard() {
    if (!self) return;
    try {
      exportJSON(
        directMode ? await api('/p2p/contact') : self,
        `${self.label || 'identity'}.${directMode ? 'peer' : 'contact'}.json`,
      );
    } catch (e) {
      fail(e);
    }
  }
  async function copyCard() {
    try {
      await navigator.clipboard.writeText(
        JSON.stringify(directMode ? await api('/p2p/contact') : self, null, 2),
      );
      notice = '身份卡已复制';
    } catch {
      error = '浏览器未允许剪贴板访问，请使用下载身份卡。';
    }
  }
  async function checkPeer() {
    if (!activePeer || busy) return;
    busy = true;
    try {
      await api(`/p2p/peers/${encodeURIComponent(activePeer.user_id)}/check`, {});
      notice = '已验证对方身份，P2P 连接成功。';
    } catch (e) {
      fail(e);
    } finally {
      busy = false;
    }
  }
  async function replyTo(message: Message) {
    reply = message;
    await tick();
    editor?.focus();
  }
  onMount(() => {
    const locked = () => {
      lock();
      error = '本机授权已失效，请重新解锁。';
    };
    window.addEventListener('topicairn:locked', locked);
    const hash = new URLSearchParams(location.hash.slice(1));
    const code = hash.get('bootstrap');
    history.replaceState(null, '', location.pathname);
    if (code) {
      busy = true;
      fetch('/ui/session', {
        method: 'POST',
        credentials: 'omit',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ code }),
      })
        .then(async (r) => {
          if (!r.ok) throw new Error('自动解锁链接已过期或已使用，请输入本机管理令牌。');
          return r.json();
        })
        .then(async (r) => {
          busy = false;
          await unlock(r.token);
        })
        .catch((e) => {
          busy = false;
          fail(e);
        });
    }
    const timer = setInterval(() => {
      if (!document.hidden) void refresh();
    }, 5000);
    return () => {
      clearInterval(timer);
      window.removeEventListener('topicairn:locked', locked);
      setToken('');
    };
  });
</script>

{#if !unlocked}
  <main class="unlock-page">
    <div class="unlock-brand">
      <span class="brand-mark">◒</span> Topicairn <span class="eyebrow">PERSONAL TRUST DOMAIN</span>
    </div>
    <section class="unlock-card">
      <span class="eyebrow accent">LOCAL WORKSPACE</span>
      <h1>让对话，<br />有自己的话题。</h1>
      <p>
        连接你的{clientMode
          ? '本设备历史副本'
          : '本机可信域中心'}。使用本机工作区查看联系人、话题和消息历史。
      </p>
      <form
        onsubmit={(e) => {
          e.preventDefault();
          void unlock(tokenInput);
        }}
      >
        <label for="token">本机管理令牌</label><input
          id="token"
          type="password"
          bind:value={tokenInput}
          autocomplete="off"
          spellcheck="false"
          placeholder="粘贴数据目录中的 admin.token"
          required
          maxlength="64"
        />
        <button class="primary full" disabled={busy || tokenInput.trim().length !== 64}
          >{busy ? '正在连接…' : '解锁本机工作区 →'}</button
        >
      </form>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <p class="hint">
        使用 <code>topicairn serve … --open</code> 可自动打开并解锁。授权只保存在当前页面内存中。
      </p>
    </section>
    <div class="unlock-foot">
      你的身份。你的历史。你的可信域。 <a
        href="/third-party-ui.txt"
        target="_blank"
        rel="noopener noreferrer">开源许可</a
      >
    </div>
  </main>
{:else}
  <div
    class="workspace"
    inert={!!modal}
    class:mobile-peers={pane === 'peers'}
    class:mobile-topics={pane === 'topics'}
    class:mobile-conversation={pane === 'conversation'}
  >
    <aside class="peer-panel">
      <div class="brand">
        <span class="brand-mark">◒</span><span>Topicairn<small>PRIVATE BY DESIGN</small></span>
      </div>
      <button class="search-trigger" onclick={() => openModal('search')}
        ><span>⌕</span> 搜索本机消息 <kbd>FTS</kbd></button
      >
      <div class="section-label">
        联系人 <span>{peers.length}</span><button
          class="icon"
          aria-label="添加联系人"
          onclick={() => openModal('peer')}>＋</button
        >
      </div>
      <div class="peer-list">
        {#each peers as peer (peer.user_id)}
          <button
            class="peer-item"
            class:active={peerId === peer.user_id}
            onclick={() => selectPeer(peer.user_id)}
            ><span class="avatar">{(peer.label || '?').slice(0, 1).toUpperCase()}</span><span
              class="peer-copy"
              ><strong>{peer.label || '未命名联系人'}</strong><small>{short(peer.user_id)}</small
              ></span
            ><span class="peer-arrow">›</span></button
          >
        {/each}
        {#if !peers.length}<div class="empty-peers">
            <p>从一个可信的人开始。</p>
            <button class="text-button" onclick={() => openModal('peer')}>导入联系人身份卡 →</button
            >
          </div>{/if}
      </div>
      <button class="devices-trigger" onclick={() => openModal('devices')}
        >{clientMode ? '◇ 设备同步与队列' : '◇ 域内设备管理'}</button
      >
      <div class="domain-note">
        <span class="small-dot"></span>{clientMode ? '本设备历史副本' : '本机可信域中心'}
        <p>
          {clientMode ? '与中心通过加密连接同步' : '外部传输端到端加密'}<br />本机保存解密后的消息
        </p>
      </div>
      <button class="self-card" onclick={() => openModal('identity')}
        ><span class="avatar self">{(self?.label || '我').slice(0, 1)}</span><span
          ><strong>{self?.label || '我的身份'}</strong><small>身份卡与完整指纹</small></span
        ><span>⚙</span></button
      >
    </aside>
    <aside class="topic-panel">
      <div class="topic-heading">
        <button class="mobile-back icon" aria-label="返回联系人" onclick={() => (pane = 'peers')}
          >‹</button
        >
        <div>
          <span class="eyebrow">TOPICS</span>
          <h2>
            {#if activePeer}<button
                class="peer-title"
                title="查看联系人完整身份"
                onclick={() => openModal('peerIdentity')}>{activePeer.label}</button
              >{:else}话题{/if}
          </h2>
        </div>
        <button
          class="icon new-topic"
          aria-label="新建话题"
          disabled={!activePeer}
          onclick={() => openModal('topic')}>＋</button
        >
      </div>
      {#if activePeer}<p class="topic-subtitle">每一件事，一段独立对话。</p>{/if}
      <div class="topic-list">
        {#each visibleTopics as topic (topic.id)}
          <button
            class="topic-item"
            class:active={topicId === topic.id}
            onclick={() => selectTopic(topic.id)}
            ><span class="topic-symbol">{topic.archived ? '□' : '#'}</span><span
              ><strong>{topic.title}</strong><small
                >{topic.archived ? '已归档 · ' : ''}{date(topic.updatedAt)}</small
              ></span
            ></button
          >
        {/each}
        {#if !visibleTopics.length}<div class="empty-topics">
            <span>✧</span>
            <h3>{activePeer ? '开始一个新话题' : '选择一位联系人'}</h3>
            <p>{activePeer ? '数学、项目，或随便聊聊。' : '独立话题让每段对话各有位置。'}</p>
            {#if activePeer}<button class="secondary" onclick={() => openModal('topic')}
                >＋ 新建话题</button
              >{/if}
          </div>{/if}
      </div>
      <label class="archive-filter"
        ><input type="checkbox" bind:checked={includeArchived} /> 显示已归档话题</label
      >
      <div class="sync-box">
        <div>
          <span class="small-dot" class:warning={!!issue}></span><span
            >{issue ? '同步需要关注' : '自动刷新 · 每 5 秒'}</span
          ><button class="icon" aria-label="立即同步" disabled={busy} onclick={sync}>↻</button>
        </div>
        <small>{outbox.length ? `${outbox.length} 个投递任务` : '本机历史已持久化'}</small
        >{#if issue}<p class="sync-error">{issue}</p>{/if}
      </div>
    </aside>
    <main class="conversation-panel">
      {#if activeTopic}
        <header class="conversation-header">
          <button class="mobile-back icon" aria-label="返回话题" onclick={() => (pane = 'topics')}
            >‹</button
          >
          <div>
            <div class="eyebrow">
              {activePeer?.label} <span>/</span>
              {activeTopic.archived ? 'ARCHIVED' : 'CONVERSATION'}
            </div>
            <h1>{activeTopic.title}</h1>
          </div>
          <div class="header-actions">
            <span class="secure-badge">◇ E2EE</span><button
              class="icon"
              aria-label="重命名话题"
              onclick={() => openModal('rename')}>✎</button
            ><button
              class="icon"
              aria-label={activeTopic.archived ? '恢复话题' : '归档话题'}
              disabled={busy}
              onclick={() => updateTopic(!activeTopic!.archived)}
              >{activeTopic.archived ? '↥' : '□'}</button
            >
          </div>
        </header>
        <div class="history" bind:this={historyElement}>
          <div class="conversation-intro">
            <span class="intro-line"></span><span>独立话题 · 消息以 Markdown 源文传输</span><span
              class="intro-line"
            ></span>
          </div>
          {#if !messages.length}<div class="empty-conversation">
              <span>✧</span>
              <h2>给这个话题写下第一句。</h2>
              <p>支持公式、代码和那些值得展开的想法。</p>
            </div>{/if}
          {#each messages as message (message.id)}
            <article
              id={`message-${message.id}`}
              class="message"
              class:mine={message.senderId === self?.user_id}
            >
              <span class="avatar message-avatar" class:self={message.senderId === self?.user_id}
                >{(message.senderId === self?.user_id
                  ? self?.label || '我'
                  : activePeer?.label || '?'
                ).slice(0, 1)}</span
              >
              <div class="message-main">
                <div class="message-meta">
                  <strong>{message.senderId === self?.user_id ? '你' : activePeer?.label}</strong
                  ><time datetime={new Date(message.timestamp * 1000).toISOString()}
                    >{date(message.timestamp)}</time
                  ><span class="delivery" title="送达表示对方端点确认接收，不代表人已阅读"
                    >{delivery[message.delivery] || message.delivery}</span
                  >
                </div>
                {#if message.replyTo}<button
                    class="reply-quote"
                    onclick={() =>
                      document
                        .getElementById(`message-${message.replyTo}`)
                        ?.scrollIntoView({ block: 'center' })}
                    >↳ {messages.find((m) => m.id === message.replyTo)?.body.slice(0, 120) ||
                      '回复一条消息'}</button
                  >{/if}
                {#if sourceIds.has(message.id)}<pre
                    class="source-text">{message.body}</pre>{:else}<Markdown
                    source={message.body}
                  />{/if}
                <div class="message-actions">
                  <button onclick={() => replyTo(message)} disabled={activeTopic.archived}
                    >↳ 回复</button
                  ><button onclick={() => showSource(message.id)}
                    >{sourceIds.has(message.id) ? '查看渲染' : '查看原文'}</button
                  >
                </div>
              </div>
            </article>
          {/each}
        </div>
        <div class="composer-area">
          {#if error}<div class="error" role="alert">
              {error}<button class="icon" aria-label="关闭错误" onclick={() => (error = '')}
                >×</button
              >
            </div>{/if}
          {#if notice}<div class="notice" role="status">
              {notice}<button class="icon" aria-label="关闭提示" onclick={() => (notice = '')}
                >×</button
              >
            </div>{/if}
          {#if activeTopic.archived}<div class="archived-banner">
              这个话题已归档。<button
                class="text-button"
                disabled={busy}
                onclick={() => updateTopic(false)}>恢复话题</button
              >
            </div>{:else}
            <div class="composer">
              {#if reply}<div class="composer-reply">
                  ↳ 回复：{reply.body.slice(0, 100)}<button
                    class="icon"
                    aria-label="取消回复"
                    onclick={() => (reply = null)}>×</button
                  >
                </div>{/if}
              <div class="editor-tabs">
                <button class:active={!preview} onclick={() => (preview = false)}>编写</button
                ><button class:active={preview} onclick={() => (preview = true)}>预览</button><span
                  >MARKDOWN · LaTeX · CODE</span
                >
              </div>
              {#if preview}<div class="draft-preview">
                  {#if draft}<Markdown source={draft} />{:else}<p class="hint">
                      写一点内容，再看看它的样子。
                    </p>{/if}
                </div>{:else}<textarea
                  aria-label="消息正文"
                  bind:this={editor}
                  bind:value={draft}
                  placeholder="写下你的想法… 支持 $公式$ 和代码块"
                  onkeydown={(e) => {
                    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
                      e.preventDefault();
                      void send();
                    }
                  }}></textarea>{/if}
              <div class="composer-footer">
                <span class:over-limit={draftsBytes > 65536}
                  >{draftsBytes > 65536 ? '超过 64 KiB 消息上限' : '⌘ / Ctrl + Enter 发送'}
                  <span class="draft-count">{draftsBytes.toLocaleString()} B</span></span
                ><button
                  class="primary"
                  onclick={send}
                  disabled={sending || !draft.trim() || draftsBytes > 65536}
                  >{sending ? '加密入队…' : '发送 ↑'}</button
                >
              </div>
            </div>{/if}
          <p class="composer-note">
            ◇ {clientMode ? '设备连接加密 · 历史从中心同步' : '可信域中心之间端到端加密'} · 本机搜索与渲染
          </p>
        </div>
      {:else}
        <div class="workspace-empty">
          <button class="mobile-back text-button" onclick={() => (pane = 'topics')}
            >‹ 返回话题</button
          ><span class="eyebrow accent">ROOM FOR EVERY THOUGHT</span>
          <div class="empty-art">◒<span>✧</span></div>
          <h1>对话有序，想法自由。</h1>
          <p>选择一个话题，继续你的对话。<br />或从一个新的想法开始。</p>
          {#if activePeer}<button class="primary" onclick={() => openModal('topic')}
              >＋ 新建话题</button
            >{:else}<button class="primary" onclick={() => openModal('peer')}
              >添加第一位联系人</button
            >{/if}
        </div>
        {#if error}<div class="floating-message error" role="alert">
            {error}
          </div>{/if}{#if notice}<div class="floating-message notice" role="status">
            {notice}
          </div>{/if}
      {/if}
    </main>
  </div>
{/if}

{#if modal && unlocked}
  <div
    class="modal-backdrop"
    role="presentation"
    onclick={(e) => {
      if (e.target === e.currentTarget && !busy) modal = null;
    }}
  >
    <div
      class="modal"
      bind:this={dialogElement}
      role="dialog"
      aria-modal="true"
      aria-labelledby="modal-title"
      tabindex="-1"
      onkeydown={modalKeys}
    >
      <div class="modal-heading">
        <div>
          <span class="eyebrow accent">LOCAL WORKSPACE</span>
          <h2 id="modal-title">
            {{
              peer: '添加联系人',
              topic: '新建话题',
              rename: '重命名话题',
              identity: '我的可信域身份',
              peerIdentity: '联系人身份',
              search: '搜索本机消息',
              devices: clientMode ? '设备同步' : '域内设备管理',
            }[modal]}
          </h2>
        </div>
        <button class="icon" aria-label="关闭窗口" disabled={busy} onclick={() => (modal = null)}
          >×</button
        >
      </div>
      {#if modal === 'peer'}
        <p>
          交换公开身份卡，并通过可信渠道核对完整 user_id 指纹。卡片自签名不等同于你已经核实了对方。
        </p>
        <label class="file-picker"
          >选择 .contact.json / .peer.json 文件<input
            type="file"
            accept=".json,application/json"
            onchange={readCard}
          /></label
        >
        <form
          onsubmit={(e) => {
            e.preventDefault();
            void importPeer();
          }}
        >
          <label for="card">或粘贴公开身份卡 JSON</label><textarea
            id="card"
            bind:value={cardText}
            spellcheck="false"
            required
            placeholder={'{"user_id": "td_…", "label": "Alice", …}'}></textarea><button
            class="primary full"
            disabled={busy || !cardText.trim()}>{busy ? '验证身份卡…' : '验证并添加联系人'}</button
          >
        </form>
      {:else if modal === 'topic' || modal === 'rename'}
        <p>与 {activePeer?.label} 的一段独立对话。标题和话题信息通过加密事件传输。</p>
        <form
          onsubmit={(e) => {
            e.preventDefault();
            if (modal === 'topic') void createTopic();
            else void updateTopic(activeTopic?.archived || false);
          }}
        >
          <label for="title">话题标题</label><input
            id="title"
            bind:value={topicTitle}
            required
            maxlength="256"
            placeholder="例如：数学、NAS、项目讨论"
          /><button class="primary full" disabled={busy || !topicTitle.trim()}
            >{busy ? '保存中…' : modal === 'topic' ? '创建话题' : '保存标题'}</button
          >
        </form>
      {:else if (modal === 'identity' && self) || (modal === 'peerIdentity' && activePeer)}
        {@const card = modal === 'identity' ? self! : activePeer!}
        <p>
          {modal === 'identity'
            ? '可以分享这张公开身份卡。它不包含私钥、管理令牌或消息历史。'
            : '通过可信渠道核对完整 user_id。身份与网络地址相互独立。'}
        </p>
        <div class="identity-detail">
          <span class="eyebrow">{card.label}</span>
          <div class="detail-label">完整身份指纹 / user_id</div>
          <code>{card.user_id}</code>
          <div class="detail-label">Ed25519 签名公钥</div>
          <code>{card.signing_key}</code>
          <div class="detail-label">Curve25519 公钥</div>
          <code>{card.curve_key}</code>
        </div>
        {#if directMode}
          <p>直接 P2P：交换 .peer.json 连接卡，其中包含签名身份、地址及公开 CA。无需 Relay。</p>
          {#if modal === 'peerIdentity'}
            <code
              >{peerRoutes.find((r) => r.identity.user_id === card.user_id)?.endpoint ||
                '请导入对方的 .peer.json 连接卡'}</code
            >
            <button class="secondary" disabled={busy} onclick={checkPeer}>测试 P2P 连接</button>
          {/if}
        {/if}
        {#if modal === 'identity'}<div class="modal-buttons">
            <button class="primary" onclick={downloadCard}>下载身份卡</button><button
              class="secondary"
              onclick={copyCard}>复制 JSON</button
            ><button class="secondary" onclick={lock}>锁定界面</button>
          </div>{/if}
      {:else if modal === 'devices'}
        {#if clientMode && status?.device}
          <p>
            消息从你的中心服务器拉取，设备拥有独立密钥。本机可离线阅读历史和排队发送；联系人和话题操作需中心确认。
          </p>
          <div class="identity-detail">
            <span class="eyebrow">{status.device.card.label}</span>
            <div class="detail-label">本设备身份</div>
            <code>{status.device.card.id}</code>
            <div class="detail-label">中心身份</div>
            <code>{status.device.domainId}</code>
            <div class="detail-label">加密连接</div>
            <code>{status.device.server}</code>
            <div class="detail-label">本机游标 / 已确认游标</div>
            <code>{status.device.cursor} / {status.device.acknowledgedCursor}</code>
          </div>
          <button
            class="secondary"
            onclick={() =>
              exportJSON(status!.device!.card, `${status!.device!.card.id}.device.json`)}
            >导出设备身份卡</button
          >
          <h3 class="device-section-title">待处理请求 · {pendingOps.length}</h3>
          <p class="hint">
            网络中断的请求会保留并重试。只能删除中心明确拒绝的请求；删除失败消息时会同时删除它的本机正文。
          </p>
          <div class="device-records">
            {#each pendingOps as op (op.id)}<article>
                <div class="device-row">
                  <strong
                    >{{
                      send: '发送消息',
                      create_topic: '创建话题',
                      update_topic: '修改话题',
                      add_peer: '添加联系人',
                    }[op.operation.type] || op.operation.type}</strong
                  ><small>{op.state === 'failed' ? '已拒绝' : '等待同步'}</small>
                </div>
                <code>{op.id}</code>{#if op.operation.body}<pre>{op.operation.body}</pre>{:else}<p>
                    {op.operation.title || op.operation.card?.label || ''}
                  </p>{/if}{#if op.error}<p class="sync-error">
                    {op.error}
                  </p>{/if}{#if op.state === 'failed'}<button
                    class="secondary"
                    disabled={busy}
                    onclick={() => discardOperation(op.id)}>删除失败记录</button
                  >{/if}
              </article>{/each}
          </div>
        {:else}
          <p>
            授权你的设备连接本可信域。每个设备持有自己的密钥，通过 HTTPS
            拉取历史；外部联系人仍只看到你的中心身份。
          </p>
          {#if deviceEnabled}
            <label class="file-picker"
              >选择客户端导出的 .device.json<input
                type="file"
                accept=".json,application/json"
                onchange={readDevice}
              /></label
            >
            <form
              onsubmit={(e) => {
                e.preventDefault();
                void enrollDevice();
              }}
            >
              <label for="device-card">设备公开身份卡 JSON</label><textarea
                id="device-card"
                bind:value={deviceText}
                required
                spellcheck="false"
                placeholder="粘贴设备身份卡，先核对完整 dev_ 指纹"></textarea><button
                class="primary full"
                disabled={busy || !deviceText.trim()}
                >{busy ? '授权中…' : '授权设备并下载配对文件'}</button
              >
            </form>
          {:else}<div class="notice">
              设备监听尚未开启。请用 --device-bind、设备 TLS 证书及 --device-url
              配置重启中心；本机管理界面仍只监听 loopback。
            </div>{/if}
          <h3 class="device-section-title">已授权设备 · {deviceRecords.length}</h3>
          <div class="device-records">
            {#each deviceRecords as device (device.card.id)}<article>
                <div class="device-row">
                  <strong>{device.card.label}</strong><small
                    >{device.revoked ? '已撤销' : '已授权'}</small
                  >
                </div>
                <code>{device.card.id}</code>
                <p>
                  已确认游标 {device.acknowledgedCursor} · {device.lastSeen
                    ? `最近连接 ${date(device.lastSeen)}`
                    : '尚未连接'}
                </p>
                {#if !device.revoked}<button
                    class="secondary"
                    disabled={busy}
                    onclick={() => revokeDevice(device.card.id)}>撤销设备</button
                  >{/if}
              </article>{/each}
          </div>
        {/if}
      {:else if modal === 'search'}
        <p>只检索这台计算机的 SQLite 历史，搜索词保留在本机。</p>
        <form
          class="search-form"
          onsubmit={(e) => {
            e.preventDefault();
            void search();
          }}
        >
          <input
            aria-label="搜索关键词"
            bind:value={query}
            placeholder="输入要查找的词句"
            maxlength="512"
            required
          /><button class="primary" disabled={searchBusy}>{searchBusy ? '搜索中…' : '搜索'}</button>
        </form>
        <div class="search-results">
          {#each results as result (result.id)}<button onclick={() => goResult(result)}
              ><small
                >{topics.find((t) => t.id === result.topicId)?.title || '话题'} · {date(
                  result.timestamp,
                )}</small
              >
              <p>{result.body.slice(0, 240)}</p></button
            >{/each}{#if !results.length}<p class="hint">搜索结果会出现在这里。</p>{/if}
        </div>
      {/if}
      {#if error}<p class="error" role="alert">{error}</p>{/if}{#if notice}<p
          class="notice"
          role="status"
        >
          {notice}
        </p>{/if}
    </div>
  </div>
{/if}
