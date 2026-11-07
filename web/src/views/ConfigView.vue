<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { api } from '../api'
import { patchSettings, refreshAll, refreshWorld, state, toast } from '../store'

const tab = ref<'users' | 'groups' | 'bots' | 'world' | 'rules'>('users')
const errorText = ref('')

const userModal = ref(false)
const userForm = reactive({ nickname: '', random: true, actor_type: 'human', behavior: 'normal', seed: '' as string | number })

const groupModal = ref(false)
const groupForm = reactive({ name: '', owner_id: '', random_members: 3, seed: '' as string | number, bot_ids: [] as string[] })

const botModal = ref(false)
const botForm = reactive({ app_id: '', name: '', scopes: 'group_message, c2c_message, media', delivery: 'ws', random: false, whitelisted: false })
const connectOpen = ref(false)
const connectBot = ref<any>(null)
const connectToken = ref('')
const connectError = ref('')

const memberModal = ref(false)
const memberGroup = ref<any>(null)
const members = ref<any[]>([])
const addUserId = ref('')
const addRole = ref<'member' | 'admin' | 'owner'>('member')

const randomCount = ref(5)
const rules = ref<any[]>([])
const ruleForm = reactive({ event_type: 'group_at_message_create', content: '', user_id: '', text: '', delay_ms: 0 })
const randomSeed = ref('' as string | number)
const randomBehavior = ref('silent')

const settings = computed(() => state.settings)
const worldUsers = computed(() => state.users)
const humanUsers = computed(() => state.users.filter((u: any) => u.actor_type === 'human'))

onMounted(async () => {
  await refreshWorld()
  await loadRules()
})

function fail(error: unknown) {
  errorText.value = error instanceof Error ? error.message : String(error)
  toast(errorText.value)
}

async function createUser() {
  try {
    const payload: any = {
      nickname: userForm.nickname || null,
      actor_type: userForm.actor_type,
      behavior: userForm.behavior,
    }
    if (userForm.seed !== '') payload.seed = Number(userForm.seed)
    if (userForm.random) {
      await api.post('/_mqs/users/random', { count: 1, seed: payload.seed ?? null, behavior: userForm.behavior, actor_type: userForm.actor_type })
    } else {
      await api.post('/_mqs/users', payload)
    }
    userModal.value = false
    userForm.nickname = ''
    await refreshWorld()
    toast('用户已创建')
  } catch (error) {
    fail(error)
  }
}

async function randomUsers(count: number, behavior: string, groupId?: string) {
  try {
    const seed = randomSeed.value === '' ? null : Number(randomSeed.value)
    await api.post('/_mqs/users/random', { count, seed, behavior, group_id: groupId ?? null })
    await refreshWorld()
    if (memberGroup.value) await loadMembers(memberGroup.value.id)
    toast(`已生成 ${count} 个用户`)
  } catch (error) {
    fail(error)
  }
}

async function deleteUser(id: string) {
  if (!confirm(`删除用户 ${id}？`)) return
  try {
    await api.del(`/_mqs/users/${id}`)
    await refreshWorld()
  } catch (error) {
    fail(error)
  }
}

async function updateUser(id: string, patch: Record<string, unknown>) {
  try {
    await api.patch(`/_mqs/users/${id}`, patch)
    await refreshWorld()
  } catch (error) {
    fail(error)
  }
}

async function createGroup() {
  try {
    await api.post('/_mqs/groups', {
      name: groupForm.name || null,
      owner_id: groupForm.owner_id || null,
      random_members: Number(groupForm.random_members) || 0,
      seed: groupForm.seed === '' ? null : Number(groupForm.seed),
      bot_ids: groupForm.bot_ids,
    })
    groupModal.value = false
    groupForm.name = ''
    groupForm.owner_id = ''
    groupForm.bot_ids = []
    await refreshWorld()
    toast('群已创建')
  } catch (error) {
    fail(error)
  }
}

async function deleteGroup(id: string) {
  if (!confirm(`删除群 ${id}？这会同时删除该群的会话。`)) return
  try {
    await api.del(`/_mqs/groups/${id}`)
    await refreshWorld()
  } catch (error) {
    fail(error)
  }
}

async function openMembers(group: any) {
  memberGroup.value = group
  memberModal.value = true
  addUserId.value = humanUsers.value[0]?.id || ''
  await loadMembers(group.id)
}

async function loadMembers(groupId: string) {
  members.value = await api.get<any[]>(`/_mqs/groups/${groupId}/members`)
}

async function addMember() {
  if (!memberGroup.value || !addUserId.value) return
  try {
    await api.post(`/_mqs/groups/${memberGroup.value.id}/members`, { user_id: addUserId.value, role: addRole.value })
    await loadMembers(memberGroup.value.id)
    toast('成员已加入')
  } catch (error) {
    fail(error)
  }
}

async function setRole(userId: string, role: string) {
  try {
    await api.patch(`/_mqs/groups/${memberGroup.value.id}/members/${userId}`, { role })
    await loadMembers(memberGroup.value.id)
  } catch (error) {
    fail(error)
  }
}

async function removeMember(userId: string) {
  try {
    await api.del(`/_mqs/groups/${memberGroup.value.id}/members/${userId}`)
    await loadMembers(memberGroup.value.id)
    await refreshWorld()
  } catch (error) {
    fail(error)
  }
}

async function addBotToGroup(botId: string) {
  try {
    await api.post(`/_mqs/groups/${memberGroup.value.id}/bots/${botId}`)
    await refreshWorld()
    toast('Bot 已加入群')
  } catch (error) {
    fail(error)
  }
}

async function removeBotFromGroup(botId: string) {
  try {
    await api.del(`/_mqs/groups/${memberGroup.value.id}/bots/${botId}`)
    await refreshWorld()
  } catch (error) {
    fail(error)
  }
}

async function loadRules() {
  try {
    rules.value = await api.get<any[]>('/_mqs/rules')
  } catch {
    rules.value = []
  }
}

async function createRule() {
  try {
    await api.post('/_mqs/rules', {
      name: `${ruleForm.event_type} -> user`,
      event_type: ruleForm.event_type,
      condition: ruleForm.content ? { content_contains: ruleForm.content } : {},
      action: { type: 'send_as_user', user_id: ruleForm.user_id, text: ruleForm.text || '{content}' },
      delay_ms: Number(ruleForm.delay_ms) || 0,
    })
    await loadRules()
    toast('规则已创建')
  } catch (error) {
    fail(error)
  }
}

async function deleteRule(id: string) {
  try {
    await api.del(`/_mqs/rules/${id}`)
    await loadRules()
  } catch (error) {
    fail(error)
  }
}

async function addRandomAdmins() {
  if (!memberGroup.value) return
  try {
    const users = await api.post<any[]>('/_mqs/users/random', { count: 3, behavior: 'chatty' })
    for (const user of users) {
      await api.post(`/_mqs/groups/${memberGroup.value.id}/members`, { user_id: user.id, role: 'admin' })
    }
    await loadMembers(memberGroup.value.id)
    await refreshWorld()
    toast('已加入 3 个管理员')
  } catch (error) {
    fail(error)
  }
}

async function createBot() {
  try {
    const created = await api.post<any>('/_mqs/bots', {
      app_id: botForm.random ? null : botForm.app_id || null,
      name: botForm.random ? null : botForm.name || null,
      scopes: botForm.random ? [] : botForm.scopes.split(',').map((s) => s.trim()).filter(Boolean),
      delivery: botForm.delivery,
      random: botForm.random,
      whitelisted: botForm.whitelisted,
    })
    botModal.value = false
    botForm.app_id = ''
    botForm.name = ''
    await refreshWorld()
    toast('Bot 已创建')
    await openConnect(created)
  } catch (error) {
    fail(error)
  }
}

async function deleteBot(id: string) {
  if (!confirm(`删除 Bot ${id}？`)) return
  try {
    await api.del(`/_mqs/bots/${id}`)
    await refreshWorld()
  } catch (error) {
    fail(error)
  }
}

async function editScopes(bot: any) {
  const next = prompt('能力集（逗号分隔）', (bot.scopes || []).join(', '))
  if (next === null) return
  try {
    await api.patch(`/_mqs/bots/${bot.id}`, { scopes: next.split(',').map((s) => s.trim()).filter(Boolean) })
    await refreshWorld()
    toast('能力集已更新')
  } catch (error) {
    fail(error)
  }
}

async function openConnect(bot: any) {
  connectBot.value = bot
  connectOpen.value = true
  connectToken.value = ''
  connectError.value = ''
  try {
    const data = await api.post<any>(`/_mqs/bots/${bot.id}/token`)
    connectToken.value = data.access_token
  } catch (error) {
    connectError.value = error instanceof Error ? error.message : String(error)
  }
}

async function toggleWhitelist(bot: any) {
  try {
    await api.patch(`/_mqs/bots/${bot.id}`, { whitelisted: !bot.whitelisted })
    await refreshWorld()
    toast(bot.whitelisted ? '已取消接口白名单' : '已加入接口白名单')
  } catch (error) {
    fail(error)
  }
}

const nonebotIntents = {
  guilds: false,
  guild_members: false,
  guild_messages: false,
  guild_message_reactions: false,
  direct_message: false,
  open_forum_event: false,
  audio_live_member: false,
  group_members: true,
  c2c_group_at_messages: true,
  interaction: false,
  message_audit: true,
  forum_event: false,
  audio_action: false,
  at_messages: false,
}

const baseUrl = computed(() => (state.status?.base_url || 'http://127.0.0.1:5700').replace(/\/$/, ''))
const wsUrl = computed(() => state.status?.ws_url || 'ws://127.0.0.1:5700/websocket')
const nonebotBots = computed(() => {
  if (!connectBot.value) return '[]'
  return JSON.stringify([
    {
      id: connectBot.value.app_id,
      token: connectToken.value,
      secret: connectBot.value.secret,
      intent: nonebotIntents,
      use_websocket: true,
    },
  ])
})
const nonebotEnv = computed(
  () =>
    `QQ_API_BASE=${baseUrl.value}/\n` +
    `QQ_AUTH_BASE=${baseUrl.value}/app/getAppAccessToken\n` +
    `QQ_CUSTOM_GATEWAY_URL=${wsUrl.value}\n` +
    `QQ_BOTS=${nonebotBots.value}`,
)

async function copyText(text: string, label = '已复制') {
  try {
    await navigator.clipboard.writeText(text)
    toast(label)
  } catch {
    toast('复制失败')
  }
}

async function rotateSecret(id: string) {
  try {
    const data = await api.post<any>(`/_mqs/bots/${id}/rotate-secret`)
    await refreshWorld()
    toast(`新 secret: ${data.secret}`)
  } catch (error) {
    fail(error)
  }
}

async function saveSetting(key: string, value: unknown) {
  try {
    await patchSettings({ [key]: value })
  } catch (error) {
    fail(error)
  }
}

async function rerollSeed() {
  const seed = Math.floor(Math.random() * 100000)
  randomSeed.value = seed
  await saveSetting('seed', seed)
  toast(`新 seed: ${seed}`)
}

const worldSettings = computed<any[]>(() => [
  {
    key: 'rate_mode',
    label: '限流档位（QQ 官方频率政策）',
    type: 'select',
    options: [
      { value: 'off', label: '关' },
      { value: 'unverified', label: '未认证个人' },
      { value: 'certified', label: '已认证（个人=企业）' },
    ],
  },
  { key: 'require_online', label: '发送消息要求 Bot 在线（保真开关）', type: 'switch' },
  { key: 'chat.auto_at_bot', label: '聊天页默认 @ 群内所有 Bot', type: 'switch' },
  { key: 'feature.whitelist', label: '模拟腾讯接口白名单（内邀接口）', type: 'switch' },
  { key: 'feature.expose', label: '沙箱暴露范围', type: 'select', options: [{ value: 'all', label: 'all（全部）' }, { value: 'public', label: 'public（仅公开）' }] },
  { key: 'active_quota_per_day', label: '主动消息配额（每天/会话）', type: 'number' },
  { key: 'passive_window_ms', label: '被动回复窗口（毫秒）', type: 'number' },
])
</script>

<template>
  <div>
    <div class="tabs">
      <button class="tab" :class="{ active: tab === 'users' }" @click="tab = 'users'">用户</button>
      <button class="tab" :class="{ active: tab === 'groups' }" @click="tab = 'groups'">群</button>
      <button class="tab" :class="{ active: tab === 'bots' }" @click="tab = 'bots'">Bot</button>
      <button class="tab" :class="{ active: tab === 'world' }" @click="tab = 'world'">世界设置</button>
      <button class="tab" :class="{ active: tab === 'rules' }" @click="tab = 'rules'">规则</button>
      <div class="spacer"></div>
    </div>

    <template v-if="tab === 'users'">
      <div class="row wrap" style="margin-bottom: 14px">
        <button class="btn primary" @click="userModal = true">＋ 新建用户</button>
        <div class="row" style="margin-left: auto">
          <input v-model="randomCount" class="input" type="number" min="1" style="width: 76px" />
          <input v-model="randomSeed" class="input" placeholder="seed（可选）" style="width: 140px" />
          <button class="btn" @click="randomUsers(Number(randomCount), 'silent')">加潜水的</button>
          <button class="btn" @click="randomUsers(Number(randomCount), 'chatty')">加水群怪</button>
        </div>
      </div>
      <div class="card" style="padding: 0">
        <table class="table">
          <thead><tr><th>ID</th><th>昵称</th><th>类型</th><th>行为</th><th>城市</th><th>seed</th><th></th></tr></thead>
          <tbody>
            <tr v-for="user in state.users" :key="user.id">
              <td class="mono small">{{ user.id }}</td>
              <td><span class="row"><img class="avatar sm" :src="user.avatar" alt="" />{{ user.nickname }}</span></td>
              <td><span class="tag" :class="user.actor_type === 'human' ? 'info' : ''">{{ user.actor_type }}</span></td>
              <td>
                <select class="select" style="width: 110px" :value="user.behavior" @change="updateUser(user.id, { behavior: ($event.target as HTMLSelectElement).value })">
                  <option value="silent">silent</option>
                  <option value="normal">normal</option>
                  <option value="chatty">chatty</option>
                  <option value="spammer">spammer</option>
                </select>
              </td>
              <td class="small muted">{{ user.city }}</td>
              <td class="mono small">{{ user.seed }}</td>
              <td class="actions"><button class="btn small danger" @click="deleteUser(user.id)">删除</button></td>
            </tr>
            <tr v-if="!state.users.length"><td colspan="7" class="muted">还没有用户</td></tr>
          </tbody>
        </table>
      </div>
    </template>

    <template v-else-if="tab === 'groups'">
      <div class="row" style="margin-bottom: 14px">
        <button class="btn primary" @click="groupModal = true">＋ 新建群</button>
        <span class="muted small">新建群会自动把所有 Bot 拉进来，也可以稍后单独管理。</span>
      </div>
      <div class="card" style="padding: 0">
        <table class="table">
          <thead><tr><th>ID</th><th>群名</th><th>成员</th><th>Bot</th><th>会话</th><th></th></tr></thead>
          <tbody>
            <tr v-for="group in state.groups" :key="group.id">
              <td class="mono small">{{ group.id }}</td>
              <td>{{ group.name }}</td>
              <td>{{ group.member_count }}</td>
              <td class="small">{{ group.bot_ids.join(', ') || '—' }}</td>
              <td class="mono small">{{ group.conversation_id }}</td>
              <td class="actions">
                <button class="btn small" @click="openMembers(group)">成员 / Bot</button>
                <button class="btn small danger" @click="deleteGroup(group.id)">删除</button>
              </td>
            </tr>
            <tr v-if="!state.groups.length"><td colspan="6" class="muted">还没有群</td></tr>
          </tbody>
        </table>
      </div>
    </template>

    <template v-else-if="tab === 'bots'">
      <div class="row" style="margin-bottom: 14px">
        <button class="btn primary" @click="botModal = true">＋ 新建 Bot</button>
        <span class="muted small">Bot 连上网关后状态会变成“在线”。</span>
      </div>
      <div class="card" style="padding: 0">
        <table class="table">
          <thead><tr><th>app_id</th><th>名称</th><th>secret</th><th>scopes</th><th>状态</th><th></th></tr></thead>
          <tbody>
            <tr v-for="bot in state.bots" :key="bot.id">
              <td class="mono small">{{ bot.app_id }}</td>
              <td><span class="row"><img class="avatar sm" :src="bot.avatar" alt="" />{{ bot.name }}</span></td>
              <td class="mono small">{{ bot.secret }}</td>
              <td class="small">{{ bot.scopes.join(', ') }}</td>
              <td>
                <span class="tag" :class="bot.online ? 'ok' : ''">{{ bot.online ? '在线' : '离线' }}</span>
                <span class="tag" :class="bot.whitelisted ? 'info' : ''">{{ bot.whitelisted ? '内邀白名单' : '公开接口' }}</span>
              </td>
              <td class="actions">
                <button class="btn small primary" @click="openConnect(bot)">接入信息</button>
                <button class="btn small" :class="bot.whitelisted ? '' : 'ghost'" @click="toggleWhitelist(bot)">
                  {{ bot.whitelisted ? '白名单' : '未白名单' }}
                </button>
                <button class="btn small" @click="editScopes(bot)">权限</button>
                <button class="btn small" @click="rotateSecret(bot.id)">重置 secret</button>
                <button class="btn small danger" @click="deleteBot(bot.id)">删除</button>
              </td>
            </tr>
            <tr v-if="!state.bots.length"><td colspan="6" class="muted">还没有 Bot</td></tr>
          </tbody>
        </table>
      </div>
    </template>

    <template v-else-if="tab === 'rules'">
      <div class="grid c2">
        <div class="card">
          <div class="card-title">规则引擎 <span class="muted">事件 → 延迟 → 以用户身份回一句</span></div>
          <div class="grid" style="gap: 10px">
            <label class="field">触发事件
              <select v-model="ruleForm.event_type" class="select">
                <option value="group_at_message_create">group_at_message_create</option>
                <option value="c2c_message_create">c2c_message_create</option>
              </select>
            </label>
            <label class="field">内容包含（留空匹配所有）<input v-model="ruleForm.content" class="input" placeholder="例如 ping" /></label>
            <label class="field">以哪个用户回复
              <select v-model="ruleForm.user_id" class="select">
                <option value="">选择用户</option>
                <option v-for="user in humanUsers" :key="user.id" :value="user.id">{{ user.nickname }}（{{ user.id }}）</option>
              </select>
            </label>
            <label class="field">回复内容<input v-model="ruleForm.text" class="input" placeholder="{content} 表示原消息" /></label>
            <label class="field">延迟（毫秒）<input v-model="ruleForm.delay_ms" class="input" type="number" min="0" /></label>
            <button class="btn primary" :disabled="!ruleForm.user_id" @click="createRule">创建规则</button>
          </div>
        </div>
        <div class="card">
          <div class="card-title">已启用规则</div>
          <table class="table">
            <thead><tr><th>事件</th><th>条件</th><th>动作</th><th>延迟</th><th></th></tr></thead>
            <tbody>
              <tr v-for="rule in rules" :key="rule.id">
                <td class="small">{{ rule.event_type }}</td>
                <td class="small">{{ rule.condition?.content_contains || '任意' }}</td>
                <td class="small">{{ rule.action?.text }} → {{ rule.action?.user_id }}</td>
                <td class="small">{{ rule.delay_ms }}ms</td>
                <td class="actions"><button class="btn small danger" @click="deleteRule(rule.id)">删除</button></td>
              </tr>
              <tr v-if="!rules.length"><td colspan="5" class="muted">还没有规则</td></tr>
            </tbody>
          </table>
        </div>
      </div>
    </template>

    <template v-else>
      <div class="grid c2">
        <div class="card">
          <div class="card-title">行为开关</div>
          <div v-for="item in worldSettings" :key="item.key" class="kv">
            <span class="k">{{ item.label }}</span>
            <span class="v">
              <select v-if="item.type === 'select'" class="select" style="width: 150px" :value="settings[item.key]" @change="saveSetting(item.key, ($event.target as HTMLSelectElement).value)">
                <option v-for="option in item.options" :key="option.value" :value="option.value">{{ option.label }}</option>
              </select>
              <label v-else-if="item.type === 'switch'" class="switch">
                <input type="checkbox" :checked="!!settings[item.key]" @change="saveSetting(item.key, ($event.target as HTMLInputElement).checked)" />
                <span class="track"><span class="thumb"></span></span>
              </label>
              <input v-else class="input" style="width: 130px" type="number" :value="settings[item.key]" @change="saveSetting(item.key, Number(($event.target as HTMLInputElement).value))" />
            </span>
          </div>
          <p class="small faint" style="margin-top: 12px">
            “白名单”模拟腾讯侧的 IP / 权限校验；“限流”是独立开关。两者都不影响控制 API。
          </p>
        </div>

        <div class="card">
          <div class="card-title">随机数据</div>
          <div class="kv"><span class="k">当前 seed</span><span class="v">{{ settings.seed }}</span></div>
          <div class="kv"><span class="k">随机重掷</span><span class="v"><button class="btn small" @click="rerollSeed">换一个 seed</button></span></div>
          <div class="kv"><span class="k">最近生成</span><span class="v">{{ state.users.length }} 用户 / {{ state.groups.length }} 群</span></div>
          <div class="row wrap" style="margin-top: 12px">
            <button class="btn small" @click="randomUsers(10, 'silent')">加 10 个潜水的</button>
            <button class="btn small" @click="randomUsers(3, 'chatty')">加 3 个话痨</button>
            <button class="btn small" @click="randomUsers(1, 'spammer')">加 1 个刷屏怪</button>
          </div>
          <p class="small faint" style="margin-top: 12px">
            相同 seed 会生成相同昵称/城市/行为序列；测试脚本里应固定 seed。
          </p>
        </div>
      </div>
    </template>

    <!-- user modal -->
    <div v-if="userModal" class="modal-mask" @click.self="userModal = false">
      <div class="modal">
        <h3>新建用户</h3>
        <div class="grid" style="gap: 12px">
          <label class="field">昵称<input v-model="userForm.nickname" class="input" placeholder="留空则随机" /></label>
          <label class="field">类型
            <select v-model="userForm.actor_type" class="select">
              <option value="human">human（可操作 / 可切换账号）</option>
              <option value="npc">npc（群内不可操作用户）</option>
            </select>
          </label>
          <label class="field">行为
            <select v-model="userForm.behavior" class="select">
              <option value="silent">silent</option><option value="normal">normal</option>
              <option value="chatty">chatty</option><option value="spammer">spammer</option>
            </select>
          </label>
          <label class="field">seed（可选）<input v-model="userForm.seed" class="input" type="number" placeholder="留空用全局 seed" /></label>
          <label class="switch"><input v-model="userForm.random" type="checkbox" /><span class="track"><span class="thumb"></span></span> 随机生成资料</label>
        </div>
        <div class="modal-actions">
          <button class="btn ghost" @click="userModal = false">取消</button>
          <button class="btn primary" @click="createUser">创建</button>
        </div>
      </div>
    </div>

    <!-- group modal -->
    <div v-if="groupModal" class="modal-mask" @click.self="groupModal = false">
      <div class="modal">
        <h3>新建群</h3>
        <div class="grid" style="gap: 12px">
          <label class="field">群名<input v-model="groupForm.name" class="input" placeholder="留空则随机" /></label>
          <label class="field">群主
            <select v-model="groupForm.owner_id" class="select">
              <option value="">默认第一个 human 用户</option>
              <option v-for="user in humanUsers" :key="user.id" :value="user.id">{{ user.nickname }} ({{ user.id }})</option>
            </select>
          </label>
          <label class="field">随机成员数<input v-model="groupForm.random_members" class="input" type="number" min="0" /></label>
          <div class="field">加入的 Bot
            <div class="row wrap" style="margin-top: 6px">
              <label v-for="bot in state.bots" :key="bot.id" class="row small" style="gap: 5px">
                <input v-model="groupForm.bot_ids" type="checkbox" :value="bot.id" /> {{ bot.name }}
              </label>
            </div>
          </div>
        </div>
        <div class="modal-actions">
          <button class="btn ghost" @click="groupModal = false">取消</button>
          <button class="btn primary" @click="createGroup">创建</button>
        </div>
      </div>
    </div>

    <!-- bot modal -->
    <div v-if="botModal" class="modal-mask" @click.self="botModal = false">
      <div class="modal">
        <h3>新建 Bot</h3>
        <div class="grid" style="gap: 12px">
          <label class="field">app_id（留空自动分配）<input v-model="botForm.app_id" class="input" placeholder="例如 10001" /></label>
          <label class="field">名称<input v-model="botForm.name" class="input" placeholder="留空则随机" /></label>
          <label class="switch" style="margin-top: 4px">
            <input v-model="botForm.random" type="checkbox" /><span class="track"><span class="thumb"></span></span>
            随机名称与能力集
          </label>
          <label v-if="!botForm.random" class="field">scopes（逗号分隔）<input v-model="botForm.scopes" class="input" /></label>
          <label class="field">投递方式
            <select v-model="botForm.delivery" class="select"><option value="ws">WebSocket</option><option value="webhook">Webhook</option></select>
          </label>
          <label class="switch"><input v-model="botForm.whitelisted" type="checkbox" /><span class="track"><span class="thumb"></span></span> 加入内邀接口白名单</label>
        </div>
        <div class="modal-actions">
          <button class="btn ghost" @click="botModal = false">取消</button>
          <button class="btn primary" @click="createBot">创建</button>
        </div>
      </div>
    </div>

    <!-- member modal -->
    <div v-if="memberModal" class="modal-mask" @click.self="memberModal = false">
      <div class="modal" style="width: min(640px, calc(100vw - 32px))">
        <h3>{{ memberGroup?.name }} · 成员与 Bot</h3>
        <div class="row wrap" style="margin-bottom: 12px">
          <select v-model="addUserId" class="select" style="width: 220px">
            <option v-for="user in humanUsers" :key="user.id" :value="user.id">{{ user.nickname }} ({{ user.id }})</option>
          </select>
          <select v-model="addRole" class="select" style="width: 110px">
            <option value="member">member</option><option value="admin">admin</option><option value="owner">owner</option>
          </select>
          <button class="btn" @click="addMember">加入成员</button>
          <button class="btn ghost" @click="randomUsers(5, 'silent', memberGroup?.id)">＋5 个潜水</button>
          <button class="btn ghost" @click="addRandomAdmins">＋3 个管理员</button>
        </div>
        <div style="max-height: 300px; overflow: auto">
          <div v-for="member in members" :key="member.user_id" class="member-row">
            <img class="avatar sm" :src="member.avatar" alt="" />
            <span class="grow">{{ member.nickname }} <span class="mono small faint">{{ member.user_id }}</span></span>
            <select class="select" style="width: 100px" :value="member.role" @change="setRole(member.user_id, ($event.target as HTMLSelectElement).value)">
              <option value="member">member</option><option value="admin">admin</option><option value="owner">owner</option>
            </select>
            <button class="btn small danger" @click="removeMember(member.user_id)">移除</button>
          </div>
        </div>
        <hr style="border-color: var(--border); margin: 16px 0" />
        <div class="panel-section-title">Bot</div>
        <div v-for="bot in state.bots" :key="bot.id" class="member-row">
          <img class="avatar sm" :src="bot.avatar" alt="" />
          <span class="grow">{{ bot.name }} <span class="mono small faint">{{ bot.app_id }}</span></span>
          <span class="tag" :class="bot.online ? 'ok' : ''">{{ bot.online ? '在线' : '离线' }}</span>
          <button v-if="memberGroup?.bot_ids?.includes(bot.id)" class="btn small danger" @click="removeBotFromGroup(bot.id)">移出</button>
          <button v-else class="btn small" @click="addBotToGroup(bot.id)">加入</button>
        </div>
        <div class="modal-actions"><button class="btn primary" @click="memberModal = false">完成</button></div>
      </div>
    </div>


    <!-- connection info modal -->
    <div v-if="connectOpen && connectBot" class="modal-mask" @click.self="connectOpen = false">
      <div class="modal" style="width: min(760px, calc(100vw - 32px)); max-height: 88vh; overflow: auto">
        <h3>接入信息 · {{ connectBot.name }}</h3>
        <div class="grid c2" style="gap: 14px">
          <div class="card" style="padding: 12px">
            <div class="panel-section-title">API / 网关</div>
            <div class="kv"><span class="k">appid</span><span class="v">{{ connectBot.app_id }}</span></div>
            <div class="kv"><span class="k">secret</span><span class="v">{{ connectBot.secret }}</span></div>
            <div class="kv"><span class="k">base_url</span><span class="v">{{ baseUrl }}</span></div>
            <div class="kv"><span class="k">ws_url</span><span class="v">{{ wsUrl }}</span></div>
            <div class="kv"><span class="k">access_token</span><span class="v mono small">{{ connectToken || (connectError ? '获取失败' : '获取中…') }}</span></div>
            <div class="kv"><span class="k">scopes</span><span class="v">{{ connectBot.scopes.join(', ') }}</span></div>
            <div class="kv"><span class="k">接口白名单</span><span class="v">{{ connectBot.whitelisted ? '是（内邀接口可用）' : '否（内邀接口 11253）' }}</span></div>
          </div>
          <div class="card" style="padding: 12px">
            <div class="panel-section-title">botpy 直连</div>
            <p class="small muted">真实 botpy 需要垫片：</p>
            <pre class="mono small" style="white-space: pre-wrap; margin: 0">import mqs.compat.botpy
from botpy import Client
Client(intents=Intents(public_messages=True)).run(
    appid="{{ connectBot.app_id }}",
    secret="{{ connectBot.secret }}",
)</pre>
            <button class="btn small" style="margin-top: 8px" @click="copyText(`MQS_BASE_URL=${baseUrl}\npython examples/botpy_echo/bot.py`, 'botpy 命令已复制')">复制 botpy 命令</button>
          </div>
        </div>

        <div class="card" style="margin-top: 14px; padding: 12px">
          <div class="row" style="margin-bottom: 8px">
            <div class="panel-section-title" style="margin: 0">nonebot-adapter-qq 环境变量</div>
            <div class="spacer"></div>
            <button class="btn small primary" :disabled="!connectToken" @click="copyText(nonebotEnv, '.env 已复制')">复制全部 .env</button>
            <button class="btn small" :disabled="!connectToken" @click="copyText(nonebotBots, 'QQ_BOTS 已复制')">复制 QQ_BOTS</button>
          </div>
          <pre class="mono small" style="white-space: pre-wrap; word-break: break-all; background: var(--panel-2); padding: 10px; border-radius: 8px; margin: 0">{{ nonebotEnv }}</pre>
          <p class="small faint" style="margin: 8px 0 0">
            兼容 nonebot-adapter-qq 1.x（<code>QQ_BOTS[].token</code> 必填）与 1.7.3+（忽略多余 token 字段）。
            token 有效期 7200 秒，过期后重新打开本弹窗获取；新版 adapter 会自己用 secret 换 token。
          </p>
        </div>

        <div class="modal-actions">
          <button class="btn ghost" @click="copyText(connectToken, 'token 已复制')" :disabled="!connectToken">复制 token</button>
          <button class="btn primary" @click="connectOpen = false">完成</button>
        </div>
      </div>
    </div>
    <p v-if="errorText" class="small" style="color: var(--red); margin-top: 12px">{{ errorText }}</p>
  </div>
</template>
