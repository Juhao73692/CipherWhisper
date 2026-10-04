<script lang="ts">
  import type { BuildInfo } from './types';
  let { onback, buildInfo }: { onback: () => void; buildInfo: BuildInfo | null } = $props();
  const builtTime = (timestamp: string) =>
    new Date(timestamp).toLocaleString('zh-CN', { hour12: false, timeZoneName: 'short' });
</script>

<main class="about-page">
  <header>
    <button class="secondary" onclick={onback}>← 返回</button><span>CipherWhisper</span>
  </header>
  <h1>About</h1>
  <div class="about-project">
    <p>
      CipherWhisper 的名字来自 Cipher +
      Whisper，意为“加密的私语”。这是一个以独立话题组织交流的端到端加密 P2P
      聊天项目，让对话围绕具体的事情展开，并把聊天身份和消息历史保存在自己的设备上。
    </p>
    <p>
      项目目前处于开发阶段，主要支持一对一聊天、按话题整理消息，以及在自己授权的多个设备之间同步联系人和聊天历史。你可以选择一台计算机作为可信域中心，再连接自己的其他设备。
    </p>
  </div>
  <section>
    <h2>可信域中心与自己的设备</h2>
    <p>
      “本机为可信域中心”表示这台计算机保存你的聊天身份和历史，并负责向联系人投递消息。你可以授权自己的其他设备连接它，同步联系人、话题和消息。每台关联设备使用独立密钥。
    </p>
    <p>
      创建工作区时会准备所需证书。此后输入工作区口令即可启动后台并恢复身份与历史；口令不会保存到配置文件。
    </p>
  </section>
  <section>
    <h2>连接与加密</h2>
    <p>
      直接 P2P 连接通过交换 .peer.json 连接卡建立。连接卡包含签名身份、连接地址和公开 CA，不需要
      Relay；使用 Relay 时，它负责中转密文。
    </p>
    <p>
      可信域中心之间的消息端到端加密；标题和话题信息也通过加密事件传输。本机保存解密后的消息，供阅读、搜索和渲染。关联设备与中心通过
      HTTPS 加密连接同步。
    </p>
  </section>
  <section>
    <h2>话题、消息与本机历史</h2>
    <p>
      可以按日常事情建立独立话题，例如周末计划、晚饭吃什么或旅行安排。消息以 Markdown 源文传输，支持
      LaTeX 公式和代码块，可以在编写时预览，阅读时查看原文。
    </p>
    <p>
      消息历史保存在本机 SQLite
      数据库中；搜索只检索这台计算机的历史，搜索词保留在本机。归档会收起话题，保留其中的消息，也可以恢复。
    </p>
  </section>
  <section>
    <h2>刷新与发送状态</h2>
    <p>
      聊天界面和后台约每半秒检查更新。网络中断时消息会保留并逐步重试，连续失败 10 次后停止自动投递。
    </p>
    <p>
      暂停的消息只保留在本机消息区，旁边显示红色感叹号。点击感叹号会以新的编号和发送时间发送一条新消息，原消息仍保留为失败记录。原消息可能已在某次尝试中被对方接收，因此重新发送的内容也可能再次出现在对方聊天中。
    </p>
    <p>“对方已接收”表示对方设备已确认保存消息，不代表对方已经阅读。</p>
  </section>
  <footer>
    {#if buildInfo}<p class="build-full">
        build {buildInfo.number} ({buildInfo.commit}{buildInfo.dirty ? '-dirty' : ''}) at {builtTime(
          buildInfo.builtAt,
        )}
      </p>{/if}
    <a href="/third-party-ui.txt" target="_blank" rel="noopener noreferrer">开源许可</a>
  </footer>
</main>
