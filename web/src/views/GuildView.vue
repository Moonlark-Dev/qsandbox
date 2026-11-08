<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import { askText } from '../dialog'
import { refreshWorld, state, toast } from '../store'

const guilds = ref<any[]>([])
const guildId = ref('')
const roles = ref<any[]>([])
const members = ref<any[]>([])
const channels = ref<any[]>([])
const dms = ref<any[]>([])
const selectedDms = ref<any | null>(null)
const dmsMessages = ref<any[]>([])
const dmsText = ref('')
const friends = ref<any[]>([])
const friendBot = ref('10000')
const friendUser = ref('')
const busy = ref(false)

const guild = computed(() => guilds.value.find((g) => g.id === guildId.value))
const guildBots = computed(() => state.bots.filter((b) => (guild.value?.bot_ids || []).includes(b.id)))
const selectedDmsSession = computed(() => dms.value.find((d) => d.dms_guild_id === selectedDms.value?.dms_guild_id))
const friendCandidates = computed(() => {
  const ids = new Set(friends.value.map((f) => f.user_id))
  return state.users.filter((u) => !ids.has(u.id))
})

onMounted(async () => {
  await refreshWorld()
  friendBot.value = state.bots[0]?.id || '10000'
  friendUser.value = state.users[0]?.id || ''
  await loadGuilds()
  await loadFriends()
  await loadDms()
})

async function loadGuilds() {
  try {
    guilds.value = await api.get<any[]>('/_mqs/guilds')
    if (!guildId.value && guilds.value.length) guildId.value = guilds.value[0].id
    if (guildId.value) await selectGuild(guildId.value)
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function selectGuild(id: string) {
  guildId.value = id
  try {
    const [g, c, r, m] = await Promise.all([
      api.get<any>(`/_mqs/guilds/${id}`),
      api.get<any[]>(`/_mqs/guilds/${id}/channels`),
      api.get<any[]>(`/_mqs/guilds/${id}/roles`),
      api.get<any[]>(`/_mqs/guilds/${id}/members`),
    ])
    guilds.value = guilds.value.map((item) => (item.id === id ? { ...item, ...g } : item))
    channels.value = c
    roles.value = r
    members.value = m
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function createChannel() {
  if (!guildId.value) return
  const name = await askText({ title: '新建子频道', label: '频道名称', value: '新频道', confirmText: '创建' })
  if (!name) return
  try {
    await api.post(`/_mqs/guilds/${guildId.value}/channels`, { name, type: 0, position: channels.value.length + 1 })
    await selectGuild(guildId.value)
    toast('子频道已创建')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function renameChannel(channel: any) {
  const name = await askText({ title: '重命名字频道', label: '频道名称', value: channel.name, confirmText: '保存' })
  if (!name) return
  try {
    await api.patch(`/_mqs/channels/${channel.id}`, { name })
    await selectGuild(guildId.value)
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function deleteChannel(channel: any) {
  if (!confirm(`删除子频道 ${channel.name}？`)) return
  try {
    await api.del(`/_mqs/channels/${channel.id}`)
    await selectGuild(guildId.value)
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function createRole() {
  if (!guildId.value) return
  const name = await askText({ title: '新建身份组', label: '身份组名称', value: '新身份组', confirmText: '创建' })
  if (!name) return
  try {
    await api.post(`/_mqs/guilds/${guildId.value}/roles`, { name, color: 0, hoist: 0 })
    await selectGuild(guildId.value)
    toast('身份组已创建')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function renameRole(role: any) {
  const name = await askText({ title: '重命名身份组', label: '身份组名称', value: role.name, confirmText: '保存' })
  if (!name) return
  try {
    await api.patch(`/_mqs/guilds/${guildId.value}/roles/${role.id}`, { name })
    await selectGuild(guildId.value)
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function deleteRole(role: any) {
  if (!confirm(`删除身份组 ${role.name}？`)) return
  try {
    await api.del(`/_mqs/guilds/${guildId.value}/roles/${role.id}`)
    await selectGuild(guildId.value)
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function assignRole(userId: string, roleId: string) {
  if (!roleId) return
  try {
    await api.put(`/_mqs/guilds/${guildId.value}/members/${userId}/roles/${roleId}`)
    await selectGuild(guildId.value)
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function removeRole(userId: string, roleId: string) {
  try {
    await api.del(`/_mqs/guilds/${guildId.value}/members/${userId}/roles/${roleId}`)
    await selectGuild(guildId.value)
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function muteMember(userId: string, seconds: number) {
  try {
    await api.patch(`/_mqs/guilds/${guildId.value}/members/${userId}/mute`, { mute_seconds: String(seconds) })
    await selectGuild(guildId.value)
    toast(seconds ? '已禁言' : '已解除禁言')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function kickMember(userId: string) {
  if (!confirm(`把 ${userId} 移出频道？`)) return
  try {
    await api.del(`/_mqs/guilds/${guildId.value}/members/${userId}`)
    await selectGuild(guildId.value)
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function muteGuild() {
  try {
    await api.patch(`/_mqs/guilds/${guildId.value}/mute`, { mute_seconds: '600' })
    await selectGuild(guildId.value)
    toast('已全员禁言 10 分钟')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function toggleProactive() {
  const bot = guildBots.value[0]
  if (!bot) return
  try {
    await api.patch(`/_mqs/bots/${bot.id}`, { allow_proactive_msg: !bot.allow_proactive_msg })
    await refreshWorld()
    toast('主动消息开关已更新')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function loadDms() {
  try {
    dms.value = await api.get<any[]>('/_mqs/dms')
    if (!selectedDms.value && dms.value.length) await selectDms(dms.value[0])
  } catch {
    dms.value = []
  }
}

async function createDms() {
  const bot = guildBots.value[0] || state.bots[0]
  const user = state.users[0]
  if (!bot || !user || !guildId.value) return
  try {
    const created = await api.post<any>('/_mqs/dms', {
      bot_id: bot.id,
      user_id: user.id,
      source_guild_id: guildId.value,
    })
    await loadDms()
    const session = dms.value.find((d) => d.dms_guild_id === created.guild_id)
    if (session) await selectDms(session)
    toast('私信会话已创建')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function selectDms(session: any) {
  selectedDms.value = session
  dmsMessages.value = (await api.get<{ items: any[] }>(`/_mqs/dms/${session.dms_guild_id}/messages`)).items
}

async function sendDms(asUser: boolean) {
  if (!selectedDmsSession.value || !dmsText.value.trim()) return
  const id = selectedDmsSession.value.dms_guild_id
  try {
    if (asUser) {
      await api.post(`/_mqs/dms/${id}/messages`, { as_user: selectedDmsSession.value.user_id, content: dmsText.value })
    } else {
      await api.post(`/_mqs/dms/${id}/bot-messages`, { content: dmsText.value })
    }
    dmsText.value = ''
    await selectDms(selectedDmsSession.value)
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function loadFriends() {
  if (!friendBot.value) return
  try {
    friends.value = await api.get<any[]>(`/_mqs/bots/${friendBot.value}/friends`)
    friendUser.value = friendCandidates.value[0]?.id || ''
  } catch {
    friends.value = []
  }
}

async function addFriend() {
  if (!friendBot.value || !friendUser.value) return
  try {
    await api.post(`/_mqs/bots/${friendBot.value}/friends/${friendUser.value}`)
    await loadFriends()
    toast('好友已添加')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function removeFriend(userId: string) {
  try {
    await api.del(`/_mqs/bots/${friendBot.value}/friends/${userId}`)
    await loadFriends()
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

function roleName(id: string): string {
  return roles.value.find((r) => r.id === id)?.name || id
}
</script>

<template>
  <div class="grid" style="gap: 14px">
    <div class="row wrap">
      <select v-model="guildId" class="select" style="width: 260px" @change="selectGuild(guildId)">
        <option v-for="item in guilds" :key="item.id" :value="item.id">{{ item.name }}（{{ item.id }}）</option>
      </select>
      <button class="btn" @click="loadGuilds">刷新</button>
      <button class="btn" @click="muteGuild">全员禁言 10 分钟</button>
      <button class="btn" @click="toggleProactive">
        主动消息：{{ guildBots[0]?.allow_proactive_msg ? '开' : '关' }}
      </button>
      <span class="small faint">频道写操作会触发 CHANNEL_* / GUILD_MEMBER_* 事件</span>
    </div>

    <div class="grid c2">
      <div class="card" style="padding: 0">
        <div class="row" style="padding: 12px 14px">
          <div class="card-title grow" style="margin: 0">子频道 · {{ channels.length }}</div>
          <button class="btn small primary" @click="createChannel">＋ 新建</button>
        </div>
        <table class="table">
          <thead><tr><th>名称</th><th>类型</th><th>排序</th><th></th></tr></thead>
          <tbody>
            <tr v-for="channel in channels" :key="channel.id">
              <td>{{ channel.name }} <span class="mono small faint">{{ channel.id }}</span></td>
              <td class="small">{{ channel.type === 0 ? '文字' : channel.type === 2 ? '语音' : channel.type }}</td>
              <td class="small">{{ channel.position }}</td>
              <td class="actions">
                <button class="btn small" @click="renameChannel(channel)">改名</button>
                <button class="btn small danger" @click="deleteChannel(channel)">删除</button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <div class="card" style="padding: 0">
        <div class="row" style="padding: 12px 14px">
          <div class="card-title grow" style="margin: 0">身份组 · {{ roles.length }}</div>
          <button class="btn small primary" @click="createRole">＋ 新建</button>
        </div>
        <table class="table">
          <thead><tr><th>名称</th><th>成员</th><th>hoist</th><th></th></tr></thead>
          <tbody>
            <tr v-for="role in roles" :key="role.id">
              <td>{{ role.name }} <span class="mono small faint">{{ role.id }}</span></td>
              <td class="small">{{ role.number ?? role.member_ids?.length ?? 0 }}</td>
              <td class="small">{{ role.hoist ? '是' : '否' }}</td>
              <td class="actions">
                <button class="btn small" @click="renameRole(role)">改名</button>
                <button class="btn small danger" @click="deleteRole(role)">删除</button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <div class="card" style="padding: 0">
      <div class="row" style="padding: 12px 14px">
        <div class="card-title grow" style="margin: 0">频道成员 · {{ members.length }}</div>
      </div>
      <table class="table">
        <thead><tr><th>成员</th><th>类型</th><th>身份组</th><th>禁言</th><th></th></tr></thead>
        <tbody>
          <tr v-for="member in members" :key="member.user_id">
            <td>{{ member.nickname || member.user_id }} <span class="mono small faint">{{ member.user_id }}</span></td>
            <td class="small">{{ member.member_type }}</td>
            <td>
              <span v-for="rid in member.roles" :key="rid" class="tag" style="margin-right: 4px">
                {{ roleName(rid) }}
                <button class="btn small ghost" title="移出身份组" @click="removeRole(member.user_id, rid)">×</button>
              </span>
              <select class="select small" style="width: 130px" @change="assignRole(member.user_id, ($event.target as HTMLSelectElement).value); ($event.target as HTMLSelectElement).value = ''">
                <option value="">＋ 身份组</option>
                <option v-for="role in roles" :key="role.id" :value="role.id">{{ role.name }}</option>
              </select>
            </td>
            <td class="small">{{ member.mute_until ? new Date(member.mute_until).toLocaleTimeString('zh-CN') : '—' }}</td>
            <td class="actions">
              <button class="btn small" @click="muteMember(member.user_id, 600)">禁言</button>
              <button class="btn small ghost" @click="muteMember(member.user_id, 0)">解除</button>
              <button class="btn small danger" @click="kickMember(member.user_id)">移出</button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <div class="grid c2">
      <div class="card">
        <div class="row" style="margin-bottom: 10px">
          <div class="card-title grow" style="margin: 0">频道私信</div>
          <button class="btn small primary" @click="createDms">＋ 新建会话</button>
        </div>
        <div class="row" style="margin-bottom: 8px">
          <select class="select grow" :value="selectedDmsSession?.dms_guild_id || ''" @change="selectDms(dms.find((d) => d.dms_guild_id === ($event.target as HTMLSelectElement).value))">
            <option v-for="session in dms" :key="session.dms_guild_id" :value="session.dms_guild_id">
              {{ session.user_name }} ↔ {{ session.bot_name }}
            </option>
          </select>
        </div>
        <div class="messages" style="max-height: 240px; min-height: 120px; background: var(--panel-2); border-radius: 8px; padding: 8px">
          <div v-for="message in dmsMessages" :key="message.msg_id" class="small" style="margin-bottom: 6px">
            <span class="mono faint">{{ message.sender_type === 'bot' ? 'BOT' : 'USER' }}</span>
            {{ message.content }}
            <img v-for="att in message.attachments || []" :key="att.url" :src="att.url" style="max-width: 90px; border-radius: 4px" alt="" />
          </div>
          <div v-if="!dmsMessages.length" class="small faint">还没有私信消息</div>
        </div>
        <div class="row" style="margin-top: 8px">
          <input v-model="dmsText" class="input grow" placeholder="私信内容" @keyup.enter="sendDms(true)" />
          <button class="btn small" @click="sendDms(true)">以用户发</button>
          <button class="btn small primary" @click="sendDms(false)">以 Bot 发</button>
        </div>
      </div>

      <div class="card">
        <div class="card-title">好友关系</div>
        <div class="row" style="margin-bottom: 8px">
          <select v-model="friendBot" class="select" style="width: 140px" @change="loadFriends">
            <option v-for="bot in state.bots" :key="bot.id" :value="bot.id">{{ bot.name }}</option>
          </select>
          <select v-model="friendUser" class="select grow">
            <option v-for="user in friendCandidates" :key="user.id" :value="user.id">{{ user.nickname }}</option>
          </select>
          <button class="btn small" :disabled="!friendUser" @click="addFriend">添加</button>
        </div>
        <div v-for="friend in friends" :key="friend.user_id" class="member-row">
          <span class="grow">{{ friend.nickname }}</span>
          <span class="mono small faint">{{ friend.openid.slice(0, 10) }}…</span>
          <button class="btn small danger" @click="removeFriend(friend.user_id)">删除</button>
        </div>
        <div v-if="!friends.length" class="small faint">还没有好友；添加会触发 FRIEND_ADD 事件</div>
        <p class="small faint" style="margin-top: 10px">
          主动消息开关会同时影响群聊、单聊、频道和频道私信的主动发送；被动回复不受影响。
        </p>
      </div>
    </div>
  </div>
</template>
