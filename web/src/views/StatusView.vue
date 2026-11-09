<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { api, formatBytes, formatDuration } from '../api'
import { gotoBotConfig, refreshStatus, refreshWorld, state, toast } from '../store'

const tokenInfo = ref<{ access_token: string; app_id: string } | null>(null)
const profiles = ref<any[]>([])
const busy = ref(false)
const desktopBridge = (window as any).mqsDesktop

const status = computed(() => state.status)
const bots = computed(() => state.bots)
const primaryBot = computed(() => state.bots[0])
const onlineBotIds = computed(() => new Set(status.value?.gateway.bots ?? []))
const connectionCounts = computed(() => {
  const counts = new Map<string, number>()
  for (const shard of status.value?.gateway.shards ?? []) {
    counts.set(shard.bot_id, (counts.get(shard.bot_id) ?? 0) + 1)
  }
  return counts
})
const coverage = computed(() => status.value?.coverage)
const coveragePercent = computed(() => {
  if (!coverage.value) return 0
  const implemented = (coverage.value.by_level.full || 0) + (coverage.value.by_level.partial || 0)
  return coverage.value.total ? Math.round((implemented / coverage.value.total) * 100) : 0
})

onMounted(async () => {
  await Promise.all([refreshStatus(), refreshWorld()])
})

// The profile list is behind the control-token/cookie check: wait for the
// bootstrap handshake instead of racing it on mount.
watch(
  () => state.ready,
  (ready) => {
    if (ready) loadProfiles()
  },
  { immediate: true },
)

function isOnline(bot: any): boolean {
  // 有实时 status 时以网关连接为准，否则退回 /_mqs/bots 返回的快照字段
  if (status.value) return onlineBotIds.value.has(bot.id)
  return Boolean(bot.online)
}

function connectionCount(bot: any): number {
  return connectionCounts.value.get(bot.id) ?? (isOnline(bot) ? 1 : 0)
}

function openConnectInfo() {
  if (!primaryBot.value) return
  gotoBotConfig(primaryBot.value.id)
}

function gotoConfig() {
  gotoBotConfig()
}

async function switchProfile(name: string) {
  if (!desktopBridge?.switchProfile) return
  if (!confirm(`切换到 profile「${name}」？本地服务会重启，窗口会自动刷新。`)) return
  busy.value = true
  const result = await desktopBridge.switchProfile(name)
  busy.value = false
  if (!result?.ok) toast(result?.error || '切换失败')
}

async function loadProfiles() {
  try {
    profiles.value = await api.get<any[]>('/_mqs/profiles')
  } catch {
    profiles.value = []
  }
}

async function fetchToken() {
  if (!primaryBot.value) return
  busy.value = true
  try {
    tokenInfo.value = await api.post<any>(`/_mqs/bots/${primaryBot.value.app_id}/token`)
  } finally {
    busy.value = false
  }
}

async function copy(text: string | undefined, label = '已复制') {
  if (!text) return
  try {
    await navigator.clipboard.writeText(text)
    toast(label)
  } catch {
    toast('复制失败，请手动选择')
  }
}

async function advance(duration: string) {
  await api.post('/_mqs/clock/advance', { duration })
  await refreshStatus()
  toast(`时钟快进 ${duration}`)
}

async function setClockReal() {
  await api.post('/_mqs/clock/set', { mode: 'real' })
  await refreshStatus()
  toast('已切回真实时钟')
}
</script>

<template>
  <div class="grid" style="gap: 16px">
    <div class="grid c4">
      <div class="stat">
        <div class="stat-label">网关连接</div>
        <div class="stat-value">{{ status?.gateway.connections ?? 0 }}</div>
        <div class="stat-sub">{{ status?.gateway.bots.join(', ') || '暂无 Bot 在线' }}</div>
      </div>
      <div class="stat">
        <div class="stat-label">消息 / 事件</div>
        <div class="stat-value">{{ status?.counts.messages ?? 0 }}<span class="faint" style="font-size: 14px"> / {{ status?.counts.events ?? 0 }}</span></div>
        <div class="stat-sub">seq {{ status?.max_event_seq ?? 0 }} · {{ status?.events_last_minute ?? 0 }}/min</div>
      </div>
      <div class="stat">
        <div class="stat-label">限流拒绝</div>
        <div class="stat-value" :style="status?.rate.rejected ? 'color: var(--red)' : ''">{{ status?.rate.rejected ?? 0 }}</div>
        <div class="stat-sub">{{ status?.rate.mode }} 档 · 允许 {{ status?.rate.allowed ?? 0 }}</div>
      </div>
      <div class="stat">
        <div class="stat-label">存储 / 运行时长</div>
        <div class="stat-value small">{{ formatBytes(status?.db_size ?? 0) }}</div>
        <div class="stat-sub">{{ formatDuration(status?.uptime_ms ?? 0) }} · {{ status?.counts.frames ?? 0 }} 帧</div>
      </div>
    </div>

    <div class="grid c2">
      <div class="card">
        <div class="card-title">接入信息卡 <span class="muted">把 Bot 指向这里</span></div>
        <div class="kv">
          <span class="k">base_url</span>
          <span class="v">{{ status?.base_url }} <button class="btn small ghost" @click="copy(status?.base_url, 'base_url 已复制')">复制</button></span>
        </div>
        <div class="kv">
          <span class="k">ws_url</span>
          <span class="v">{{ status?.ws_url }} <button class="btn small ghost" @click="copy(status?.ws_url, 'ws_url 已复制')">复制</button></span>
        </div>
        <div class="kv">
          <span class="k">appid</span>
          <span class="v">{{ primaryBot?.app_id ?? '—' }} <button class="btn small ghost" @click="copy(primaryBot?.app_id, 'appid 已复制')">复制</button></span>
        </div>
        <div class="kv">
          <span class="k">secret</span>
          <span class="v">{{ primaryBot?.secret ?? '—' }} <button class="btn small ghost" @click="copy(primaryBot?.secret, 'secret 已复制')">复制</button></span>
        </div>
        <div class="kv">
          <span class="k">access_token</span>
          <span class="v">
            <span v-if="tokenInfo">{{ tokenInfo.access_token }}</span>
            <span v-else class="faint">点击右侧获取（7200s）</span>
            <button class="btn small ghost" :disabled="busy" @click="fetchToken">{{ tokenInfo ? '刷新' : '获取' }}</button>
            <button v-if="tokenInfo" class="btn small ghost" @click="copy(tokenInfo.access_token, 'token 已复制')">复制</button>
          </span>
        </div>
        <p class="small faint" style="margin: 12px 0 0">
          假凭据以 <code class="mono">MQS_</code> / <code class="mono">mqs-</code> 开头，方便一眼识别。
        </p>

        <div class="panel-section-title" style="margin: 16px 0 6px">Bot 连接状态</div>
        <div v-for="bot in bots" :key="bot.id" class="member-row">
          <span class="dot" :class="isOnline(bot) ? 'on' : 'off'"></span>
          <img class="avatar sm" :src="bot.avatar" alt="" />
          <span>{{ bot.name }}</span>
          <span class="mono small faint">{{ bot.app_id }}</span>
          <span class="spacer"></span>
          <span v-if="isOnline(bot)" class="small faint">{{ connectionCount(bot) }} 个连接</span>
          <span class="tag" :class="isOnline(bot) ? 'ok' : ''">{{ isOnline(bot) ? '在线' : '离线' }}</span>
        </div>
        <div v-if="!bots.length" class="small faint">暂无 Bot</div>

        <div class="row wrap" style="margin-top: 14px">
          <button class="btn small primary" :disabled="!primaryBot" @click="openConnectInfo">打开接入信息</button>
          <button class="btn small" @click="gotoConfig">转到 Bot 配置</button>
        </div>
      </div>

      <div class="card">
        <div class="card-title">运行概况</div>
        <div class="kv"><span class="k">profile</span><span class="v">{{ status?.profile }}</span></div>
        <div class="kv"><span class="k">世界</span><span class="v">{{ status?.counts.users }} 用户 · {{ status?.counts.groups }} 群 · {{ status?.counts.bots }} Bot</span></div>
        <div class="kv"><span class="k">时钟</span><span class="v">{{ status?.clock.mode }} · {{ status?.clock.now }}</span></div>
        <div class="kv"><span class="k">监听</span><span class="v">{{ status?.host }} · sim {{ status?.sim_port }} · control {{ status?.control_port }}</span></div>
        <div class="kv"><span class="k">profile 路径</span><span class="v">{{ status?.profile_dir }}</span></div>
        <div class="row wrap" style="margin-top: 12px">
          <button class="btn small" @click="advance('5m')">时钟 +5m</button>
          <button class="btn small" @click="advance('2h')">+2h</button>
          <button class="btn small" @click="setClockReal">回到真实时钟</button>
        </div>
      </div>
    </div>

    <div class="grid c2">
      <div class="card">
        <div class="card-title">接口覆盖率 <span class="muted">{{ coverage?.ascii }}</span></div>
        <div class="progress"><i :style="{ width: coveragePercent + '%' }"></i></div>
        <div class="grid c4" style="margin-top: 14px">
          <div class="stat"><div class="stat-label">full</div><div class="stat-value small">{{ coverage?.by_level.full || 0 }}</div></div>
          <div class="stat"><div class="stat-label">partial</div><div class="stat-value small">{{ coverage?.by_level.partial || 0 }}</div></div>
          <div class="stat"><div class="stat-label">stub</div><div class="stat-value small">{{ coverage?.by_level.stub || 0 }}</div></div>
          <div class="stat"><div class="stat-label">n/a</div><div class="stat-value small">{{ coverage?.by_level['n/a'] || 0 }}</div></div>
        </div>
      </div>

      <div class="card">
        <div class="card-title">Profile <span class="muted">同一时间每个 profile 只能有一个进程</span></div>
        <table class="table">
          <thead>
            <tr><th>名称</th><th>状态</th><th>端口</th><th></th></tr>
          </thead>
          <tbody>
            <tr v-for="profile in profiles" :key="profile.name">
              <td>{{ profile.name }} <span v-if="profile.current" class="tag info">当前</span></td>
              <td><span class="tag" :class="profile.running ? 'ok' : ''">{{ profile.running ? '运行中' : '未运行' }}</span></td>
              <td class="mono small">{{ profile.sim_port ?? '—' }} / {{ profile.control_port ?? '—' }}</td>
              <td class="actions">
                <button
                  v-if="desktopBridge && !profile.current"
                  class="btn small primary"
                  :disabled="busy"
                  @click="switchProfile(profile.name)"
                >
                  切换
                </button>
                <button v-if="profile.running && !profile.current && !desktopBridge" class="btn small" @click="copy(profile.gui_url, 'GUI 链接已复制')">复制 GUI 链接</button>
                <button v-if="!profile.running && !desktopBridge" class="btn small ghost" @click="copy(`mqs serve --profile ${profile.name}`, '启动命令已复制')">复制启动命令</button>
              </td>
            </tr>
            <tr v-if="!profiles.length"><td colspan="4" class="muted">暂无 profile</td></tr>
          </tbody>
        </table>
      </div>
    </div>
  </div>
</template>
