# 任务交接：思源「任务笔记管理」插件 + 独立番茄钟（不开思源界面）

## 背景与目标

- 思源桌面端每多开一个页面就多一份渲染进程，内存越占越多。
- 目标：**后台只跑思源内核（不开界面）**，通过 CLI/内核接口使用「任务笔记管理」插件的任务数据，再用一个**独立的番茄钟小程序**对已有任务计时，结束后写回番茄记录。思源界面里的统计效果要和原版番茄钟**完全一致**。
- 插件：Achuan-2/siyuan-plugin-task-note-management（v7.1.8，要求思源 ≥ 3.7.0，AGPL-3.0）。我是付费会员。

## 已确定的决策

1. **不 fork 插件**，一直用作者原版（自动更新、会员不受影响）。不碰 `src/components/vip/`。
2. 番茄钟**不完全脱离内核**：不直接改工作空间里的 JSON 文件（有同步冲突、数据损坏风险），所有读写走内核接口。
3. 倒计时**支持自定义时长**（原版不止 25 分钟）。第一版不做正计时（原因见下文）。
4. 同时做两件事：自己先实现 + 给作者提 Issue。

## 已核实的事实（来自插件源码）

- 插件有内核部分：`src/kernel.ts`、`src/kernel/tools/`，注册了 4 个工具：`task`、`project`、`habit`、`stats`。说明文档在仓库 `skills/` 目录。
- `task` 工具：`search_task`（支持传 `"today"`，结果和侧栏今日任务一致）、`get_task`、`create_task`（可选 `blockId` 绑定段落块，内核会校验块存在，并写入块属性 `custom-bind-reminders`）等。
- `stats` 工具：`create_focus`、`get_focuses_by_time`、`get_focus`、`delete_focus`、`get_task_summary`。
- 番茄记录存为 `pomodoroRecords/YYYY-MM-DD.json`（插件存储目录），结构见 `src/components/dataManager/pomodoroRecord.ts` 里的 `PomodoroSession` / `PomodoroRecord`。
- 链路：**段落块 → 任务（blockId）→ 番茄记录（eventId = 任务 ID）**。我的任务都已经在界面里由段落创建好，小程序只需要“找任务 → 计时 → 写回”。

### `create_focus` 与原版番茄钟写入的差异

| 项目 | 原版 `recordWorkSession` | `create_focus` |
|---|---|---|
| 基本字段（type/eventId/eventTitle/startTime/endTime/duration/plannedDuration/completed/note） | 有 | 有 |
| `count`（番茄数） | 按 `round(实际/计划)` 计算，最少 1 | **不写**；统计时缺 count 一律按 1 算 |
| `isCountUp`（正计时） | 有 | **不写** |
| 时长取整 | `roundSessionMinutes`：四舍五入，>0 且不足 1 分钟按 1 | 不取整，原样写入 |
| 开始时预写 `inProgress` 记录 | 有（防崩溃丢记录） | 无，只能结束时一次性写 |
| 更新任务番茄数缓存 `eventStats` | `updateStatsIndex` | **不更新**，只调 `recalculateRecordTotals`；内核侧查任务番茄数可能要等重新加载 |

**结论**：
- 自定义倒计时：只要 `plannedDuration` = 本次设定时长、`duration` = 按原版规则取整后的实际分钟，结果与原版一致（完成算 1，中断也算 1）。
- 正计时：无法一致（原版按时长折算多个番茄），等作者给 `create_focus` 补 `count`/`isCountUp` 参数后再做。
- 小程序要自己在本地保存计时状态，防止中途退出丢记录。

## 单独启动内核（社区实测方法，参数需在本机确认）

Windows：
```
"C:\Users\<用户名>\AppData\Local\Programs\SiYuan\resources\kernel\SiYuan-Kernel.exe" --port=6806 --wd="C:\Users\<用户名>\AppData\Local\Programs\SiYuan\resources" --workspace="<工作空间路径>" --resident=true
```
关闭：`taskkill /IM SiYuan-Kernel.exe /F`

Mac：
```
/Applications/SiYuan.app/Contents/Resources/kernel/SiYuan-Kernel --port=6806 --wd=/Applications/SiYuan.app/Contents/Resources --workspace="<工作空间路径>" --resident=true
```

- `--resident=true` 必须加，否则没有会话时内核约 7 秒后自动退出。
- 验证：浏览器打开 `http://127.0.0.1:6806` 能看到思源网页界面。
- 可选 CLI：`@frostime/siyuan-cli`（npm，Node ≥ 20），本身为 Claude Code/Codex 设计，带 SKILL；例：`siyuan api query.sql "SELECT ..."`。

## 尚未验证的关键前提（按优先级，先做这些）

1. **外部怎么调用插件的内核工具**（task/stats）——走哪个接口、siyuan-cli 是否支持。读插件 `src/kernel.ts`、`src/kernel/tools/index.ts` 和思源 3.7 内核插件相关源码确认。**不通则整个方案要改**（退路：Issue 请作者开放调用方式）。
2. 单独启动的内核是否加载插件内核部分。
3. 单独内核运行时再打开桌面端，是复用内核还是冲突抢工作空间。**一律先在测试工作空间上试**，不要动正式笔记。
4. 量内存：只跑内核 vs 桌面端开多个页面。如果差距不大，重新考虑方案（比如浏览器访问 6806 代替桌面端）。

## 下一步任务清单

1. [ ] 查清第 1 点（读源码）。
2. [ ] 写本机的内核启动/关闭脚本，在**测试工作空间**验证 2、3、4。
3. [ ] 用 CLI 调 `search_task` 和 `create_focus` 写一条测试记录，再开界面确认任务上的番茄数、统计页都正确。
4. [ ] 写给作者的 Issue 草稿（中文）：请 `create_focus` 补 `count`、`isCountUp` 参数并复用原版计算规则（`calculateWorkSessionCount`、`roundSessionMinutes`），写入后更新 `eventStats`；请保持番茄记录格式稳定或写进文档；可选：番茄钟计时状态做成内核工具。我审过再发。
5. [ ] 小程序第一版：找任务（关键词 / today）→ 自定义时长倒计时（暂停、结束提醒，本地保存状态）→ 按原版规则调用 `create_focus` 写回。形态（命令行 / 托盘 / 悬浮窗）待定。

## 工作约定

- 写操作（创建任务、写番茄记录、改块属性）执行前先让我确认；查询随意。
- 原版番茄钟的其他功能（全局悬浮窗、强制休息弹窗、随机微休息、提示音、系统通知）第一版不做，按需再加。
