<script lang="ts">
  import Markdown from './Markdown.svelte';
  import type { Message } from './types';
  let {
    message,
    source = false,
    mine = false,
    busy = false,
    accept,
    downloadFile,
  }: {
    message: Message;
    source?: boolean;
    mine?: boolean;
    busy?: boolean;
    accept: (message: Message) => void;
    downloadFile: (message: Message) => void;
  } = $props();
  const states: Record<string, string> = {
    offered: '等待接收方确认',
    accepted: '已确认，等待发送方传输',
    transferring: '传输中',
    sent: '文件已进入发送队列',
    complete: '传输完成，校验通过',
    failed: '传输失败',
  };
</script>

{#if message.withdrawn || message.format === 'withdrawn'}
  <p class="withdrawn">这条消息已撤回</p>
{:else if message.file && source}
  <pre class="source-text">{message.body}</pre>
{:else if message.file}
  <div class="file-card">
    <strong>📄 {message.file.name}</strong>
    <small>{message.file.size.toLocaleString()} B · {message.file.mime || '文件'}</small>
    <p>{states[message.file.state] || message.file.state}</p>
    {#if message.file.acceptId && message.file.chunks > 0}
      <progress value={message.file.received} max={message.file.chunks} aria-label="文件传输进度"
      ></progress>
      <small>{message.file.received} / {message.file.chunks} 块</small>
    {/if}
    {#if message.file.error}<p class="file-error">{message.file.error}</p>{/if}
    {#if !mine && message.file.state === 'offered'}
      <button class="secondary" disabled={busy} onclick={() => accept(message)}>接收文件</button>
    {/if}
    {#if !mine && message.file.state === 'complete'}
      <button class="secondary" disabled={busy} onclick={() => downloadFile(message)}
        >下载文件</button
      >
    {/if}
    <details><summary>文件信息</summary><code>SHA-256: {message.file.sha256}</code></details>
  </div>
{:else if message.specialKind || message.format === 'unknown'}
  <div class="unknown-message">
    <strong>收到未知的消息</strong>
    {#if message.specialError}<p>{message.specialError}</p>{/if}
    <pre>{message.body}</pre>
  </div>
{:else if source}
  <pre class="source-text">{message.body}</pre>
{:else}
  <Markdown source={message.body} />
{/if}

<style>
  .file-card,
  .unknown-message {
    padding: 14px;
    border: 1px solid var(--border, #d9d9d9);
    border-radius: 10px;
    max-width: 520px;
  }
  .file-card strong,
  .file-card small {
    display: block;
    overflow-wrap: anywhere;
  }
  .file-card small,
  .withdrawn {
    opacity: 0.65;
  }
  .file-card p {
    margin: 10px 0;
  }
  progress {
    width: 100%;
  }
  code,
  pre {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: 12px;
  }
  pre {
    max-height: 300px;
    overflow: auto;
  }
  details {
    margin-top: 12px;
  }
  .file-error {
    color: #bb3840;
  }
</style>
