<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { api, formatBytes, formatDuration } from '../api'
import { patchSettings, refreshSettings, refreshStatus, state, toast } from '../store'

const emit = defineEmits<{ close: [] }>()

const form = reactive({
  theme: 'auto',
  language: 'zh-CN',
  log_level: 'info',
  telemetry: false,
  autostart: false,
  feature_whitelist: true,
  feature_expose: 'all',
})

const desktop = computed(() => (window as any).mqsDesktop as any)
const status = computed(() => state.status)
const exporting = ref(false)
const profiles = ref<any[]>([])
const currentProfile = ref('')
const newProfile = ref('')
const switching = ref(false)

onMounted(async () => {
  await refreshSettings()
  sync()
  await loadProfiles()
})

function sync() {
  form.theme = state.settings.theme || 'auto'
  form.language = state.settings.language || 'zh-CN'
  form.log_level = state.settings.log_level || 'info'
  form.telemetry = !!state.settings.telemetry
  form.autostart = !!state.settings.autostart
  form.feature_whitelist = !!state.settings['feature.whitelist']
  form.feature_expose = state.settings['feature.expose'] || 'all'
}

async function loadProfiles() {
  if (!desktop.value?.listProfiles) return
  try {
    const data = await desktop.value.listProfiles()
    profiles.value = data.profiles || []
    currentProfile.value = data.current || state.status?.profile || ''
  } catch {
    profiles.value = []
  }
}

async function switchTo(name: string) {
  if (!desktop.value?.switchProfile || !name || name === currentProfile.value) return
  switching.value = true
  try {
    const result = await desktop.value.switchProfile(name)
    if (!result?.ok) toast(result?.error || '切换失败')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  } finally {
    switching.value = false
  }
}

async function createAndSwitch() {
  const name = newProfile.value.trim()
  if (!name) return
  await switchTo(name)
  newProfile.value = ''
}

async function save(key: string, value: unknown) {
  try {
    await patchSettings({ [key]: value })
    toast('设置已保存')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  }
}

async function toggleAutostart(value: boolean) {
  form.autostart = value
  await save('autostart', value)
  try {
    await desktop.value?.setAutostart?.(value)
  } catch {
    /* only supported in the desktop shell */
  }
}

async function setClock(mode: 'real' | 'virtual') {
  await api.post('/_mqs/clock/set', { mode })
  await refreshStatus()
  toast(mode === 'real' ? '已切回真实时钟' : '已切到虚拟时钟')
}

async function advance(duration: string) {
  await api.post('/_mqs/clock/advance', { duration })
  await refreshStatus()
  toast(`时钟快进 ${duration}`)
}

async function exportProfile() {
  exporting.value = true
  try {
    const token = sessionStorage.getItem('mqs-token') || ''
    const response = await fetch('/_mqs/snapshot', {
      method: 'POST',
      credentials: 'include',
      headers: token ? { 'X-MQS-Token': token } : {},
    })
    if (!response.ok) throw new Error(`export failed: ${response.status}`)
    const blob = await response.blob()
    const url = URL.createObjectURL(blob)
    const link = document.createElement('a')
    link.href = url
    link.download = `mqs-${state.status?.profile || 'profile'}.mqsprofile`
    link.click()
    URL.revokeObjectURL(url)
    toast('profile 已导出')
  } catch (error) {
    toast(error instanceof Error ? error.message : String(error))
  } finally {
    exporting.value = false
  }
}

function copyCommand() {
  const command = `mqs serve --profile ${status.value?.profile || 'dev'} --port ${status.value?.sim_port || 5700} --control-port ${status.value?.control_port || 5701}`
  navigator.clipboard.writeText(command).then(() => toast('启动命令已复制'), () => toast('复制失败'))
}
</script>

<template>
  <div class="modal-mask" @click.self="emit('close')">
    <div class="modal" style="width: min(720px, calc(100vw - 32px)); max-height: 88vh; overflow: auto">
      <h3>设置</h3>

      <div class="grid c2" style="gap: 20px">
        <div>
          <div class="panel-section-title">外观与语言</div>
          <label class="field" style="margin-bottom: 10px">主题
            <select v-model="form.theme" class="select" @change="save('theme', form.theme)">
              <option value="auto">跟随系统</option>
              <option value="dark">深色</option>
              <option value="light">亮色</option>
            </select>
          </label>
          <label class="field" style="margin-bottom: 10px">语言
            <select v-model="form.language" class="select" @change="save('language', form.language)">
              <option value="zh-CN">简体中文</option>
            </select>
          </label>
          <label class="field">日志级别（重启生效）
            <select v-model="form.log_level" class="select" @change="save('log_level', form.log_level)">
              <option value="debug">debug</option>
              <option value="info">info</option>
              <option value="warning">warning</option>
              <option value="error">error</option>
            </select>
          </label>
          <label class="switch" style="margin-top: 14px">
            <input type="checkbox" v-model="form.telemetry" @change="save('telemetry', form.telemetry)" />
            <span class="track"><span class="thumb"></span></span>
            匿名遥测（默认关闭，QSandbox 不上报任何数据）
          </label>
          <label class="switch" style="margin-top: 12px; opacity: desktop ? 1 : 0.55">
            <input type="checkbox" v-model="form.autostart" :disabled="!desktop" @change="toggleAutostart(form.autostart)" />
            <span class="track"><span class="thumb"></span></span>
            开机自启（仅桌面版）
          </label>
        </div>

        <div>
          <div class="panel-section-title">虚拟时钟</div>
          <div class="kv"><span class="k">当前</span><span class="v">{{ status?.clock.mode }} · {{ status?.clock.now }}</span></div>
          <div class="row wrap" style="margin: 10px 0">
            <button class="btn small" @click="advance('5m')">+5m</button>
            <button class="btn small" @click="advance('2h')">+2h</button>
            <button class="btn small" @click="advance('1d')">+1d</button>
          </div>
          <div class="row wrap">
            <button class="btn small" @click="setClock('virtual')">切到虚拟时钟</button>
            <button class="btn small ghost" @click="setClock('real')">回到真实时钟</button>
          </div>
          <p class="small faint" style="margin-top: 10px">
            被动回复窗口、token 过期、主动消息每日配额都走这时钟。
          </p>
        </div>
      </div>

      <hr style="border-color: var(--border); margin: 18px 0" />

      <div class="grid c2" style="gap: 20px">
        <div>
          <div class="panel-section-title">运行</div>
          <div class="kv"><span class="k">profile</span><span class="v">{{ currentProfile || status?.profile }}</span></div>
          <div v-if="desktop && profiles.length" class="field" style="margin-top: 10px">
            切换 profile
            <div class="row" style="gap: 6px">
              <select class="select" :value="currentProfile" :disabled="switching" @change="switchTo(($event.target as HTMLSelectElement).value)">
                <option v-for="item in profiles" :key="item.name" :value="item.name">
                  {{ item.name }}{{ item.current ? '（当前）' : '' }}
                </option>
              </select>
            </div>
          </div>
          <div v-if="desktop" class="field" style="margin-top: 10px">
            新建并切换
            <div class="row" style="gap: 6px">
              <input v-model="newProfile" class="input grow" placeholder="例如 dev / test-2" @keyup.enter="createAndSwitch" />
              <button class="btn small" :disabled="!newProfile.trim() || switching" @click="createAndSwitch">创建</button>
            </div>
          </div>
          <p v-if="desktop" class="small faint" style="margin-top: 8px">
            切换会重启本地服务并刷新窗口；每个 profile 有独立的 DB / 媒体 / 账号世界。
          </p>
          <div class="kv"><span class="k">profile 路径</span><span class="v">{{ status?.profile_dir }}</span></div>
          <div class="kv"><span class="k">端口</span><span class="v">sim {{ status?.sim_port }} · control {{ status?.control_port }}</span></div>
          <div class="kv"><span class="k">DB / 运行时长</span><span class="v">{{ formatBytes(status?.db_size || 0) }} · {{ formatDuration(status?.uptime_ms || 0) }}</span></div>
          <div class="row wrap" style="margin-top: 12px">
            <button class="btn small" @click="copyCommand">复制启动命令</button>
            <button class="btn small" @click="exportProfile" :disabled="exporting">导出 profile</button>
          </div>
        </div>
        <div>
          <div class="panel-section-title">安全</div>
          <div class="kv"><span class="k">控制 API</span><span class="v">token + Origin 校验</span></div>
          <div class="kv"><span class="k">监听</span><span class="v">{{ status?.host }}（默认仅本机）</span></div>
          <label class="switch" style="margin-top: 12px">
            <input type="checkbox" v-model="form.feature_whitelist" @change="save('feature.whitelist', form.feature_whitelist)" />
            <span class="track"><span class="thumb"></span></span>
            模拟腾讯接口白名单（内邀接口返回 11253）
          </label>
          <label class="field" style="margin-top: 12px">
            沙箱暴露范围
            <select v-model="form.feature_expose" class="select" @change="save('feature.expose', form.feature_expose)">
              <option value="all">all — 暴露全部接口</option>
              <option value="public">public — 只暴露公开接口（内邀接口 404）</option>
            </select>
          </label>
          <p class="small faint" style="margin-top: 10px">
            “接口白名单”模拟腾讯侧内邀权限（如获取群成员列表 11253）；
            “暴露范围”决定沙箱是否把这些接口挂出来。两者独立。
          </p>
        </div>
      </div>

      <div class="modal-actions">
        <button class="btn primary" @click="emit('close')">关闭</button>
      </div>
    </div>
  </div>
</template>
