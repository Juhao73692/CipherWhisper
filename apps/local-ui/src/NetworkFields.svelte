<script lang="ts">
  import type { NetworkConfig } from './types';
  let { network = $bindable(), disabled = false }: { network: NetworkConfig; disabled?: boolean } =
    $props();
</script>

<fieldset class="setup-fields" {disabled}>
  <label for="network-host">这台电脑的连接地址</label>
  <input
    id="network-host"
    bind:value={network.host}
    required
    maxlength="253"
    placeholder="例如 192.168.1.10 或你的 Tailscale 地址"
  />
  <p class="hint">
    填写对方能访问的 IP 或域名。证书会自动生成并绑定这个地址；仅在本机试用可填写 localhost。
  </p>
  <label for="peer-port">聊天连接端口</label>
  <input id="peer-port" type="number" bind:value={network.peerPort} min="1" max="65535" required />
  <label class="setup-check"
    ><input type="checkbox" bind:checked={network.devices} />允许自己的其他设备连接</label
  >
  {#if network.devices}
    <label for="device-port">设备连接端口</label>
    <input
      id="device-port"
      type="number"
      bind:value={network.devicePort}
      min="1"
      max="65535"
      required
    />
  {/if}
  <p class="hint">跨电脑需要可达网络，例如同一局域网或 Tailscale。首次聊天时双方都需在线。</p>
</fieldset>
