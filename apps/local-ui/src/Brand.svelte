<script lang="ts">
  import type { BuildInfo } from './types';
  let {
    buildInfo,
    onabout,
    startup = false,
  }: { buildInfo: BuildInfo | null; onabout: () => void; startup?: boolean } = $props();
</script>

{#snippet aboutLink()}
  <a
    class="brand-about"
    href="#about"
    onclick={(event) => {
      event.preventDefault();
      onabout();
    }}>About</a
  >
{/snippet}

<div class={startup ? 'unlock-brand' : 'brand'}>
  <span class="brand-mark">◒</span>
  <span class="brand-copy"
    ><span class="brand-title">CipherWhisper{@render aboutLink()}</span>
    <small class="brand-caption">{startup ? 'PRIVATE P2P CHAT' : 'PRIVATE BY DESIGN'}</small>
    {#if buildInfo && (startup || buildInfo.debug)}<small class="build-short"
        >build {buildInfo.number}</small
      >{/if}
  </span>
</div>
