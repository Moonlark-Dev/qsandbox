<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { api, formatBytes } from '../api'
import { refreshWorld, state, toast } from '../store'

// 素材面板：只看「素材管理」（/v2/media-images）上传的图片。数据来自控制 API
// `/_mqs/media-images`——它是沙箱扩展，官方只有 Bot 视角的游标列表。
interface MediaImage {
  image_id: string
  bot_id: string
  token: string
  url: string
  file_name: string
  content_type: string
  size: number
  md5: string
  sha1: string
  preview_url: string
  created_at: number
  expires_at: number
  deleted_at: number | null
  expired: boolean
  deleted: boolean
  markdown: string
}

type Scope = 'valid' | 'expired' | 'all'

const items = ref<MediaImage[]>([])
const counts = reactive({ total: 0, valid: 0, expired: 0, deleted: 0 })
const limit = ref(100)
const loading = ref(false)
const busy = ref(false)
const botFilter = ref('')
const scope = ref<Scope>('valid')
const keyword = ref('')

const detail = ref<MediaImage | null>(null)
const editTarget = ref<MediaImage | null>(null)
const editForm = reactive({ file_name: '', expires_at: '' })
const lightbox = ref('')

const visible = computed(() => {
  const text = keyword.value.trim().toLowerCase()
  if (!text) return items.value
  return items.value.filter((item) =>
    [item.file_name, item.image_id, item.token, item.bot_id]
      .filter(Boolean)
      .some((value) => value.toLowerCase().includes(text)),
  )
})

function botName(id: string): string {
  const bot = state.bots.find((entry: any) => entry.id === id)
  return bot?.name || id
}

/** 面板里的「现在」跟着沙箱虚拟时钟走，别用浏览器时间。 */
function nowMs(): number {
  return state.status?.clock?.now_ms ?? Date.now()
}

function formatDateTime(seconds: number | null | undefined): string {
  if (!seconds) return '—'
  return new Date(seconds * 1000).toLocaleString('zh-CN', { hour12: false })
}

function remaining(expires: number): string {
  const diff = expires * 1000 - nowMs()
  if (diff <= 0) return '已过期'
  const days = Math.floor(diff / 86_400_000)
  if (days >= 1) return `${days} 天后过期`
  const hours = Math.floor(diff / 3_600_000)
  if (hours >= 1) return `${hours} 小时后过期`
  return `${Math.max(1, Math.floor(diff / 60_000))} 分钟后过期`
}

function statusOf(item: MediaImage): { label: string; cls: string } {
  if (item.deleted) return { label: '已删除', cls: 'err' }
  if (item.expired) return { label: '已过期', cls: 'warn' }
  return { label: '有效', cls: 'ok' }
}

function scopeLabel(value: Scope): string {
  return value === 'valid' ? '仅有效' : value === 'expired' ? '含已过期' : '全部（含已删除）'
}

function fail(error: unknown) {
  toast(error instanceof Error ? error.message : String(error))
}

async function load() {
  loading.value = true
  try {
    const query = new URLSearchParams({ include: scope.value })
    if (botFilter.value) query.set('bot_id', botFilter.value)
    const data = await api.get<any>(`/_mqs/media-images?${query.toString()}`)
    items.value = data.items || []
    Object.assign(counts, data.counts || {})
    limit.value = data.limit ?? 100
  } catch (error) {
    fail(error)
  } finally {
    loading.value = false
  }
}

function openDetail(item: MediaImage) {
  detail.value = item
}

function openEdit(item: MediaImage) {
  editTarget.value = item
  editForm.file_name = item.file_name
  editForm.expires_at = toLocalInput(item.expires_at)
}

/** `datetime-local` 只认本地时间字符串；后端存的是 Unix 秒。 */
function toLocalInput(seconds: number): string {
  const date = new Date(seconds * 1000)
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}

function extendBy(days: number) {
  const target = Math.floor((nowMs() + days * 86_400_000) / 1000)
  editForm.expires_at = toLocalInput(target)
}

async function saveEdit() {
  if (!editTarget.value) return
  busy.value = true
  try {
    const body: Record<string, unknown> = {}
    const name = editForm.file_name.trim()
    if (name && name !== editTarget.value.file_name) body.file_name = name
    const expires = editForm.expires_at ? Math.floor(new Date(editForm.expires_at).getTime() / 1000) : 0
    if (expires && expires !== editTarget.value.expires_at) body.expires_at = expires
    if (!body.file_name && !body.expires_at) {
      editTarget.value = null
      return
    }
    const updated = await api.patch<MediaImage>(`/_mqs/media-images/${editTarget.value.image_id}`, body)
    editTarget.value = null
    if (detail.value?.image_id === updated.image_id) detail.value = updated
    toast('素材已更新')
    await load()
  } catch (error) {
    fail(error)
  } finally {
    busy.value = false
  }
}

async function remove(item: MediaImage) {
  if (!confirm(`删除素材「${item.file_name}」？引用它的 Markdown 消息会变成失效引用（40035009）。`)) return
  try {
    await api.del(`/_mqs/media-images/${item.image_id}`)
    if (detail.value?.image_id === item.image_id) detail.value = null
    toast('素材已删除')
    await load()
  } catch (error) {
    fail(error)
  }
}

async function copy(text: string, message: string) {
  try {
    await navigator.clipboard.writeText(text)
    toast(message)
  } catch {
    toast('复制失败')
  }
}

watch([scope, botFilter], load)

onMounted(async () => {
  await refreshWorld()
  await load()
})
</script>

<template>
  <div class="grid" style="gap: 14px">
    <div class="grid c4">
      <div class="stat">
        <div class="stat-label">有效素材</div>
        <div class="stat-value">{{ counts.valid }}</div>
        <div class="stat-sub">每 Bot 上限 {{ limit }} 张</div>
      </div>
      <div class="stat">
        <div class="stat-label">已过期</div>
        <div class="stat-value">{{ counts.expired }}</div>
        <div class="stat-sub">最后引用 + 90 天后过期</div>
      </div>
      <div class="stat">
        <div class="stat-label">已删除</div>
        <div class="stat-value">{{ counts.deleted }}</div>
        <div class="stat-sub">软删除，预览仍可访问</div>
      </div>
      <div class="stat">
        <div class="stat-label">全部记录</div>
        <div class="stat-value">{{ counts.total }}</div>
        <div class="stat-sub">含过期与已删除</div>
      </div>
    </div>

    <div class="card">
      <div class="card-title">
        素材管理
        <span class="muted">通过 /v2/media-images 上传的图片（prepare → 预签名 PUT → complete）</span>
        <span class="spacer"></span>
        <button class="btn small ghost" :disabled="loading" @click="load">刷新</button>
      </div>

      <div class="row wrap" style="margin-bottom: 12px">
        <select v-model="botFilter" class="select" style="width: 200px" title="按 Bot 过滤">
          <option value="">全部 Bot</option>
          <option v-for="bot in state.bots" :key="bot.id" :value="bot.id">{{ bot.name }}（{{ bot.id }}）</option>
        </select>
        <select v-model="scope" class="select" style="width: 160px" title="是否包含过期/已删除">
          <option value="valid">仅有效</option>
          <option value="expired">含已过期</option>
          <option value="all">全部（含已删除）</option>
        </select>
        <input v-model="keyword" class="input grow" placeholder="搜索文件名 / image_id / token" style="max-width: 360px" />
        <span class="pill">{{ visible.length }} 条 · {{ scopeLabel(scope) }}</span>
      </div>

      <table class="table">
        <thead>
          <tr>
            <th style="width: 56px">预览</th>
            <th>文件名</th>
            <th>Bot</th>
            <th>类型</th>
            <th>大小</th>
            <th>状态</th>
            <th>创建</th>
            <th>过期</th>
            <th class="actions">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="item in visible" :key="item.image_id">
            <td>
              <img class="media-thumb" :src="item.preview_url" :alt="item.file_name" @click="lightbox = item.preview_url" />
            </td>
            <td>
              <div>{{ item.file_name }}</div>
              <div class="media-token">{{ item.image_id }}</div>
            </td>
            <td>
              <div>{{ botName(item.bot_id) }}</div>
              <div class="media-token">{{ item.bot_id }}</div>
            </td>
            <td><span class="tag">{{ item.content_type }}</span></td>
            <td class="mono small">{{ formatBytes(item.size) }}</td>
            <td><span class="tag" :class="statusOf(item).cls">{{ statusOf(item).label }}</span></td>
            <td class="small faint">{{ formatDateTime(item.created_at) }}</td>
            <td class="small faint">
              {{ formatDateTime(item.expires_at) }}
              <div v-if="!item.deleted" class="small faint">{{ remaining(item.expires_at) }}</div>
            </td>
            <td class="actions">
              <button class="btn small ghost" @click="openDetail(item)">查看</button>
              <button class="btn small" :disabled="item.deleted" @click="openEdit(item)">修改</button>
              <button class="btn small danger" :disabled="item.deleted" @click="remove(item)">删除</button>
            </td>
          </tr>
          <tr v-if="!visible.length">
            <td colspan="9" class="empty">
              {{ loading ? '加载中…' : '还没有素材。用 /v2/media-images/prepare → PUT → complete 上传，或换一个过滤条件。' }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <p class="small faint">
      素材图片与 Bot 绑定，Markdown 里用 <code class="mono">![描述](qqbot-media://image/v1/&lt;token&gt;)</code>
      引用；发送成功会把有效期顺延到「现在 + 90 天」。过期/删除后用引用发消息会返回 40035009。
      面板的「修改」与「删除」是沙箱扩展（官方没有这些接口），只改本地库。
    </p>
  </div>

  <div v-if="detail" class="modal-mask" @click.self="detail = null">
    <div class="modal raw-modal">
      <h3>素材详情</h3>
      <div class="row" style="align-items: flex-start; gap: 16px">
        <img class="media-preview" :src="detail.preview_url" :alt="detail.file_name" @click="lightbox = detail.preview_url" />
        <div class="grow">
          <div class="kv"><span class="k">文件名</span><span class="v">{{ detail.file_name }}</span></div>
          <div class="kv"><span class="k">Bot</span><span class="v">{{ botName(detail.bot_id) }} · {{ detail.bot_id }}</span></div>
          <div class="kv"><span class="k">状态</span><span class="v"><span class="tag" :class="statusOf(detail).cls">{{ statusOf(detail).label }}</span></span></div>
          <div class="kv"><span class="k">类型 / 大小</span><span class="v">{{ detail.content_type }} · {{ formatBytes(detail.size) }}</span></div>
          <div class="kv"><span class="k">创建 / 过期</span><span class="v">{{ formatDateTime(detail.created_at) }} → {{ formatDateTime(detail.expires_at) }}</span></div>
          <div v-if="detail.deleted_at" class="kv"><span class="k">删除时间</span><span class="v">{{ formatDateTime(detail.deleted_at) }}</span></div>
        </div>
      </div>

      <div class="row" style="margin-top: 14px">
        <span class="small faint">image_id</span>
        <code class="mono small grow" style="word-break: break-all">{{ detail.image_id }}</code>
        <button class="btn small ghost" @click="copy(detail.image_id, 'image_id 已复制')">复制</button>
      </div>
      <div class="row" style="margin-top: 6px">
        <span class="small faint">引用地址</span>
        <code class="mono small grow" style="word-break: break-all">{{ detail.url }}</code>
        <button class="btn small ghost" @click="copy(detail.url, '引用地址已复制')">复制</button>
      </div>
      <div class="row" style="margin-top: 6px">
        <span class="small faint">Markdown</span>
        <code class="mono small grow" style="word-break: break-all">{{ detail.markdown }}</code>
        <button class="btn small ghost" @click="copy(detail.markdown, 'Markdown 已复制')">复制</button>
      </div>
      <div class="kv" style="margin-top: 6px"><span class="k">md5 / sha1</span><span class="v">{{ detail.md5 }}<br />{{ detail.sha1 }}</span></div>

      <div class="modal-actions">
        <button class="btn ghost" @click="detail = null">关闭</button>
        <button class="btn" :disabled="detail.deleted" @click="openEdit(detail!)">修改</button>
        <button class="btn danger" :disabled="detail.deleted" @click="remove(detail!)">删除</button>
      </div>
    </div>
  </div>

  <div v-if="editTarget" class="modal-mask" @click.self="editTarget = null">
    <div class="modal">
      <h3>修改素材</h3>
      <p class="small faint" style="margin-top: -8px">
        {{ editTarget.image_id }} · 扩展名必须与 <code class="mono">{{ editTarget.content_type }}</code> 匹配。
      </p>
      <label class="field" style="margin-top: 14px">
        文件名
        <input v-model="editForm.file_name" class="input" :placeholder="editTarget.file_name" />
      </label>
      <label class="field" style="margin-top: 12px">
        过期时间（{{
          state.status?.clock?.mode === 'virtual' ? '虚拟时钟' : '真实时钟'
        }}，留空 = 不修改，改成过去的时间即过期）
        <input v-model="editForm.expires_at" class="input" type="datetime-local" />
      </label>
      <div class="row wrap" style="margin-top: 10px">
        <button class="btn small" @click="extendBy(90)">再续 90 天</button>
        <button class="btn small" @click="extendBy(1)">+1 天</button>
        <button class="btn small danger" @click="extendBy(-1)">设为已过期</button>
      </div>
      <div class="modal-actions">
        <button class="btn ghost" @click="editTarget = null">取消</button>
        <button class="btn primary" :disabled="busy" @click="saveEdit">保存</button>
      </div>
    </div>
  </div>

  <div v-if="lightbox" class="lightbox" @click="lightbox = ''">
    <img :src="lightbox" alt="" />
    <button class="btn small lightbox-close" @click="lightbox = ''">关闭</button>
  </div>
</template>
