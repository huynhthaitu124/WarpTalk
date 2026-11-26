import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import {
  ArrowDown, CaretDown, Check, ClosedCaptioning, Globe, Lightbulb, Microphone, MicrophoneSlash, Stop, CheckSquare, X,
} from '@phosphor-icons/react';
import { cn } from '@v2/lib/cn';
import { LANGUAGES, languageLabel } from '@v2/lib/languages';
import { getNative, type OverlayCommand, type OverlayLine, type OverlayState } from '@v2/lib/native';

/**
 * Copilot window for external meetings (macOS `OverlayWebController`), modelled on the upstream desktop widget.
 * It never authenticates or calls the API: it renders only the state the main page relays (`warptalk-overlay`
 * events) and sends user actions back with `overlayCommand`.
 */
const EMPTY: OverlayState = {
  title: '', status: 'connecting', myLang: 'vi', farLang: 'en', micOn: true, lines: [], suggestions: [], followUps: [], suggestionsAvailable: false,
};

function readInitial(): OverlayState {
  const w = window as unknown as { __warptalkOverlayState?: OverlayState };
  return w.__warptalkOverlayState ?? EMPTY;
}

function send(command: OverlayCommand, value?: string) {
  void getNative()?.overlayCommand?.(command, value).catch(() => undefined);
}

/** Pill <-> card bookkeeping only: the native window keeps one fixed size (resizing it flashed stale frames). */
function setExpanded(on: boolean) {
  send(on ? 'expand' : 'collapse');
}

export function OverlayPage() {
  const [state, setState] = useState<OverlayState>(readInitial);
  const [open, setOpen] = useState(false);
  const [closing, setClosing] = useState(false);
  /** Pill is wide only while something new is happening (or on hover); compact when idle or when the card is open. */
  const [active, setActive] = useState(false);
  const [hover, setHover] = useState(false);
  /**
   * Pill width pinned while the card opens, is open, and closes. The native window resizes under the cursor during
   * those steps, which fires spurious mouseleave / mouseenter; with the width pinned they cannot flip the pill.
   */
  const [pinned, setPinned] = useState<boolean | null>(null);
  /** A transition is running: ignore clicks until it has finished (no half-open states). */
  const busy = useRef(false);
  /** Hover with hysteresis, so a pass over the edge does not start and reverse the width animation. */
  const hoverTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  /** Set when a click closes the card: the cursor is still on the pill, but it must stay compact until the mouse leaves. */
  const noHoverWiden = useRef(false);
  const hoverIn = () => {
    clearTimeout(hoverTimer.current);
    if (noHoverWiden.current) return;
    hoverTimer.current = setTimeout(() => setHover(true), 90);
  };
  const hoverOut = () => {
    noHoverWiden.current = false;
    clearTimeout(hoverTimer.current);
    hoverTimer.current = setTimeout(() => setHover(false), 220);
  };
  const openRef = useRef(false);
  const lastSig = useRef('');
  const idleTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const pillRef = useRef<HTMLDivElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  /** When speech / a tip last changed, and a 1 s clock while translating: drives the pill's phase label. */
  const [speechAt, setSpeechAt] = useState(0);
  const [tipAt, setTipAt] = useState(0);
  const lastTips = useRef(-1);
  const [now, setNow] = useState(() => Date.now());

  function wake() {
    setActive(true);
    pillRef.current?.animate(
      [{ transform: 'scale(1)' }, { transform: 'scale(1.08)' }, { transform: 'scale(0.97)' }, { transform: 'scale(1.02)' }, { transform: 'scale(1)' }],
      { duration: 520, easing: 'ease-out' },
    );
    clearTimeout(idleTimer.current);
    idleTimer.current = setTimeout(() => setActive(false), 4000);
  }

  // Card entrance: grows down out of the pill.
  useLayoutEffect(() => {
    if (!open || closing) return;
    cardRef.current?.animate(
      [{ opacity: 0, transform: 'translateY(-14px) scale(0.94)', filter: 'blur(4px)' }, { opacity: 1, transform: 'none', filter: 'blur(0)' }],
      { duration: 320, easing: 'cubic-bezier(.2,.9,.25,1.15)' },
    );
  }, [open, closing]);

  useEffect(() => {
    const root = document.documentElement;
    const prev = root.dataset.theme;
    root.dataset.theme = 'dark';
    // The native window is transparent: only the pill / card paint.
    root.style.background = 'transparent';
    document.body.style.background = 'transparent';
    document.title = 'WarpTalk';
    const on = (e: Event) => {
      const d = (e as CustomEvent<OverlayState>).detail;
      if (!d) return;
      setState(d);
      const n = d.suggestions.length + d.followUps.length;
      if (lastTips.current >= 0 && n > lastTips.current) setTipAt(Date.now());
      lastTips.current = n;
      const l = d.lines[d.lines.length - 1];
      const lineSig = `${l?.id ?? ''}|${l?.text.length ?? 0}`;
      if (l && lineSig !== lastSig.current.split('#')[0]) setSpeechAt(Date.now());
      // New speech or a new tip while the card is closed: widen the pill to show it, with a bounce.
      const sig = `${lineSig}#${n}`;
      if (sig !== lastSig.current) {
        const fresh = lastSig.current !== '';
        lastSig.current = sig;
        if (fresh && !openRef.current) wake();
      }
    };
    window.addEventListener('warptalk-overlay', on);
    const ready = () => { send('ready'); send('collapse'); };
    ready();
    window.addEventListener('warptalk-native-ready', ready);
    return () => {
      window.removeEventListener('warptalk-overlay', on);
      window.removeEventListener('warptalk-native-ready', ready);
      clearTimeout(idleTimer.current);
        clearTimeout(hoverTimer.current);
      if (prev) root.dataset.theme = prev;
    };
  }, []);

  /**
   * Strictly sequential so no two size changes overlap:
   * open:  pin pill width -> grow native window -> wait for layout -> mount card + entrance animation.
   * close: exit animation -> unmount card -> shrink native window -> wait for layout -> unpin pill width.
   */
  async function toggle(next = !open) {
    if (busy.current || next === openRef.current) return;
    busy.current = true;
    openRef.current = next;
    clearTimeout(idleTimer.current);
    if (next) {
      setPinned(active || hover);
      setExpanded(true);
      setOpen(true);
      setTimeout(() => { busy.current = false; }, 320);
      return;
    }
    // Closing: the pill goes compact right away and stays so (even under the cursor) until the mouse leaves it.
    // Both animations run inside the still-large window; the window shrinks only after they finished.
    noHoverWiden.current = true;
    clearTimeout(hoverTimer.current);
    setHover(false);
    setActive(false);
    setPinned(false);
    setClosing(true);
    const anim = cardRef.current?.animate(
      [{ opacity: 1, transform: 'none' }, { opacity: 0, transform: 'translateY(-10px) scale(0.95)' }],
      { duration: 180, easing: 'ease-in', fill: 'forwards' },
    );
    await Promise.all([anim?.finished.catch(() => undefined), new Promise((r) => setTimeout(r, 400))]);
    setOpen(false);
    setClosing(false);
    setExpanded(false);
    setPinned(null);
    busy.current = false;
  }

  const ended = state.status === 'ended';
  const translating = state.status === 'translating';
  const tips = state.suggestions.length + state.followUps.length;
  const last = state.lines[state.lines.length - 1];
  const wide = pinned ?? (active || hover);
  /** Hide the whole overlay (X on pill or card); reset so the pill starts closed when it is shown again. */
  const hideOverlay = () => {
    openRef.current = false;
    busy.current = false;
    setOpen(false);
    setClosing(false);
    setPinned(null);
    send('collapse');
    send('hide');
  };

  // The window never resizes; only the pill (at its widest) and the open card take the mouse, the rest is
  // click-through. Report that area to the native side whenever it may have changed.
  const lastHit = useRef('');
  useEffect(() => {
    const report = () => {
      const p = pillRef.current?.getBoundingClientRect();
      if (!p) return;
      const cx = p.left + p.width / 2;
      let x0 = cx - 212, x1 = cx + 212, y1 = p.bottom + 8;
      const c = open ? cardRef.current?.getBoundingClientRect() : undefined;
      if (c) { x0 = Math.min(x0, c.left); x1 = Math.max(x1, c.right); y1 = c.bottom + 12; }
      const v = [x0, 0, x1 - x0, y1].map((n) => Math.round(n)).join(',');
      if (v !== lastHit.current) { lastHit.current = v; send('hitrect', v); }
    };
    report();
    const ro = new ResizeObserver(report);
    if (cardRef.current) ro.observe(cardRef.current);
    window.addEventListener('resize', report);
    window.addEventListener('warptalk-native-ready', report);
    return () => { ro.disconnect(); window.removeEventListener('resize', report); window.removeEventListener('warptalk-native-ready', report); };
  }, [open]);

  // Once the window ignores the mouse WebKit never sees it leave; the native side tells us instead.
  useEffect(() => {
    const on = (e: Event) => {
      const inside = (e as CustomEvent<{ inside: boolean }>).detail?.inside;
      if (inside === false) hoverOut();
    };
    window.addEventListener('warptalk-overlay-pointer', on);
    return () => window.removeEventListener('warptalk-overlay-pointer', on);
  });
  useEffect(() => {
    if (!translating) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [translating]);
  const phase = pillPhase(state, now, speechAt, tipAt);
  const ticker = last ? (last.translation && last.translation !== last.text ? last.translation : last.text) : '';

  return (
    // Transparent margins (px/pb) leave room inside the native window for the shadows.
    <div className="flex h-screen flex-col items-center overflow-hidden bg-transparent px-6 pb-7 pt-2 text-ink">
      {/* Light sweep across "Listening…" (gradient clipped to the text). */}
      <style>{`
        .wt-shimmer {
          display: inline-block;
          background: linear-gradient(100deg, rgba(255,255,255,.5) 30%, #fff 50%, rgba(255,255,255,.5) 70%) 0 0 / 200% 100% repeat-x;
          -webkit-background-clip: text; background-clip: text; color: transparent;
          /* Own compositing layer: repainting the text must not re-run the pill's backdrop blur every frame. */
          will-change: background-position; transform: translateZ(0);
          animation: wt-shimmer 2s linear infinite;
        }
        /* Shift by exactly one tile (200% size => -200% = +2W) so the loop has no jump. */
        @keyframes wt-shimmer { from { background-position: 0 0; } to { background-position: -200% 0; } }
        @media (prefers-reduced-motion: reduce) { .wt-shimmer { animation: none; } }
      `}</style>
      {/* Pill: always visible at the top centre; click to open / close the card. */}
      <div ref={pillRef} onMouseEnter={hoverIn} onMouseLeave={hoverOut}
        style={{ width: wide ? 412 : 208, transition: 'width 380ms cubic-bezier(.34,1.32,.64,1), background-color 150ms' }}
        // Only the width animates; the height is fixed.
        className="flex h-10 shrink-0 items-center gap-1 overflow-hidden rounded-full bg-[#131415]/60 pr-1.5 backdrop-blur shadow-[0_6px_16px_rgba(0,0,0,0.25)] ring-1 ring-white/10 hover:bg-[#1b1c1e]/75">
      <button type="button" onClick={() => void toggle()} aria-expanded={open} data-testid="copilot-pill"
        className="flex h-full min-w-0 flex-1 items-center gap-2 pl-3.5 text-left">
        <PillDot status={state.status} />
        <span className="min-w-0 flex-1 truncate whitespace-nowrap text-[12.5px] font-medium text-ink">
          {wide && !ended && ticker ? ticker : phase.live ? <span className="wt-shimmer">{phase.label}</span> : <span className="text-ink-muted">{phase.label}</span>}
        </span>
        {tips > 0 && (
          <span className="flex shrink-0 items-center gap-1 rounded-full bg-primary/20 px-2 py-0.5 text-[11px] font-semibold text-primary">
            <Lightbulb size={12} weight="fill" />{tips}
          </span>
        )}
        <span className="grid size-7 shrink-0 place-items-center rounded-full bg-white/5 text-ink-muted">
          <CaretDown size={12} weight="bold" className={cn('transition-transform duration-300', open && !closing && 'rotate-180')} />
        </span>
      </button>
      <button type="button" aria-label="Close copilot" title="Close copilot" data-testid="copilot-pill-close"
        onClick={hideOverlay}
        className="grid size-7 shrink-0 place-items-center rounded-full bg-white/5 text-ink-muted hover:bg-white/10 hover:text-ink">
        <X size={12} weight="bold" />
      </button>
      </div>

      {open && (
        <div ref={cardRef} style={{ transformOrigin: 'top center' }} className="relative mt-2 flex min-h-[150px] w-full flex-1 flex-col overflow-hidden rounded-2xl bg-[#131415]/70 shadow-[0_8px_20px_rgba(0,0,0,0.3)] backdrop-blur ring-1 ring-white/10" data-testid="copilot-card">
          {/* Transcript card. One compact header row (status, controls, close) keeps it short. */}
          <div className="flex h-12 shrink-0 items-center gap-2 px-3">
            <StatusDot status={state.status} />
            {!ended && (
              <>
                <span className="mx-1 h-5 w-px bg-border" />
                <DockButton label={state.micOn ? 'Mute my mic' : 'Unmute my mic'} danger={!state.micOn} onClick={() => send('mic', state.micOn ? 'off' : 'on')}>
                  {state.micOn ? <Microphone size={15} weight="bold" /> : <MicrophoneSlash size={15} weight="bold" />}
                </DockButton>
                {/* One language per person: I speak it and read / hear everything in it. */}
                <LangPicker value={state.myLang} onChange={(v) => send('listen', v)} />
              </>
            )}
            <button type="button" aria-label="Hide copilot" title="Hide copilot" onClick={hideOverlay}
              className="ml-auto grid size-7 place-items-center rounded-full text-ink-muted hover:bg-surface-3 hover:text-ink">
              <X size={14} weight="bold" />
            </button>
          </div>

          {ended ? (
            <div className="grid flex-1 place-items-center px-8 text-center">
              <div>
                <Stop size={24} weight="fill" className="mx-auto mb-2 text-ink-muted" />
                <p className="text-[14px] font-medium">WarpTalk has stopped listening and translating</p>
                <p className="mt-1 text-[12px] text-ink-muted">You can close this window.</p>
              </div>
            </div>
          ) : (
            <Transcript lines={state.lines} myLang={state.myLang} tips={stamped(state)} />
          )}

          {state.testMode && !ended && <TestLineBox />}

          {state.error && <div className="shrink-0 border-t border-border bg-danger/10 px-3 py-1.5 text-[12px] text-danger-text">{state.error}</div>}

          {/* Drag to resize: bottom edge = height, corner = width + height (window grows around its centre). */}
          <ResizeGrip axes="y" className="absolute inset-x-16 bottom-0 h-2 cursor-ns-resize" />
          <ResizeGrip axes="xy" className="absolute bottom-0 right-0 size-4 cursor-nwse-resize" />
        </div>
      )}

    </div>
  );
}

/** Pointer-drag handle that asks the native window to resize (`overlayCommand('resize', 'WxH')`). */
function ResizeGrip({ axes, className }: { axes: 'y' | 'xy'; className: string }) {
  function onPointerDown(e: React.PointerEvent<HTMLDivElement>) {
    e.preventDefault();
    const el = e.currentTarget;
    el.setPointerCapture(e.pointerId);
    const start = { x: e.screenX, y: e.screenY, w: window.innerWidth, h: window.innerHeight };
    let frame = 0;
    const move = (ev: PointerEvent) => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        // Centred window: the right edge moves by half the width change, so double dx to track the cursor.
        const w = axes === 'xy' ? start.w + 2 * (ev.screenX - start.x) : start.w;
        const h = start.h + (ev.screenY - start.y);
        send('resize', `${Math.round(w)}x${Math.round(h)}`);
      });
    };
    const up = () => {
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      el.removeEventListener('pointercancel', up);
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  }
  return (
    <div role="separator" aria-label="Resize copilot" onPointerDown={onPointerDown} className={cn('z-10 touch-none', className)}>
      {axes === 'y' && <span className="mx-auto mt-[3px] block h-1 w-10 rounded-full bg-white/15 opacity-0 transition-opacity hover:opacity-100" />}
    </div>
  );
}

function PillDot({ status }: { status: OverlayState['status'] }) {
  const c = status === 'translating' ? 'bg-emerald-500 animate-pulse' : status === 'connecting' ? 'bg-amber-400' : 'bg-ink-muted';
  return <span className={cn('size-2 shrink-0 rounded-full', c)} />;
}

function StatusDot({ status }: { status: OverlayState['status'] }) {
  const map = {
    connecting: ['bg-amber-400', 'Connecting'],
    ready: ['bg-ink-muted', 'Ready'],
    translating: ['bg-emerald-500 animate-pulse', 'Translating'],
    stopped: ['bg-ink-muted', 'Translation stopped'],
    ended: ['bg-ink-muted', 'Ended'],
  } as const;
  const [dot, label] = map[status];
  return (
    <span className="flex items-center gap-1.5 text-[11px] font-medium text-ink-muted">
      <span className={cn('size-1.5 rounded-full', dot)} />{label}
    </span>
  );
}

function initials(name: string) {
  return name.split(/\s+/).filter(Boolean).slice(0, 2).map((w) => w[0]!.toUpperCase()).join('') || '?';
}
const fmtTime = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });

type Tip =
  | { kind: 'suggestion'; id: string; at: number; category: string; content: string; detail?: string }
  | { kind: 'followUp'; id: string; at: number; text: string; owner?: string };

/** Suggestions and follow-ups with the time they first arrived (they carry no segment id). */
/** First time each suggestion / follow-up id reached this window (one copilot window per page load). */
const SEEN = new Map<string, number>();
function stamped(state: OverlayState): Tip[] {
  const at = (id: string) => { let t = SEEN.get(id); if (t === undefined) { t = Date.now(); SEEN.set(id, t); } return t; };
  return [
    ...state.suggestions.map((x): Tip => ({ kind: 'suggestion', ...x, at: at(`s:${x.id}`) })),
    ...state.followUps.map((x): Tip => ({ kind: 'followUp', ...x, at: at(`f:${x.id}`) })),
  ];
}

function Transcript({ lines, myLang, tips }: { lines: OverlayLine[]; myLang: string; tips: Tip[] }) {
  const ref = useRef<HTMLDivElement>(null);
  const [stuck, setStuck] = useState(true);
  const items = [
    ...lines.map((l) => ({ at: l.at, key: `l:${l.id}`, node: <Bubble line={l} myLang={myLang} /> })),
    ...tips.map((t) => ({ at: t.at, key: `t:${t.kind}:${t.id}`, node: <TipCard tip={t} /> })),
  ].sort((a, b) => a.at - b.at);
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && stuck) el.scrollTop = el.scrollHeight;
  }, [items.length, lines, stuck]);
  const onScroll = () => {
    const el = ref.current;
    if (el) setStuck(el.scrollHeight - el.scrollTop - el.clientHeight < 48);
  };
  if (!items.length) {
    return (
      <div className="grid flex-1 place-items-center px-8 text-center text-ink-muted">
        <div>
          <ClosedCaptioning size={28} className="mx-auto mb-2" />
          <p className="text-[13px]">Waiting for the first thing anyone says…</p>
        </div>
      </div>
    );
  }
  return (
    <div className="relative min-h-0 flex-1">
      <div ref={ref} onScroll={onScroll} className="h-full space-y-3 overflow-y-auto px-3 py-3">
        {items.map((i) => <div key={i.key}>{i.node}</div>)}
      </div>
      {!stuck && (
        <button type="button" onClick={() => setStuck(true)}
          className="absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-1 rounded-full bg-surface-3 px-2.5 py-1 text-[11px] font-medium text-ink shadow-sm">
          <ArrowDown size={12} weight="bold" />Latest
        </button>
      )}
    </div>
  );
}

function Bubble({ line: l, myLang }: { line: OverlayLine; myLang: string }) {
  const src = l.lang ?? '';
  const showTr = !!l.translation && l.translation !== l.text;
  return (
    <div className={cn('flex gap-2', l.self && 'flex-row-reverse')}>
      <div className="relative mt-4 size-7 shrink-0">
        <div className={cn('grid size-7 place-items-center rounded-full text-[10px] font-semibold', l.self ? 'bg-primary/25 text-primary' : 'bg-surface-3 text-ink-muted')}>
          {initials(l.self ? 'You' : l.speaker)}
        </div>
        {src && (
          <span className="absolute -bottom-0.5 -right-0.5 grid h-4 place-items-center rounded-full bg-surface-1 px-1 text-[9px] font-semibold uppercase leading-none text-ink-muted ring-1 ring-border">
            {src.slice(0, 2)}
          </span>
        )}
      </div>
      <div className={cn('flex max-w-[85%] flex-col', l.self ? 'items-end' : 'items-start')}>
        <div className="mb-1 flex items-center gap-1.5 px-1 text-[10px]">
          <span className="text-[11px] font-semibold text-ink-muted">{l.self ? 'You' : l.speaker}</span>
          <span className="font-mono tabular-nums text-ink-subtle">{fmtTime(l.at)}</span>
        </div>
        <div className={cn('rounded-2xl px-3 py-2 shadow-sm', l.self ? 'rounded-tr-sm bg-primary text-white' : 'rounded-tl-sm border border-border bg-surface-2/60', !l.final && 'opacity-75')}>
          <p className={cn('text-[13px] leading-relaxed', l.self ? 'text-white/80' : showTr ? 'text-ink-muted' : 'text-ink')}>{l.text}</p>
          {showTr && <p className={cn('mt-1.5 text-[13px] font-medium leading-relaxed', l.self ? 'text-white' : 'text-ink')}>{l.translation}</p>}
          {src && (
            <div className={cn('mt-2 text-[10px] font-medium', l.self ? 'text-white/70' : 'text-ink-subtle')}>
              {showTr ? `${languageLabel(src)} → ${languageLabel(myLang)}` : languageLabel(src)}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** A suggestion / follow-up shown inline, right under the transcript line that prompted it. */
function TipCard({ tip }: { tip: Tip }) {
  const isSuggestion = tip.kind === 'suggestion';
  return (
    <div className="ml-9 rounded-xl border border-primary/25 bg-primary/10 px-3 py-2">
      <div className={cn('mb-0.5 flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide', isSuggestion ? 'text-primary' : 'text-ink-muted')}>
        {isSuggestion ? <Lightbulb size={12} weight="fill" /> : <CheckSquare size={12} />}
        {isSuggestion ? tip.category.replace(/_/g, ' ') : 'Follow-up'}
      </div>
      {isSuggestion ? (
        <>
          <p className="text-[12.5px] font-medium leading-snug">{tip.content}</p>
          {tip.detail && <p className="mt-0.5 text-[11.5px] leading-snug text-ink-muted">{tip.detail}</p>}
        </>
      ) : (
        <p className="text-[12.5px] leading-snug">{tip.text}{tip.owner && <span className="text-ink-muted"> · {tip.owner}</span>}</p>
      )}
    </div>
  );
}

/** Dev test mode only (the main page sets `testMode` when the API runs a suggestion test script). */
function TestLineBox() {
  const [text, setText] = useState('');
  return (
    <form
      className="flex shrink-0 gap-1.5 border-t border-dashed border-warning/60 px-3 py-2"
      onSubmit={(e) => { e.preventDefault(); const t = text.trim(); if (t) { send('testLine', t); setText(''); } }}
    >
      <span className="self-center text-[10px] font-semibold uppercase tracking-wide text-warning-text">Test</span>
      <input value={text} onChange={(e) => setText(e.target.value)} maxLength={1000} aria-label="Test transcript line"
        placeholder="Gõ câu thoại theo kịch bản rồi Enter…"
        className="min-w-0 flex-1 rounded-md border border-border bg-surface-2 px-2 py-1 text-[12px] outline-none focus:border-accent" />
    </form>
  );
}

function DockButton({ label, danger, onClick, children }: { label: string; danger?: boolean; onClick(): void; children: React.ReactNode }) {
  return (
    <button type="button" aria-label={label} title={label} onClick={onClick}
      className={cn('grid size-[34px] shrink-0 place-items-center rounded-[9px] transition-colors',
        danger ? 'bg-destructive/15 text-destructive hover:bg-destructive/25' : 'bg-surface-3 text-ink hover:bg-surface-3/70')}>
      {children}
    </button>
  );
}

/**
 * In-page dropdown (not a native <select>: its popup does not open in the non-activating copilot panel).
 * The list opens inside the card, which is always tall enough for its scroll area.
 */
function LangPicker({ value, onChange }: { value: string; onChange(v: string): void }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="relative">
      <button type="button" title="My language: you speak it and read everything in it" aria-haspopup="listbox" aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        className="flex h-[34px] min-w-0 items-center gap-1.5 rounded-full bg-surface-3 pl-3 pr-2 text-[12px] font-medium text-ink hover:bg-surface-3/70">
        <Globe size={14} />
        <span className="truncate">{languageLabel(value)}</span>
        <CaretDown size={11} weight="bold" className={cn('shrink-0 text-ink-muted transition-transform', open && 'rotate-180')} />
      </button>
      {open && (
        <>
          <div className="fixed inset-0 z-20" onClick={() => setOpen(false)} />
          <ul role="listbox" aria-label="My language"
            className="absolute left-0 top-[calc(100%+6px)] z-30 max-h-[150px] w-48 overflow-y-auto rounded-xl bg-[#1b1c1e] p-1 shadow-[0_8px_20px_rgba(0,0,0,0.4)] ring-1 ring-white/10">
            {LANGUAGES.map((l) => (
              <li key={l.code} role="option" aria-selected={l.code === value}>
                <button type="button" onClick={() => { setOpen(false); if (l.code !== value) onChange(l.code); }}
                  className={cn('flex w-full items-center justify-between rounded-lg px-2.5 py-1.5 text-left text-[12px] hover:bg-white/10', l.code === value ? 'text-ink' : 'text-ink-muted')}>
                  {l.label}{l.code === value && <Check size={12} weight="bold" />}
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

/** What the copilot is doing right now, for the compact pill. `live` phases get the light sweep. */
function pillPhase(s: OverlayState, now: number, speechAt: number, tipAt: number): { label: string; live: boolean } {
  switch (s.status) {
    case 'ended': return { label: 'Stopped', live: false };
    case 'connecting': return { label: 'Connecting…', live: true };
    case 'ready': return { label: 'Ready', live: false };
    case 'stopped': return { label: 'Paused', live: false };
    case 'translating': break;
  }
  const last = s.lines[s.lines.length - 1];
  if (tipAt && now - tipAt < 6000) return { label: 'Suggestion ready', live: true };
  if (last && (!last.final || now - speechAt < 2500)) return { label: 'Listening…', live: true };
  // Someone else just finished a sentence: the assistant is weighing whether to suggest something.
  if (s.suggestionsAvailable && last && !last.self && now - speechAt < 8000 && tipAt < speechAt) return { label: 'Thinking…', live: true };
  return { label: 'Idle', live: false };
}
