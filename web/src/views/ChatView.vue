<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { api, formatTime } from '../api'
import { onLiveMessage, refreshStatus, refreshWorld, state, toast } from '../store'

const currentId = ref<string>('')
const dmTarget = ref<string | null>(null)
const messages = ref<any[]>([])
const text = ref('')
const replyTo = ref<any>(null)
const selectedBots = ref<string[]>([])
const currentUserId = ref('')
const members = ref<any[]>([])
const addMemberId = ref('')
const hud = ref('')
const loading = ref(false)
const messagePane = ref<HTMLElement | null>(null)

const newGroupModal = ref(false)
const groupForm = reactive({ name: '', members: 5, bot_ids: [] as string[] })
const switchTimer = ref<number>()

const humanUsers = computed(() => state.users.filter((u: any) => u.actor_type === 'human'))
const activeConversation = computed(() => state.conversations.find((c: any) => c.id === currentId.value))
const activeGroup = computed(() => {
  const conv = activeConversation.value
  if (!conv || conv.type !== 'group') return null
  return state.groups.find((g: any) => g.id === conv.peer_id) || null
})
const activeConversationId = computed(() => {
  if (dmTarget.value) return `dm:${dmTarget.value}:${currentUserId.value}`
  return currentId.value
})
const title = computed(() => {
  if (dmTarget.value) return `私聊 · ${state.bots.find((b) => b.id === dmTarget.value)?.name || dmTarget.value}`
  return activeConversation.value?.name || '选择一个会话'
})
const groupBots = computed(() => {
  if (!activeGroup.value) return []
  return state.bots.filter((bot) => activeGroup.value.bot_ids.includes(bot.id))
})
const currentUser = computed(() => state.users.find((u) => u.id === currentUserId.value))
const isMember = computed(() => {
  if (dmTarget.value) return true
  const conv = activeConversation.value
  if (!conv || conv.type !== 'group') return false
  return (conv.member_ids || []).includes(currentUserId.value)
})
const canSend = computed(
  () => !!activeConversationId.value && !!currentUserId.value && !loading.value && isMember.value,
)
const nonMembers = computed(() => {
  if (!activeGroup.value) return []
  const ids = new Set(members.value.map((m) => m.user_id))
  return state.users.filter((u) => !ids.has(u.id))
})

onMounted(async () => {
  await refreshWorld()
  if (!currentUserId.value && humanUsers.value.length) currentUserId.value = humanUsers.value[0].id
  if (!currentId.value) {
    const firstGroup =
      state.conversations.find((c) => c.type === 'group' && (c.member_ids || []).includes(currentUserId.value)) ||
      state.conversations.find((c) => c.type === 'group')
    if (firstGroup) await selectConversation(firstGroup.id)
  }
  window.addEventListener('keydown', onKeydown)
  offMessage = onLiveMessage(handleLiveMessage)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown)
  offMessage?.()
})

let offMessage: (() => void) | undefined

watch(activeConversationId, async (id) => {
  if (id && !dmTarget.value) await loadMessages(id)
})

watch(currentUserId, async () => {
  if (dmTarget.value) {
    await loadMessages(activeConversationId.value)
  }
})

async function selectConversation(id: string) {
  dmTarget.value = null
  currentId.value = id
  selectedBots.value = []
  replyTo.value = null
  await loadMessages(id)
  if (activeGroup.value) await loadMembers(activeGroup.value.id)
}

async function startDm(botId: string) {
  dmTarget.value = botId
  currentId.value = ''
  selectedBots.value = [botId]
  replyTo.value = null
  await loadMessages(activeConversationId.value)
}

async function loadMessages(id: string) {
  if (!id) return
  loading.value = true
  try {
    const data = await api.get<{ items: any[] }>(`/_mqs/conversations/${encodeURIComponent(id)}/messages?limit=60`)
    messages.value = data.items
    await scrollToBottom()
  } catch {
    messages.value = []
  } finally {
    loading.value = false
  }
}

async function loadMembers(groupId: string) {
  try {
    members.value = await api.get<any[]>(`/_mqs/groups/${groupId}/members`)
    addMemberId.value = nonMembers.value[0]?.id || ''
  } catch {
    members.value = []
  }
}

function handleLiveMessage(data: any) {
  if (data.conversation_id === activeConversationId.value) {
    messages.value = [...messages.value, data.message]
    scrollToBottom()
  }
}

async function scrollToBottom() {
  await nextTick()
  const pane = messagePane.value
  if (pane) pane.scrollTop = pane.scrollHeight
}

async function send() {
  const content = text.value.trim()
  if (!content || !canSend.value) return
  const conversation = activeConversationId.value
  try {
    await api.post('/_mqs/chat/send', {
      as_user: currentUserId.value,
      conversation_id: conversation,
      content,
      at_bots: selectedBots.value.length ? selectedBots.value : null,
      reply_to: replyTo.value?.msg_id || null,
    })
    text.value = ''
    replyTo.value = null
    await loadMessages(conversation)
    await refreshWorld()
    await refreshStatus()
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.ctrlKey && event.key === 'Tab') {
    event.preventDefault()
    const users = humanUsers.value
    if (!users.length) return
    const index = users.findIndex((u) => u.id === currentUserId.value)
    const next = users[(index + (event.shiftKey ? -1 : 1) + users.length) % users.length]
    switchAccount(next.id)
    return
  }
  if (event.ctrlKey && /^[1-9]$/.test(event.key)) {
    const user = humanUsers.value[Number(event.key) - 1]
    if (user) {
      event.preventDefault()
      switchAccount(user.id)
    }
  }
}

function switchAccount(id: string) {
  currentUserId.value = id
  const user = state.users.find((u) => u.id === id)
  hud.value = `切换为 ${user?.nickname || id}`
  window.clearTimeout(switchTimer.value)
  switchTimer.value = window.setTimeout(() => (hud.value = ''), 1400)
}

function toggleBot(id: string) {
  const index = selectedBots.value.indexOf(id)
  if (index >= 0) selectedBots.value.splice(index, 1)
  else selectedBots.value.push(id)
}

function quote(message: any) {
  replyTo.value = message
}

async function joinGroup() {
  if (!activeGroup.value || !currentUserId.value) return
  try {
    await api.post(`/_mqs/groups/${activeGroup.value.id}/members`, { user_id: currentUserId.value, role: 'member' })
    await refreshWorld()
    await loadMembers(activeGroup.value.id)
    toast('已加入该群')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function addMember() {
  if (!activeGroup.value || !addMemberId.value) return
  try {
    await api.post(`/_mqs/groups/${activeGroup.value.id}/members`, { user_id: addMemberId.value, role: 'member' })
    await loadMembers(activeGroup.value.id)
    await refreshWorld()
    toast('成员已加入')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function removeMember(userId: string) {
  if (!activeGroup.value) return
  try {
    await api.del(`/_mqs/groups/${activeGroup.value.id}/members/${userId}`)
    await loadMembers(activeGroup.value.id)
    await refreshWorld()
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function toggleGroupBot(botId: string) {
  if (!activeGroup.value) return
  try {
    if (activeGroup.value.bot_ids.includes(botId)) {
      await api.del(`/_mqs/groups/${activeGroup.value.id}/bots/${botId}`)
    } else {
      await api.post(`/_mqs/groups/${activeGroup.value.id}/bots/${botId}`)
    }
    await refreshWorld()
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function createGroup() {
  try {
    const group = await api.post<any>('/_mqs/groups', {
      name: groupForm.name || null,
      random_members: Number(groupForm.members) || 0,
      bot_ids: groupForm.bot_ids,
    })
    newGroupModal.value = false
    groupForm.name = ''
    await refreshWorld()
    await selectConversation(`group:${group.id}`)
    toast('群已创建')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

function openNewGroup() {
  groupForm.bot_ids = state.bots.map((b) => b.id)
  newGroupModal.value = true
}

function display(content: string): string {
  return (content || '').replace(/<@![^>]+>/g, '@').trim()
}

function msgTag(message: any): string {
  const map: Record<number, string> = { 2: 'markdown', 3: 'ark', 4: 'embed', 7: 'media' }
  return map[message.msg_type] || ''
}

function attachments(message: any): any[] {
  return message.attachments || message.raw?.attachments || []
}

function isImage(att: any): boolean {
  const mime = att.content_type || ''
  return mime.startsWith('image/')
}
</script>

<template>
  <div class="chat-layout">
    <aside class="conv-list">
      <div class="row" style="padding: 4px 4px 10px">
        <button class="btn small primary grow" @click="openNewGroup">＋ 新建群</button>
        <button class="btn small" @click="refreshWorld">↻</button>
      </div>
      <div
        v-for="conv in state.conversations"
        :key="conv.id"
        class="conv-item"
        :class="{ active: !dmTarget && currentId === conv.id }"
        @click="selectConversation(conv.id)"
      >
        <img class="avatar" :src="conv.avatar" alt="" />
        <div class="grow">
          <div class="conv-name">{{ conv.name }}</div>
          <div class="conv-preview">{{ conv.last_message ? display(conv.last_message.content) || '[非文本消息]' : '还没有消息' }}</div>
        </div>
        <span v-if="conv.type === 'group'" class="tag">{{ conv.member_count }}</span>
      </div>
      <div v-if="!state.conversations.length" class="empty">还没有会话<br />点上方新建群</div>
    </aside>

    <section class="chat-main">
      <header class="chat-head">
        <div class="grow">
          <div style="font-weight: 600">{{ title }}</div>
          <div class="small faint">
            <span v-if="dmTarget" class="mono">{{ activeConversationId }}</span>
            <span v-else-if="activeConversation" class="mono">{{ activeConversation.id }}</span>
            <span v-else>从左侧选择一个会话，或在右侧私聊 Bot</span>
          </div>
        </div>
        <span v-if="activeConversation?.type === 'group'" class="pill">群聊</span>
        <span v-if="dmTarget" class="pill">私聊</span>
        <button v-if="dmTarget" class="btn small ghost" @click="dmTarget = null">退出私聊</button>
      </header>

      <div ref="messagePane" class="messages">
        <div v-if="!messages.length" class="empty">
          {{ dmTarget ? '还没有私聊消息，发一句试试' : '还没有消息' }}
        </div>
        <div v-for="message in messages" :key="message.msg_id" class="msg" :class="{ out: message.direction === 'out' }">
          <img v-if="message.sender" class="avatar sm" :src="message.sender.avatar" alt="" />
          <div>
            <div class="msg-meta">
              <span>{{ message.sender?.name || message.sender_id }}</span>
              <span>{{ formatTime(message.sent_at) }}</span>
              <span v-if="msgTag(message)" class="tag">{{ msgTag(message) }}</span>
              <span v-if="message.recalled_at" class="tag err">已撤回</span>
            </div>
            <div class="msg-bubble" :style="message.recalled_at ? 'opacity: .55; text-decoration: line-through' : ''">
              {{ display(message.content) || '[空消息]' }}
              <div v-for="att in attachments(message)" :key="att.id || att.url" style="margin-top: 6px">
                <img v-if="isImage(att) && att.url" :src="att.url" style="max-width: 240px; border-radius: 8px" alt="" />
                <a v-else-if="att.url" :href="att.url" target="_blank" class="small mono">{{ att.filename || att.url }}</a>
              </div>
            </div>
            <div class="msg-meta">
              <button class="btn small ghost" @click="quote(message)">引用</button>
            </div>
          </div>
        </div>
      </div>

      <div class="composer">
        <div v-if="activeGroup && !isMember" class="row small" style="margin-bottom: 8px; color: var(--yellow)">
          <span>当前账号不是该群成员，无法发言。</span>
          <button class="btn small" @click="joinGroup">以 {{ currentUser?.nickname }} 加入群</button>
        </div>
        <div v-if="replyTo" class="row small" style="margin-bottom: 8px; color: var(--muted)">
          引用 {{ replyTo.msg_id }}
          <button class="btn small ghost" @click="replyTo = null">取消</button>
        </div>
        <div class="row wrap" style="margin-bottom: 8px">
          <span class="small faint">发送身份</span>
          <span class="tag info">{{ currentUser?.nickname || '未选择' }}</span>
          <span class="small faint" style="margin-left: 8px">@</span>
          <button
            v-for="bot in groupBots"
            :key="bot.id"
            class="btn small"
            :class="selectedBots.includes(bot.id) ? 'primary' : 'ghost'"
            @click="toggleBot(bot.id)"
          >
            {{ bot.name }}
          </button>
          <span v-if="!groupBots.length" class="small faint">（此会话没有 Bot）</span>
          <span v-if="groupBots.length && !selectedBots.length" class="small faint">默认按设置自动 @</span>
        </div>
        <div class="composer-row">
          <textarea
            v-model="text"
            class="textarea"
            :placeholder="dmTarget ? '私聊消息，Enter 发送，Shift+Enter 换行' : '群消息，Enter 发送，Shift+Enter 换行'"
            @keydown.enter.exact.prevent="send"
          ></textarea>
          <button class="btn primary" :disabled="!canSend || !text.trim()" @click="send">发送</button>
        </div>
      </div>
    </section>

    <aside class="side-panel">
      <div>
        <div class="panel-section-title">切换账号 <span class="faint">Ctrl+Tab / Ctrl+1..9</span></div>
        <div
          v-for="(user, index) in humanUsers"
          :key="user.id"
          class="account-item"
          :class="{ active: user.id === currentUserId }"
          @click="switchAccount(user.id)"
        >
          <img class="avatar sm" :src="user.avatar" alt="" />
          <span class="grow">{{ user.nickname }}</span>
          <span class="faint small" v-if="index < 9">Ctrl+{{ index + 1 }}</span>
        </div>
        <div v-if="!humanUsers.length" class="small faint">没有可操作账号</div>
      </div>

      <div>
        <div class="panel-section-title">私聊 Bot</div>
        <div v-for="bot in state.bots" :key="bot.id" class="account-item" @click="startDm(bot.id)">
          <img class="avatar sm" :src="bot.avatar" alt="" />
          <span class="grow">{{ bot.name }}</span>
          <span class="tag" :class="bot.online ? 'ok' : ''">{{ bot.online ? '在线' : '离线' }}</span>
        </div>
      </div>

      <div v-if="activeGroup">
        <div class="panel-section-title">群成员 · {{ members.length }}</div>
        <div class="row" style="margin-bottom: 8px">
          <select v-model="addMemberId" class="select small">
            <option v-for="user in nonMembers" :key="user.id" :value="user.id">{{ user.nickname }}</option>
          </select>
          <button class="btn small" :disabled="!addMemberId" @click="addMember">加入</button>
        </div>
        <div v-for="member in members" :key="member.user_id" class="member-row">
          <img class="avatar sm" :src="member.avatar" alt="" />
          <span class="grow" style="overflow: hidden; text-overflow: ellipsis">{{ member.nickname }}</span>
          <span class="tag">{{ member.role }}</span>
          <button class="btn small ghost" @click="removeMember(member.user_id)">×</button>
        </div>
      </div>

      <div v-if="activeGroup">
        <div class="panel-section-title">群内 Bot</div>
        <div v-for="bot in state.bots" :key="bot.id" class="member-row">
          <img class="avatar sm" :src="bot.avatar" alt="" />
          <span class="grow">{{ bot.name }}</span>
          <button class="btn small" :class="activeGroup.bot_ids.includes(bot.id) ? 'danger' : ''" @click="toggleGroupBot(bot.id)">
            {{ activeGroup.bot_ids.includes(bot.id) ? '移出' : '加入' }}
          </button>
        </div>
      </div>
    </aside>
  </div>

  <div v-if="hud" class="hud">{{ hud }}</div>

  <div v-if="newGroupModal" class="modal-mask" @click.self="newGroupModal = false">
    <div class="modal">
      <h3>新建群</h3>
      <div class="grid" style="gap: 12px">
        <label class="field">群名<input v-model="groupForm.name" class="input" placeholder="留空则随机" /></label>
        <label class="field">随机成员数<input v-model="groupForm.members" class="input" type="number" min="0" /></label>
        <div class="field">加入的 Bot
          <div class="row wrap" style="margin-top: 6px">
            <label v-for="bot in state.bots" :key="bot.id" class="row small" style="gap: 5px">
              <input v-model="groupForm.bot_ids" type="checkbox" :value="bot.id" /> {{ bot.name }}
            </label>
          </div>
        </div>
      </div>
      <div class="modal-actions">
        <button class="btn ghost" @click="newGroupModal = false">取消</button>
        <button class="btn primary" @click="createGroup">创建</button>
      </div>
    </div>
  </div>
</template>
