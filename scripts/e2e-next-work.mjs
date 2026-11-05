// 端到端验证「休息结束后一键接续番茄」：通过 WebView2 的 CDP 直接调后端命令。
// 用法：先用 --remote-debugging-port=9222 启动番茄钟，再 node scripts/e2e-next-work.mjs
const CDP = 'http://127.0.0.1:9222';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const targets = await (await fetch(`${CDP}/json/list`)).json();
const page = targets.find((t) => t.type === 'page' && t.webSocketDebuggerUrl);
if (!page) { console.log('没有找到可调试的页面'); process.exit(1); }
console.log('目标页面:', page.url);

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });

let id = 0;
const pending = new Map();
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
};
const send = (method, params) => new Promise((res) => {
  const myId = ++id;
  pending.set(myId, res);
  ws.send(JSON.stringify({ id: myId, method, params }));
});

const evaluate = async (expression) => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.result?.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails));
  return r.result?.result?.value;
};

const script = `(async () => {
  const { invoke } = window.__TAURI__.core;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const waitFor = async (pred, ms = 20000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      const s = await invoke('timer_get');
      if (pred(s)) return s;
      await sleep(200);
    }
    throw new Error('等待状态超时');
  };
  const out = {};

  // 0) 清场
  try { const s = await invoke('timer_get'); if (s) { await invoke('timer_discard').catch(() => {}); await invoke('timer_clear').catch(() => {}); } } catch {}

  // 1) 3 秒的番茄（0.05 分钟）
  await invoke('timer_start', { task: { id: '', title: 'E2E 接续测试', block_id: null }, minutes: 0.05, port: 6806 });
  let s = await invoke('timer_get');
  out.workStarted = { kind: s.kind, planned_ms: s.planned_ms, work_ms: s.work_ms, title: s.task.title };
  await waitFor((x) => x && x.status === 'finished');
  await invoke('timer_submit_note', { note: 'E2E 接续测试' });
  s = await waitFor((x) => x && x.written);
  out.workWritten = { written: s.written, duration: s.duration };

  // 2) 3 秒的短休息
  await invoke('timer_start_break', { minutes: 0.05, long: false, port: 6806 });
  s = await invoke('timer_get');
  out.breakStarted = { kind: s.kind, planned_ms: s.planned_ms, work_ms: s.work_ms, title: s.task.title };
  await waitFor((x) => x && x.status === 'finished' && x.written);
  s = await invoke('timer_get');
  out.breakFinished = { kind: s.kind, written: s.written, work_ms: s.work_ms };

  // 3) 一键接续
  await invoke('timer_next_work', { minutes: null, port: 6806 });
  s = await invoke('timer_get');
  out.afterNext = { status: s.status, kind: s.kind, planned_ms: s.planned_ms, title: s.task.title, taskId: s.task.id, work_ms: s.work_ms };

  // 4) 收尾：提前结束并放弃，避免留下正在跑的计时
  await invoke('timer_end');
  await sleep(500);
  await invoke('timer_discard').catch(() => {});
  out.cleanedUp = (await invoke('timer_get')) === null;
  return out;
})()`;

const result = await evaluate(script);
console.log(JSON.stringify(result, null, 2));
ws.close();
