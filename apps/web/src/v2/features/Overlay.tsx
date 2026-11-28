import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import {
  ArrowDown, CaretDown, ChatCircleDots, Check, ClosedCaptioning, Globe, Lightbulb, Microphone, MicrophoneSlash, PaperPlaneRight, Sparkle,
  Pause, Play, Stop, CheckSquare, X,
} from '@phosphor-icons/react';
import { actionsFor, replyLanguageFor } from '@/lib/meeting/suggestion-actions';
import { cn } from '@v2/lib/cn';
import { LANGUAGES, defaultReadingLanguage, languageLabel } from '@v2/lib/languages';
import { Markdown } from '@v2/lib/markdown';
import { getNative, useNative, type OverlayAsk, type OverlayCommand, type OverlayLine, type OverlayState } from '@v2/lib/native';

/**
 * Copilot window for external meetings (macOS `OverlayWebController`), modelled on the upstream desktop widget.
 * It never authenticates or calls the API: it renders only the state the main page relays (`warptalk-overlay`
 * events) and sends user actions back with `overlayCommand`.
 */
const EMPTY: OverlayState = {
  title: '', status: 'connecting', myLang: defaultReadingLanguage(), farLang: 'en', micOn: true, lines: [], suggestions: [], followUps: [], suggestionsAvailable: false,
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
  const pillDrag = usePillDrag();
  const cardRef = useRef<HTMLDivElement>(null);
  /** When speech / a tip last changed, and a 1 s clock while translating: drives the pill's phase label. */
  const [speechAt, setSpeechAt] = useState(0);
  const [tipAt, setTipAt] = useState(0);
  const lastTips = useRef(-1);
  const [now, setNow] = useState(() => Date.now());
  /** Card view: the live transcript (with tips) or the WarpBot chat. */
  const [tab, setTab] = useState<'live' | 'warpbot'>('live');
  /** A WarpBot answer finished while the transcript tab was showing. */
  const [unread, setUnread] = useState(false);
  const tabRef = useRef(tab);
  useEffect(() => { tabRef.current = tab; }, [tab]);
  const lastAnswer = useRef('');
  /** Ask WarpBot from the composer or a suggestion's action; the answer shows on the WarpBot tab. */
  const ask = (q: string) => { send('ask', q); setTab('warpbot'); setUnread(false); };

  /** Mirrors `active` for the event handler: partials arrive many times a second and must not re-run the bounce. */
  const activeRef = useRef(false);
  function wake() {
    // Bounce only when the pill wakes from idle; while speech keeps streaming just keep it wide.
    if (!activeRef.current) {
      activeRef.current = true;
      setActive(true);
      pillRef.current?.animate(
        [{ transform: 'scale(1)' }, { transform: 'scale(1.04)' }, { transform: 'scale(1)' }],
        { duration: 360, easing: 'ease-out' },
      );
    }
    clearTimeout(idleTimer.current);
    idleTimer.current = setTimeout(() => { activeRef.current = false; setActive(false); }, 6000);
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
      const done = d.ask?.messages.filter((m) => m.role === 'assistant' && m.status !== 'streaming').at(-1)?.key ?? '';
      if (lastAnswer.current && done && done !== lastAnswer.current && tabRef.current !== 'warpbot') setUnread(true);
      lastAnswer.current = done || lastAnswer.current;
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
    activeRef.current = false;
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

  // Native app: real refracting glass (NSGlassEffectView) is drawn under the transparent page, so tell it
  // where the pill and card are every frame (they animate). Sent only when something changed.
  const nativeGlass = !!useNative();   // inside the macOS app rather than a plain browser tab
  const lastGlass = useRef('');
  useEffect(() => {
    if (!nativeGlass) return;
    let raf = 0;
    const shape = (id: string, el: HTMLElement | null, radius: (b: DOMRect, s: number) => number) => {
      if (!el) return '';
      const b = el.getBoundingClientRect();
      const s = el.offsetWidth ? b.width / el.offsetWidth : 1;   // scale of a running transform animation
      const o = Number(getComputedStyle(el).opacity) || 0;
      return `${id}:${[b.left, b.top, b.width, b.height, radius(b, s)].map((n) => n.toFixed(1)).join(',')},${o.toFixed(2)}`;
    };
    const tick = () => {
      const v = [
        shape('p', pillRef.current, (b) => b.height / 2),
        shape('c', cardRef.current, (_b, s) => 24 * s),
      ].filter(Boolean).join('|');
      if (v !== lastGlass.current) { lastGlass.current = v; send('glass', v); }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [nativeGlass]);

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
    <div className={cn('flex h-screen flex-col items-center overflow-hidden bg-transparent px-6 pb-7 pt-2 text-ink', nativeGlass && 'wt-native')}>
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
        @media (prefers-reduced-motion: reduce) { .wt-shimmer, .wt-dots { animation: none; } }
        /* Liquid glass, macOS Siri style. Pill = near-black smoked capsule; card = clear glass that
           shows the refracted desktop behind it. Both: thin bright rim, strongest on the top and bottom edges. */
        .wt-glass {
          position: relative; isolation: isolate; color: #f5f5f7;
          -webkit-backdrop-filter: blur(24px) saturate(180%); backdrop-filter: blur(24px) saturate(180%);
          transition: filter 150ms;
        }
        .wt-pill {
          background: linear-gradient(180deg, rgba(30,30,32,.82), rgba(8,8,10,.88));
          box-shadow: inset 0 1px 0 rgba(255,255,255,.16), inset 0 -1px 0 rgba(255,255,255,.10), 0 10px 28px rgba(0,0,0,.45);
        }
        .wt-glass-card {
          background: linear-gradient(180deg, rgba(60,60,66,.34) 0%, rgba(30,30,34,.42) 50%, rgba(16,16,18,.50) 100%);
          box-shadow: inset 0 1px 0 rgba(255,255,255,.32), inset 0 -1.5px 0 rgba(255,255,255,.28),
            inset 0 0 28px rgba(255,255,255,.06), 0 16px 40px rgba(0,0,0,.35);
          text-shadow: 0 1px 2px rgba(0,0,0,.45);
        }
        .wt-glass::before { /* 1px rim: bright at the corners where light catches the edge */
          content: ""; position: absolute; inset: 0; border-radius: inherit; padding: 1px; pointer-events: none;
          background: linear-gradient(170deg, rgba(255,255,255,.60), rgba(255,255,255,.14) 22%, rgba(255,255,255,.04) 50%, rgba(255,255,255,.16) 78%, rgba(255,255,255,.55));
          -webkit-mask: linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0); -webkit-mask-composite: xor;
          mask: linear-gradient(#000 0 0) content-box exclude, linear-gradient(#000 0 0);
        }
        .wt-glass::after { /* soft sheen inside the glass */
          content: ""; position: absolute; inset: 0; border-radius: inherit; pointer-events: none; z-index: -1;
          background: radial-gradient(120% 60% at 50% -25%, rgba(255,255,255,.14), transparent 60%),
            radial-gradient(60% 40% at 50% 120%, rgba(255,255,255,.08), transparent 70%);
        }
        .wt-glass .text-ink-muted { color: rgba(255,255,255,.74); }
        .wt-glass .text-ink-subtle { color: rgba(255,255,255,.56); }
        .wt-glass:hover { filter: brightness(1.1); }
        .wt-glass-card:hover { filter: none; }
        /* Nested card inside the glass (suggestions). */
        .wt-inset {
          background: rgba(255,255,255,.10);
          box-shadow: inset 0 1px 0 rgba(255,255,255,.18), inset 0 0 0 1px rgba(255,255,255,.10);
        }
        .wt-light-btn { background: linear-gradient(180deg, #ffffff, #d9d9de); color: #111; box-shadow: inset 0 1px 0 #fff, 0 2px 8px rgba(0,0,0,.35); }
        /* Native app: the refraction comes from NSGlassEffectView under the page (CSS backdrop-filter sees
           nothing through the transparent window), so the page keeps only a light tint for legibility. */
        .wt-native .wt-glass { -webkit-backdrop-filter: none; backdrop-filter: none; }
        .wt-native .wt-glass::before, .wt-native .wt-glass::after { display: none; }
        .wt-native .wt-pill { background: transparent; box-shadow: none; }
        .wt-native .wt-glass-card { background: rgba(0,0,0,.08); box-shadow: none; }
        .wt-native .wt-glass:hover { filter: none; }
        .wt-dots { animation: wt-spin 1s steps(8) infinite; transform-origin: center; }
        @keyframes wt-spin { to { transform: rotate(360deg); } }
      `}</style>
      {/* Pill: always visible at the top centre; click to open / close the card. */}
      <div ref={pillRef} onMouseEnter={hoverIn} onMouseLeave={hoverOut} onPointerDown={pillDrag.onPointerDown} onClickCapture={pillDrag.onClickCapture}
        style={{ width: wide ? 412 : 208, transition: 'width 380ms cubic-bezier(.34,1.32,.64,1), filter 150ms' }}
        // Only the width animates; the height is fixed.
        className="wt-glass wt-pill flex h-10 shrink-0 items-center gap-1 overflow-hidden rounded-full pr-1.5">
      <button type="button" onClick={() => void toggle()} aria-expanded={open} data-testid="copilot-pill"
        className="flex h-full min-w-0 flex-1 items-center gap-2 pl-4 text-left">
        <span className="min-w-0 flex-1 truncate whitespace-nowrap text-[13px] font-semibold text-ink">
          {wide && !ended && ticker ? ticker : phase.live ? <span className="wt-shimmer">{phase.label}</span> : <span className="text-[#f5f5f7]">{phase.label}</span>}
        </span>
        {tips > 0 && (
          <span className="flex shrink-0 items-center gap-1 rounded-full bg-[#ffffff]/15 px-2 py-0.5 text-[11px] font-semibold text-[#ffffff] ring-1 ring-[#ffffff]/25">
            <Lightbulb size={12} weight="fill" />{tips}
          </span>
        )}
        <span className="grid size-7 shrink-0 place-items-center rounded-full bg-[#ffffff]/[0.08] text-[#e5e5ea]/80">
          <CaretDown size={12} weight="bold" className={cn('transition-transform duration-300', open && !closing && 'rotate-180')} />
        </span>
      </button>
      {(translating || state.status === 'stopped') && (
        <button type="button" aria-label={translating ? 'Pause translation' : 'Resume translation'} title={translating ? 'Pause translation' : 'Resume translation'}
          data-testid="copilot-pill-pause" onClick={() => send(translating ? 'stop' : 'start')}
          className={cn('grid size-7 shrink-0 place-items-center rounded-full hover:bg-[#ffffff]/15 hover:text-[#ffffff]',
            translating ? 'bg-[#ffffff]/[0.08] text-[#e5e5ea]/80' : 'bg-[#ffffff]/20 text-[#ffffff]')}>
          {translating ? <Pause size={12} weight="fill" /> : <Play size={12} weight="fill" />}
        </button>
      )}
      <button type="button" aria-label="Close copilot" title="Close copilot" data-testid="copilot-pill-close"
        onClick={hideOverlay}
        className="grid size-7 shrink-0 place-items-center rounded-full bg-[#ffffff]/[0.08] text-[#e5e5ea]/80 hover:bg-[#ffffff]/15 hover:text-[#ffffff]">
        <X size={12} weight="bold" />
      </button>
      </div>

      {open && (
        <div ref={cardRef} style={{ transformOrigin: 'top center' }} className="wt-glass wt-glass-card relative mt-2 flex min-h-[150px] w-full flex-1 flex-col overflow-hidden rounded-[24px]" data-testid="copilot-card">
          {/* Transcript card. One compact header row (status, controls, close) keeps it short. */}
          <div className="flex h-12 shrink-0 items-center gap-2 px-3">
            {!ended && (
              <>
                <DockButton label={state.micOn ? 'Mute my mic' : 'Unmute my mic'} danger={!state.micOn} onClick={() => send('mic', state.micOn ? 'off' : 'on')}>
                  {state.micOn ? <Microphone size={15} weight="bold" /> : <MicrophoneSlash size={15} weight="bold" />}
                </DockButton>
                {/* One language per person: I speak it and read / hear everything in it. */}
                <LangPicker value={state.myLang} onChange={(v) => send('listen', v)} />
              </>
            )}
            {state.ask && (
              <div role="tablist" aria-label="Copilot view" className="ml-auto flex shrink-0 rounded-full bg-black/20 p-0.5 ring-1 ring-white/5">
                <TabButton active={tab === 'live'} onClick={() => setTab('live')} label="Live">
                  <ClosedCaptioning size={13} weight="bold" />
                </TabButton>
                <TabButton active={tab === 'warpbot'} onClick={() => { setTab('warpbot'); setUnread(false); }} label="WarpBot" dot={unread} testId="copilot-tab-warpbot">
                  <Sparkle size={13} weight="fill" />
                </TabButton>
              </div>
            )}
            <button type="button" aria-label="Hide copilot" title="Hide copilot" onClick={hideOverlay}
              className={cn(!state.ask && 'ml-auto', 'grid size-7 place-items-center rounded-full text-ink-muted hover:bg-[#ffffff]/15 hover:text-ink')}>
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
          ) : tab === 'warpbot' && state.ask ? (
            <WarpBotChat ask={state.ask} onAsk={ask} />
          ) : (
            <Transcript lines={state.lines} myLang={state.myLang} tips={stamped(state)} onAsk={state.ask ? ask : undefined} needsJoin={state.needsJoin} link={state.link} />
          )}

          {state.testMode && !ended && tab === 'live' && <TestLineBox />}

          {state.error && !state.needsJoin && <div className="shrink-0 border-t border-white/10 px-3 py-1.5 text-[12px] text-ink-muted">{state.error}</div>}

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

/** Siri-style dotted spinner: eight dots fading around a ring. */
export function DotSpinner({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" aria-hidden className="wt-dots shrink-0 text-[#ffffff]">
      {Array.from({ length: 8 }, (_, i) => {
        const a = (i / 8) * Math.PI * 2;
        return <circle key={i} cx={8 + 5.6 * Math.sin(a)} cy={8 - 5.6 * Math.cos(a)} r={1.35} fill="currentColor" opacity={0.2 + (i / 7) * 0.8} />;
      })}
    </svg>
  );
}

function initials(name: string) {
  return name.split(/\s+/).filter(Boolean).slice(0, 2).map((w) => w[0]!.toUpperCase()).join('') || '?';
}
const fmtTime = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });

type Tip =
  | { kind: 'suggestion'; id: string; at: number; category: string; content: string; detail?: string; actions?: { label: string; prompt: string }[] }
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

/**
 * Follow the newest message only while the reader is at the bottom. Any upward scroll (wheel,
 * trackpad, touch, scrollbar drag) releases it at once and nothing pulls the list down again until
 * the reader scrolls back to the very bottom or presses the jump button. Our own programmatic
 * scrolls are ignored, so a stream of interim lines cannot fight the reader.
 */
function useStickToBottom() {
  const ref = useRef<HTMLDivElement>(null);
  const [stuck, setStuckState] = useState(true);
  const stuckRef = useRef(true);
  const lastTop = useRef(0);
  const programmatic = useRef(false);
  const setStuck = useCallback((v: boolean) => {
    stuckRef.current = v;
    setStuckState(v);
  }, []);
  const stick = useCallback(() => {
    const el = ref.current;
    if (!el || !stuckRef.current) return;
    programmatic.current = true;
    el.scrollTop = el.scrollHeight;
    lastTop.current = el.scrollTop;
  }, []);
  const release = useCallback(() => { if (stuckRef.current) setStuck(false); }, [setStuck]);
  const handlers = {
    onScroll: () => {
      const el = ref.current;
      if (!el) return;
      const top = el.scrollTop;
      if (programmatic.current) {
        programmatic.current = false;
        lastTop.current = top;
        return;
      }
      if (top < lastTop.current - 1) release();
      else if (!stuckRef.current && el.scrollHeight - top - el.clientHeight < 4) setStuck(true);
      lastTop.current = top;
    },
    onWheel: (e: React.WheelEvent) => { if (e.deltaY < 0) release(); },
    onTouchMove: release,
  };
  return { ref, stuck, setStuck, stick, handlers };
}

/**
 * A long sentence reaches us as several segments (the endpointer cuts on pauses). Show them as one
 * bubble: consecutive lines of the same speaker join while the previous one did not end a sentence
 * (within MERGE_OPEN_MS) or followed right on (within MERGE_GAP_MS), up to MERGE_MAX_CHARS.
 */
const MERGE_GAP_MS = 4_000;
const MERGE_OPEN_MS = 15_000;
const MERGE_MAX_CHARS = 420;
const SENTENCE_END = /[.!?。！？…]["')\]」』]*$/;
const NO_SPACE = /[\u3040-\u30ff\u3400-\u9fff\u0e00-\u0e7f]/;
function joinText(a: string, b: string) {
  if (!a) return b;
  if (!b) return a;
  return NO_SPACE.test(a.slice(-1)) && NO_SPACE.test(b[0]) ? a + b : `${a} ${b}`;
}
function mergeLines(lines: OverlayLine[]): OverlayLine[] {
  const out: OverlayLine[] = [];
  let lastAt = 0;
  for (const l of lines) {
    const prev = out[out.length - 1];
    const gap = l.at - lastAt;
    const join = prev && prev.self === l.self && prev.speaker === l.speaker
      && (gap < MERGE_GAP_MS || (gap < MERGE_OPEN_MS && !SENTENCE_END.test(prev.text.trim())))
      && prev.text.length + l.text.length <= MERGE_MAX_CHARS;
    if (join) {
      const tr = prev.translation || l.translation ? joinText(prev.translation ?? prev.text, l.translation ?? l.text) : undefined;
      out[out.length - 1] = {
        ...prev, text: joinText(prev.text, l.text), ...(tr ? { translation: tr } : {}),
        lang: prev.lang ?? l.lang, final: prev.final && l.final,
      };
    } else {
      out.push(l);
    }
    lastAt = l.at;
  }
  return out;
}

function Transcript({ lines, myLang, tips, onAsk, needsJoin, link }: { lines: OverlayLine[]; myLang: string; tips: Tip[]; onAsk?: (q: string) => void; needsJoin?: boolean; link?: string }) {
  const { ref, stuck, setStuck, stick, handlers } = useStickToBottom();
  const items = [
    ...mergeLines(lines).map((l) => ({ at: l.at, key: `l:${l.id}`, node: <Bubble line={l} myLang={myLang} /> })),
    ...tips.map((t) => ({ at: t.at, key: `t:${t.kind}:${t.id}`, node: <TipCard tip={t} myLang={myLang} onAsk={onAsk} /> })),
  ].sort((a, b) => a.at - b.at);
  useLayoutEffect(() => { stick(); }, [items.length, lines, stuck, stick]);
  if (!items.length && needsJoin) return <JoinFirst link={link} />;
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
      <div ref={ref} {...handlers} className="h-full space-y-3 overflow-y-auto px-3 py-3">
        {items.map((i) => <div key={i.key}>{i.node}</div>)}
      </div>
      {!stuck && (
        <button type="button" onClick={() => setStuck(true)}
          className="absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-1 rounded-full bg-[#2c2c2e]/90 px-2.5 py-1 text-[11px] font-medium text-[#f5f5f7] shadow-sm ring-1 ring-[#ffffff]/25 backdrop-blur">
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
        <div className={cn('grid size-7 place-items-center rounded-full text-[10px] font-semibold', l.self ? 'bg-[#ffffff]/20 text-[#ffffff] ring-1 ring-[#ffffff]/30' : 'bg-white/[0.08] text-ink-muted ring-1 ring-white/10')}>
          {initials(l.self ? 'You' : l.speaker)}
        </div>
        {src && (
          <span className="absolute -bottom-0.5 -right-0.5 grid h-4 place-items-center rounded-full bg-[#1c1c1e] px-1 text-[9px] font-semibold uppercase leading-none text-ink-muted ring-1 ring-border">
            {src.slice(0, 2)}
          </span>
        )}
      </div>
      <div className={cn('flex max-w-[85%] flex-col', l.self ? 'items-end' : 'items-start')}>
        <div className="mb-1 flex items-center gap-1.5 px-1 text-[10px]">
          <span className="text-[11px] font-semibold text-ink-muted">{l.self ? 'You' : l.speaker}</span>
          <span className="font-mono tabular-nums text-ink-subtle">{fmtTime(l.at)}</span>
        </div>
        <div className={cn('rounded-2xl px-3 py-2 shadow-sm', l.self ? 'rounded-tr-sm bg-white/[0.14] text-white ring-1 ring-inset ring-white/15' : 'rounded-tl-sm bg-white/[0.07] ring-1 ring-inset ring-white/10', !l.final && 'opacity-75')}>
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

function withReplyLanguage(prompt: string, myLang: string) {
  return `${prompt}\n\nReply in ${replyLanguageFor(myLang)}.`;
}

/** A suggestion / follow-up shown inline, right under the transcript line that prompted it. */
function TipCard({ tip, myLang, onAsk }: { tip: Tip; myLang: string; onAsk?: (q: string) => void }) {
  const isSuggestion = tip.kind === 'suggestion';
  // A suggestion offers only what the agent wrote for it (0-3 buttons, chosen per situation); a follow-up
  // keeps its one natural next step.
  const actions = !onAsk ? []
    : isSuggestion ? (tip.actions ?? []).map((a, i) => ({ id: `agent-${i}`, label: a.label, prompt: withReplyLanguage(a.prompt, myLang) }))
      : actionsFor({ category: 'action', content: tip.text }, myLang).slice(0, 1);
  return (
    <div className="wt-inset ml-9 rounded-2xl px-3 py-2">
      <div className={cn('mb-0.5 flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide', isSuggestion ? 'text-[#ffffff]' : 'text-ink-muted')}>
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
      {actions.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-1">
          {actions.map((a) => (
            <button key={a.id} type="button" onClick={() => onAsk?.(a.prompt)} data-testid="tip-action"
              className="inline-flex items-center gap-1 rounded-full bg-[#ffffff]/[0.08] px-2.5 py-0.5 text-[11px] font-medium text-[#e5e5ea] ring-1 ring-[#ffffff]/25 hover:bg-[#ffffff]/15 hover:text-[#ffffff]">
              <ChatCircleDots size={11} />{a.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function TabButton({ active, onClick, label, dot, testId, children }: { active: boolean; onClick(): void; label: string; dot?: boolean; testId?: string; children: React.ReactNode }) {
  return (
    <button type="button" role="tab" aria-selected={active} onClick={onClick} data-testid={testId}
      className={cn('relative flex h-6 items-center gap-1 rounded-full px-2 text-[11px] font-semibold transition-colors',
        active ? 'bg-[#ffffff]/15 text-[#ffffff] shadow-[inset_0_1px_0_rgba(255,255,255,.2)]' : 'text-ink-muted hover:text-ink')}>
      {children}{label}
      {dot && <span className="absolute -right-0.5 -top-0.5 size-2 rounded-full bg-[#ffffff]" />}
    </button>
  );
}

/** WarpBot strings follow the app locale (the legacy UI's WARPTALK_LOCALE cookie, else the browser). */
const BOT_TEXT = {
  en: { quick: ['Summarize the meeting so far', 'What has been decided?', 'Any open questions left?'], title: 'Ask WarpBot about this meeting', hint: 'WarpBot reads the transcript and workspace documents.', thinking: 'Thinking…', cut: '(answer was cut off)', placeholder: 'Ask WarpBot…' },
  vi: { quick: ['Tóm tắt cuộc họp đến giờ', 'Đã chốt những gì?', 'Còn câu hỏi nào chưa được trả lời?'], title: 'Hỏi WarpBot về cuộc họp này', hint: 'WarpBot đọc transcript và tài liệu workspace.', thinking: 'Đang suy nghĩ…', cut: '(câu trả lời bị ngắt)', placeholder: 'Hỏi WarpBot…' },
  ja: { quick: ['ここまでの会議を要約', '何が決まりましたか？', '未解決の質問はありますか？'], title: 'この会議について WarpBot に質問', hint: 'WarpBot は文字起こしとワークスペースの資料を参照します。', thinking: '考え中…', cut: '（回答が途中で終了しました）', placeholder: 'WarpBot に質問…' },
} as const;
type BotLocale = keyof typeof BOT_TEXT;
function botLocale(): BotLocale {
  const isBot = (v: string | undefined): v is BotLocale => !!v && v in BOT_TEXT;
  try {
    const c = document.cookie.split('; ').find((p) => p.startsWith('WARPTALK_LOCALE='))?.split('=')[1];
    if (isBot(c)) return c;
  } catch { /* no cookie access */ }
  const nav = navigator.language?.slice(0, 2);
  return isBot(nav) ? nav : 'en';
}

/** WarpBot chat for this meeting. The main page runs the conversation; this only renders it and sends questions. */
function WarpBotChat({ ask, onAsk }: { ask: OverlayAsk; onAsk(q: string): void }) {
  const [text, setText] = useState('');
  const [t] = useState(() => BOT_TEXT[botLocale()]);
  const { ref, stuck, setStuck, stick, handlers } = useStickToBottom();
  const last = ask.messages[ask.messages.length - 1];
  useLayoutEffect(() => { stick(); }, [ask.messages.length, last?.content, stuck, stick]);
  const submit = () => {
    const q = text.trim();
    if (!q || ask.busy) return;
    setText('');
    setStuck(true);
    onAsk(q);
  };
  return (
    <>
      <div ref={ref} {...handlers} className="min-h-0 flex-1 space-y-4 overflow-y-auto px-3 py-3 [&>*]:mx-auto [&>*]:max-w-[640px] [&>*]:px-1" data-testid="warpbot-log">
        {ask.messages.length === 0 ? (
          <div className="grid h-full place-items-center text-center">
            <div>
              <p className="text-[13px] font-medium">{t.title}</p>
              <p className="mt-0.5 text-[11.5px] text-ink-muted">{t.hint}</p>
              <div className="mt-3 flex flex-wrap justify-center gap-1.5">
                {t.quick.map((q) => (
                  <button key={q} type="button" onClick={() => onAsk(q)}
                    className="rounded-full bg-[#ffffff]/[0.07] px-2.5 py-1 text-[11.5px] text-[#e5e5ea] ring-1 ring-[#ffffff]/20 hover:bg-[#ffffff]/15 hover:text-[#ffffff]">{q}</button>
                ))}
              </div>
            </div>
          </div>
        ) : ask.messages.map((m) => (m.role === 'user' ? (
          <div key={m.key} className="flex justify-end">
            <p className="line-clamp-4 max-w-[85%] whitespace-pre-wrap break-words rounded-[19px] bg-white/[0.09] px-3.5 py-[7px] text-[13.5px] font-medium leading-5 text-[#f5f5f7] ring-1 ring-inset ring-white/10">{m.content}</p>
          </div>
        ) : (
          <div key={m.key} data-testid="warpbot-answer" data-status={m.status ?? 'done'}>
            <div className="min-w-0 text-[14px] font-semibold leading-snug tracking-[-0.005em] text-white">
              {m.content ? <Markdown text={m.content} className="space-y-1.5" /> : m.status === 'streaming' ? <span className="flex items-center gap-2"><DotSpinner /><span className="wt-shimmer">{t.thinking}</span></span> : null}
              {m.incomplete && <p className="mt-1 text-[11px] text-ink-subtle">(câu trả lời bị ngắt)</p>}
              {m.error && <p className="mt-1 text-[11.5px] text-danger-text">{m.error}</p>}
            </div>
          </div>
        )))}
      </div>
      <form className="mx-auto flex w-full max-w-[640px] shrink-0 items-end gap-1.5 px-3 pb-2.5 pt-1" onSubmit={(e) => { e.preventDefault(); submit(); }}>
        <textarea value={text} onChange={(e) => setText(e.target.value)} rows={1} maxLength={1000} aria-label="Ask WarpBot" data-testid="warpbot-input"
          placeholder={t.placeholder}
          onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); submit(); } }}
          className="max-h-24 min-h-[32px] min-w-0 flex-1 resize-none rounded-[16px] bg-white/[0.08] px-3.5 py-1.5 text-[13px] leading-5 text-[#f5f5f7] outline-none ring-1 ring-inset ring-white/10 placeholder:text-[#e5e5ea]/45 focus:ring-[#ffffff]/45" />
        {ask.busy ? (
          <button type="button" aria-label="Stop answer" title="Stop" onClick={() => send('askStop')}
            className="grid size-[32px] shrink-0 place-items-center rounded-full bg-white/10 text-ink hover:bg-white/15"><Stop size={13} weight="fill" /></button>
        ) : (
          <button type="submit" aria-label="Send to WarpBot" title="Send" disabled={!text.trim()}
            className="wt-light-btn grid size-[32px] shrink-0 place-items-center rounded-full disabled:opacity-40"><PaperPlaneRight size={13} weight="fill" /></button>
        )}
      </form>
    </>
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

/** No meeting audio yet: ask the user to join the call first, and take a Meet/Teams link to open. */
function JoinFirst({ link }: { link?: string }) {
  const [url, setUrl] = useState('');
  const ok = /^(https?:\/\/)?(meet\.google\.com|teams\.(microsoft|live)\.com)\//i.test(url.trim());
  const submit = () => { if (ok) { send('link', /^https?:/i.test(url.trim()) ? url.trim() : `https://${url.trim()}`); setUrl(''); } };
  return (
    <div className="grid flex-1 place-items-center px-8 text-center">
      <div className="w-full max-w-[520px]">
        <ClosedCaptioning size={28} className="mx-auto mb-2 text-ink-muted" />
        <p className="text-[14px] font-medium text-[#f5f5f7]">Join the meeting first</p>
        <p className="mt-1 text-[12px] text-ink-muted">
          {link ? 'Open the meeting and join the call; translation starts as soon as there is audio.' : 'Paste the Google Meet or Teams link to open it, then join the call.'}
        </p>
        <form className="mt-3 flex items-center gap-2" onSubmit={(e) => { e.preventDefault(); submit(); }}>
          <input
            value={url} onChange={(e) => setUrl(e.target.value)} spellCheck={false} autoComplete="off"
            placeholder={link ?? 'https://meet.google.com/abc-defg-hij'}
            className="h-8 min-w-0 flex-1 rounded-full bg-[#ffffff]/[0.08] px-3 text-[13px] text-[#f5f5f7] ring-1 ring-inset ring-white/10 placeholder:text-ink-muted focus:outline-none focus:ring-white/25"
          />
          <button type="submit" disabled={!ok && !link} onClick={(e) => { if (!url.trim() && link) { e.preventDefault(); send('link', link); } }}
            className="h-8 shrink-0 rounded-full bg-[#ffffff]/15 px-3.5 text-[12.5px] font-medium text-[#f5f5f7] hover:bg-[#ffffff]/25 disabled:opacity-40">
            Open meeting
          </button>
        </form>
      </div>
    </div>
  );
}

/** Drag the pill to move the copilot window; released near its home it snaps back (native `drag`). */
function usePillDrag() {
  const dragged = useRef(false);
  function onPointerDown(e: React.PointerEvent<HTMLDivElement>) {
    if (e.button !== 0) return;
    const sx = e.screenX, sy = e.screenY;
    let started = false, frame = 0;
    dragged.current = false;
    const move = (ev: PointerEvent) => {
      const dx = ev.screenX - sx, dy = ev.screenY - sy;
      if (!started) {
        if (Math.hypot(dx, dy) < 5) return;
        started = true; dragged.current = true;
        send('drag', 'begin');
      }
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => send('drag', `${Math.round(dx)},${Math.round(dy)}`));
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', up);
      cancelAnimationFrame(frame);
      if (started) send('drag', 'end');
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', up);
  }
  // A drag must not also toggle the card / close the copilot.
  function onClickCapture(e: React.MouseEvent) {
    if (dragged.current) { e.stopPropagation(); e.preventDefault(); dragged.current = false; }
  }
  return { onPointerDown, onClickCapture };
}

function DockButton({ label, danger, onClick, children }: { label: string; danger?: boolean; onClick(): void; children: React.ReactNode }) {
  return (
    <button type="button" aria-label={label} title={label} onClick={onClick}
      className={cn('grid size-[34px] shrink-0 place-items-center rounded-full transition-colors',
        danger ? 'bg-destructive/15 text-destructive hover:bg-destructive/25' : 'bg-[#ffffff]/[0.08] text-[#f5f5f7] ring-1 ring-inset ring-white/10 hover:bg-[#ffffff]/15')}>
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
        className="flex h-[34px] min-w-0 items-center gap-1.5 rounded-full bg-[#ffffff]/[0.08] pl-3 pr-2 text-[12px] font-medium text-[#f5f5f7] ring-1 ring-inset ring-white/10 hover:bg-[#ffffff]/15">
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
  return { label: 'Listening', live: true };
}
