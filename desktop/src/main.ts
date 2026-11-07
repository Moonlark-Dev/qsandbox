import { app, BrowserWindow, dialog, ipcMain, shell } from 'electron'
import { ChildProcess, spawn } from 'node:child_process'
import crypto from 'node:crypto'
import fs from 'node:fs'
import path from 'node:path'

const isDev = !app.isPackaged
const DEFAULT_PROFILE = 'desktop'
const PROFILE_RE = /^[A-Za-z0-9_-]{1,32}$/

let serverProcess: ChildProcess | null = null
let window: BrowserWindow | null = null
let currentProfile = process.env.MQS_DESKTOP_PROFILE || DEFAULT_PROFILE
let shuttingDown = false
let allowQuit = false

interface Handshake {
  gui_url: string
  base_url: string
  ws_url: string
  token: string
  sim_port: number
  control_port: number
}

interface DesktopState {
  profile?: string
}

// ---------------------------------------------------------------------------
// profile persistence / discovery
// ---------------------------------------------------------------------------

function profilesHome(): string {
  return path.join(app.getPath('userData'), 'mqs-home')
}

function statePath(): string {
  return path.join(app.getPath('userData'), 'desktop.json')
}

function loadState(): DesktopState {
  try {
    return JSON.parse(fs.readFileSync(statePath(), 'utf-8')) as DesktopState
  } catch {
    return {}
  }
}

function saveState(state: DesktopState): void {
  try {
    fs.mkdirSync(path.dirname(statePath()), { recursive: true })
    fs.writeFileSync(statePath(), JSON.stringify({ ...loadState(), ...state }, null, 2))
  } catch (error) {
    console.error('[desktop] failed to save state', error)
  }
}

function listProfiles(): Array<Record<string, unknown>> {
  const root = path.join(profilesHome(), 'profiles')
  const entries: Array<Record<string, unknown>> = []
  try {
    for (const name of fs.readdirSync(root)) {
      const directory = path.join(root, name)
      if (!fs.statSync(directory).isDirectory()) continue
      let createdAt = ''
      try {
        const meta = JSON.parse(fs.readFileSync(path.join(directory, 'profile.json'), 'utf-8'))
        createdAt = meta.created_at || ''
      } catch {
        /* profile.json may not exist yet */
      }
      entries.push({ name, path: directory, created_at: createdAt, current: name === currentProfile })
    }
  } catch {
    /* no profiles yet */
  }
  if (!entries.some((entry) => entry.name === currentProfile)) {
    entries.unshift({ name: currentProfile, path: path.join(root, currentProfile), current: true })
  }
  return entries.sort((a, b) => String(a.name).localeCompare(String(b.name)))
}

// ---------------------------------------------------------------------------
// sidecar lifecycle
// ---------------------------------------------------------------------------

function sidecarPath(): string | null {
  const name = process.platform === 'win32' ? 'mqs.exe' : 'mqs'
  const candidates = [
    path.join(process.resourcesPath, 'sidecar', name),
    path.join(__dirname, '..', '..', 'build-sidecar', name),
  ]
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) return candidate
  }
  return null
}

function serverInvocation(): { command: string; args: string[] } {
  const sidecar = sidecarPath()
  if (sidecar) return { command: sidecar, args: [] }
  const python = process.env.MQS_PYTHON || 'python3'
  return { command: python, args: ['-m', 'mqs'] }
}

function startServer(profile: string): Promise<Handshake> {
  const home = profilesHome()
  fs.mkdirSync(home, { recursive: true })
  const token = crypto.randomBytes(24).toString('base64url')
  const { command, args } = serverInvocation()
  const fullArgs = [
    ...args,
    'serve',
    '--profile',
    profile,
    '--host',
    '127.0.0.1',
    '--port',
    '0',
    '--control-port',
    '0',
    '--token',
    token,
  ]
  const env = {
    ...process.env,
    MQS_HOME: home,
    MQS_PROFILE: profile,
    PYTHONUNBUFFERED: '1',
  }
  shuttingDown = false
  const child = spawn(command, fullArgs, { env, stdio: ['ignore', 'pipe', 'pipe'] })
  serverProcess = child

  return new Promise<Handshake>((resolve, reject) => {
    let buffer = ''
    const timer = setTimeout(() => reject(new Error('QSandbox server did not report a handshake within 40s')), 40_000)

    const settle = (fn: () => void) => {
      clearTimeout(timer)
      fn()
    }

    const onData = (chunk: Buffer) => {
      buffer += chunk.toString('utf-8')
      let index = buffer.indexOf('\n')
      while (index >= 0) {
        const line = buffer.slice(0, index).trim()
        buffer = buffer.slice(index + 1)
        if (line.startsWith('{')) {
          try {
            const parsed = JSON.parse(line)
            if (parsed.mqs && parsed.gui_url) {
              settle(() => resolve(parsed as Handshake))
              return
            }
          } catch {
            /* not the handshake line */
          }
        }
        index = buffer.indexOf('\n')
      }
    }

    child.stdout?.on('data', onData)
    child.stderr?.on('data', (chunk: Buffer) => {
      const text = chunk.toString('utf-8').trim()
      if (text) console.error(`[mqs] ${text}`)
    })
    child.on('error', (error) => settle(() => reject(error)))
    child.on('exit', (code) => {
      if (!shuttingDown) {
        settle(() => reject(new Error(`QSandbox server exited with code ${code}`)))
      }
    })
  })
}

function stopServer(): Promise<void> {
  const child = serverProcess
  serverProcess = null
  shuttingDown = true
  if (!child || child.exitCode !== null) return Promise.resolve()
  return new Promise<void>((resolve) => {
    let done = false
    let timer: NodeJS.Timeout
    const finish = () => {
      if (done) return
      done = true
      clearTimeout(timer)
      resolve()
    }
    child.once('exit', finish)
    try {
      child.kill('SIGTERM')
    } catch {
      finish()
      return
    }
    timer = setTimeout(() => {
      try {
        child.kill('SIGKILL')
      } catch {
        /* already gone */
      }
      finish()
    }, 3000)
  })
}

async function switchProfile(name: string): Promise<{ ok: boolean; profile?: string; error?: string }> {
  const profile = name.trim()
  if (!PROFILE_RE.test(profile)) {
    return { ok: false, error: 'profile 名只能包含字母、数字、下划线和短横线（1-32 位）' }
  }
  if (profile === currentProfile && serverProcess) {
    return { ok: true, profile }
  }
  try {
    await stopServer()
    currentProfile = profile
    saveState({ profile })
    const handshake = await startServer(profile)
    if (window && !window.isDestroyed()) {
      await window.loadURL(handshake.gui_url)
    } else {
      createWindow(handshake)
    }
    return { ok: true, profile }
  } catch (error) {
    return { ok: false, error: String(error) }
  }
}

// ---------------------------------------------------------------------------
// window
// ---------------------------------------------------------------------------

function createWindow(handshake: Handshake): void {
  window = new BrowserWindow({
    width: 1320,
    height: 880,
    minWidth: 1024,
    minHeight: 700,
    backgroundColor: '#0b0e14',
    title: 'QSandbox — QQ Bot Sandbox',
    autoHideMenuBar: true,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  })
  window.webContents.setWindowOpenHandler(({ url }) => {
    shell.openExternal(url)
    return { action: 'deny' }
  })
  window.loadURL(handshake.gui_url)
  window.on('closed', () => {
    window = null
  })
  const screenshot = process.env.MQS_DESKTOP_SCREENSHOT
  if (screenshot) {
    window.webContents.once('did-finish-load', async () => {
      const switchTo = process.env.MQS_DESKTOP_SWITCH_TO
      if (switchTo) {
        const result = await switchProfile(switchTo)
        if (!result.ok) console.error('[desktop] profile switch failed', result.error)
      }
      await new Promise((resolve) => setTimeout(resolve, 3000))
      const image = await window?.webContents.capturePage()
      if (image) fs.writeFileSync(screenshot, image.toPNG())
      app.quit()
    })
  }
}

async function boot(): Promise<void> {
  try {
    const handshake = await startServer(currentProfile)
    createWindow(handshake)
  } catch (error) {
    dialog.showErrorBox(
      'QSandbox 启动失败',
      [
        String(error),
        '',
        '开发模式请确认已在仓库根目录创建 .venv 并安装 qsandbox（mqs）；',
        '或设置 MQS_PYTHON 指向带 mqs 的 Python。',
        '发行版应包含 resources/sidecar/mqs。',
        `profile: ${currentProfile}`,
      ].join('\n'),
    )
    app.quit()
  }
}

app.whenReady().then(async () => {
  const saved = loadState()
  if (!process.env.MQS_DESKTOP_PROFILE && saved.profile && PROFILE_RE.test(saved.profile)) {
    currentProfile = saved.profile
  }

  ipcMain.handle('mqs:setAutostart', (_event, enabled: boolean) => {
    app.setLoginItemSettings({ openAtLogin: Boolean(enabled) })
    return true
  })
  ipcMain.handle('mqs:getAutostart', () => app.getLoginItemSettings().openAtLogin)
  ipcMain.handle('mqs:platform', () => process.platform)
  ipcMain.handle('mqs:profiles', () => ({ current: currentProfile, profiles: listProfiles() }))
  ipcMain.handle('mqs:switchProfile', (_event, name: string) => switchProfile(name))
  ipcMain.handle('mqs:profilesHome', () => profilesHome())

  await boot()

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) boot()
  })
})

app.on('window-all-closed', () => {
  app.quit()
})

app.on('before-quit', (event) => {
  if (allowQuit) return
  if (serverProcess && serverProcess.exitCode === null) {
    event.preventDefault()
    let finished = false
    const finish = () => {
      if (finished) return
      finished = true
      allowQuit = true
      app.quit()
    }
    stopServer().finally(finish)
    setTimeout(finish, 3500)
  }
})

process.on('SIGINT', () => app.quit())
process.on('SIGTERM', () => app.quit())
