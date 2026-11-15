# 5h limit 时区（迷你窗显示所选时区时间 + 距目标还剩多久）

## 目标

迷你窗口里显示所选时区（目前只有 Phoenix）的当前时间；设了目标 limit 时间后，在时间后面显示 `hh:mm left`。

- 普通迷你窗（250×200）：底部一行 `凤凰城 03:31:20 · 02:28 left`
- 横条迷你窗（250×46，加宽自 190×46）：倒计时右侧 `03:31 · 02:28 left`
- 圆环 64×64、极简 150×22 太小，不加
- 点这行时间 → 唤起主窗口并滚到设置页的「5h limit 时区」卡片
- 目标时间已过（或没设）→ 不显示 `left`，只显示当前时间
- 时区选「不显示」→ 整行不出现

## 职责划分

纯展示功能，不动后端状态：时区与目标时间存同源 `localStorage`（键 `tzZone` / `tzTarget`），主窗口写、迷你窗口读，和现有 `breakShort` / `breakLong` 的做法一致。时间计算用 `Intl.DateTimeFormat`，不需要 tzdata。

后端只加一条命令 `open_tz_settings`（显示主窗口 + `emit("open-tz-settings")`），因为跨窗口 show/focus 没配 ACL 权限，写命令比放开权限省事。

## 数据

- `tzZone`：`""`（不显示）或 `"phoenix"`
- `tzTarget`：`datetime-local` 原始字符串 `YYYY-MM-DDTHH:mm`，**按所选时区的当地时间**填

## 关键实现

`common.js` 加 `TZKit`：

- `parts(tz, ms)` / `offset(tz, ms)`：用 `Intl` 拿到某时刻在目标时区的墙上时间，反推偏移
- `instant(tz, y, mo, d, h, mi)`：当地时间 → 真实时刻（先按 UTC 猜一次，再用该时刻偏移修正一次，夏令时切换也对）
- `hhmmss()` / `left(ms)` / `view()`：迷你窗口要显示的内容

`left` 超过 24 小时按累计小时显示（`27:12 left`），不回绕成「1 天」。

主窗口设置页加分组「5h limit 时区」：时区下拉 + 目标时间输入（旁边一个小问号，悬浮解释，省掉三行小字）+ 清除按钮。时区选「不显示」时隐藏目标时间那一行。

## 不做

- 目标时间不加时区下拉之外的选项（只有 Phoenix）
- 不写进 `config.json`，不进后端计时状态，不写番茄记录
