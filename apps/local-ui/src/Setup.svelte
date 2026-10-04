<script lang="ts">
  import { untrack } from 'svelte';
  import { api } from './api';
  import NetworkFields from './NetworkFields.svelte';
  import type { LauncherStatus, NetworkConfig } from './types';
  let {
    initial,
    onready,
    pairingOnly = false,
  }: {
    initial: LauncherStatus;
    onready: (initial: LauncherStatus) => Promise<void>;
    pairingOnly?: boolean;
  } = $props();
  let current = $state(untrack(() => initial));
  let name = $state(''),
    role = $state<'center' | 'device'>('center');
  let password = $state(''),
    confirmation = $state(''),
    error = $state(''),
    busy = $state(false);
  let pairingText = $state(''),
    fingerprint = $state('');
  let network = $state<NetworkConfig>(
    untrack(() => ({
      host: initial.suggestedHost,
      peerPort: 8800,
      devices: false,
      devicePort: 8802,
    })),
  );
  async function create() {
    error = '';
    if (new TextEncoder().encode(password).length < 12) {
      error = '口令至少需要 12 字节。';
      return;
    }
    if (!current.config && password !== confirmation) {
      error = '两次输入的口令不一致。';
      return;
    }
    busy = true;
    try {
      current = await api<LauncherStatus>('/launcher/start', {
        passphrase: password,
        name: name.trim(),
        role,
        network,
      });
      password = '';
      confirmation = '';
      if (current.running) await onready(current);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
  function downloadDevice() {
    const url = URL.createObjectURL(
      new Blob([JSON.stringify(current.deviceCard, null, 2)], { type: 'application/json' }),
    );
    const a = document.createElement('a');
    a.href = url;
    a.download = `${current.config?.name || 'device'}.device.json`;
    a.click();
    URL.revokeObjectURL(url);
  }
  async function readPairing(event: Event) {
    const file = (event.target as HTMLInputElement).files?.[0];
    if (!file) return;
    if (file.size > 300 * 1024) {
      error = '配对文件过大';
      return;
    }
    pairingText = await file.text();
  }
  async function pair() {
    busy = true;
    error = '';
    try {
      current = await api<LauncherStatus>('/launcher/pair', {
        pairing: JSON.parse(pairingText),
        trustedDomain: fingerprint.trim(),
      });
      pairingText = '';
      fingerprint = '';
      await onready(current);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<section class="setup-content">
  {#if (current.unlocked && current.config?.role === 'device') || pairingOnly}
    <span class="eyebrow accent">DEVICE PAIRING</span>
    <h2>连接自己的中心</h2>
    {#if current.deviceCard}
      <p>下载本设备的公开身份卡，在中心的「域内设备管理」授权后，把中心生成的配对文件导入这里。</p>
      <code class="setup-fingerprint">{current.deviceCard.id}</code>
      <button class="secondary" onclick={downloadDevice} disabled={busy}>下载设备身份卡</button>
    {:else}<p>从中心下载更新的配对文件，在这里导入以更新连接地址和证书。</p>{/if}
    <form
      onsubmit={(e) => {
        e.preventDefault();
        void pair();
      }}
    >
      <label class="file-picker"
        >选择中心配对文件<input
          type="file"
          accept=".json,application/json"
          onchange={readPairing}
          disabled={busy}
        /></label
      >
      <label for="pairing-json">或粘贴配对文件内容</label>
      <textarea
        id="pairing-json"
        bind:value={pairingText}
        required
        disabled={busy}
        spellcheck="false"></textarea>
      <label for="center-fingerprint">已核对的完整中心身份指纹</label>
      <input
        id="center-fingerprint"
        bind:value={fingerprint}
        required
        disabled={busy}
        placeholder="td_…，通过可信渠道向中心核对"
        spellcheck="false"
      />
      <button class="primary full" disabled={busy || !pairingText.trim() || !fingerprint.trim()}
        >{busy ? '验证并连接中…' : '验证配对并连接'}</button
      >
    </form>
  {:else}
    <span class="eyebrow accent"
      >{current.config ? 'WELCOME BACK' : 'WELCOME TO CIPHERWHISPER'}</span
    >
    <h2>{current.config ? `解锁 ${current.config.name}` : '创建你的工作区'}</h2>
    <p>
      {current.config
        ? '输入原口令，继续使用已有身份和历史。'
        : '选择用途并设置口令，程序会保存身份和配置，自动生成连接证书。'}
    </p>
    <form
      onsubmit={(e) => {
        e.preventDefault();
        void create();
      }}
    >
      <fieldset class="setup-fields" disabled={busy}>
        {#if !current.config}
          <div class="setup-roles" aria-label="工作区用途">
            <label
              ><input type="radio" bind:group={role} value="center" />创建聊天中心<small
                >保存身份，与联系人直接聊天</small
              ></label
            >
            <label
              ><input type="radio" bind:group={role} value="device" />连接自己的中心<small
                >作为另一台设备同步和聊天</small
              ></label
            >
          </div>
          <label for="setup-name">{role === 'center' ? '你的名称' : '设备名称'}</label>
          <input
            id="setup-name"
            bind:value={name}
            required
            maxlength="128"
            autocomplete="nickname"
          />
        {/if}
        <label for="setup-password">工作区口令</label>
        <input
          id="setup-password"
          type="password"
          bind:value={password}
          required
          maxlength="1024"
          autocomplete={current.config ? 'current-password' : 'new-password'}
        />
        {#if !current.config}
          <label for="setup-confirmation">再次输入口令</label>
          <input
            id="setup-confirmation"
            type="password"
            bind:value={confirmation}
            required
            maxlength="1024"
            autocomplete="new-password"
          />
          <p class="hint">
            至少 12 字节。口令不会保存到配置文件；请妥善记住，目前没有口令找回功能。
          </p>
        {/if}
      </fieldset>
      {#if !current.config && role === 'center'}<NetworkFields bind:network disabled={busy} />{/if}
      <button class="primary full" disabled={busy}
        >{busy ? '正在准备工作区…' : current.config ? '解锁并启动' : '创建并继续'}</button
      >
    </form>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <p class="hint setup-storage">身份与历史保存在 {current.dataDirectory}</p>
</section>
