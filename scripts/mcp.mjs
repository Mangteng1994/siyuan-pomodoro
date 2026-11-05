// 用法: node mcp.mjs <task|stats|project|habit> '<json 参数>' [端口]
// 内核开了访问授权码时，需要带 API token：从番茄钟的 config.json 里读（可用 SIYUAN_TOKEN 覆盖）
import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

const [,, kind, argsJson = '{}', port = '6807'] = process.argv;
const url = `http://127.0.0.1:${port}/mcp`;
const token = process.env.SIYUAN_TOKEN || (() => {
  try {
    const dir = join(process.env.APPDATA || join(homedir(), 'AppData/Roaming'), 'com.local.siyuan-pomodoro');
    return JSON.parse(readFileSync(join(dir, 'config.json'), 'utf8')).api_token || '';
  } catch { return ''; }
})();
const headers = {
  'Content-Type': 'application/json',
  Accept: 'application/json, text/event-stream',
  ...(token ? { Authorization: `Token ${token}` } : {}),
};
const post = async (body, sid) => {
  const r = await fetch(url, { method: 'POST', headers: sid ? { ...headers, 'Mcp-Session-Id': sid } : headers, body: JSON.stringify(body) });
  const text = await r.text();
  return { sid: r.headers.get('mcp-session-id'), json: text ? JSON.parse(text.replace(/^data: /m, '').trim() || 'null') : null };
};
const init = await post({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-03-26', capabilities: {}, clientInfo: { name: 'pomodoro', version: '0' } } });
const sid = init.sid;
await post({ jsonrpc: '2.0', method: 'notifications/initialized' }, sid);
const list = await post({ jsonrpc: '2.0', id: 2, method: 'tools/list' }, sid);
const tool = list.json.result.tools.find(t => t.name.includes(`__${kind}__`));
if (!tool) throw new Error(`找不到工具 ${kind}`);
const res = await post({ jsonrpc: '2.0', id: 3, method: 'tools/call', params: { name: tool.name, arguments: JSON.parse(argsJson) } }, sid);
console.log(JSON.stringify(res.json.result ?? res.json.error, null, 2));
