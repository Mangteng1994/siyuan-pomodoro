# 思源番茄钟

独立的 Windows 番茄钟工具：启动或连接思源内核，对任务计时并写回番茄记录。支持自由专注、迷你计时、悬浮笔记、随机微休息和托盘常驻。功能状态见 [FEATURES.md](FEATURES.md)。

## 下载与使用

从 [Releases](https://github.com/Mangteng1994/siyuan-pomodoro/releases) 下载最新版的 `siyuan-pomodoro-windows-x64.exe`（v0.1.8 及更早版本的文件名带版本号），直接运行。

**发布采用与早期单文件版本一致的单 exe 方案：通过 MSVC 静态链接 WebView2 加载器，无需下载或在 exe 旁放置 `WebView2Loader.dll`，无需解压双文件 ZIP。** 系统仍需安装 Microsoft Edge WebView2 Runtime；加载器静态链接不等于将整个浏览器运行时打入 exe。

在设置中填写思源内核路径、工作空间、端口及所需的 API token，再启动或连接内核。微休息默认关闭，需在“设置 → 随机微休息”中启用并保存。升级前从托盘退出旧版，避免旧版进程继续运行。

主窗口保留任务栏图标，迷你计时、悬浮笔记和微休息弹窗不显示任务栏图标。主窗口隐藏到托盘时，辅助窗口继续独立显示，可从托盘菜单重新打开；只有托盘“退出”才退出整个程序。

## 编译与发布

本机已有 Rust MSVC 工具链和 Visual Studio 2022 Build Tools。发布目标固定为 `x86_64-pc-windows-msvc`。PATH 中可能优先出现 LLVM/GNU Rust，**不得直接用默认 cargo 的 GNU 产物发布**：它会重新引入外置 WebView2 加载器 DLL。

在项目根目录执行：

```powershell
pwsh -File .\scripts\build-release.ps1
```

脚本默认使用 `$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin`；其他安装位置可用 `-ToolchainBin` 指定。脚本检查工具链类型，使用锁定依赖离线编译，并仅从 `app/src-tauri/target/x86_64-pc-windows-msvc/release/` 取产物。首次在其他电脑构建时，需先准备依赖缓存、MSVC C++ 构建工具和 Windows SDK。

产物位于 `dist/`：单 exe `siyuan-pomodoro-windows-x64.exe`（文件名固定，不带版本号）和 `SHA256SUMS.txt`。Release 上传这两个文件，发布前检查 exe 的依赖列表中没有 `WebView2Loader.dll`；不发布 GNU exe 或双文件 ZIP。当前工作约定为不主动运行测试，编译和依赖检查不代表功能实测。
