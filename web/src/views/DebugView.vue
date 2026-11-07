<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { formatTime } from '../api'
import { loadEvents, loadFrames, onLiveEvent, onLiveFrame, state, toast } from '../store'

type Tab = 'frames' | 'events'

const tab = ref<Tab>('frames')
const paused = ref(false)
const kind = ref('')
const botId = ref('')
const search = ref('')
const selected = ref<any>(null)
const events = ref<any[]>([])
const autoScroll = ref(true)

let offFrame: (() => void) | undefined
let offEvent: (() => void) | undefined
let timer: number | undefined

const frames = computed(() => state.frames)

const filteredFrames = computed(() => {
  const q = search.value.trim().toLowerCase()
  return frames.value.filter((frame) => {
    if (kind.value && frame.kind !== kind.value) return false
    if (botId.value && frame.bot_id !== botId.value) return false
    if (q && !(frame.summary || '').toLowerCase().includes(q) && !JSON.stringify(frame.data || {}).toLowerCase().includes(q)) return false
    return true
  })
})

const filteredEvents = computed(() => {
  const q = search.value.trim().toLowerCase()
  return events.value.filter((event) => {
    if (botId.value && event.target_bot_id !== botId.value) return false
    if (q && !(event.type || '').toLowerCase().includes(q) && !JSON.stringify(event.payload || {}).toLowerCase().includes(q)) return false
    return true
  })
})

onMounted(async () => {
  await Promise.all([loadLatestFrames(), loadLatestEvents()])
  offFrame = onLiveFrame((frame) => {
    if (!paused.value) {
      // store already prepended; nothing else to do
      if (!selected.value) selected.value = { ...frame, data: frame.data }
    }
  })
  offEvent = onLiveEvent((event) => {
    if (!paused.value) events.value.unshift(event)
    if (events.value.length > 500) events.value.pop()
  })
  timer = window.setInterval(() => {
    if (!paused.value) {
      loadLatestEvents(true)
      if (!state.frames.length) loadLatestFrames()
    }
  }, 5000)
})

onUnmounted(() => {
  offFrame?.()
  offEvent?.()
  window.clearInterval(timer)
})

async function loadLatestFrames() {
  const existing = new Set(state.frames.map((frame) => `${frame.ts}-${frame.summary}`))
  const items = await loadFrames({ limit: 200 })
  const merged = [...items.reverse().filter((frame) => !existing.has(`${frame.ts}-${frame.summary}`)), ...state.frames]
  state.frames = merged.slice(0, 500)
}

async function loadLatestEvents(quiet = false) {
  try {
    const items = await loadEvents({ limit: 100 })
    const seen = new Set(events.value.map((event) => event.seq))
    const fresh = items.filter((event) => !seen.has(event.seq))
    events.value = [...items.reverse(), ...events.value.filter((event) => !items.some((item) => item.seq === event.seq))].slice(0, 500)
    void fresh
  } catch {
    if (!quiet) toast('加载事件失败')
  }
}

function selectFrame(frame: any) {
  selected.value = frame
}

function pretty(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

async function copy(text: string, label = '已复制') {
  try {
    await navigator.clipboard.writeText(text)
    toast(label)
  } catch {
    toast('复制失败')
  }
}

function copyCurl(frame: any) {
  const data = frame.data || {}
  const base = state.status?.base_url || 'http://127.0.0.1:5700'
  const method = (data.method || 'GET').toUpperCase()
  const url = `${base}${data.path || ''}`
  const body = data.request ? ` \\\n  -H 'Content-Type: application/json' \\\n  -d '${JSON.stringify(data.request)}'` : ''
  copy(`curl -X ${method} '${url}' \\\n  -H 'Authorization: QQBot <ACCESS_TOKEN>'${body}`.replace(/\n/g, '\n'), 'curl 已复制')
}

function clearFrames() {
  state.frames = []
  selected.value = null
}

function markClass(frame: any): string {
  if (frame.direction === 'in') return 'info'
  if (frame.direction === 'out') return 'ok'
  return ''
}
</script>

<template>
  <div class="debug-layout">
    <section class="frame-list">
      <div class="row wrap" style="padding: 12px 14px; border-bottom: 1px solid var(--border); gap: 8px">
        <div class="tabs" style="margin: 0; border: 0">
          <button class="tab" :class="{ active: tab === 'frames' }" @click="tab = 'frames'">帧</button>
          <button class="tab" :class="{ active: tab === 'events' }" @click="tab = 'events'">事件</button>
        </div>
        <select v-if="tab === 'frames'" v-model="kind" class="select" style="width: 110px">
          <option value="">全部类型</option>
          <option value="http">http</option>
          <option value="ws">ws</option>
          <option value="internal">internal</option>
        </select>
        <select v-model="botId" class="select" style="width: 140px">
          <option value="">全部 Bot</option>
          <option v-for="bot in state.bots" :key="bot.id" :value="bot.id">{{ bot.name }}</option>
        </select>
        <input v-model="search" class="input" placeholder="搜索 summary / JSON" style="width: 200px" />
        <div class="spacer"></div>
        <label class="switch small">
          <input v-model="paused" type="checkbox" /><span class="track"><span class="thumb"></span></span> 暂停
        </label>
        <button class="btn small ghost" @click="clearFrames">清空</button>
      </div>

      <div v-if="tab === 'frames'" class="frame-scroll">
        <div
          v-for="(frame, index) in filteredFrames"
          :key="`${frame.ts}-${index}`"
          class="frame-row"
          :class="{ active: selected === frame }"
          @click="selectFrame(frame)"
        >
          <span class="frame-time">{{ formatTime(frame.ts) }}</span>
          <span class="tag" :class="markClass(frame)">{{ frame.direction }}</span>
          <span class="frame-dir">{{ frame.kind }}</span>
          <span class="frame-summary">{{ frame.summary }}</span>
        </div>
        <div v-if="!filteredFrames.length" class="empty">还没有帧。让 Bot 连一次网关，或用聊天页发条消息。</div>
      </div>

      <div v-else class="frame-scroll">
        <div
          v-for="event in filteredEvents"
          :key="event.seq"
          class="frame-row"
          :style="{ gridTemplateColumns: '70px 70px 1fr' }"
          :class="{ active: selected?.seq === event.seq }"
          @click="selected = event"
        >
          <span class="frame-time">#{{ event.seq }}</span>
          <span class="tag info">{{ event.target_bot_id }}</span>
          <span class="frame-summary">{{ event.type }} · {{ event.conversation_id }}</span>
        </div>
        <div v-if="!filteredEvents.length" class="empty">还没有事件</div>
      </div>
    </section>

    <aside class="detail">
      <div class="detail-head">
        <span class="grow small mono">{{ selected ? selected.summary || selected.type : '详情' }}</span>
        <button v-if="selected?.kind === 'http'" class="btn small" @click="copyCurl(selected)">复制 curl</button>
        <button v-if="selected" class="btn small ghost" @click="copy(pretty(selected.data || selected), 'JSON 已复制')">复制 JSON</button>
      </div>
      <pre v-if="selected">{{ pretty(selected.data || selected) }}</pre>
      <div v-else class="empty">点左侧任意一条查看原始报文</div>
    </aside>
  </div>
</template>
