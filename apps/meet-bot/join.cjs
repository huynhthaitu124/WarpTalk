// Hidden Meet join, then hand over to bot.cjs.
//
//   MEET_URL=https://meet.google.com/xxx BOT_EMAIL=... BOT_PASSWORD=... node apps/meet-bot/join.cjs
//
// HIDE=xvfb (the Docker image, see Dockerfile): headed Chromium on a virtual X display. A raw CDP socket
// (DOM domain only) finds the buttons, then it is closed and the click/typing is real X input via
// xdotool: Meet rejects guests whose input comes over CDP ("You can't join this video call").
// HIDE=window: visible Chrome on this machine, CDP input (debug only, Meet usually rejects it).
// Once admitted ("Leave call" exists) bot.cjs attaches as usual.
// Env: BOT_NAME (WarpTalk Bot), CDP_PORT (9333), HIDE, ADMIT_TIMEOUT_S (600), NO_BOT=1 (only join),
// SHOT_DIR (screenshots on failure).
const { spawn, execFileSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const CHROME = process.env.CHROME_PATH || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const MEET_URL = process.env.MEET_URL;
const BOT_NAME = process.env.BOT_NAME || 'WarpTalk Bot';
const PORT = Number(process.env.CDP_PORT || 9333);
const HIDE = process.env.HIDE || 'window';
const XVFB = HIDE === 'xvfb';
const ADMIT_TIMEOUT = Number(process.env.ADMIT_TIMEOUT_S || 600) * 1000;
const profile = path.join(os.tmpdir(), 'warptalk-meetbot-profile');
const log = (...a) => console.log(new Date().toISOString().slice(11, 23), '[join]', ...a);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
if (!MEET_URL) { console.error('MEET_URL is required'); process.exit(2); }

const version = execFileSync(CHROME, ['--version']).toString().match(/[\d.]+/)?.[0] || '?';
const args = [
  `--user-data-dir=${profile}`, `--remote-debugging-port=${PORT}`, '--no-first-run', '--no-default-browser-check',
  '--autoplay-policy=no-user-gesture-required', '--mute-audio', '--lang=en-US', '--accept-lang=en-US',
  '--window-size=1280,800',
  // Kiosk = no browser UI, so page coordinates are screen coordinates for xdotool. --no-sandbox because
  // the container has no user namespaces; --test-type hides the "unsupported flag" infobar.
  ...(XVFB ? ['--kiosk', '--window-position=0,0', '--no-sandbox', '--test-type', '--disable-dev-shm-usage'] : []),
  MEET_URL,
];

const pageTargetId = async () => {
  const list = await fetch(`http://127.0.0.1:${PORT}/json/list`).then((r) => r.json()).catch(() => null);
  return list?.find((x) => x.type === 'page' && x.url.includes('meet.google.com'))?.id;
};

async function rawCdp() {
  for (let i = 0; i < 50; i++) {
    const list = await fetch(`http://127.0.0.1:${PORT}/json/list`).then((r) => r.json()).catch(() => null);
    const t = list?.find((x) => x.type === 'page' && x.url.includes('meet.google.com'));
    if (t) {
      const ws = new WebSocket(t.webSocketDebuggerUrl);
      await new Promise((ok, bad) => { ws.onopen = ok; ws.onerror = bad; });
      let id = 0; const wait = new Map();
      ws.onmessage = (m) => { const d = JSON.parse(m.data); wait.get(d.id)?.(d); wait.delete(d.id); };
      const send = (method, params = {}) => new Promise((ok) => { wait.set(++id, ok); ws.send(JSON.stringify({ id, method, params })); })
        .then((d) => { if (d.error) throw new Error(`${method}: ${d.error.message}`); return d.result; });
      return { send, close: () => ws.close(), targetId: t.id };
    }
    await sleep(300);
  }
  throw new Error('meet tab never appeared on CDP port ' + PORT);
}

(async () => {
  const chrome = spawn(CHROME, args, { stdio: 'ignore', detached: true });
  chrome.unref();
  log(`chrome ${version} (${HIDE}) pid ${chrome.pid}`);
  // The hidden Chrome is the bot's presence in the call: take it down with this process.
  const killChrome = () => { try { process.kill(-chrome.pid, 'SIGTERM'); } catch { /* already gone */ } };
  process.on('exit', killChrome);
  for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => setTimeout(() => process.exit(0), 400));
  // Meet swaps the page target while it boots; a session opened too early never sees the real DOM,
  // so reattach whenever the page target id changes.
  let cdp = await rawCdp();
  const reattach = async () => {
    const id = await pageTargetId();
    if (id && id !== cdp.targetId) { cdp.close(); cdp = await rawCdp(); log('reattached to new page target'); }
  };
  // XPath search via the DOM domain (no Runtime): returns the centre of the first visible match.
  const find = async (xpath) => {
    await cdp.send('DOM.getDocument', { depth: 0 });
    const { searchId, resultCount } = await cdp.send('DOM.performSearch', { query: xpath });
    try {
      if (!resultCount) return null;
      const { nodeIds } = await cdp.send('DOM.getSearchResults', { searchId, fromIndex: 0, toIndex: Math.min(resultCount, 10) });
      for (const nodeId of nodeIds) {
        const box = await cdp.send('DOM.getBoxModel', { nodeId }).catch(() => null);
        const q = box?.model?.content;
        if (q && box.model.width > 0) return { x: (q[0] + q[4]) / 2, y: (q[1] + q[5]) / 2 };
      }
      return null;
    } finally { await cdp.send('DOM.discardSearchResults', { searchId }).catch(() => {}); }
  };
  // Real X input: detach CDP first so nothing about the event comes from the debugger.
  const xdo = (...a) => execFileSync('xdotool', a.map(String));
  const viaX = async (fn) => {
    cdp.close();
    await sleep(150 + Math.random() * 150);
    fn();
    await sleep(250);
    cdp = await rawCdp();
  };
  const typeText = async (text) => {
    if (XVFB) return viaX(() => xdo('type', '--delay', 70 + Math.floor(Math.random() * 60), text));
    await cdp.send('Input.insertText', { text });
  };
  const click = async ({ x, y }) => {
    if (XVFB) {
      return viaX(() => {
        xdo('mousemove', '--sync', Math.round(x - 40 + Math.random() * 20), Math.round(y + 30));
        xdo('mousemove', '--sync', Math.round(x + Math.random() * 6 - 3), Math.round(y + Math.random() * 4 - 2));
        xdo('click', '1');
      });
    }
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
    await sleep(60 + Math.random() * 80);
    await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
    await sleep(40 + Math.random() * 60);
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
  };
  const btn = (text) => `//button[contains(normalize-space(.), "${text}")] | //*[@role="button"][contains(normalize-space(.), "${text}")]`;
  const shot = async (name) => {
    const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' }).catch(() => ({}));
    if (!data) return;
    const f = path.join(process.env.SHOT_DIR || os.tmpdir(), `meetbot-${name}.png`);
    fs.writeFileSync(f, Buffer.from(data, 'base64'));
    log('screenshot', f);
  };

  // Pre-join screen: dismiss the device dialog, type the name, ask to join.
  let asked = false;
  let typed = false;
  let noDevClicks = 0;
  const t0 = Date.now();
  while (!asked && Date.now() - t0 < 60000) {
    await sleep(800);
    await reattach();
    if (await find('//*[contains(., "You can\'t join this video call")]')) { await shot('rejected'); throw new Error('Meet rejected the join'); }
    // A fresh profile gets a device dialog on top of everything; dismiss it (twice at most).
    const noDev = noDevClicks < 2 && (await find(btn('Continue without microphone')));
    if (noDev) { await click(noDev); noDevClicks++; log('continue without mic/cam'); await sleep(800); continue; }
    // The name field comes after "Continue as guest"; that button stays in the DOM behind the dialog,
    // so look for the name field first.
    const name = await find('//input[@type="text" and (contains(@aria-label, "name") or contains(@placeholder, "name"))]');
    if (name && !typed) {
      await click(name);
      await typeText(BOT_NAME);
      typed = true;
      await sleep(600);
    }
    if (name) {
      const join = (await find(btn('Ask to join'))) || (await find(btn('Join now')));
      if (join) { await click(join); asked = true; log('asked to join as', BOT_NAME); }
      continue;
    }
    const guest = await find(btn('Continue as guest'));
    if (guest) { await click(guest); log('continue as guest'); await sleep(1500); }
  }
  if (!asked) { await shot('prejoin'); throw new Error('no "Ask to join" button within 60 s'); }

  // Wait for the host.
  for (;;) {
    await sleep(1500);
    await reattach();
    if (await find('//button[@aria-label="Leave call"]')) break;
    if (await find('//*[contains(., "You can\'t join this video call") or contains(., "denied your request") or contains(., "Someone in the call denied")]')) {
      await shot('denied'); throw new Error('join was refused');
    }
    if (Date.now() - t0 > ADMIT_TIMEOUT) { await shot('timeout'); throw new Error('host did not admit the bot in time'); }
  }
  log('admitted');
  cdp.close();
  if (process.env.NO_BOT !== '1') require('./bot.cjs');
})().catch((e) => { console.error('[join]', e.message); process.exit(1); });
