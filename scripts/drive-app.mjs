// 通过 WebView2 调试端口在小工具窗口里执行 JS，用于自动化验证。
// 用法: node drive-app.mjs '<js 表达式>'   （端口 9222）
// 用法: node drive-app.mjs <端口> '<js 表达式>'；表达式里可用 i(命令,参数)、t(命令,参数)（报错返回 "ERR …"）、cfg（含端口）
const port = Number(process.argv[2]);
const prelude = `const i=window.__TAURI__.core.invoke;
const cfg={kernel_path:${JSON.stringify(process.env.LOCALAPPDATA + '\\Programs\\SiYuan\\resources\\kernel\\SiYuan-Kernel.exe')},workspace:${JSON.stringify('D:\\user\\Code\\siyuan-pomodoro\\test-workspace')},port:${port}};
const t=async(n,a)=>{try{return await i(n,a)}catch(e){return "ERR "+e}};`;
const expr = `(async()=>{${prelude} return ${process.argv[3]}})()`;
const targets = await (await fetch('http://127.0.0.1:9222/json')).json();
// 环境变量 TARGET=mini 选迷你窗口，默认选主窗口
const want = process.env.TARGET;
const page = targets.find((t) => t.type === 'page' && (want ? t.url.includes(want) : !/mini\.html|notes\.html/.test(t.url)));
if (!page) throw new Error('没有找到页面');
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
ws.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression: expr, awaitPromise: true, returnByValue: true } }));
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id !== 1) return;
  const r = d.result;
  console.log(r.exceptionDetails ? 'EXCEPTION: ' + JSON.stringify(r.exceptionDetails.exception?.value ?? r.exceptionDetails) : JSON.stringify(r.result.value, null, 1));
  ws.close();
};
