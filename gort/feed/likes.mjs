// feed/likes.mjs - first-time likes: node likes.mjs <total> <concurrency> [base]. Random (user, post) pairs over a keep-alive pool; prints status counts and requests/s.
import http from 'node:http';
import fs from 'node:fs';
const [total, conc, base = 'http://127.0.0.1:3000'] = process.argv.slice(2), url = new URL(base);
const tokens = JSON.parse(fs.readFileSync(new URL('../../../challenge/seed/tokens.json', import.meta.url)));
const agent = new http.Agent({ keepAlive: true, maxSockets: +conc }), codes = {}; let sent = 0, done = 0;
const one = () => new Promise(res => {
  const u = tokens[Math.floor(Math.random() * tokens.length)], id = 1 + Math.floor(Math.random() * 500000);
  const r = http.request({ agent, host: url.hostname, port: url.port, method: 'POST', path: `/posts/${id}/like`, headers: { Authorization: 'Bearer ' + u.token } }, x => { x.resume(); x.on('end', () => { codes[x.statusCode] = (codes[x.statusCode] || 0) + 1; res(); }); });
  r.on('error', () => { codes.err = (codes.err || 0) + 1; res(); }); r.end();
});
const t0 = performance.now();
await Promise.all(Array.from({ length: +conc }, async () => { while (sent < +total) { sent++; await one(); done++; } }));
console.log(JSON.stringify(codes), ((done / (performance.now() - t0)) * 1000).toFixed(0), 'req/s (client-bound)');
