# AGENTS.md

给在这个仓库里干活的 AI 助手和协作者看的约定。先读完再动手。

## 项目是什么

独立的 Windows 番茄钟（Tauri 2 + Rust 后端 + 纯 HTML/JS 前端，无前端构建步骤）。后台连思源内核，对任务计时，结束后把番茄记录写回思源，界面里的统计要和「任务笔记管理」插件的原版番茄钟一致。

- `app/src/`：前端页面。`index.html`（主窗口）、`mini.html`（迷你计时）、`notes.html`（悬浮笔记）、`micro-break.html`（微休息弹窗）、`common.js`（计时视图的共用逻辑）。
- `app/src-tauri/src/`：后端。`timer.rs` 是计时状态机（唯一真相，窗口只是视图，靠 `timer-changed` 事件同步）；`mcp.rs` 写记录；`sync.rs` 触发思源同步；`micro_break.rs`、`notes.rs`、`updater.rs`、`single_instance.rs`。
- `scripts/`：构建和联调脚本，发布用 `build-release.ps1`。
- 计时状态落盘在 `%APPDATA%\com.local.siyuan-pomodoro`（`timer.json`、`notes.json`、`config.json`），不同实例共用，改状态结构要兼容老文件。

## 文档规矩（每次都要做）

1. **改了功能就记到 `FEATURES.md`**：新增、修改、删除功能，或者改变了行为，都要在同一次提交里更新它。
   - 表格里每项写清楚：做了什么、怎么用、当前验证状态。
   - 状态要诚实：只写实际做过的，例如“已验证（……已实测）”或“完成（未运行测试，待手试）”；没跑过的不要写成已验证。
   - 临时调整、更新交付、已知问题写在文末对应的小节里，带日期（绝对日期，不写“今天”“上周”）。
2. **用户可见的变化记到 `CHANGELOG.md`**：按版本倒序，每版一小段，只写用户关心的变化，用中文。发版时 Release 说明和软件内“更新内容”都从这里取，所以措辞要面向使用者，不写内部实现。
3. 不要在根目录新建 `RELEASE_NOTES_*.md`。交接笔记、调试记录和一次性调试脚本放 `docs/dev/`。
4. 版本号要同步改：`app/src-tauri/Cargo.toml`、`app/src-tauri/tauri.conf.json`（`package.json` 里的版本是占位，不用管）。

## 界面开发：用 token，不写死颜色和尺寸

- 颜色、间距、圆角、字号一律走 CSS 变量（token），在页面的 `:root` 里定义，深色模式用 `@media (prefers-color-scheme: dark)` 覆盖同一批变量。组件样式里**不要出现写死的色值**（`#e5483d`、`rgba(...)` 之类），要用的颜色先加成 token 再引用。
- 现有 token（`index.html`）：`--bg` `--card` `--line` `--fg` `--sub` `--accent` `--accent-bg` `--ok` `--bad` `--track`。`mini.html`、`micro-break.html` 各有一份相近的，改一个语义时（比如休息阶段的绿色）三处一起看。新增页面或大改时，优先把这批 token 抽到一个共用的 `tokens.css`，不要再复制第四份。
- 状态色有固定含义：专注红（`--accent`）、休息绿（`--ok`）、暂停橙。不要为单个组件另起一套颜色。
- 复用已有的组件样式：分组卡片 `.sh/.sc/.sr`（设置页）、滑动开关 `input.sw`、主按钮 `.primary`、幽灵按钮 `.ghost`。
- 文案：中文，句末不加多余标点，按钮用动词开头、1 到 3 个字。
- **小窗口里不要用系统自带的 `confirm()`**（会被截断），用就地二次确认（第一次点按钮变成“确定？”，几秒内再点才执行）。主窗口的 `confirm()` 可以用。
- **改界面前先给草图或示意图**，用户确认后再改。改完要在开发版里截图核对，不要只看代码。

## 后端和数据

- 计时、写记录、重试、休息、微休息都在后端线程里跑，主窗口关掉也要继续。新增会改计时状态的逻辑，放在 `timer.rs`，前端只发命令、读状态。
- 写记录要保证不丢：失败自动重试，写不进去的要给用户“重试”和“放弃”两条路。
- 和 GitHub、思源内核的 HTTP 请求用 `ureq`，要设超时，错误信息要能让用户看懂下一步该做什么。
- 加新的 Tauri 命令要同时在 `main.rs` 的 `invoke_handler` 里注册。

## 测试环境和安全边界（重要）

- **用户的真实思源工作空间（`C:\SiYuanData`，带云同步）不要碰**：不写、不复制它的配置、不启动指向它的测试内核。只读排查可以，动任何文件前先问并先备份。
- 测试只用仓库里的 `test-workspace/`（已 gitignore），内核端口 6807。用户日常连的内核在 6806，往里写记录会进他的真实数据，要写之前先问。
- 用户常常开着自己的发布版（release exe），它和开发版共用 `%APPDATA%\com.local.siyuan-pomodoro` 下的状态文件，还占着 WebView2。测试前先看进程，要关它先问用户，别把测试数据写进共享目录。
- 调接口不要用 Git Bash 的 `curl` 发中文（编码问题），用 Node 或程序内请求。
- 开发版（`npm run tauri dev`，在 `app/` 下）前端是实时读 `app/src/` 的，改 HTML 后在窗口里按 F5 即可；改 Rust 会自动重编译并重启。

## 构建、测试、发布

- 开发：`cd app && npm run tauri dev`。快速检查：`cd app/src-tauri && cargo check`，单元测试 `cargo test`。
- 发布只用 MSVC 工具链，**不要用默认 cargo 的 GNU 产物发布**（会重新引入外置 `WebView2Loader.dll`）：

  ```powershell
  pwsh -File .\scripts\build-release.ps1
  ```

  产物在 `dist/`：单 exe 加 `SHA256SUMS-v版本号.txt`。发布前确认依赖列表里没有 `WebView2Loader.dll`。
- **推送、重建 exe、发布 Release 都要用户明确说了才做**，不要主动推送。批准只对这一次有效。
- Release 资产命名固定为 `siyuan-pomodoro-v版本号-windows-x64.exe`，软件内更新靠结尾的 `windows-x64.exe` 找安装包，并用 GitHub 给的 SHA-256 校验，改命名会让更新失效。
- 代码里的注释和提交说明都用中文，提交信息第一行写清楚改了什么，结尾带用户给定的署名行。

## 沟通

- 用中文。先说结论，再说细节；做了什么、没做什么、哪些没验证，都如实讲。
- 做不到或需要用户决定的事（推送、发布、写进真实数据、删文件）先问，不要自己决定。
