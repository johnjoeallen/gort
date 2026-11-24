// feed/load.mjs - the challenge's bench/load.js user model without k6: node load.mjs <vus> <rampSec> <holdSec> [baseUrl] [tokens.json]
// One virtual user = one keep-alive connection: GET /feed, think 3-7 s, GET /posts/:id, think 3-8 s, 15% like, 2% post, idle 5-15 s, repeat.
import http from 'node:http';
import fs from 'node:fs';
const [vus, rampS, holdS, base = 'http://127.0.0.1:3000', tokFile = '../../../challenge/seed/tokens.json'] = process.argv.slice(2);
const VUS = +vus, RAMP = +rampS * 1000, HOLD = +holdS * 1000, url = new URL(base);
const tokens = JSON.parse(fs.readFileSync(new URL(tokFile, import.meta.url)));
const lat = {}, bad = { n: 0 }; let total = 0, stop = false;
const between = (lo, hi) => lo + Math.random() * (hi - lo);
const sleep = ms => new Promise(r => setTimeout(r, ms));

function call(agent, name, method, path, token, body) {
  return new Promise(resolve => {
    const t0 = performance.now(); let done = false;
    const fin = (status, text) => {
      if (done) return; done = true;
      (lat[name] ??= []).push(performance.now() - t0); total++;
      const ok = name === 'POST /posts' ? status === 201 : name.startsWith('POST') ? status === 200 || status === 201 : status === 200;
      if (!ok) bad.n++;
      resolve({ status, text });
    };
    const req = http.request({ agent, host: url.hostname, port: url.port, method, path, timeout: 10000,
      headers: token ? { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json', ...(body ? { 'Content-Length': Buffer.byteLength(body) } : {}) } : {} }, res => {
      let s = ''; res.setEncoding('utf8'); res.on('data', d => s += d); res.on('end', () => fin(res.statusCode, s));
    });
    req.on('error', () => fin(0, '')); req.on('timeout', () => { req.destroy(); fin(0, ''); });
    req.end(body);
  });
}

async function user(n) {
  const u = tokens[n % tokens.length], agent = new http.Agent({ keepAlive: true, maxSockets: 1, localAddress: url.hostname === '127.0.0.1' ? `127.0.${1 + Math.floor(n / 20000)}.1` : undefined })   // one source address per 20000 users: a source has ~28k ports to one destination;
  await sleep(RAMP * n / VUS);                                   // the ramp: users start spread over RAMP
  await sleep(between(0, 5000));
  while (!stop) {
    const feed = await call(agent, 'GET /feed', 'GET', '/feed');
    let posts; try { posts = JSON.parse(feed.text).posts } catch { await sleep(between(3000, 7000)); continue }
    const post = posts[Math.floor(Math.random() * posts.length)];
    await sleep(between(3000, 7000));
    await call(agent, 'GET /posts/:id', 'GET', '/posts/' + post.id);
    await sleep(between(3000, 8000));
    if (Math.random() < 0.15) await call(agent, 'POST /posts/:id/like', 'POST', `/posts/${post.id}/like`, u.token);
    if (Math.random() < 0.02) await call(agent, 'POST /posts', 'POST', '/posts', u.token, JSON.stringify({ body: `${u.username} says hi at ${new Date().toISOString()} (VU ${n})` }));
    await sleep(between(5000, 15000));
  }
  agent.destroy();
}

const pct = (a, p) => a.length ? a[Math.min(a.length - 1, Math.floor(a.length * p))] : NaN;
const t0 = Date.now();
for (let i = 0; i < VUS; i++) user(i);
setTimeout(() => { stop = true }, RAMP + HOLD);
await sleep(RAMP + HOLD + 100);
const secs = (Date.now() - t0) / 1000, all = Object.values(lat).flat().sort((a, b) => a - b);
console.log(`vus=${VUS} requests=${total} (${(total / secs).toFixed(0)}/s) failed=${bad.n} (${(100 * bad.n / total).toFixed(2)}%)`);
for (const [k, v] of [['ALL', all], ...Object.entries(lat)]) { v.sort((a, b) => a - b); console.log(`  ${k.padEnd(22)} n=${String(v.length).padStart(7)}  p50=${pct(v, .5).toFixed(2)}  p95=${pct(v, .95).toFixed(2)}  p99=${pct(v, .99).toFixed(2)}  max=${v[v.length - 1].toFixed(1)} ms`); }
const pass = pct(all, .95) < 500 && pct(all, .99) < 1000 && bad.n / total < 0.01;
console.log(pass ? 'PASS (p95<500, p99<1000, <1% errors)' : 'FAIL'); process.exit(0);
