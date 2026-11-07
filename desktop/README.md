# QSandbox Desktop (Electron)

文档原方案用 Tauri（Rust 壳）。按用户 2026-11-07 的决定改为 **Electron**：
业务仍然是同一份 Vue 3 前端，主进程只负责拉起 MQS sidecar、开窗口、传开机自启开关。

## 结构

```
desktop/
  src/main.ts      Electron 主进程：启动 sidecar -> 读握手 -> 打开 GUI
  src/preload.ts   暴露 window.mqsDesktop（autostart / platform）
  package.json     electron-builder 配置（AppImage/deb、nsis、dmg）
```

sidecar 查找顺序：

1. `resources/sidecar/mqs`（打包后）
2. `build-sidecar/mqs`（开发模式）
3. 回退到 `MQS_PYTHON` / `python3 -m mqs`

## 开发

```bash
# 终端 1：构建前端 + sidecar
cd web && npm install && npm run build
cd .. && python tools/build_sidecar.py

# 终端 2：起桌面端（xvfb 下也可）
cd desktop && npm install && npm start
```

## 打包

```bash
python tools/build_sidecar.py          # 产物 build-sidecar/mqs
cd desktop && npm run dist:linux       # release/QSandbox-*.AppImage + .deb
```

Windows/macOS：`npm run dist:win` / `npm run dist:mac`，需要对应平台或 CI matrix。
产物里 `resources/sidecar/mqs` 是 PyInstaller onefile，内含 Python 运行时与 `web/dist`。

## 已验证

- Linux AppImage 在 Xvfb 下启动、加载 GUI、自动登录、退出时回收 sidecar。
- 数据目录：`app.getPath('userData')/mqs-home`（Linux 为 `~/.config/qsandbox-desktop/mqs-home`）。

## 说明

- 当前用 PyInstaller 打 sidecar；文档写的是 Nuitka，CI 可替换 `tools/build_sidecar.py` 的实现，Electron 侧不变。
- 桌面端没有额外业务逻辑；协议、CLI、MCP 仍全部在 Python 侧。
