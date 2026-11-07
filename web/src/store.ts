import { reactive } from 'vue'
import { ApiError, api, authCheck, exchangeNonce, openLiveSocket } from './api'

export interface Status {
  profile: string
  boot_id: string
  started_at: number
  uptime_ms: number
  clock: { mode: string; now_ms: number; now: string }
  counts: Record<string, number>
  max_event_seq: number
  max_message_id: number
  max_frame_id: number
  events_last_minute: number
  rate: { mode: string; buckets: number; allowed: number; rejected: number }
  gateway: { connections: number; sessions: number; bots: string[]; shards: any[] }
  db_size: number
  settings: Record<string, any>
  coverage: { total: number; by_level: Record<string, number>; ascii: string }
  hub_subscribers: number
  base_url: string
  ws_url: string
  control_port: number
  host: string
  gui_url: string
  profile_dir: string
  sim_port: number
}

export const state = reactive({
  ready: false,
  needAuth: false,
  live: false,
  status: null as Status | null,
  users: [] as any[],
  bots: [] as any[],
  groups: [] as any[],
  conversations: [] as any[],
  settings: {} as Record<string, any>,
  events: [] as any[],
  frames: [] as any[],
  toast: '',
})

type Listener = (data: any) => void
const messageListeners = new Set<Listener>()
const eventListeners = new Set<(event: any) => void>()
const frameListeners = new Set<(frame: any) => void>()

export function onLiveMessage(fn: Listener): () => void {
  messageListeners.add(fn)
  return () => messageListeners.delete(fn)
}

export function onLiveEvent(fn: (event: any) => void): () => void {
  eventListeners.add(fn)
  return () => eventListeners.delete(fn)
}

export function onLiveFrame(fn: (frame: any) => void): () => void {
  frameListeners.add(fn)
  return () => frameListeners.delete(fn)
}

export function toast(text: string): void {
  state.toast = text
  window.clearTimeout((toast as any)._timer)
  ;(toast as any)._timer = window.setTimeout(() => (state.toast = ''), 2200)
}

let socket: WebSocket | null = null
let reconnectTimer: number | undefined

export async function bootstrap(): Promise<void> {
  await exchangeNonce()
  if (!(await authCheck())) {
    state.needAuth = true
    return
  }
  state.needAuth = false
  await refreshAll()
  connectLive()
  state.ready = true
}

export async function refreshAll(): Promise<void> {
  await Promise.all([refreshStatus(), refreshWorld(), refreshSettings()])
}

export async function refreshStatus(): Promise<void> {
  try {
    state.status = await api.get<Status>('/_mqs/status')
  } catch (error) {
    handleAuthError(error)
  }
}

export async function refreshWorld(): Promise<void> {
  try {
    const [users, bots, groups, conversations] = await Promise.all([
      api.get<any[]>('/_mqs/users'),
      api.get<any[]>('/_mqs/bots'),
      api.get<any[]>('/_mqs/groups'),
      api.get<any[]>('/_mqs/conversations'),
    ])
    state.users = users
    state.bots = bots
    state.groups = groups
    state.conversations = conversations
  } catch (error) {
    handleAuthError(error)
  }
}

export async function refreshSettings(): Promise<void> {
  try {
    state.settings = await api.get<Record<string, any>>('/_mqs/settings')
  } catch (error) {
    handleAuthError(error)
  }
}

export async function patchSettings(patch: Record<string, unknown>): Promise<void> {
  state.settings = await api.patch<Record<string, any>>('/_mqs/settings', patch)
  await refreshStatus()
  toast('设置已保存')
}

function handleAuthError(error: unknown): void {
  if (error instanceof ApiError && error.status === 401) state.needAuth = true
}

export function connectLive(): void {
  if (socket && socket.readyState <= 1) return
  openLiveSocket(handleLive)
    .then((ws) => {
      socket = ws
      ws.onopen = () => {
        state.live = true
      }
      ws.onclose = () => {
        state.live = false
        window.clearTimeout(reconnectTimer)
        reconnectTimer = window.setTimeout(connectLive, 2000)
      }
    })
    .catch(() => {
      state.live = false
      window.clearTimeout(reconnectTimer)
      reconnectTimer = window.setTimeout(connectLive, 3000)
    })
}

function handleLive(data: any): void {
  if (data.kind === 'hello') {
    state.live = true
    return
  }
  if (data.kind === 'message') {
    const conversation = state.conversations.find((c) => c.id === data.conversation_id)
    if (conversation) conversation.last_message = data.message
    messageListeners.forEach((fn) => fn(data))
    return
  }
  if (data.kind === 'event') {
    state.events.unshift(data.event)
    if (state.events.length > 500) state.events.pop()
    eventListeners.forEach((fn) => fn(data.event))
    return
  }
  if (data.kind === 'frame') {
    state.frames.unshift(data.frame)
    if (state.frames.length > 500) state.frames.pop()
    frameListeners.forEach((fn) => fn(data.frame))
    return
  }
  if (data.kind === 'gateway') {
    refreshStatus()
  }
}

export async function loadFrames(params: Record<string, string | number> = {}): Promise<any[]> {
  const query = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== '' && value !== undefined) query.set(key, String(value))
  }
  const data = await api.get<{ items: any[] }>(`/_mqs/debug/frames?${query.toString()}`)
  return data.items
}

export async function loadEvents(params: Record<string, string | number> = {}): Promise<any[]> {
  const query = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== '' && value !== undefined) query.set(key, String(value))
  }
  const data = await api.get<{ items: any[] }>(`/_mqs/debug/events?${query.toString()}`)
  return data.items
}
