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
  info(s) {
    if (!s) return "";
    if (s.status === "finished") {
      const head = `${s.completed ? "已完成" : "已提前结束"}：${s.task.title}（实际 ${(s.actual_ms / 60000).toFixed(1)} 分钟）　`;
      if (s.written) return head + `✔ 已写入记录（${s.duration} 分钟）`;
      if (s.writing) return head + "写入记录中…";
      return head + `✘ 写入失败：${s.error ?? "尚未写入"}（会每 30 秒自动重试，也可手动重试）`;
    }
    return `${s.status === "paused" ? "已暂停" : "计时中"}：${s.task.title}`;
  },
  flags(s) {
    const finished = !!s && s.status === "finished";
    const pending = finished && !s.written && !s.writing;
    return {
      canStart: !s,
      canPause: !!s && !finished,
      pauseLabel: s && s.status === "paused" ? "继续" : "暂停",
      canEnd: !!s && !finished,
      canClear: finished && s.written,
      canRetry: pending,
      canDiscard: pending,
    };
  },
};
