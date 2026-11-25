// bench/loadmp.mjs - the challenge's user model (bench/load.js) spread over several node processes, for user counts k6 cannot hold in memory here.
//   node loadmp.mjs <vus> <rampSec> <holdSec> <base> <serverPid> <clientCpus> [usersPerProc=20000]
// Exit 0 if p95 < 500 ms, p99 < 1 s and under 1% of requests failed; the last stdout line is a JSON summary. A user = one keep-alive connection:
// GET /feed, think 3-7 s, GET /posts/:id, think 3-8 s, 15% like, 2% post, idle 5-15 s, repeat. Each user binds to a loopback source address
// (one per 20,000 users) because one source address has about 28,000 ports to one destination.
// Across a network: LOCAL_IPS=a,b,c binds users round-robin to those local addresses (add them with `ip addr add`), and SERVER_SSH=host reads the
// server's CPU and memory from /proc over ssh (serverPid is then the pid on that host).
import http from 'node:http';
import fs from 'node:fs';
import { spawn, execFileSync } from 'node:child_process';

const NB = 2000, bucket = ms => Math.min(NB - 1, Math.round(100 * Math.log(ms * 1000 + 1))), fromBucket = b => Math.exp(b / 100) / 1000;   // 1% resolution, in ms

if (process.argv[2] === 'worker') {
  const { from, count, total, ramp, hold, base, localIps } = JSON.parse(process.argv[3]);
  const url = new URL(base), tokens = JSON.parse(fs.readFileSync(new URL('../../../../challenge/seed/tokens.json', import.meta.url)));
  const hist = {}, perSec = [], bad = { n: 0 }, errs = {}; let stop = false; const t0 = Date.now();
  const between = (lo, hi) => lo + Math.random() * (hi - lo), sleep = ms => new Promise(r => setTimeout(r, ms));
  const call = (agent, name, method, path, token, body) => new Promise(resolve => {
    const s = performance.now(); let done = false;
    const fin = (status, text, why) => {
      if (done) return; done = true;
      if (why || !(status >= 200 && status < 400)) errs[why || 'http ' + status] = (errs[why || 'http ' + status] || 0) + 1;
      const b = bucket(performance.now() - s); (hist[name] ??= new Int32Array(NB))[b]++; (hist.ALL ??= new Int32Array(NB))[b]++;
      const sec = Math.floor((Date.now() - t0) / 1000); perSec[sec] = (perSec[sec] || 0) + 1;
      if (!(status >= 200 && status < 400)) bad.n++;
      resolve({ status, text });
    };
    const req = http.request({ agent, host: url.hostname, port: url.port, method, path, timeout: 15000,
      headers: token ? { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json', ...(body ? { 'Content-Length': Buffer.byteLength(body) } : {}) } : {} }, res => {
      let s = ''; res.setEncoding('utf8'); res.on('data', d => s += d); res.on('end', () => fin(res.statusCode, s));
    });
    req.on('error', e => fin(0, '', e.code || e.message)); req.on('timeout', () => { req.destroy(); fin(0, '', 'timeout'); });
    req.end(body);
  });
  const user = async n => {
    const u = tokens[n % tokens.length];
    const agent = new http.Agent({ keepAlive: true, maxSockets: 1, localAddress: localIps.length ? localIps[n % localIps.length] : url.hostname === '127.0.0.1' ? `127.0.${1 + Math.floor(n / 20000)}.1` : undefined });
    await sleep(ramp * 1000 * n / total + between(0, 5000));                  // spread over the ramp, then up to 5 s more (as load.js does)
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
  };
  for (let i = 0; i < count; i++) user(from + i);
  setTimeout(() => { stop = true }, (ramp + hold) * 1000);
  await sleep((ramp + hold) * 1000 + 200);
  process.stdout.write(JSON.stringify({ hist: Object.fromEntries(Object.entries(hist).map(([k, v]) => [k, Array.from(v)])), perSec, bad: bad.n, errs }) + '\n');
  process.exit(0);
}

const [vus, rampS, holdS, base, serverPid, cpus, per = '20000'] = process.argv.slice(2);
const V = +vus, RAMP = +rampS, HOLD = +holdS, nproc = Math.ceil(V / +per);
const localIps = (process.env.LOCAL_IPS || '').split(',').filter(Boolean), sshHost = process.env.SERVER_SSH;
const proc = (pid, file) => sshHost ? execFileSync('ssh', ['-o', 'BatchMode=yes', sshHost, 'cat', `/proc/${pid}/${file}`], { encoding: 'utf8' }) : fs.readFileSync(`/proc/${pid}/${file}`, 'utf8');
const cpu = pid => { try { const f = proc(pid, 'stat').split(') ')[1].split(' '); return +f[11] + +f[12]; } catch { return NaN; } };   // utime + stime, ticks
const tick = 100, hwm = pid => { try { return +/VmHWM:\s+(\d+)/.exec(proc(pid, 'status'))[1] / 1024; } catch { return NaN; } };
const outs = [], procs = [];
for (let i = 0; i < nproc; i++) {
  const from = i * +per, count = Math.min(+per, V - from);
  const p = spawn('taskset', ['-c', cpus, process.execPath, new URL(import.meta.url).pathname, 'worker', JSON.stringify({ from, count, total: V, ramp: RAMP, hold: HOLD, base, localIps })], { stdio: ['ignore', 'pipe', 'inherit'] });
  let o = ''; p.stdout.on('data', d => o += d); procs.push(new Promise(r => p.on('close', () => { outs.push(o); r(); })));
}
let c0 = NaN; setTimeout(() => { c0 = cpu(serverPid) }, RAMP * 1000);                // the CPU the server uses during the hold only
const t0 = Date.now(); await Promise.all(procs); const c1 = cpu(serverPid);
const merged = {}, perSec = []; let bad = 0; const errs = {};
for (const o of outs) {
  if (!o.trim()) { bad += 1e9; continue; }                                            // a worker that died counts as a failed run
  const r = JSON.parse(o.trim().split('\n').pop()); bad += r.bad; for (const [k, c] of Object.entries(r.errs || {})) errs[k] = (errs[k] || 0) + c;
  for (const [k, v] of Object.entries(r.hist)) { const m = merged[k] ??= new Array(NB).fill(0); v.forEach((c, i) => m[i] += c); }
  r.perSec.forEach((c, i) => { if (c) perSec[i] = (perSec[i] || 0) + c; });
}
const pct = (h, p) => { const n = h.reduce((a, b) => a + b, 0); let s = 0; for (let i = 0; i < NB; i++) { s += h[i]; if (s >= n * p) return fromBucket(i); } return NaN; };
const total = merged.ALL ? merged.ALL.reduce((a, b) => a + b, 0) : 0;
const holdReq = perSec.slice(RAMP, RAMP + HOLD).reduce((a, b) => a + (b || 0), 0), rps = holdReq / HOLD;
const s = { vus: V, requests: total, rps: Math.round(rps), failed: bad, errPct: +(100 * bad / Math.max(1, total)).toFixed(3), p50: pct(merged.ALL || [0], .5), p95: pct(merged.ALL || [0], .95), p99: pct(merged.ALL || [0], .99),
  serverCpuPct: +(100 * (c1 - c0) / tick / HOLD).toFixed(1), usPerReq: +(1e6 * (c1 - c0) / tick / Math.max(1, holdReq)).toFixed(1), serverPeakMB: Math.round(hwm(serverPid)), elapsedS: Math.round((Date.now() - t0) / 1000) };
s.errs = errs;
s.pass = total > 0 && s.p95 < 500 && s.p99 < 1000 && s.errPct < 1;
for (const [k, h] of Object.entries(merged)) if (k !== 'ALL') console.log(`  ${k.padEnd(22)} n=${String(h.reduce((a, b) => a + b, 0)).padStart(8)}  p95=${pct(h, .95).toFixed(2)}  p99=${pct(h, .99).toFixed(2)} ms`);
console.log(JSON.stringify(s)); process.exit(s.pass ? 0 : 1);
