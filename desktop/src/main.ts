import { app, BrowserWindow, dialog, ipcMain, shell } from 'electron'
import { ChildProcess, spawn } from 'node:child_process'
import crypto from 'node:crypto'
import fs from 'node:fs'
import path from 'node:path'

const PROFILE = process.env.MQS_DESKTOP_PROFILE || 'desktop'
const isDev = !app.isPackaged

let serverProcess: ChildProcess | null = null
let window: BrowserWindow | null = null
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

function startServer(): Promise<Handshake> {
  const home = path.join(app.getPath('userData'), 'mqs-home')
  fs.mkdirSync(home, { recursive: true })
  const token = crypto.randomBytes(24).toString('base64url')
  const { command, args } = serverInvocation()
  const fullArgs = [
    ...args,
    'serve',
    '--profile',
    PROFILE,
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
    MQS_PROFILE: PROFILE,
    PYTHONUNBUFFERED: '1',
  }
  serverProcess = spawn(command, fullArgs, { env, stdio: ['ignore', 'pipe', 'pipe'] })

  return new Promise<Handshake>((resolve, reject) => {
    let buffer = ''
    const timer = setTimeout(() => reject(new Error('QSandbox server did not report a handshake within 40s')), 40_000)

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
              clearTimeout(timer)
              resolve(parsed as Handshake)
              return
            }
          } catch {
            /* not the handshake line */
          }
        }
        index = buffer.indexOf('\n')
      }
    }

    serverProcess?.stdout?.on('data', onData)
    serverProcess?.stderr?.on('data', (chunk: Buffer) => {
      const text = chunk.toString('utf-8').trim()
      if (text) console.error(`[mqs] ${text}`)
    })
    serverProcess?.on('error', (error) => {
      clearTimeout(timer)
      reject(error)
    })
    serverProcess?.on('exit', (code) => {
      if (!shuttingDown) {
        clearTimeout(timer)
        reject(new Error(`QSandbox server exited with code ${code}`))
      }
    })
  })
}

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
      await new Promise((resolve) => setTimeout(resolve, 3000))
      const image = await window?.webContents.capturePage()
      if (image) fs.writeFileSync(screenshot, image.toPNG())
      app.quit()
    })
  }
}

async function boot(): Promise<void> {
  try {
    const handshake = await startServer()
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
      ].join('\n'),
    )
    app.quit()
  }
}

function stopServer(): void {
  shuttingDown = true
  if (serverProcess && !serverProcess.killed) {
    serverProcess.kill('SIGTERM')
    setTimeout(() => {
      if (serverProcess && !serverProcess.killed) serverProcess.kill('SIGKILL')
    }, 3000)
  }
}

app.whenReady().then(async () => {
  ipcMain.handle('mqs:setAutostart', (_event, enabled: boolean) => {
    app.setLoginItemSettings({ openAtLogin: Boolean(enabled) })
    return true
  })
  ipcMain.handle('mqs:getAutostart', () => app.getLoginItemSettings().openAtLogin)
  ipcMain.handle('mqs:platform', () => process.platform)

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
    const finish = () => {
      if (allowQuit) return
      allowQuit = true
      app.quit()
    }
    serverProcess.once('exit', finish)
    stopServer()
    setTimeout(finish, 3500)
  }
})

process.on('SIGINT', () => app.quit())
process.on('SIGTERM', () => app.quit())
