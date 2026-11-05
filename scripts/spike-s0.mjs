// S0.5 spike：同步进行中写番茄记录，检查记录是否丢失/重复。
// 只用一个合成 eventId（free_spike_*），测完用 delete_focus 清理，不碰真实任务。
import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

const port = process.argv[2] || '6806';
const base = `http://127.0.0.1:${port}`;
const cfg = JSON.parse(readFileSync(join(process.env.APPDATA || join(homedir(), 'AppData/Roaming'), 'com.local.siyuan-pomodoro', 'config.json'), 'utf8'));
const auth = { Authorization: `Token ${cfg.api_token}` };

const j = async (r) => { const t = await r.text(); const s = t.trim(); if (!s) return null; return JSON.parse(s.replace(/^data: /m, '')); };

// ---- MCP：调插件工具 ----
let sid = null;
const mcp = async (body) => {
  const r = await fetch(`${base}/mcp`, {
    method: 'POST',
    headers: { ...auth, 'Content-Type': 'application/json', Accept: 'application/json, text/event-stream', ...(sid ? { 'Mcp-Session-Id': sid } : {}) },
    body: JSON.stringify(body),
  });
  sid = r.headers.get('mcp-session-id') || sid;
  return j(r);
};
await mcp({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-03-26', capabilities: {}, clientInfo: { name: 'spike', version: '0' } } });
await mcp({ jsonrpc: '2.0', method: 'notifications/initialized' });
const list = await mcp({ jsonrpc: '2.0', id: 2, method: 'tools/list' });
if (!list?.result?.tools) { console.log('TOOLS/LIST 失败:', JSON.stringify(list)); process.exit(1); }
const statsTool = list.result.tools.find((t) => t.name.includes('__stats__')).name;

const callStats = async (args) => {
  const res = await mcp({ jsonrpc: '2.0', id: 3, method: 'tools/call', params: { name: statsTool, arguments: args } });
  const text = res?.result?.content?.[0]?.text;
  return text ? JSON.parse(text) : res;
};

const today = new Date().toISOString().slice(0, 10);
const eventId = `free_spike_${Date.now()}`;
const end = new Date();
const start = new Date(end.getTime() - 2 * 60000);

const before = await callStats({ action: 'get_focuses_by_time', startDate: today, endDate: today });
console.log('基线：今天记录数 =', (before.data || []).length);

// 1) 先发起同步（不 await）
const syncSw = Date.now();
const syncP = fetch(`${base}/api/sync/performSync`, {
  method: 'POST', headers: { ...auth, 'Content-Type': 'application/json' }, body: '{}',
}).then(async (r) => ({ status: r.status, body: await r.text(), ms: Date.now() - syncSw }));

// 2) 同步进行中写一条记录
await new Promise((r) => setTimeout(r, 800));
const createSw = Date.now();
const created = await callStats({
  action: 'create_focus', type: 'work', eventId, eventTitle: 'spike-S0.5',
  startTime: start.toISOString(), endTime: end.toISOString(), duration: 2, plannedDuration: 2,
  completed: true, note: 'spike S0.5 同步并发测试',
});
const createMs = Date.now() - createSw;
console.log('同步进行中 create_focus：耗时', createMs, 'ms →', JSON.stringify(created).slice(0, 200));

// 3) 等同步结束
const sync = await syncP;
console.log('同步结果：HTTP', sync.status, sync.ms + 'ms', sync.body.slice(0, 120));

// 4) 校验记录是否还在、是否重复
const after = await callStats({ action: 'get_focuses_by_time', startDate: today, endDate: today });
const mine = (after.data || []).filter((s) => s.eventId === eventId);
console.log('同步后：今天记录数 =', (after.data || []).length, '| 本次记录条数 =', mine.length);
console.log('记录内容：', JSON.stringify(mine));

// 5) 清理
for (const s of mine) {
  const del = await callStats({ action: 'delete_focus', id: s.id });
  console.log('清理 delete_focus', s.id, '→', JSON.stringify(del).slice(0, 120));
}
const final = await callStats({ action: 'get_focuses_by_time', startDate: today, endDate: today });
console.log('清理后：今天记录数 =', (final.data || []).length);
