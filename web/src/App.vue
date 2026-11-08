<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { setToken } from './api'
import { bootstrap, refreshStatus, refreshWorld, state } from './store'
import TextPrompt from './components/TextPrompt.vue'
import StatusView from './views/StatusView.vue'
import ConfigView from './views/ConfigView.vue'
import ChatView from './views/ChatView.vue'
import DebugView from './views/DebugView.vue'
import GuildView from './views/GuildView.vue'
import SettingsPanel from './views/SettingsPanel.vue'

const tabs = [
  { id: 'status', label: '状态', icon: '◎' },
  { id: 'config', label: '配置', icon: '⚙' },
  { id: 'chat', label: '聊天', icon: '💬' },
  { id: 'guild', label: '频道', icon: '🏛' },
  { id: 'debug', label: '调试器', icon: '⌘' },
] as const

const views: Record<string, any> = {
  status: StatusView,
  config: ConfigView,
  chat: ChatView,
  guild: GuildView,
  debug: DebugView,
}

const view = ref<string>(localStorage.getItem('mqs-view') || 'status')
watch(view, (value) => localStorage.setItem('mqs-view', value))

const current = computed(() => tabs.find((tab) => tab.id === view.value) || tabs[0])
const tokenInput = ref('')
const loggingIn = ref(false)
const settingsOpen = ref(false)

let timer: number | undefined

watch(
  () => state.settings.theme,
  (theme) => {
    if (theme === 'dark' || theme === 'light') {
      document.documentElement.dataset.theme = theme
    } else {
      delete document.documentElement.dataset.theme
    }
    document.documentElement.lang = state.settings.language || 'zh-CN'
  },
  { immediate: true },
)

onMounted(async () => {
  await bootstrap()
  timer = window.setInterval(() => {
    if (state.ready) refreshStatus()
  }, 3000)
})

onUnmounted(() => window.clearInterval(timer))

async function login() {
  if (!tokenInput.value.trim()) return
  loggingIn.value = true
  setToken(tokenInput.value.trim())
  await bootstrap()
  loggingIn.value = false
}

function refresh() {
  refreshWorld()
  refreshStatus()
}
</script>

<template>
  <div v-if="state.needAuth" class="auth-mask">
    <div class="card auth-card">
      <h1>QSandbox 控制台</h1>
      <p class="muted">
        需要控制 API token。用 <code class="mono">mqs status</code> 查看，或从启动日志里的
        <code class="mono">Control token</code> 复制。
      </p>
      <div class="row" style="margin-top: 16px">
        <input v-model="tokenInput" class="input grow" type="password" placeholder="粘贴 control token" @keyup.enter="login" />
        <button class="btn primary" :disabled="loggingIn" @click="login">进入</button>
      </div>
      <p class="small faint" style="margin-top: 14px">
        也可以直接用 <code class="mono">mqs serve</code> 输出的 GUI 链接打开，nonce 会自动换取 HttpOnly cookie。
      </p>
    </div>
  </div>

  <div class="app">
    <aside class="sidebar">
      <div class="brand">
        <div class="brand-mark">M</div>
        <div>
          <div class="brand-text">QSandbox</div>
          <div class="brand-sub">QQ Bot Sandbox</div>
        </div>
      </div>
      <button
        v-for="tab in tabs"
        :key="tab.id"
        class="nav-item"
        :class="{ active: view === tab.id }"
        @click="view = tab.id"
      >
        <span class="nav-icon">{{ tab.icon }}</span>
        <span>{{ tab.label }}</span>
      </button>
      <div class="sidebar-foot">
        非官方沙箱 · 与腾讯无关联<br />
        所有数据均为本地假数据
      </div>
    </aside>

    <div class="main">
      <header class="topbar">
        <div class="topbar-title">{{ current.label }}</div>
        <span class="pill">
          <span class="dot" :class="state.live ? 'on' : 'off'"></span>
          {{ state.live ? '实时' : '未连接' }}
        </span>
        <span v-if="state.status" class="pill">profile · {{ state.status.profile }}</span>
        <span v-if="state.status" class="pill">
          {{ state.status.clock.mode === 'virtual' ? '虚拟时钟' : '真实时钟' }} · {{ state.status.clock.now.slice(11, 19) }}
        </span>
        <div class="spacer"></div>
        <button class="btn small ghost" @click="refresh">刷新</button>
        <button class="btn small ghost" @click="settingsOpen = true" title="设置">⚙ 设置</button>
      </header>
      <main class="content" :style="view === 'chat' || view === 'debug' ? 'padding: 0; overflow: hidden' : ''">
        <KeepAlive>
          <component :is="views[view]" :key="view" />
        </KeepAlive>
      </main>
    </div>
  </div>

  <SettingsPanel v-if="settingsOpen" @close="settingsOpen = false" />

  <!-- 应用内文本输入弹窗：Electron 不支持 window.prompt()，各视图统一用它 -->
  <TextPrompt />

  <div v-if="state.toast" class="toast">{{ state.toast }}</div>
</template>
