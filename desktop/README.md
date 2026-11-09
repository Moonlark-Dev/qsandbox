# QSandbox 桌面端（Tauri v2）

窗口只是外壳：沙箱服务跑在**同一个进程**里（`crates/mqs` 作为库被 `mqs-desktop` 依赖）。
所以安装包是一个二进制，没有 sidecar、没有 Python、没有临时解包。

- 启动顺序：`setup()` 里 `mqs::server::start()`（随机端口）→ 用返回的 GUI URL
  （`http://127.0.0.1:<control>/#t=<nonce>`）创建窗口 → 退出时优雅关闭服务。
- 数据目录：`app_data_dir()/mqs-home`，不污染仓库或用户主目录。
- 前端资源：构建 `desktop/src-tauri` 之前先 `cd web && npm run build`；`crates/mqs` 的
  `build.rs` 会把 `web/dist` 内嵌进二进制（缺失时退化为“前端尚未构建”页）。

```bash
cd web && npm install && npm run build
cd ../desktop && npm install
npm run dev        # tauri dev
npm run build      # tauri build → src-tauri/../target/release/bundle/
```

CI（`.github/workflows/ci.yml` 的 `desktop` job）在 Linux/Windows/macOS 三平台构建并校验
安装包确实产出；release.yml 复用同一份 CI 定义。
