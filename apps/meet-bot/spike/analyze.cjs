// Analyse a spike log: can we tell who is speaking in a big meeting?
//   node apps/meet-bot/spike/analyze.cjs [apps/meet-bot/spike/out/<file>.jsonl]   (default: newest file)
//
// Ground truth = the speaker name Meet shows in captions (last caption line that equals a tile name).
// For every RTP contributing source (CSRC) we count which caption name was active while it was loud.
// A CSRC that maps to one name >= 80% of the time is a stable per-participant id (= attribution works
// for people who are not on screen). Also reports overlap (>= 2 loud tracks at once) and which tile
// classes toggle while someone talks (fallback "speaking" indicator).
const fs = require('node:fs');
const path = require('node:path');

const dir = path.join(__dirname, 'out');
const file = process.argv[2] || fs.readdirSync(dir).filter((f) => f.endsWith('.jsonl')).sort().map((f) => path.join(dir, f)).pop();
if (!file) { console.error('no log in', dir); process.exit(2); }
const rows = fs.readFileSync(file, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l));
const samples = rows.filter((r) => r.ev === 's');
console.log(`${path.basename(file)}: ${samples.length} samples (${Math.round(samples.length / 10)} s)`);
console.log('events:', rows.filter((r) => r.ev !== 's').map((r) => r.ev).join(', '));

const LOUD_DB = -50;       // track RMS above this = someone audible on the track
const CSRC_LVL = 0.02;     // RTP audioLevel (0..1) above this = that source is talking
const names = new Set();
const csrcNames = new Map(); // csrc -> Map(name -> count)
const tileNameByPid = new Map();
const classOnWhileNamed = new Map(); // class -> {speak, idle}
let overlap = 0, loudSamples = 0, maxTracks = 0, captioned = 0;
const lastCls = new Map();

for (const s of samples) {
  for (const t of s.tiles || []) {
    if (t.name) { tileNameByPid.set(t.pid, t.name); names.add(t.name); }
    if (t.cls !== undefined) lastCls.set(t.pid, t.cls);
  }
  // Current caption speaker: last caption line that is a known participant name.
  let speaker = null;
  for (const line of s.captions || []) if (names.has(line.trim())) speaker = line.trim();
  if (speaker) captioned++;

  const audio = s.audio || [];
  maxTracks = Math.max(maxTracks, audio.length);
  const loud = audio.filter((a) => a.db > LOUD_DB);
  if (loud.length) loudSamples++;
  if (loud.length >= 2) overlap++;

  if (speaker) {
    for (const a of loud) for (const c of a.csrc || []) {
      if ((c.lvl ?? 0) < CSRC_LVL) continue;
      const m = csrcNames.get(c.src) || new Map();
      m.set(speaker, (m.get(speaker) || 0) + 1);
      csrcNames.set(c.src, m);
    }
    // Which classes are present on the speaker's tile vs on others' tiles.
    for (const [pid, cls] of lastCls) {
      const isSpeaker = tileNameByPid.get(pid) === speaker;
      for (const c of cls.split(' ').filter(Boolean)) {
        const k = classOnWhileNamed.get(c) || { speak: 0, idle: 0 };
        if (isSpeaker) k.speak++; else k.idle++;
        classOnWhileNamed.set(c, k);
      }
    }
  }
}

console.log(`\naudio tracks seen at once: max ${maxTracks}`);
console.log(`samples with audible audio: ${loudSamples}, with >=2 loud tracks (overlap): ${overlap}`);
console.log(`samples with a caption speaker: ${captioned}`);
console.log(`participants (tiles): ${[...names].join(', ') || '(none)'}`);

console.log('\nCSRC -> caption name (purity = share of the top name):');
let stable = 0;
for (const [src, m] of [...csrcNames].sort((a, b) => sum(b[1]) - sum(a[1]))) {
  const total = sum(m);
  const [top, n] = [...m].sort((a, b) => b[1] - a[1])[0];
  const purity = n / total;
  if (purity >= 0.8 && total >= 10) stable++;
  console.log(`  ${src}: ${top} ${(purity * 100).toFixed(0)}% of ${total}${m.size > 1 ? '  others: ' + [...m].filter(([k]) => k !== top).map(([k, v]) => `${k}=${v}`).join(' ') : ''}`);
}
console.log(`stable CSRC (>=80%, >=10 samples): ${stable}/${csrcNames.size}`);

console.log('\ntile classes most specific to the current speaker (fallback indicator):');
[...classOnWhileNamed].map(([c, k]) => [c, k.speak / Math.max(1, k.speak + k.idle), k.speak])
  .filter(([, , n]) => n >= 10).sort((a, b) => b[1] - a[1]).slice(0, 5)
  .forEach(([c, r, n]) => console.log(`  .${c}: ${(r * 100).toFixed(0)}% on speaker tile (${n})`));

function sum(m) { let s = 0; for (const v of m.values()) s += v; return s; }
