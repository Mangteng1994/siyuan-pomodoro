// 在普通浏览器里测悬浮笔记页面：给 notes.html 注入一个假的 __TAURI__（数据只存在内存里），不碰真实数据。
// 用法: node serve-notes-mock.mjs [端口，默认 8099]，然后打开 http://127.0.0.1:8099/
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const dir = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'app', 'src');
const stub = `<script>
window.__MOCK_SAVES = [];
let store = [];
window.__TAURI__ = {
  core: { invoke: async (cmd, args) => {
    if (cmd === 'notes_get') return JSON.parse(JSON.stringify(store));
    if (cmd === 'notes_save') { store = JSON.parse(JSON.stringify(args.items)); window.__MOCK_SAVES.push(store.map(i => i.text)); return null; }
    throw new Error('mock: ' + cmd);
  } },
  event: { listen: async () => () => {} },
};
</script>`;
http.createServer((req, res) => {
  const html = fs.readFileSync(path.join(dir, 'notes.html'), 'utf8').replace('<body>', '<body>' + stub);
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
  res.end(html);
}).listen(Number(process.argv[2] || 8099), '127.0.0.1', () => console.log('mock notes page on http://127.0.0.1:' + (process.argv[2] || 8099)));
