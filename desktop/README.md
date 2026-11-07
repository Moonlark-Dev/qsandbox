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

1. `resources/sidecar/mqs`（打包后；`build-sidecar/mqs.dist/` 整个目录被复制成 `sidecar/`）
2. `build-sidecar/mqs.dist/mqs`（开发模式，Nuitka standalone）
3. `build-sidecar/mqs`（开发模式，旧的 onefile 构建）
4. 回退到 `MQS_PYTHON` / `python3 -m mqs`

启动失败时对话框会写清楚是哪一种：找不到 sidecar 就提示打包/开发步骤，
sidecar 崩了则附上它的 stderr 尾部。

## 开发

```bash
# 终端 1：构建前端 + sidecar
cd web && npm install && npm run build
cd .. && python tools/build_sidecar.py          # -> build-sidecar/mqs.dist/

# 终端 2：起桌面端（xvfb 下也可）
cd desktop && npm install && npm start
```

## 打包

```bash
python tools/build_sidecar.py          # 产物 build-sidecar/mqs.dist/
cd desktop && npm run dist:linux       # release/QSandbox-*.AppImage + .deb
```

Windows/macOS：`npm run dist:win` / `npm run dist:mac`，需要对应平台或 CI matrix。
产物里 `resources/sidecar/` 是 Nuitka **standalone** 目录（可执行文件 + Python 运行时 +
`web/dist`），用户机器不需要装 Python。

## 验证

```bash
python tools/smoke_sidecar.py                       # 起 sidecar：握手 + /_mqs/health + GUI
python tools/verify_desktop_bundle.py --target linux  # 打包产物里确实有 mqs，并真的能启动
```

CI（`.github/workflows/desktop-build.yml`）在三平台都跑这两步：sidecar 编译完先冒烟，
打包后再从安装包里把 mqs 拉起来一次，缺了就 red。

## 已验证

- Linux AppImage 在 Xvfb 下启动、加载 GUI、自动登录、退出时回收 sidecar。
- 数据目录：`app.getPath('userData')/mqs-home`（Linux 为 `~/.config/qsandbox-desktop/mqs-home`）。

## 说明

- sidecar 用 Nuitka standalone 打包（`pip install -e ".[build]"`）。早期用 onefile，但它每次
  启动要把 ~200 MiB 解包到 `TMPDIR`，临时目录小或满时 sidecar 直接 exit 1（桌面端只能报
  `exited with code 1`）；standalone 启动不解包，代价是安装包体积大一些。
- Electron 侧只认 `resources/sidecar/mqs`，换打包器不影响桌面壳。
- 桌面端没有额外业务逻辑；协议、CLI、MCP 仍全部在 Python 侧。
