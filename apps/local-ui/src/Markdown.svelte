<script lang="ts">
  import { onMount } from 'svelte';
  import { initializeRenderer, renderMarkdown } from './markdown';
  let { source }: { source: string } = $props();
  let ready = $state(false);
  const rendered = $derived.by(() => {
    void ready;
    return renderMarkdown(source);
  });
  onMount(() => {
    let active = true;
    initializeRenderer()
      .then(() => {
        if (active) ready = true;
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  });
</script>

<div class="markdown">{@html rendered}</div>
