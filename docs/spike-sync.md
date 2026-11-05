# 番茄钟内置「思源同步」：spike 记录与实现说明

创建 2026-11-05 · 状态：阶段 0 已结论，阶段 1 已实现并实机验证通过

## 1. 根因（已证实）

番茄记录本身写得好好的，手机看不到是因为**内核不会自己定时同步**。

| 证据 | 内容 |
|---|---|
| `siyuan sync status -w C:\SiYuanData` | `Enabled: true`、**`Mode: manual`**、`Provider: S3`、`Cloud: your-bucket`、`Interval: 30` |
| `conf.json` | `sync.enabled=true`、`sync.mode=2`、`sync.interval=30` |
| `kernel.log`（同一进程 11/04 09:29 起） | 11/04 只有开机 `sync before boot`；**全天 0 次 `syncing data repo`** |
| 11/05 的同步 | 09:22、09:30–09:41（631s，上传 578 文件/12.62MB，conflicts=11）、09:47、10:12 —— 后两次是本次联调触发的 |
| 进程 | 6806 上的内核由番茄钟启动（`kernel-6806.pid.json`），SiYuan 桌面端未运行时无人触发同步 |

**结论：不是「启动内核就会自动同步」。** 必须有人按间隔调用 `POST /api/sync/performSync`，这就是本功能。

## 2. 最终功能范围（按用户确认收敛）

| 项 | 决定 |
|---|---|
| 调用方式 | HTTP 直连内核 API（不 shell 调 CLI） |
| 同步间隔 | **完全跟随思源自己的设置** `conf.json → sync.interval`，**单位是秒**（思源文档：默认 30 秒，范围 30–43200 秒），本工具不另设默认值 |
| 记录写完立刻同步 | **不做**（已移除） |
| 代理 | 方案丁：不修改思源代理设置，只在设置页检测并提示（见 §5） |
| 手动入口 | 设置页「立即同步」按钮 + 顶部状态条显示上次同步时间/结果 |

## 3. 内核 API 契约（实测）

```
POST /api/sync/performSync  {}                → 阻塞到结束，{"code":0,"msg":"","data":null}
                              {upload:true}   → 需要选方向时指定方向
POST /api/sync/getSyncInfo  {}                → {"code":0,"data":{"synced":<ms>,"stat":"<html>","kernel":"..."}}
POST /api/sync/status                         → 不存在，返回空 body，别用
```

- 成功判定：`code == 0`，再用 `getSyncInfo.synced` 是否推进做交叉验证
- 并发调用：内核排队处理，不报错
- 超时：必须给到 30 分钟级（曾有 631 秒的同步）
- 鉴权：`Authorization: Token <api_token>`

## 4. 实现

**后端**

- `src/sync.rs`（新增）
  - 每 10 秒 tick（思源间隔最小 30 秒，检查要更密才跟得上）：`本工具已启用同步` && `思源 sync.enabled` && `sync.interval > 0` && `距上次 ≥ interval` && `内核可达` → 跑一次同步
  - 间隔按**秒**读，下限 30 秒（思源自己的下限），防止配置被改坏后疯狂同步
  - 失败后 5 分钟重试；每次 tick 都重读 conf.json，所以**改思源里的间隔即时生效**
  - 状态落盘 `sync-state.json`：`last_sync_at` / `last_attempt_at` / `last_ok` / `last_error` / `duration_ms` / `last_stat`
  - 互斥闸门：同步前等正在写的番茄记录收尾（最多 60 秒）；写记录前若同步正在进行，最多等 60 秒再写（实测并发写不丢记录，所以不会无限等）
  - 命令：`sync_now` / `sync_status`
- `src/mcp.rs`：新增 `perform_sync()`、`sync_info()`，以及 `#[ignore]` 的联调测试 `live_sync_roundtrip`
- `src/timer.rs`：写记录前后加闸门
- `src/main.rs`：`Config` 新增 `sync_enabled`（默认 true），`mod sync` 与命令注册

**前端**（`src/index.html`）

- 设置页「同步」区块：开关、思源当前设置（间隔/是否启用）、立即同步按钮、上次同步时间与耗时、代理提示
- 顶部状态条追加 `· 同步中…` / `· 上次同步 12 分钟前` / `· 同步失败`
- 监听 `sync-changed` 事件，3 秒轮询状态

## 5. 代理（方案丁：只提示不改）

- 内核启动首行日志 `runtime.go:234: use network proxy [http://127.0.0.1:7893]`，**整份日志只出现一次 → 代理只在启动时读取**
- 来源：`conf.json → system.networkProxy = {scheme:"http", host:"127.0.0.1", port:"7893"}`
- 同一请求实测：直连 0.61s vs 走代理 1.48s（2.4×）
- 本工具自己的请求不走代理（ureq 未启用 `proxy-from-env`，`ureq-2.12.1/src/agent.rs:252-290`）
- 实现：`sync_status` 返回 `proxy_warning`，设置页用红字提示「同步会走代理，大同步可能明显变慢」，**不自动改思源的设置**

## 6. 验证记录（2026-11-05）

| 项目 | 结果 |
|---|---|
| `cargo test -- --ignored` 打真实内核 | ✅ `before synced=1793843236619` → `after synced=1793844441788`，摘要「上传/下载 0/1 …」 |
| 并发：同步中写番茄记录 | ✅ create_focus 6ms 成功，记录保留、无重复（测完已删，未污染数据） |
| 端到端：新构建替换现有 exe 后启动 | ✅ 10:11:35 启动 → **10:12:05 自动同步** → `sync-state.json` 写入 `last_ok=true` |
| 间隔按秒跟随思源（30 秒） | ✅ 10:27:20 / 10:27:54 / 10:28:25 连续三次，间隔 30~35 秒，与 `sync.interval=30` 一致 |
| 前端内联脚本 | ✅ `node --check` 通过 |

## 7. 踩过的坑（下次别再踩）

1. **构建时 TEMP 必须可写**：本会话 `%TEMP%` 对子进程不可写，`windres` 报 `Unable to create temp file: Permission denied`，tauri-winres 随之 panic。解法：`$env:TEMP/TMP/TMPDIR` 指到工程内目录再 `cargo build`。
2. **新 exe 需要 `WebView2Loader.dll`**：只拷 exe 到安装目录会以退出码 `0xC0000135`（STATUS_DLL_NOT_FOUND）秒退。发布时把 `target\release\WebView2Loader.dll` 一起放过去。
3. **不要试图用不同 identifier 起第二个实例**：`TAURI_CONFIG` 覆盖 identifier 后启动会 `拒绝访问 (os error 5)`；同一 identifier 又会因为 WebView2 用户数据目录被占用而 `0x800700AA 请求的资源在使用中`。验证新构建只能「停掉现有实例再换 exe」。
4. **用 pwsh 工具启动 GUI 必须重定向 stdout/stderr**，否则命令会一直挂住。
5. **`sync.interval` 的单位是秒不是分钟**：CLI 打印 `Interval: 30s` 才是对的；一开始按分钟实现，差了 60 倍。依据是思源自带指南 `resources/guide/.../20230808120347-3cob0nb.sy` 里写的「同步间隔 … 默认 30 秒，可设为 30 至 43200 秒」。

## 8. 现状（v0.1.3）

- 发布包：`dist/siyuan-pomodoro-v0.1.3-windows-x64.zip`（exe + `WebView2Loader.dll`，两个文件必须同目录）
- 安装目录 `E:\Programs\siyuan-pomodoro\`：
  - `siyuan-pomodoro-v0.1.3-windows-x64.exe` + `WebView2Loader.dll` —— 当前版本
  - `siyuan-pomodoro-v0.1.2-windows-x64.exe.bak` —— 原始 v0.1.2 备份（出问题用它回滚）
- 为什么这版是 zip 而不是单文件：本机 MSVC 工具链（rustc 1.99）把 `windows-sys` 放进了 sysroot，`embed-resource` 直接报 `rustc_private`；gnullvm 工具链无法静态链接 WebView2 加载器。两条路都走不通，只能带 DLL。
