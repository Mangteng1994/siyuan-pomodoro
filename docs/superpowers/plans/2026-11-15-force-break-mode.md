# 强制休息全屏遮罩 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development (recommended) or executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给微休息、短休息、长休息各加一个「强制模式」开关，开启后到点弹出全屏遮罩跑倒计时，期间关不掉，走完自动收起。

**Architecture:** 新增一个 Tauri 窗口 `break-screen`（全屏、无边框、置顶、不进任务栏）承载遮罩页面。显示与隐藏全部由后端决定：计时线程看「休息运行中 + 开关开着」就 show，条件不成立就 hide；微休息同理。前端页面只读状态画倒计时，不判断该不该显示。

**Tech Stack:** Tauri 2（Rust 后端 + 原生 HTML/JS 前端）、serde、WebView2。

**Spec:** `docs/superpowers/specs/2026-11-15-force-break-mode-design.md`

## Global Constraints

- 不加新依赖（`app/src-tauri/Cargo.toml` 的 `[dependencies]` 不变）。
- 不改计时状态机、写记录、同步逻辑：`Status`、`TimerState`、`plan_next_work`、`spawn_write` 一律不动。
- 配置字段必须 `#[serde(default)]`：老的 `config.json` 里没有 `force_break` 时读成全关，不能报「配置文件损坏」。
- 遮罩窗口创建或显示失败时只记日志并跳过，不 `unwrap`、不 `panic`、不阻塞计时。
- 前端页面不新增 Tauri 权限：只用已有的 `timer_get`、`micro_break_get` 两个命令。
- 版本号这次只落在 `Cargo.toml` 与 `tauri.conf.json`（0.1.11 → 0.1.12），`app/package.json` 不动。

## Review Focus

- **休息被别的路径结束掉**（迷你窗结束按钮、`timer_clear`、重启恢复）：遮罩必须跟着消失，不能把人锁在已经结束的休息里。
- **开着强制时中途关掉开关**：下一个 250ms tick 就要收起遮罩，不用等这次休息结束。
- **老 config.json 没有 `force_break`**：设置页必须照常打开、三个开关显示为关，不能整页读不出配置。
- **遮罩窗口创建失败**（WebView2 异常、显示器变化）：计时照常、记录照常写，只是这次没有遮罩。
- **微休息与休息交替时 owner 串台**：微休息结束不得把休息的遮罩收掉，反之亦然。

---

### Task 1: ForceBreak 配置与判定函数

**Files:**
- Create: `app/src-tauri/src/force_break.rs`
- Modify: `app/src-tauri/src/main.rs:14-45`（`Config` 加字段）、`app/src-tauri/src/main.rs:291-296`（加 `mod force_break;`）
- Test: `app/src-tauri/src/force_break.rs` 内的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: 无。
- Produces:
  - `pub struct ForceBreak { pub micro: bool, pub short: bool, pub long: bool }`，`Default` 全 `false`，`#[serde(default)]`。
  - `pub fn break_wants_overlay(kind: &str, running: bool, force: &ForceBreak) -> bool`——`kind` 用计时状态里的字符串 `"shortBreak"` / `"longBreak"`。
  - `Config.force_break: ForceBreak`。

- [ ] **Step 1: 写失败的测试**

在 `app/src-tauri/src/force_break.rs` 建文件，只放结构体骨架与测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_all_off() {
        let f = ForceBreak::default();
        assert!(!f.micro && !f.short && !f.long);
    }

    #[test]
    fn old_config_without_force_break_still_parses() {
        let json = r#"{"kernel_path":"","workspace":"","port":6806}"#;
        let c: crate::Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.force_break, ForceBreak::default());
    }

    #[test]
    fn overlay_only_for_running_break_with_its_switch_on() {
        let short_on = ForceBreak { micro: false, short: true, long: false };
        assert!(break_wants_overlay("shortBreak", true, &short_on));
        assert!(!break_wants_overlay("longBreak", true, &short_on));
        assert!(!break_wants_overlay("work", true, &short_on));

        let long_on = ForceBreak { micro: false, short: false, long: true };
        assert!(break_wants_overlay("longBreak", true, &long_on));
        assert!(!break_wants_overlay("longBreak", false, &long_on));
        assert!(!break_wants_overlay("shortBreak", false, &short_on));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd app/src-tauri; cargo test force_break -- --nocapture`
Expected: 编译失败，`cannot find type ForceBreak` / `break_wants_overlay not found`。

- [ ] **Step 3: 实现结构体与判定函数**

`app/src-tauri/src/force_break.rs` 顶部（测试模块之前）：

```rust
//! 强制模式：休息时全屏遮罩。显示与隐藏全由后端决定，前端只负责画倒计时。
use serde::{Deserialize, Serialize};

/// 三个开关，存在 config.json 里。默认全关。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct ForceBreak {
    pub micro: bool,
    pub short: bool,
    pub long: bool,
}

/// 这次休息该不该全屏遮挡。`kind` 用计时状态里的 `"shortBreak"` / `"longBreak"`。
pub fn break_wants_overlay(kind: &str, running: bool, force: &ForceBreak) -> bool {
    running
        && match kind {
            "shortBreak" => force.short,
            "longBreak" => force.long,
            _ => false,
        }
}
```

`main.rs` 里加 `mod force_break;`（挨着 `mod micro_break;`），`Config` 加：

```rust
    #[serde(default)]
    pub force_break: force_break::ForceBreak,
```

并在 `impl Default for Config` 里补 `force_break: force_break::ForceBreak::default(),`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd app/src-tauri; cargo test force_break -- --nocapture`
Expected: 3 个测试通过。

- [ ] **Step 5: 提交**

```bash
git add app/src-tauri/src/force_break.rs app/src-tauri/src/main.rs
git commit -m "feat: 强制模式开关配置与判定函数"
```

---

### Task 2: 遮罩窗口与遮罩页面

**Files:**
- Modify: `app/src-tauri/src/force_break.rs`（追加窗口管理）
- Create: `app/src/break-screen.html`
- Modify: `app/src-tauri/src/main.rs:391-407`（`setup` 里 `force_break::init(app)?`）

**Interfaces:**
- Consumes: Task 1 的 `ForceBreak`。
- Produces:
  - `pub enum Owner { Micro, Break }`（`Clone, Copy, PartialEq`）。
  - `pub struct Screen(pub Mutex<Option<Owner>>)`，`pub fn init(app: &mut tauri::App)` 里 `app.manage(...)`。
  - `pub fn show(app: &AppHandle, owner: Owner)`——幂等：同 owner 已显示就只保证可见与置顶。
  - `pub fn hide_if(app: &AppHandle, owner: Owner)`——幂等：只有当前 owner 匹配才隐藏并清空。

- [ ] **Step 1: 写窗口管理代码**

在 `force_break.rs` 追加（要 `use std::sync::Mutex; use tauri::{AppHandle, Manager};`）：

```rust
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Owner {
    Micro,
    Break,
}

pub struct Screen(pub Mutex<Option<Owner>>);

pub fn init(app: &mut tauri::App) {
    app.manage(Screen(Mutex::new(None)));
}

const LABEL: &str = "break-screen";

/// 显示遮罩。窗口第一次用时创建，之后复用。
pub fn show(app: &AppHandle, owner: Owner) {
    let state = app.state::<Screen>();
    {
        let mut cur = state.0.lock().unwrap();
        if *cur == Some(owner) {
            if let Some(w) = app.get_webview_window(LABEL) {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
            return;
        }
        *cur = Some(owner);
    }
    let built = match app.get_webview_window(LABEL) {
        Some(w) => Ok(w),
        None => tauri::WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::App("break-screen.html".into()))
            .title("休息")
            .fullscreen(true)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(true)
            .build(),
    };
    match built {
        Ok(w) => {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        Err(e) => {
            // 遮罩建不出来不影响计时，清掉 owner 下一次还能重试
            eprintln!("break-screen 创建失败：{e}");
            *state.0.lock().unwrap() = None;
        }
    }
}

/// 只有当前 owner 才能把遮罩关掉，避免微休息与休息互相串台。
pub fn hide_if(app: &AppHandle, owner: Owner) {
    let state = app.state::<Screen>();
    {
        let mut cur = state.0.lock().unwrap();
        if *cur != Some(owner) {
            return;
        }
        *cur = None;
    }
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}
```

`main.rs` 的 `setup` 里，在 `micro_break::init(app);` 之后加 `force_break::init(app);`。

- [ ] **Step 2: 写遮罩页面**

Create `app/src/break-screen.html`：深色底（`#111`）、居中三行、无任何按钮。脚本每 200ms 读一次状态，按时间片算剩余：

```html
<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>休息</title>
<style>
  html, body { height: 100%; margin: 0; }
  body { background: #111; color: #f5f5f5; display: flex; flex-direction: column;
         align-items: center; justify-content: center; gap: 18px;
         font-family: "Microsoft YaHei UI", system-ui, sans-serif;
         user-select: none; cursor: default; }
  #title { font-size: 22px; letter-spacing: 4px; color: #9a9a9a; }
  #clock { font-size: 132px; font-weight: 600; font-variant-numeric: tabular-nums; line-height: 1; }
  #sub { font-size: 18px; color: #9a9a9a; }
</style>
</head>
<body>
  <div id="title">休息一下</div>
  <div id="clock">00:00</div>
  <div id="sub"></div>
<script>
  const inv = (cmd) => window.__TAURI__.core.invoke(cmd);
  const pad = (n) => String(n).padStart(2, "0");
  const fmt = (ms) => {
    const t = Math.max(0, Math.round(ms / 1000));
    return pad(Math.floor(t / 60)) + ":" + pad(t % 60);
  };
  const human = (ms) => {
    const t = Math.max(0, Math.round(ms / 1000));
    const m = Math.floor(t / 60), s = t % 60;
    return m > 0 ? `还有 ${m} 分 ${s} 秒` : `还有 ${s} 秒`;
  };

  async function tick() {
    try {
      const micro = await inv("micro_break_get");
      if (micro && micro.active) {
        document.getElementById("title").textContent = "起来动一下";
        const ms = micro.remaining_seconds * 1000;
        document.getElementById("clock").textContent = fmt(ms);
        document.getElementById("sub").textContent = human(ms);
        return;
      }
      const s = await inv("timer_get");
      if (!s || s.kind === "work") return;
      const paused = s.paused_at ? Date.now() - s.paused_at : 0;
      const elapsed = Date.now() - s.started_at - s.paused_total - paused;
      const left = s.status === "running" ? s.planned_ms - elapsed : 0;
      document.getElementById("title").textContent =
        s.kind === "longBreak" ? "长休息" : "休息一下";
      document.getElementById("clock").textContent = fmt(left);
      document.getElementById("sub").textContent = human(left);
    } catch (e) {
      /* 读不到就保持上一帧，遮罩本身不负责报错 */
    }
  }
  tick();
  setInterval(tick, 200);
</script>
</body>
</html>
```

- [ ] **Step 3: 编译与静态预览**

Run: `cd app/src-tauri; cargo check`
Expected: 无警告通过。

静态预览（不改 `app/src`，桩掉 `__TAURI__`）：把 `app/src/break-screen.html` 复制到工作区预览目录，`<body>` 前插一段

```html
<script>window.__TAURI__ = { core: { invoke: (c) => Promise.resolve(
  c === "micro_break_get" ? { active: false, remaining_seconds: 0 }
  : { kind: "shortBreak", status: "running", planned_ms: 300000,
      started_at: Date.now() - 96000, paused_total: 0, paused_at: null }) } };</script>
```

再用 `python -m http.server` + 浏览器截图，确认 150% DPI 下 132px 数字没截断、三行居中。

- [ ] **Step 4: 提交**

```bash
git add app/src-tauri/src/force_break.rs app/src-tauri/src/main.rs app/src/break-screen.html
git commit -m "feat: 全屏休息遮罩窗口与页面"
```

---

### Task 3: 短休息 / 长休息触发遮罩

**Files:**
- Modify: `app/src-tauri/src/timer.rs:200-238`（后台线程循环内）

**Interfaces:**
- Consumes: `force_break::break_wants_overlay(kind, running, &ForceBreak)`、`force_break::show(app, Owner::Break)`、`force_break::hide_if(app, Owner::Break)`、`crate::read_config(app).force_break`。
- Produces: 无新接口。

- [ ] **Step 1: 在循环里接线**

在 `timer.rs` 后台线程 `loop` 内、`if finished { ... }` 之后追加（注意：先取状态再释放锁，窗口操作必须在锁外）：

```rust
// 强制模式：休息运行中且开关开着就盖满屏，条件不成立就收起来。
// 每次 250ms 读一次 config，和 micro_break 线程同样的做法。
let resting = {
    let g = shared.lock().unwrap();
    g.as_ref().map(|s| (s.kind.clone(), s.status == Status::Running))
};
let force = crate::read_config(&handle).force_break;
let want = resting
    .as_ref()
    .map_or(false, |(kind, running)| crate::force_break::break_wants_overlay(kind, *running, &force));
if want {
    crate::force_break::show(&handle, crate::force_break::Owner::Break);
} else {
    crate::force_break::hide_if(&handle, crate::force_break::Owner::Break);
}
```

- [ ] **Step 2: 编译**

Run: `cd app/src-tauri; cargo check`
Expected: 无警告通过。

- [ ] **Step 3: 提交**

```bash
git add app/src-tauri/src/timer.rs
git commit -m "feat: 短休息与长休息按开关弹出全屏遮罩"
```

---

### Task 4: 微休息触发遮罩

**Files:**
- Modify: `app/src-tauri/src/micro_break.rs:86`（一次读整份配置）、`app/src-tauri/src/micro_break.rs:104-135`（收尾与弹窗分支）

**Interfaces:**
- Consumes: `force_break::show(app, Owner::Micro)`、`force_break::hide_if(app, Owner::Micro)`。
- Produces: 无新接口。

- [ ] **Step 1: 读整份配置并接上遮罩**

把第 86 行 `let config = crate::read_config(&app).micro_break;` 改成：

```rust
let cfg = crate::read_config(&app);
let force = cfg.force_break;
let config = cfg.micro_break;
```

弹窗分支（原 `if settings.popup {` 那段）改成：

```rust
// 强制模式开着就全屏遮罩，否则还是原来 300×250 的小窗；
// 两个都没开就不弹（只看提示音）。
if force.micro {
    crate::force_break::show(&app, crate::force_break::Owner::Micro);
} else if settings.popup {
    let result = if let Some(w) = app.get_webview_window("micro-break") { Ok(w) } else {
        tauri::WebviewWindowBuilder::new(&app, "micro-break", tauri::WebviewUrl::App("micro-break.html".into()))
            .title("微休息").skip_taskbar(true)
            .inner_size(300.0, 250.0).resizable(false).always_on_top(true).build()
    };
    if let Ok(w) = result {
        let _ = w.show(); let _ = w.unminimize(); let _ = w.set_focus();
    }
}
```

同一条分支里，若强制没开但小窗还在显示（开关中途被关掉），要先隐藏小窗：

```rust
} else if let Some(w) = app.get_webview_window("micro-break") {
    let _ = w.hide();
}
```

在循环末尾（`continue` 之前不需要，放在 `running` 判断之后那一段的最后）加一行让它每次 tick 收尾：

```rust
// 微休息结束或开关被关掉：收起属于自己的遮罩，别去动休息的遮罩。
if !app.state::<Handle>().0.lock().unwrap().active {
    crate::force_break::hide_if(&app, crate::force_break::Owner::Micro);
}
```

- [ ] **Step 2: 编译**

Run: `cd app/src-tauri; cargo check`
Expected: 无警告通过。

- [ ] **Step 3: 提交**

```bash
git add app/src-tauri/src/micro_break.rs
git commit -m "feat: 微休息按开关弹出全屏遮罩"
```

---

### Task 5: 锁定遮罩

**Files:**
- Modify: `app/src-tauri/src/main.rs:408-416`（`on_window_event`）
- Modify: `app/src/break-screen.html`（按键拦截）

**Interfaces:**
- Consumes: 窗口 label `"break-screen"`。
- Produces: 无新接口。

- [ ] **Step 1: 拦截关闭事件**

`on_window_event` 里 `CloseRequested` 分支改成：

```rust
if let tauri::WindowEvent::CloseRequested { api, .. } = event {
    api.prevent_close();
    // 遮罩只拦不藏：锁到倒计时结束，由后端在休息结束时隐藏。
    if window.label() == "break-screen" {
        return;
    }
    if window.label() == "micro-break" {
        micro_break::micro_break_dismiss(window.app_handle().clone(), window.state::<micro_break::Handle>());
    }
    let _ = window.hide();
}
```

- [ ] **Step 2: 拦截键盘与右键**

在 `break-screen.html` 的 `<script>` 顶部加：

```js
  // 遮罩期间不接受任何"关掉它"的操作；Alt+F4 由后端的 CloseRequested 拦。
  for (const type of ["keydown", "contextmenu"]) {
    window.addEventListener(type, (e) => { e.preventDefault(); e.stopPropagation(); }, true);
  }
```

- [ ] **Step 3: 编译与语法检查**

Run: `cd app/src-tauri; cargo check`
Expected: 无警告通过。

Run: `node -e "const s=require('fs').readFileSync('app/src/break-screen.html','utf8'); new Function(s.split('<script>')[1].split('</script>')[0]); console.log('inline script ok')"`
Expected: `inline script ok`

- [ ] **Step 4: 提交**

```bash
git add app/src-tauri/src/main.rs app/src/break-screen.html
git commit -m "feat: 遮罩期间关不掉（拦截关闭与按键）"
```

---

### Task 6: 设置页三个开关

**Files:**
- Modify: `app/src/index.html:252`（「随机微休息」卡片之后插入分组）、`app/src/index.html:300-315`（`readConfig`）、`app/src/index.html:675-690`（回填）

**Interfaces:**
- Consumes: `Config.force_break`（`get_config` / `save_config` 已有）。
- Produces: DOM id `force_micro`、`force_short`、`force_long`。

- [ ] **Step 1: 插入设置分组**

在 `app/src/index.html` 的 `</div>`（「随机微休息」卡片结束）之后、`<div class="sh">同步</div>` 之前插入：

```html
      <div class="sh">强制模式</div>
      <div class="sc">
        <label class="sr"><span class="sl">微休息时全屏遮挡<small>到点盖满屏幕跑倒计时，中途关不掉</small></span><input id="force_micro" type="checkbox" class="sw"></label>
        <label class="sr"><span class="sl">短休息时全屏遮挡<small>休息结束自动收起，回到迷你窗</small></span><input id="force_short" type="checkbox" class="sw"></label>
        <label class="sr"><span class="sl">长休息时全屏遮挡<small>逃生口：托盘菜单里还能退出程序</small></span><input id="force_long" type="checkbox" class="sw"></label>
      </div>
```

- [ ] **Step 2: 接上读写**

`readConfig()` 返回的对象里加：

```js
    force_break: {
      micro: $("force_micro").checked,
      short: $("force_short").checked,
      long: $("force_long").checked,
    },
```

在 `$("sync_enabled").onchange = ...` 附近加（和它同一个风格，改了立即保存）：

```js
  for (const id of ["force_micro", "force_short", "force_long"]) {
    $(id).onchange = () => invoke("save_config", { config: readConfig() });
  }
```

回填（在 `$("micro_enabled").checked = micro.enabled === true;` 之后）加：

```js
  const force = c.force_break || {};
  $("force_micro").checked = force.micro === true;
  $("force_short").checked = force.short === true;
  $("force_long").checked = force.long === true;
```

- [ ] **Step 3: 静态预览确认排版**

用现有的 `tz_preview_build.py` 那套桩页面重新生成 `tz-preview/index.html`，浏览器打开进设置页，确认「强制模式」卡片三行对齐、开关位置与「开机自动启动」一致，150% DPI 下小字不换行错位。

- [ ] **Step 4: 提交**

```bash
git add app/src/index.html
git commit -m "feat: 设置页强制模式三个开关"
```

---

### Task 7: 版本号与更新日志

**Files:**
- Modify: `app/src-tauri/Cargo.toml:3`、`app/src-tauri/tauri.conf.json:4`（0.1.11 → 0.1.12）
- Modify: `CHANGELOG.md`（`## 未发布` 段）

**Interfaces:**
- Consumes: 无。
- Produces: 无。

- [ ] **Step 1: 改版本号**

两处都改成 `0.1.12`（漏改 `Cargo.toml` 会让更新检查一直提示有新版）。

- [ ] **Step 2: 写更新日志**

在 `## 未发布` 下追加用户能感觉到的变化：

```markdown
- 新增强制模式：微休息 / 短休息 / 长休息可以各自设成"全屏遮挡"，到点盖满屏幕跑倒计时，中途关不掉，走完自动收起
```

- [ ] **Step 3: 编译**

Run: `cd app/src-tauri; cargo check`
Expected: 无警告通过，`Cargo.lock` 里版本变成 0.1.12。

- [ ] **Step 4: 提交**

```bash
git add app/src-tauri/Cargo.toml app/src-tauri/Cargo.lock app/src-tauri/tauri.conf.json CHANGELOG.md
git commit -m "chore: v0.1.12 版本号与更新日志"
```

---

### Task 8: 真机验证

**Files:** 无文件改动（临时 seed 脚本用完就删）。

**Interfaces:**
- Consumes: 前面所有任务。
- Produces: 一份验证结论，异常则回到对应任务修。

- [ ] **Step 1: 起 dev 版**

`cd app; pnpm tauri dev`（先确认桌面上的发布版 exe 没在跑，否则单实例互斥会让新进程立刻退出）。

- [ ] **Step 2: 三条正向**

设置里打开全部三个开关。短休息设 0.5 分钟、长休息设 0.5 分钟，微休息间隔设 0.1–0.1 分钟、时长 10 秒。分别触发，确认：遮罩出现 → 倒计时在走 → 走完自己消失 → 迷你窗回到正确状态（休息结束显示「继续专注」）。

- [ ] **Step 3: 四条锁定**

遮罩期间依次按 Esc、F5、Alt+F4，再点托盘「打开主窗口」、点遮罩本身——遮罩都还在。然后托盘「退出」能退出程序。

- [ ] **Step 4: 关掉开关回归**

把三个开关关掉，重复 Step 2：微休息回到 300×250 小窗，短休息只在迷你窗里倒计时，遮罩不再出现。

- [ ] **Step 5: 中途关开关**

休息进行中（遮罩开着）去设置页把对应开关关掉，确认 1 秒内遮罩自己收起。

- [ ] **Step 6: 记录结论**

把上面结果写进交付说明；有失败项就地修，不打包不发版，等确认。
