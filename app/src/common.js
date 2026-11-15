// 主窗口和迷你窗口共用：计时状态在后端，这里只负责把状态算成界面要显示的东西。
const TimerView = {
  elapsed(s, now = Date.now()) {
    return now - s.started_at - s.paused_total - (s.paused_at ? now - s.paused_at : 0);
  },
  remaining(s) {
    return s.status === "finished" ? 0 : Math.max(0, s.planned_ms - this.elapsed(s));
  },
  fmt(ms) {
    const sec = Math.max(0, Math.ceil(ms / 1000));
    return `${String(Math.floor(sec / 60)).padStart(2, "0")}:${String(sec % 60).padStart(2, "0")}`;
  },
  // 把悬浮笔记里已勾选的事项，按“- 事项”逐行追加到备注输入框
  async insertDoneNotes(textarea, invoke) {
    let items = [];
    try { items = await invoke("notes_get"); } catch {}
    const done = items.filter((i) => i.done && i.text.trim());
    if (!done.length) { textarea.placeholder = "悬浮笔记里还没有勾选完成的事项"; return; }
    const text = done.map((i) => `- ${i.text.replace(/\n+/g, " ")}`).join("\n");
    textarea.value = textarea.value.trim() ? textarea.value.replace(/\s+$/, "") + "\n" + text : text;
    textarea.focus();
  },
  // 休息时长（分钟），主窗口里改，迷你窗口读同一份（同源 localStorage）
  breakMinutes(long) {
    let v = NaN;
    try { v = Number(localStorage.getItem(long ? "breakLong" : "breakShort")); } catch {}
    return v > 0 ? v : long ? 15 : 5;
  },
  info(s) {
    if (!s) return "";
    const brk = s.kind === "longBreak" ? "长休息" : s.kind === "shortBreak" ? "短休息" : null;
    if (s.status === "finished") {
      const head = brk
        ? `${brk}结束：${s.task.title}（${(s.actual_ms / 60000).toFixed(1)} 分钟）　`
        : `${s.completed ? "已完成" : "已提前结束"}：${s.task.title}（实际 ${(s.actual_ms / 60000).toFixed(1)} 分钟）　`;
      if (s.awaiting_note) return head + "等你填备注（保存或跳过后写入记录）";
      if (s.written) return head + `✔ 已写入记录（${s.duration} 分钟）`;
      if (s.writing) return head + "写入记录中…";
      return head + `✘ 写入失败：${s.error ?? "尚未写入"}（会每 30 秒自动重试，也可手动重试）`;
    }
    return `${s.status === "paused" ? "已暂停" : brk ? "休息中" : "计时中"}${brk ? `（${brk}）` : ""}：${s.task.title}`;
  },
  flags(s) {
    const finished = !!s && s.status === "finished";
    const awaitingNote = finished && !!s.awaiting_note;
    const pending = finished && !s.written && !s.writing && !awaitingNote;
    return {
      canStart: !s,
      canPause: !!s && !finished,
      pauseLabel: s && s.status === "paused" ? "继续" : "暂停",
      canEnd: !!s && !finished,
      canClear: finished && s.written,
      // 番茄记录写入后可以接着休息
      canBreak: finished && s.written && (!s.kind || s.kind === "work"),
      // 休息写入后可以一键接续下一个番茄（沿用同一个任务和上次的时长）
      canNext: finished && s.written && !!s.kind && s.kind !== "work",
      awaitingNote,
      canRetry: pending,
      canDiscard: pending,
    };
  },
};

// 5h limit 时区：设置和状态都放在同源 localStorage，主窗口写、迷你窗口读（同 breakShort/breakLong）。
// 时间换算用 Intl，不依赖 tzdata；目标时间按所选时区的「当地时间」填。
const TZKit = {
  ZONES: { phoenix: { label: "Phoenix", short: "凤凰城", tz: "America/Phoenix" } },
  zone() {
    let v = "";
    try { v = localStorage.getItem("tzZone") || ""; } catch {}
    return this.ZONES[v] ? v : "";
  },
  setZone(v) {
    try { localStorage.setItem("tzZone", this.ZONES[v] ? v : ""); } catch {}
  },
  // datetime-local 原样字符串，例：2026-11-16T06:00
  targetInput() {
    try { return localStorage.getItem("tzTarget") || ""; } catch { return ""; }
  },
  setTarget(v) {
    try { v ? localStorage.setItem("tzTarget", v) : localStorage.removeItem("tzTarget"); } catch {}
  },
  // 某一时刻在指定时区的墙上时间
  parts(tz, ms) {
    const f = new Intl.DateTimeFormat("en-US", {
      timeZone: tz, hourCycle: "h23",
      year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit",
    });
    const o = {};
    for (const p of f.formatToParts(ms)) if (p.type !== "literal") o[p.type] = Number(p.value);
    return o;
  },
  // 该时刻目标时区相对 UTC 的偏移（毫秒）
  offset(tz, ms) {
    const p = this.parts(tz, ms);
    return Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second) - Math.floor(ms / 1000) * 1000;
  },
  // 当地时间 → 真实时刻：先按 UTC 猜一次，再用该时刻的偏移修正一次（夏令时切换那天也对）
  instant(tz, y, mo, d, h, mi) {
    const wall = Date.UTC(y, mo - 1, d, h, mi);
    return wall - this.offset(tz, wall - this.offset(tz, wall));
  },
  // 目标时间的真实时刻；没设置或不合法返回 0
  targetMs(zone = this.zone()) {
    const z = this.ZONES[zone];
    if (!z) return 0;
    const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(this.targetInput());
    if (!m) return 0;
    return this.instant(z.tz, +m[1], +m[2], +m[3], +m[4], +m[5]);
  },
  hhmmss(zone = this.zone(), now = Date.now()) {
    const z = this.ZONES[zone];
    if (!z) return "";
    const p = this.parts(z.tz, now);
    const two = (n) => String(n).padStart(2, "0");
    return `${two(p.hour)}:${two(p.minute)}:${two(p.second)}`;
  },
  // 剩余时间 → hh:mm，超过 24 小时按累计小时显示（27:12），不回绕成「1 天」
  left(ms) {
    const sec = Math.max(0, Math.floor(ms / 1000));
    const two = (n) => String(n).padStart(2, "0");
    return `${two(Math.floor(sec / 3600))}:${two(Math.floor((sec % 3600) / 60))}`;
  },
  // 迷你窗口要显示的内容；时区没选返回 null
  view(zone = this.zone(), now = Date.now()) {
    const z = this.ZONES[zone];
    if (!z) return null;
    const target = this.targetMs(zone);
    const rest = target - now;
    return {
      name: z.short,
      time: this.hhmmss(zone, now),
      left: target && rest > 0 ? this.left(rest) : "",
      target,
    };
  },
};
