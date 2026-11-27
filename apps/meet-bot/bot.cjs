// Meet bot -> WarpTalk STT (prototype). Attaches to a Chrome that is ALREADY in the Meet call
// (joined without CDP, see docs/architecture/meet-bot.md), captures each received <audio> track
// as PCM16 16 kHz, cuts utterances with a simple energy VAD and streams them to the copilot
// audio socket on channels bot0..bot2. Each commit carries the speaker name Meet shows in its
// captions while that track was talking.
//
//   API_URL=http://localhost:8090 BOT_EMAIL=demo-host@example.com BOT_PASSWORD=... \
//   MEET_URL=https://meet.google.com/xxx node apps/meet-bot/bot.cjs
//
// Env: CDP_PORT (9333), SESSION_ID (reuse a copilot session instead of creating one),
// LANG_ME / LANG_OTHERS (vi / en), VAD_DB (-48), BOT_NAME (WarpTalk Bot, excluded as speaker).
const path = require('node:path');
const { chromium } = (() => { try { return require('playwright-core'); } catch { return require(path.join(__dirname, '../web/node_modules/playwright')); } })();

const API = (process.env.API_URL || 'http://localhost:8090').replace(/\/$/, '') + '/api/v1';
const CDP_PORT = Number(process.env.CDP_PORT || 9333);
const BOT_NAME = process.env.BOT_NAME || 'WarpTalk Bot';
const VAD_DB = Number(process.env.VAD_DB || -48);
const log = (...a) => console.log(new Date().toISOString().slice(11, 23), ...a);

async function api(method, p, token, body) {
  const r = await fetch(API + p, {
    method,
    headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await r.text();
  if (!r.ok) throw new Error(`${method} ${p} -> ${r.status} ${text.slice(0, 200)}`);
  return text ? JSON.parse(text) : null;
}

// In-page capture. Every track gets a ScriptProcessor at 16 kHz (the AudioContext resamples);
// chunks of 2048 samples (128 ms) are gated by RMS with pre-roll and a 768 ms hangover.
const CAPTURE = (vadDb) => {
  const ctx = new AudioContext({ sampleRate: 16000 });
  const slots = [null, null, null]; // track id per bot channel
  const CHUNK = 2048, HANG = 6, MIN_CHUNKS = 3, MAX_CHUNKS = 117; // ~15 s
  const b64 = (i16) => {
    const u8 = new Uint8Array(i16.buffer); let s = '';
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
    return btoa(s);
  };
  const attach = (track) => {
    const ch = slots.findIndex((t) => t === null || t.readyState === 'ended');
    if (ch < 0) return;
    slots[ch] = track;
    const src = ctx.createMediaStreamSource(new MediaStream([track]));
    const proc = ctx.createScriptProcessor(CHUNK, 1, 1);
    const mute = ctx.createGain(); mute.gain.value = 0;
    src.connect(proc); proc.connect(mute); mute.connect(ctx.destination);
    let st = { on: false, n: 0, quiet: 0, start: 0, prev: null };
    proc.onaudioprocess = (e) => {
      const f = e.inputBuffer.getChannelData(0);
      let s = 0; for (let i = 0; i < f.length; i++) s += f[i] * f[i];
      const db = 20 * Math.log10(Math.sqrt(s / f.length) || 1e-9);
      const pcm = new Int16Array(f.length);
      for (let i = 0; i < f.length; i++) pcm[i] = Math.max(-32768, Math.min(32767, Math.round(f[i] * 32767)));
      const now = Date.now();
      const loud = db > vadDb;
      if (!st.on) {
        if (loud) {
          st = { on: true, n: 0, quiet: 0, start: now - 256, prev: null };
          if (st.prev) window.__wtAudio(JSON.stringify({ ch, pcm: b64(st.prev) }));
        } else { st.prev = pcm; return; }
      }
      window.__wtAudio(JSON.stringify({ ch, pcm: b64(pcm) }));
      st.n++; st.quiet = loud ? 0 : st.quiet + 1;
      if (st.quiet >= HANG || st.n >= MAX_CHUNKS) {
        const ok = st.n - st.quiet >= MIN_CHUNKS;
        window.__wtAudio(JSON.stringify({ ch, end: ok ? 'commit' : 'discard', start: st.start, stop: now }));
        st = { on: false, n: 0, quiet: 0, start: 0, prev: null };
      }
    };
    console.log('[wt] track', track.id, '-> bot' + ch);
  };
  const seen = new Set();
  const scan = () => {
    if (ctx.state === 'suspended') ctx.resume().catch(() => {});
    for (const el of document.querySelectorAll('audio')) {
      for (const t of el.srcObject?.getAudioTracks?.() || []) {
        if (!seen.has(t.id)) { seen.add(t.id); attach(t); }
      }
    }
  };
  scan(); setInterval(scan, 1000);
};

// Current caption speaker: last caption line that equals a participant tile name.
const SPEAKER = (botName) => {
  const names = new Set([...document.querySelectorAll('[data-participant-id]')]
    .map((el) => el.innerText.split('\n')[0].trim()).filter((n) => n && n !== botName));
  const region = document.querySelector('[role="region"][aria-label*="aption"]');
  if (!region) return { speaker: null, names: [...names], captions: false };
  let speaker = null;
  for (const l of region.innerText.split('\n')) if (names.has(l.trim())) speaker = l.trim();
  return { speaker, names: [...names], captions: true };
};

(async () => {
  const cdp = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`);
  const page = cdp.contexts()[0].pages().find((p) => p.url().includes('meet.google.com'));
  if (!page) throw new Error('no meet.google.com tab in the Chrome on port ' + CDP_PORT);
  const meetUrl = process.env.MEET_URL || page.url().replace(/\?.*$/, '');

  // Session + single-use audio ticket.
  const login = await api('POST', '/auth/login', null, { email: process.env.BOT_EMAIL, password: process.env.BOT_PASSWORD });
  const token = login.accessToken;
  let sessionId = process.env.SESSION_ID;
  let startedAt = Date.now();
  if (!sessionId) {
    const ws = await api('GET', '/workspaces', token);
    const wsId = (Array.isArray(ws) ? ws : ws.items || ws.workspaces || [])[0]?.id;
    if (!wsId) throw new Error('user has no workspace');
    const s = await api('POST', '/copilot/sessions', token, {
      workspaceId: wsId, title: 'Meet bot ' + new Date().toISOString().slice(0, 16), platform: 'google_meet',
      meetingUrl: meetUrl, languages: { me: process.env.LANG_ME || 'vi', others: process.env.LANG_OTHERS || 'en' },
      displayNames: { me: 'Tôi', others: 'Others' },
    });
    sessionId = s.id;
    startedAt = Date.parse(s.startedAt || s.createdAt) || startedAt;
  }
  log('copilot session', sessionId);
  const { ticket } = await api('POST', `/copilot/sessions/${sessionId}/audio-ticket`, token);
  const wsUrl = API.replace(/^http/, 'ws') + `/copilot/sessions/${sessionId}/audio?token=${encodeURIComponent(ticket)}`;
  const sock = new WebSocket(wsUrl);
  sock.binaryType = 'arraybuffer';
  await new Promise((ok, bad) => { sock.onopen = ok; sock.onerror = (e) => bad(new Error('ws error ' + (e.message || ''))); });
  sock.send(JSON.stringify({ type: 'hello', runId: 'meetbot-' + sessionId.slice(0, 8), languageMode: 'auto', languages: ['vi', 'en'] }));
  sock.onmessage = (m) => {
    if (typeof m.data !== 'string') return;
    const msg = JSON.parse(m.data);
    if (msg.type === 'segment') log(`[${msg.channel}] ${msg.text}`);
    else if (msg.type === 'error') log('server error', msg.code, msg.message);
    else if (msg.type !== 'partial' && msg.type !== 'pong') log('server', msg.type);
  };
  sock.onclose = (e) => { log('socket closed', e.code, e.reason); process.exit(1); };
  setInterval(() => sock.readyState === 1 && sock.send('{"type":"ping"}'), 15000);

  // Caption speaker samples (wall time) for labelling utterances.
  const samples = [];
  let lastInfo = '';
  setInterval(async () => {
    const r = await page.evaluate(SPEAKER, BOT_NAME).catch(() => null);
    if (!r) return;
    samples.push({ t: Date.now(), s: r.speaker, names: r.names });
    while (samples.length && samples[0].t < Date.now() - 60000) samples.shift();
    const info = `captions=${r.captions} people=${r.names.join(', ')}`;
    if (info !== lastInfo) { log(info); lastInfo = info; }
  }, 200);
  const speakerFor = (start, stop) => {
    const votes = new Map();
    let names = [];
    for (const x of samples) {
      if (x.t < start - 500 || x.t > stop + 300) continue;
      names = x.names;
      if (x.s) votes.set(x.s, (votes.get(x.s) || 0) + 1);
    }
    const top = [...votes].sort((a, b) => b[1] - a[1])[0];
    if (top) return top[0];
    return names.length === 1 ? names[0] : null; // one other person in the call = it was them
  };

  const seq = [0, 0, 0];
  await page.exposeFunction('__wtAudio', (json) => {
    if (sock.readyState !== 1) return;
    const m = JSON.parse(json);
    if (m.pcm) {
      const pcm = Buffer.from(m.pcm, 'base64');
      const frame = Buffer.alloc(pcm.length + 1);
      frame[0] = 2 + m.ch; pcm.copy(frame, 1);
      sock.send(frame);
    } else if (m.end === 'commit') {
      const speaker = speakerFor(m.start, m.stop);
      sock.send(JSON.stringify({ type: 'commit', channel: 'bot' + m.ch, seq: ++seq[m.ch],
        startMs: Math.max(0, m.start - startedAt), endMs: Math.max(0, m.stop - startedAt), speaker }));
      log(`bot${m.ch} utterance ${((m.stop - m.start) / 1000).toFixed(1)}s speaker=${speaker ?? '?'}`);
    } else if (m.end === 'discard') {
      sock.send(JSON.stringify({ type: 'discard', channel: 'bot' + m.ch }));
    }
  });
  page.on('console', (c) => c.text().startsWith('[wt]') && log(c.text()));
  await page.evaluate(CAPTURE, VAD_DB);
  // Captions give the speaker names; make sure they are on.
  const cap = await page.evaluate(SPEAKER, BOT_NAME);
  if (!cap.captions) { await page.keyboard.press('c').catch(() => {}); log('pressed c (captions on)'); }
  log('streaming; Ctrl+C to stop (the bot stays in the call)');
  process.on('SIGINT', () => { sock.close(1000, 'bot stopped'); setTimeout(() => process.exit(0), 300); });
})().catch((e) => { console.error(e.message); process.exit(1); });
