<script lang="ts">
  import { untrack } from 'svelte';
  import { api } from './api';
  import NetworkFields from './NetworkFields.svelte';
  import Setup from './Setup.svelte';
  import type { LauncherStatus, NetworkConfig } from './types';
  let {
    initial,
    onready,
  }: { initial: LauncherStatus; onready: (initial: LauncherStatus) => Promise<void> } = $props();
  let current = $state(untrack(() => initial));
  let network = $state<NetworkConfig>(untrack(() => ({ ...initial.config!.network })));
  let renew = $state(false),
    busy = $state(false),
    error = $state(''),
    notice = $state('');
  async function save() {
    busy = true;
    error = '';
    notice = '';
    try {
      const changedIdentityCard =
        renew ||
        network.host !== current.config?.network.host ||
        network.peerPort !== current.config?.network.peerPort;
      current = await api<LauncherStatus>('/launcher/network', {
        network,
        renewCertificate: renew,
      });
      renew = false;
      await onready(current);
      notice = changedIdentityCard
        ? '设置已生效。请重新下载身份卡并交给联系人；已授权设备也需导入重新导出的配对文件。'
        : '设置已保存并生效。';
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

{#if initial.config?.role === 'device'}
  <Setup {initial} {onready} pairingOnly />
{:else}
  <div class="setup-content">
    <p>修改连接地址和端口后，后台自动应用设置。身份、联系人和历史继续保留。</p>
    <form
      onsubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <NetworkFields bind:network disabled={busy} />
      {#if current.certificateExpires}<p class="hint">
          当前证书有效期至 {new Date(current.certificateExpires * 1000).toLocaleDateString('zh-CN')}
        </p>{/if}
      <label class="setup-check"
        ><input type="checkbox" bind:checked={renew} disabled={busy} />重新生成连接证书</label
      >
      <p class="hint">改变地址时会自动换发证书。更新后需向联系人和自己的设备分享新的连接配置。</p>
      <button class="primary full" disabled={busy}
        >{busy ? '正在应用设置…' : '保存并应用设置'}</button
      >
    </form>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if notice}<p class="notice" role="status">{notice}</p>{/if}
    <p class="hint setup-storage">数据目录：{initial.dataDirectory}</p>
  </div>
{/if}
