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
      if (s.awaiting_note) return head + `等你填备注（${Math.max(0, Math.ceil((s.note_deadline - Date.now()) / 1000))} 秒后自动跳过）`;
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
      awaitingNote,
      canRetry: pending,
      canDiscard: pending,
    };
  },
};
