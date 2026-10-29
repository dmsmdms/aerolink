// End-to-end smoke test of the operator console (run via tools/e2e_console.sh): real backend, real console
// build, and a fake robot = Node signaling client + a Chromium tab doing WebRTC with a fake camera. The fake
// robot is also the reference for the robot side of docs/08-backend.md ("Signaling", "Control channel").
// Node ≥ 22 (global WebSocket with headers), Chromium driven over the DevTools protocol, no npm packages.
import { spawn } from 'node:child_process';
import { writeFileSync } from 'node:fs';

const [, , robotToken, outDir, consolePort, backendPort] = process.argv;
const BASE = `http://localhost:${consolePort}`;
let failures = 0;
const check = (ok, what) => {
  console.log(ok ? '  ✓' : '  ✗', what);
  if (!ok) failures++;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log('[e2e]', ...a);

// --- Chromium + minimal CDP client ---
const chrome = spawn(process.env.CHROMIUM ?? 'chromium', [
  '--headless=new', '--remote-debugging-port=9333', `--user-data-dir=${outDir}/profile`, '--no-first-run',
  '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', '--autoplay-policy=no-user-gesture-required',
  '--window-size=1280,900', 'about:blank',
], { stdio: 'ignore' });
process.on('exit', () => chrome.kill());

async function cdpTargets() {
  for (let i = 0; i < 50; i++) {
    try { return await (await fetch('http://127.0.0.1:9333/json')).json(); } catch { await sleep(100); }
  }
  throw new Error('chromium did not start');
}

async function page(url) {
  await cdpTargets();
  const target = await (await fetch(`http://127.0.0.1:9333/json/new?${encodeURIComponent(url)}`, { method: 'PUT' })).json();
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));
  let id = 0;
  const waiting = new Map();
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
    if (m.method === 'Runtime.exceptionThrown') log('[exception]', JSON.stringify(m.params.exceptionDetails).slice(0, 400));
  };
  const send = (method, params = {}) => new Promise((resolve) => { const i = ++id; waiting.set(i, resolve); ws.send(JSON.stringify({ id: i, method, params })); });
  await send('Runtime.enable');
  const evaluate = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.result?.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails).slice(0, 500));
    return r.result?.result?.value;
  };
  const until = async (expression, what, ms = 8000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) { if (await evaluate(expression)) return; await sleep(100); }
    throw new Error(`timeout: ${what}`);
  };
  return { send, evaluate, until };
}

// --- fake robot: signaling over WS (Node), WebRTC in a tab ---
const robotPage = await page(`${BASE}/robots.txt`);
await sleep(500);
await robotPage.evaluate(`(() => {
  window.drive = []; window.ctl = []; window.early = []; window.state = 'DISARMED'; window.out = []; window.light = false;
  window.offers = 0;
  window.robot = {
    async offer(sdp, iceServers) {
      window.offers++;
      if (window.pc) { // ICE restart (SIG-007): same connection, new ICE credentials
        await window.pc.setRemoteDescription(sdp);
        await window.pc.setLocalDescription(await window.pc.createAnswer());
        return window.pc.localDescription.toJSON();
      }
      const pc = new RTCPeerConnection({ iceServers });
      window.pc = pc;
      const media = await navigator.mediaDevices.getUserMedia({ video: true, audio: true });
      for (const track of media.getTracks()) pc.addTrack(track, media);
      pc.onicecandidate = (e) => { if (e.candidate) window.out.push({ type: 'ice', candidate: e.candidate.toJSON() }); };
      pc.ondatachannel = ({ channel }) => {
        if (channel.label === 'drive') channel.onmessage = (e) => window.drive.push(JSON.parse(e.data));
        if (channel.label === 'ctl') {
          const tele = () => channel.readyState === 'open' && channel.send(JSON.stringify({ t: 'telemetry', base: true, state: window.state, faults: 0, speed_l: window.drive.at(-1)?.speed ?? 0, speed_r: window.drive.at(-1)?.speed ?? 0, battery_mv: 36500, phone_battery: 55, light: window.light }));
          channel.onopen = () => setInterval(tele, 200);
          channel.onmessage = (e) => {
            const m = JSON.parse(e.data);
            window.ctl.push(m.t);
            if (m.t === 'ping') channel.send(JSON.stringify({ t: 'pong', ts: m.ts }));
            if (m.t === 'arm') window.state = 'ARMED';
            if (m.t === 'disarm') window.state = 'DISARMED';
            if (m.t === 'light') window.light = m.on;
          };
        }
      };
      await pc.setRemoteDescription(sdp);
      for (const c of window.early.splice(0)) await pc.addIceCandidate(c);
      await pc.setLocalDescription(await pc.createAnswer());
      return pc.localDescription.toJSON();
    },
    ice(c) { if (!window.pc?.remoteDescription) { window.early.push(c); return; } return window.pc.addIceCandidate(c); },
  };
})()`);

let sig = null;
const sigSend = (m) => sig.send(JSON.stringify(m));
let session = null;
let ended = false;
let resumed = false;
let endReason = null;
/** `resume`: the session to keep across the reconnect (SIG-007), as the phone does after a network change. */
async function connectRobot(resume) {
  const query = resume ? `?session=${resume}` : '';
  sig = new WebSocket(`ws://127.0.0.1:${backendPort}/ws/robot${query}`, { headers: { authorization: `Bearer ${robotToken}` } });
  await new Promise((resolve, reject) => { sig.onopen = resolve; sig.onerror = reject; });
  sig.onmessage = async (e) => {
    const m = JSON.parse(e.data);
    if (m.type === 'welcome') sigSend({ type: 'status', mode: 'NEEDS_HELP', phone_battery: 55, robot_battery_mv: 36500 });
    if (m.type === 'session') {
      resumed = session?.session === m.session;
      session = m;
    }
    if (m.type === 'end') {
      ended = m.reason === 'released';
      endReason = m.reason;
      await robotPage.evaluate('window.pc?.close(); window.pc = null');
    }
    if (m.type === 'offer') {
      const answer = await robotPage.evaluate(`robot.offer(${JSON.stringify(m.sdp)}, ${JSON.stringify(session.ice_servers)})`);
      sigSend({ type: 'answer', session: m.session, sdp: answer });
    }
    if (m.type === 'ice') await robotPage.evaluate(`robot.ice(${JSON.stringify(m.candidate)})`);
  };
}
await connectRobot(null);
setInterval(async () => {
  const out = await robotPage.evaluate('window.out.splice(0)');
  for (const c of out ?? []) if (session) sigSend({ ...c, session: session.session });
}, 100);

// --- operator console ---
const op = await page(`${BASE}/console/`);
await op.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
const shot = async (name) => {
  const r = await op.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(`${outDir}/${name}.png`, Buffer.from(r.result.data, 'base64'));
};
await op.until(`!!document.querySelector('form.login')`, 'login form');
await shot('1-login');
log('login, take the robot, arm, drive with W+D, stop with Space, release');
await op.evaluate(`(() => {
  const set = (sel, v) => { const el = document.querySelector(sel); el.value = v; el.dispatchEvent(new Event('input', { bubbles: true })); };
  set('input[type=email]', 'op@x.org'); set('input[type=password]', 'password1234');
  document.querySelector('form.login').requestSubmit();
})()`);
await op.until(`document.body.innerText.includes('r1')`, 'robot list');
await op.until(`[...document.querySelectorAll('li button')].some(b => !b.disabled)`, 'robot online');
await shot('2-list');
const listText = await op.evaluate('document.querySelector("aside").innerText');
check(listText.includes('36.5 V') && listText.includes('55%'), 'robot list shows status from the robot');

await op.evaluate(`document.querySelector('li button').click()`);
await op.until(`document.querySelector('video')?.readyState >= 2`, 'video playing', 15000);
check(true, 'video from the robot plays');
await op.until(`[...document.querySelectorAll('button.primary')].some(b => !b.disabled && b.closest('.controls'))`, 'arm enabled');
await op.evaluate(`document.querySelector('.controls button.primary').click()`);
await op.until(`document.querySelector('.drive.active') !== null`, 'armed');
const key = (type, code, shift = false) => op.send('Input.dispatchKeyEvent', { type, code, key: code === 'Space' ? ' ' : code, modifiers: shift ? 8 : 0, windowsVirtualKeyCode: { KeyW: 87, KeyD: 68, Space: 32 }[code] });
await key('keyDown', 'KeyW');
await key('keyDown', 'KeyD');
await sleep(600);
await shot('3-driving');
await key('keyUp', 'KeyD');
await key('keyUp', 'KeyW');
await sleep(200);
const drive = await robotPage.evaluate('window.drive');
const moving = drive.filter((d) => d.speed === 500 && d.steer === 600);
check(moving.length >= 8, `W+D held 600 ms → ${moving.length} DRIVE(speed 500, steer 600)`);
check(drive.every((d, i) => i === 0 || d.seq > drive[i - 1].seq), 'DRIVE seq strictly increasing');
const period = drive.length > 1 ? (drive.at(-1).ts - drive[0].ts) / (drive.length - 1) : NaN;
check(Math.abs(period - 50) < 10, `DRIVE period ${period.toFixed(1)} ms (20 Hz)`);
await key('keyDown', 'Space');
await key('keyUp', 'Space');
await op.until(`document.querySelector('.drive.active') === null`, 'disarmed by space');
const before = (await robotPage.evaluate('window.drive')).length;
await sleep(500);
const after = (await robotPage.evaluate('window.drive')).length;
check(after - before === 0, `no DRIVE while disarmed (${after - before})`);
const panel = await op.evaluate('document.querySelector(".panel").innerText');
check(/connected/.test(panel) && /36\.5/.test(panel), 'panel shows link state and telemetry');
const ctl = new Set(await robotPage.evaluate('window.ctl'));
check(['arm', 'ping', 'disarm'].every((t) => ctl.has(t)), 'ctl channel carried arm, ping, disarm');
await shot('4-stopped');

log('light, talk, then the robot reconnects (network change) and keeps the session');
await op.until(`[...document.querySelectorAll('.controls button')].some(b => b.innerText.startsWith('Light'))`, 'light button');
await key('keyDown', 'KeyL');
await key('keyUp', 'KeyL');
await op.until(`[...document.querySelectorAll('.controls button')].some(b => b.innerText.startsWith('Light off'))`, 'light on');
check(await robotPage.evaluate('window.light'), 'L switched the robot light on, the console shows it');
const audioIn = `(async () => { let b = 0; (await window.pc.getStats()).forEach((s) => { if (s.type === 'inbound-rtp' && s.kind === 'audio') b = s.bytesReceived; }); return b; })()`;
const silent = await robotPage.evaluate(audioIn);
await key('keyDown', 'KeyT');
await key('keyUp', 'KeyT');
await op.until(`[...document.querySelectorAll('.controls button')].some(b => b.innerText.startsWith('Stop talking'))`, 'talking');
await sleep(1500);
const talked = await robotPage.evaluate(audioIn);
check(talked - silent > 2000, `T sends the operator's voice to the robot (${talked - silent} bytes)`);
await key('keyDown', 'KeyT');
await key('keyUp', 'KeyT');
await op.until(`[...document.querySelectorAll('.controls button')].some(b => b.innerText.startsWith('Talk'))`, 'talk off');

const offersBefore = await robotPage.evaluate('window.offers');
// On a real network change the old connection just goes silent: the new one replaces it on the server.
const stale = sig;
await connectRobot(session.session);
stale.close();
for (let i = 0; i < 50 && (await robotPage.evaluate('window.offers')) === offersBefore; i++) await sleep(100);
check(resumed, 'reconnected robot got its session back');
check((await robotPage.evaluate('window.offers')) === offersBefore + 1, 'console restarted ICE with a new offer');
await sleep(1000);
const panelAfter = await op.evaluate('document.querySelector(".panel")?.innerText ?? ""');
check(/connected/.test(panelAfter) && (await op.evaluate(`document.querySelector('video')?.readyState >= 2`)), 'session and video still up');

// Release → robot hears "end", list shows robot free again.
await op.evaluate(`[...document.querySelectorAll('.title button')].find(b => b).click()`);
await op.until(`!document.querySelector('.panel')`, 'panel closed');
await sleep(300);
check(ended, 'robot heard the release');

log('admin: fleet table, map, robot events; take the robot over from the operator');
sigSend({ type: 'location', lat: 50.4501, lon: 30.5234, accuracy_m: 6, at: Math.floor(Date.now() / 1000) });
sigSend({ type: 'event', level: 'error', code: 'base_fault', message: 'motor_fault', at: Math.floor(Date.now() / 1000) });
await op.evaluate(`document.querySelector('li button').click()`);
await op.until(`document.querySelector('video')?.readyState >= 2`, 'operator drives again', 15000);
const admin = await page(`${BASE}/robots.txt`);
await admin.send('Network.enable');
await admin.send('Network.setBlockedURLs', { urls: ['*tile.openstreetmap.org*'] }); // no load on OSM from tests
await admin.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
// One browser profile: logging in here replaces the operator's cookie, their open WebSocket stays as it is.
await admin.evaluate(`fetch('/api/v1/login', { method: 'POST', headers: { 'content-type': 'application/json' },
  body: JSON.stringify({ email: 'admin@x.org', password: 'password1234' }) }).then((r) => r.status)`);
await admin.evaluate(`location.href = '/console/'`);
await admin.until(`[...document.querySelectorAll('nav button')].length === 4`, 'admin tabs');
await admin.evaluate(`[...document.querySelectorAll('nav button')][1].click()`);
await admin.until(`document.querySelector('table tbody tr')?.innerText.includes('r1')`, 'fleet table');
const row = await admin.evaluate(`document.querySelector('table tbody tr').innerText`);
check(row.includes('Op') && row.includes('1'), 'fleet table: r1 driven by Op, one unread error');
check(await admin.evaluate(`document.querySelector('.marker')?.innerText === 'r1'`), 'map shows r1 at its position');
await admin.evaluate(`document.querySelector('table tbody tr').click()`);
await admin.until(`document.querySelector('.events')?.innerText.includes('base_fault')`, 'robot events');
check(true, 'robot page lists the error event');
const shotOf = async (p, name) => {
  const r = await p.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(`${outDir}/${name}.png`, Buffer.from(r.result.data, 'base64'));
};
await shotOf(admin, '5-fleet');
await admin.evaluate(`window.confirm = () => true`);
await admin.evaluate(`[...document.querySelectorAll('section .actions button')].find(b => b.innerText === 'Take over').click()`);
await admin.until(`document.querySelector('video')?.readyState >= 2`, 'admin gets video', 15000);
check(endReason === 'admin', 'robot heard end(admin) before the admin session');
check(session.peer.name === 'Admin', 'new session belongs to the admin');
await op.until(`document.body.innerText.includes('An administrator ended the session.')`, 'operator notice');
check(!(await op.evaluate(`!!document.querySelector('.panel')`)), 'operator lost the robot');
await shotOf(admin, '6-admin-drives');
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
