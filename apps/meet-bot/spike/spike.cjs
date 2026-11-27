// Spike (docs/architecture/meet-bot.md step 1): join a Google Meet as a guest bot and log, every 100 ms,
//   * each remote WebRTC audio track: level (dBFS) + RTP contributing/synchronization sources (CSRC/SSRC),
//   * Meet captions: speaker name + text (captions are turned on by the bot),
//   * participant tiles: name + whether a "speaking" class set is active (raw class diff, for analysis).
// Output: JSON lines in apps/meet-bot/spike/out/<timestamp>.jsonl. Analyse with analyze.cjs.
//
//   node apps/meet-bot/spike/spike.cjs "https://meet.google.com/abc-defg-hij" ["WarpTalk Bot"]
//   node apps/meet-bot/spike/spike.cjs --login   # once: sign a Google account into the bot profile (Meet blocks anonymous bots)
//
// Uses the installed Google Chrome (channel "chrome") with its own throwaway profile, never the user's.
const path = require('node:path');
const fs = require('node:fs');
const os = require('node:os');
const { chromium } = require(path.join(__dirname, '../../web/node_modules/playwright'));

const url = process.argv[2];
const botName = process.argv[3] || 'WarpTalk Bot';
const loginMode = url === '--login'; // open Google sign-in in the bot profile; the user signs in, then closes the window
const reattach = process.env.SPIKE_REATTACH === '1'; // Chrome from an earlier run is still in the call: just attach
const noCdp = reattach || process.env.SPIKE_NOCDP === '1'; // join with no CDP attached; attach after admission (touch out/attach)
if (!loginMode && (!url || !/^https:\/\/meet\.google\.com\//.test(url))) {
  console.error('usage: node spike.cjs https://meet.google.com/xxx-xxxx-xxx [bot name]');
  process.exit(2);
}
const outDir = path.join(__dirname, 'out');
fs.mkdirSync(outDir, { recursive: true });
const outFile = path.join(outDir, `${new Date().toISOString().replace(/[:.]/g, '-')}.jsonl`);
const out = fs.createWriteStream(outFile);
const log = (o) => out.write(JSON.stringify({ t: Date.now(), ...o }) + '\n');

// Runs in the page before Meet's scripts: capture every remote audio track.
const INIT = () => {
  const tracks = []; // { id, track, receiver, analyser, buf }
  let ctx;
  const Orig = window.RTCPeerConnection;
  window.RTCPeerConnection = function (...args) {
    const pc = new Orig(...args);
    pc.addEventListener('track', (ev) => {
      if (ev.track.kind !== 'audio') return;
      try {
        ctx = ctx || new AudioContext();
        const src = ctx.createMediaStreamSource(new MediaStream([ev.track]));
        const analyser = ctx.createAnalyser();
        analyser.fftSize = 2048;
        src.connect(analyser);
        tracks.push({ id: ev.track.id, track: ev.track, receiver: ev.receiver, analyser, buf: new Float32Array(2048) });
      } catch (e) { console.warn('wt-spike track hook failed', e); }
    });
    return pc;
  };
  window.RTCPeerConnection.prototype = Orig.prototype;
  Object.setPrototypeOf(window.RTCPeerConnection, Orig);

  window.__wtSample = () => {
    if (ctx && ctx.state === 'suspended') ctx.resume().catch(() => {});
    return tracks.filter((t) => t.track.readyState === 'live').map((t) => {
      t.analyser.getFloatTimeDomainData(t.buf);
      let s = 0; for (const v of t.buf) s += v * v;
      const rms = Math.sqrt(s / t.buf.length);
      const csrc = (t.receiver.getContributingSources?.() || []).map((c) => ({ src: c.source, lvl: c.audioLevel, ts: c.timestamp }));
      const ssrc = (t.receiver.getSynchronizationSources?.() || []).map((c) => ({ src: c.source, lvl: c.audioLevel }));
      return { id: t.id, db: rms > 0 ? Math.round(20 * Math.log10(rms)) : -120, csrc, ssrc };
    });
  };
};

// Late attach (no init script): remote audio comes from Meet's <audio> elements. No RTCRtpReceiver, so no CSRC.
const LATE = () => {
  const ctx = new AudioContext();
  const seen = new Map();
  window.__wtSample = () => {
    if (ctx.state === 'suspended') ctx.resume().catch(() => {});
    for (const el of document.querySelectorAll('audio')) {
      const ms = el.srcObject; if (!ms) continue;
      for (const track of ms.getAudioTracks()) {
        if (seen.has(track.id)) continue;
        const analyser = ctx.createAnalyser(); analyser.fftSize = 2048;
        ctx.createMediaStreamSource(new MediaStream([track])).connect(analyser);
        seen.set(track.id, { track, analyser, buf: new Float32Array(2048) });
      }
    }
    return [...seen.values()].filter((t) => t.track.readyState === 'live').map((t) => {
      t.analyser.getFloatTimeDomainData(t.buf);
      let s = 0; for (const v of t.buf) s += v * v;
      const rms = Math.sqrt(s / t.buf.length);
      return { id: t.track.id, db: rms > 0 ? Math.round(20 * Math.log10(rms)) : -120, csrc: [], ssrc: [] };
    });
  };
};

// DOM probe (Meet selectors are unstable; this is deliberately loose for the spike).
const DOM = () => {
  const tiles = [...document.querySelectorAll('[data-participant-id]')].map((el) => {
    // innerText starts with the display name; .notranslate children are mostly material-icon ligatures (mic_off...).
    const name = el.innerText.split('\n')[0].trim().slice(0, 60);
    const icons = [...el.querySelectorAll('.notranslate')].map((n) => n.innerText.trim()).filter((t) => /^[a-z_]+$/.test(t));
    const classes = new Set();
    el.querySelectorAll('*').forEach((d) => d.classList.forEach((c) => classes.add(c)));
    return { pid: el.getAttribute('data-participant-id'), name, icons: icons.join(' '), cls: [...classes].sort().join(' ') };
  });
  // Captions region: Meet renders it as a region labelled "Captions" (language-dependent label).
  const region = document.querySelector('[role="region"][aria-label*="aption"], [jsname="dsyhDe"]');
  const captions = region ? region.innerText.trim().split('\n').slice(-6) : null;
  return { tiles, captions };
};

(async () => {
  const profile = path.join(os.tmpdir(), process.env.SPIKE_PROFILE || 'warptalk-meetbot-spike-profile');
  // Start a plain Chrome ourselves and attach over CDP: no Playwright launch flags at all, which Meet
  // otherwise detects ("You can't join this video call" before the host ever sees a request).
  const { spawn } = require('node:child_process');
  const port = 9333;
  const chrome = reattach ? { on: () => {}, kill: () => {} } : spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    `--user-data-dir=${profile}`, `--remote-debugging-port=${port}`, '--no-first-run', '--no-default-browser-check',
    // No --use-fake-device-*: Meet sees "Fake Default Audio Input" / "fake_device_0" and rejects the guest as a bot.
    '--autoplay-policy=no-user-gesture-required',
    '--window-size=1280,800', noCdp ? url : 'about:blank'], { stdio: 'ignore' });
  if (noCdp && !reattach) {
    // Experiment: nothing attaches to Chrome while joining (Meet rejects CDP-driven guests). A human / OS-level
    // input clicks "Ask to join"; once admitted, `touch apps/meet-bot/spike/out/attach` makes us attach.
    const trigger = path.join(outDir, 'attach');
    fs.rmSync(trigger, { force: true });
    log({ ev: 'open-nocdp', url: url.replace(/\?.*$/, '') });
    console.log('Chrome dang mo Meet, KHONG gan CDP. Nhap ten + bam Ask to join bang tay. Khi duoc vao: touch', trigger);
    while (!fs.existsSync(trigger)) await new Promise((r) => setTimeout(r, 1000));
  }
  let cdp;
  for (let i = 0; i < 50 && !cdp; i++) {
    await new Promise((r) => setTimeout(r, 200));
    cdp = await chromium.connectOverCDP(`http://127.0.0.1:${port}`).catch(() => null);
  }
  if (!cdp) throw new Error('could not attach to Chrome on port ' + port);
  const ctx = cdp.contexts()[0];
  const browser = { pages: () => ctx.pages(), newPage: () => ctx.newPage(), on: (e, f) => chrome.on('exit', f),
    close: async () => { chrome.kill(); } };
  const page = noCdp ? (ctx.pages().find((p) => p.url().includes('meet.google.com')) || ctx.pages()[0])
    : (browser.pages()[0] || (await browser.newPage()));
  if (loginMode) {
    await page.goto('https://accounts.google.com/');
    console.log('Dang nhap Google trong cua so nay, xong thi dong cua so.');
    await new Promise((r) => chrome.on('exit', r));
    process.exit(0);
  }
  if (noCdp) {
    await page.evaluate(LATE);
    log({ ev: 'attached-late' });
  } else {
  await page.addInitScript(() => { Object.defineProperty(navigator, 'webdriver', { get: () => undefined }); });
  await page.addInitScript(INIT);
  log({ ev: 'open', url: url.replace(/\?.*$/, '') });
  await page.goto(url, { waitUntil: 'domcontentloaded' });

  // Pre-join: dismiss "continue without mic/camera", turn mic+camera off, enter name, ask to join.
  const clickText = async (re, ms = 3000) => {
    const b = page.getByRole('button', { name: re }).first();
    try { await b.waitFor({ state: 'visible', timeout: ms }); await b.click(); return true; } catch { return false; }
  };
  // Pre-join screens appear in varying order (mic/camera popup, guest vs sign-in, name field): loop over them.
  let asked = false;
  for (let i = 0; i < 40 && !asked; i++) {
    await clickText(/continue without|tiếp tục mà không/i, 300);
    await clickText(/continue as guest|tiếp tục với tư cách khách/i, 300);
    const name = page.locator('input[type="text"]').first();
    if (await name.isVisible().catch(() => false) && (await name.inputValue().catch(() => '')) !== botName) {
      await name.fill(botName).catch(() => {}); log({ ev: 'name-filled' });
    }
    asked = await clickText(/ask to join|join now|yêu cầu tham gia|tham gia ngay/i, 500);
  }
  if (!asked) {
    log({ ev: 'no-join-button' });
    // Debug: what screen are we stuck on?
    await page.screenshot({ path: outFile.replace(/\.jsonl$/, '-prejoin.png') }).catch(() => {});
    const txt = await page.evaluate(() => document.body.innerText.slice(0, 1500)).catch(() => '');
    log({ ev: 'prejoin-text', txt });
    console.log('Khong thay nut xin vao. Anh chup:', outFile.replace(/\.jsonl$/, '-prejoin.png'));
  }
  log({ ev: 'asked-to-join' });
  console.log('Đã xin vào. Host bấm "Admit" để cho bot vào...');
  }

  if (!noCdp) {
  // While waiting: screenshot every 5 s so we can see what Meet shows (out/<ts>-wait.png).
  const shot = outFile.replace(/\.jsonl$/, '-wait.png');
  const waitShots = setInterval(() => page.screenshot({ path: shot }).catch(() => {}), 5000);
  // In call when the "Leave call" button exists.
  await page.getByRole('button', { name: /leave call|rời khỏi cuộc gọi/i }).waitFor({ timeout: 10 * 60_000 });
  clearInterval(waitShots);
  }
  log({ ev: 'in-call' });
  console.log('Bot đã vào. Ghi log tại', outFile, '— Ctrl+C để dừng.');
  if (!reattach) await page.keyboard.press('c').catch(() => {}); // captions on ('c' toggles, so not on reattach)
  await page.waitForTimeout(1500);
  log({ ev: 'captions-toggled' });

  let lastCls = new Map();
  const timer = setInterval(async () => {
    try {
      const [audio, dom] = await Promise.all([page.evaluate('window.__wtSample ? window.__wtSample() : []'), page.evaluate(DOM)]);
      // Only ship class lists when they change, to keep the log small.
      const tiles = dom.tiles.map((t) => {
        const changed = lastCls.get(t.pid) !== t.cls; lastCls.set(t.pid, t.cls);
        return changed ? t : { pid: t.pid, name: t.name, icons: t.icons };
      });
      log({ ev: 's', audio, tiles, captions: dom.captions });
    } catch (e) { log({ ev: 'err', msg: String(e).slice(0, 200) }); }
  }, 100);

  const stop = async () => { clearInterval(timer); log({ ev: 'stop' }); out.end(); await browser.close().catch(() => {}); process.exit(0); };
  process.on('SIGINT', stop);
  page.on('close', stop);
})().catch((e) => { console.error(e); log({ ev: 'fatal', msg: String(e) }); out.end(); process.exit(1); });
