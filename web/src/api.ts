export class ApiError extends Error {
  status: number
  code: number
  constructor(status: number, code: number, message: string) {
    super(message)
    this.status = status
    this.code = code
  }
}

const TOKEN_KEY = 'mqs-token'

export function getToken(): string {
  return sessionStorage.getItem(TOKEN_KEY) || ''
}

export function setToken(value: string): void {
  if (value) sessionStorage.setItem(TOKEN_KEY, value)
  else sessionStorage.removeItem(TOKEN_KEY)
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers)
  if (init.body && !headers.has('Content-Type')) headers.set('Content-Type', 'application/json')
  const token = getToken()
  if (token) headers.set('X-MQS-Token', token)
  const response = await fetch(path, { ...init, headers, credentials: 'include' })
  if (response.status === 204) return undefined as T
  const text = await response.text()
  let body: any = null
  try {
    body = text ? JSON.parse(text) : null
  } catch {
    body = { code: response.status, message: text.slice(0, 300) }
  }
  if (!response.ok) {
    throw new ApiError(response.status, body?.code ?? response.status, body?.message ?? response.statusText)
  }
  return body as T
}

export const api = {
  get: <T>(path: string) => request<T>(path),
  post: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: 'POST', body: body === undefined ? undefined : JSON.stringify(body) }),
  patch: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: 'PATCH', body: body === undefined ? undefined : JSON.stringify(body) }),
  put: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: 'PUT', body: body === undefined ? undefined : JSON.stringify(body) }),
  del: <T>(path: string) => request<T>(path, { method: 'DELETE' }),
  upload: <T>(path: string, file: File): Promise<T> => {
    const body = new FormData()
    body.append('file', file)
    const headers = new Headers()
    const token = getToken()
    if (token) headers.set('X-MQS-Token', token)
    return fetch(path, { method: 'POST', body, headers, credentials: 'include' }).then(async (response) => {
      const text = await response.text()
      const data = text ? JSON.parse(text) : null
      if (!response.ok) throw new ApiError(response.status, data?.code ?? response.status, data?.message ?? response.statusText)
      return data as T
    })
  },
}

/** Exchange the one-time nonce in the URL fragment for an HttpOnly cookie. */
export async function exchangeNonce(): Promise<boolean> {
  const match = location.hash.match(/(?:^#|&)t=([^&]+)/)
  if (!match) return false
  try {
    await api.post('/_mqs/session', { nonce: decodeURIComponent(match[1]) })
    history.replaceState(null, '', location.pathname + location.search)
    return true
  } catch {
    history.replaceState(null, '', location.pathname + location.search)
    return false
  }
}

export async function authCheck(): Promise<boolean> {
  try {
    await api.get('/_mqs/auth-check')
    return true
  } catch {
    return false
  }
}

export async function openLiveSocket(onMessage: (data: any) => void): Promise<WebSocket> {
  const { ticket } = await api.post<{ ticket: string }>('/_mqs/ws-ticket')
  const protocol = location.protocol === 'https:' ? 'wss' : 'ws'
  const socket = new WebSocket(`${protocol}://${location.host}/_mqs/ws?ticket=${encodeURIComponent(ticket)}`)
  socket.onmessage = (event) => {
    try {
      onMessage(JSON.parse(event.data))
    } catch {
      /* ignore malformed frame */
    }
  }
  return socket
}

export function formatTime(ts: number | string | undefined): string {
  if (!ts) return ''
  const date = typeof ts === 'number' ? new Date(ts) : new Date(ts)
  if (Number.isNaN(date.getTime())) return String(ts)
  return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
}

export function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KiB`
  return `${(size / 1024 / 1024).toFixed(1)} MiB`
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  if (ms < 60000) return `${(ms / 1000).toFixed(0)}s`
  if (ms < 3600000) return `${(ms / 60000).toFixed(0)}m`
  return `${(ms / 3600000).toFixed(1)}h`
}
