// Per-speaker colour: a decorative accent only (the speaker name is always printed next to it).
// Same palette as the Avatar primitive (>= 4.5:1 against white), so a speaker's caption dot matches their tile avatar.
const PALETTE = ['#5e6ad2', '#1f7a6e', '#a8650f', '#b0386f', '#2f6fd0', '#6a3fc0'];

export function speakerColor(name: string): string {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return PALETTE[h % PALETTE.length] ?? '#5e6ad2';
}
