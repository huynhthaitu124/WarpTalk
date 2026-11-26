import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { ApiError } from '@v2/lib/api';
import { chatKey, CHAT_PAGE, fetchChat, postChat, useChatLatest, useHideChat } from '@v2/lib/chat';
import type { ChatMessageDto } from '@v2/lib/schemas';
import { errMsg } from '../errors';
import { parseChat, type ChatMessage } from './protocol';
import type { MeetingSession } from './types';

interface Pending { clientId: string; content: string; ts: number; status: 'sending' | 'failed'; error?: string }
interface Live { dto: ChatMessageDto; receivedAt: number }

const newClientId = () => (typeof crypto !== 'undefined' && 'randomUUID' in crypto ? crypto.randomUUID() : `c-${Date.now()}-${Math.random().toString(16).slice(2)}`);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
/** A reply was never received (network) or the server failed: safe to repeat because `clientMessageId` is idempotent. */
const transient = (e: unknown) => !(e instanceof ApiError) || e.status >= 500;

async function postWithRetry(roomId: string, content: string, clientId: string) {
  let last: unknown;
  for (let i = 0; i < 3; i++) {
    try { return await postChat(roomId, content, clientId); } catch (e) { last = e; if (!transient(e)) break; await sleep(500 * (i + 1)); }
  }
  throw last;
}

/** Newest REST message may lag a freshly received live copy by this much before the live copy is considered gone (hidden). */
const LIVE_GRACE_MS = 2500;

export function useMeetingChat(o: {
  roomId: string;
  session: MeetingSession;
  selfUserId: string | null;
  selfName: string;
  panelOpen: boolean;
  /** Called once per message from somebody else that arrives after the history was loaded. */
  onIncoming(): void;
}) {
  const { roomId, session, selfUserId, selfName, panelOpen, onIncoming } = o;
  const qc = useQueryClient();
  // Without a LiveKit `chat.translated` push, translations are discovered by polling every 5 s while the panel is open.
  const [pushedTranslations, setPushedTranslations] = useState(false);
  const interval = panelOpen ? (pushedTranslations ? 20_000 : 5_000) : 30_000;
  const latest = useChatLatest(roomId, interval);
  const hide = useHideChat(roomId);
  const [older, setOlder] = useState<ChatMessageDto[]>([]);
  const [exhausted, setExhausted] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [live, setLive] = useState<Record<string, Live>>({});
  const [pending, setPending] = useState<Pending[]>([]);
  const seen = useRef(new Set<string>());
  const ready = useRef(false);
  const incoming = useRef(onIncoming);
  incoming.current = onIncoming;

  const refetch = useCallback(() => { void qc.invalidateQueries({ queryKey: chatKey(roomId) }); }, [qc, roomId]);

  // Instant delivery over the LiveKit data channel (de-duplicated by messageId against REST).
  const { subscribe } = session;
  useEffect(() => subscribe('chat', (m) => {
    const w = parseChat(m.payload);
    if (!w) return;
    if (w.kind === 'message') {
      // The data channel is not authenticated per message: an identity that disagrees with the claimed sender is dropped.
      if (m.from && w.message.senderId && m.from.identity !== w.message.senderId) return;
      setLive((cur) => (cur[w.message.messageId] ? cur : { ...cur, [w.message.messageId]: { dto: w.message, receivedAt: Date.now() } }));
    } else if (w.kind === 'translated') {
      setPushedTranslations(true);
      setLive((cur) => {
        const x = cur[w.messageId];
        return x ? { ...cur, [w.messageId]: { ...x, dto: { ...x.dto, translations: { ...x.dto.translations, ...w.translations } } } } : cur;
      });
      refetch(); // REST is the source of truth: refetch instead of trusting the pushed copy
    } else {
      refetch();
    }
  }), [subscribe, refetch]);

  const messages = useMemo<ChatMessage[]>(() => {
    const restLatest = latest.data ?? [];
    const rest = new Map<string, ChatMessageDto>();
    for (const m of [...older, ...restLatest]) rest.set(m.messageId, m);
    const fetchedAt = latest.dataUpdatedAt;
    const all = new Map(rest);
    for (const [id, l] of Object.entries(live)) {
      const r = rest.get(id);
      if (r) all.set(id, { ...r, translations: { ...l.dto.translations, ...r.translations } });
      else if (!(fetchedAt > l.receivedAt + LIVE_GRACE_MS)) all.set(id, l.dto);
    }
    const out: ChatMessage[] = [...all.values()]
      .sort((a, b) => Date.parse(a.createdAt) - Date.parse(b.createdAt) || a.messageId.localeCompare(b.messageId))
      .map((m) => ({
        id: m.messageId, senderId: m.senderId, from: m.senderName, text: m.content, ts: Date.parse(m.createdAt), mine: !!selfUserId && m.senderId === selfUserId,
        hidden: m.hidden, translations: m.translations, status: 'sent' as const,
      }));
    for (const p of pending) out.push({ id: p.clientId, clientId: p.clientId, from: selfName, text: p.content, ts: p.ts, mine: true, hidden: false, translations: {}, status: p.status, error: p.error });
    return out;
  }, [latest.data, latest.dataUpdatedAt, older, live, pending, selfUserId, selfName]);

  // Unread: anything from somebody else that was not part of the first history load.
  useEffect(() => {
    const wasReady = ready.current;
    for (const m of messages) {
      if (m.status !== 'sent' || seen.current.has(m.id)) continue;
      seen.current.add(m.id);
      if (!m.mine && !m.hidden && (wasReady || !!live[m.id])) incoming.current();
    }
    if (latest.isSuccess) ready.current = true;
  }, [messages, latest.isSuccess, live]);

  const run = useCallback(async (clientId: string, content: string) => {
    try {
      const dto = await postWithRetry(roomId, content, clientId);
      setLive((cur) => ({ ...cur, [dto.messageId]: { dto, receivedAt: Date.now() } }));
      seen.current.add(dto.messageId);
      setPending((cur) => cur.filter((p) => p.clientId !== clientId));
      // Instant copy for everybody else (best effort: REST already has it).
      void session.send('chat', { type: 'chat.message', ...dto }).catch(() => undefined);
      refetch();
    } catch (e) {
      setPending((cur) => cur.map((p) => (p.clientId === clientId ? { ...p, status: 'failed', error: errMsg(e) } : p)));
    }
  }, [roomId, session, refetch]);

  const send = useCallback((text: string) => {
    const clientId = newClientId();
    setPending((cur) => [...cur, { clientId, content: text, ts: Date.now(), status: 'sending' }]);
    void run(clientId, text);
  }, [run]);
  const retry = useCallback((clientId: string) => {
    const p = pending.find((x) => x.clientId === clientId);
    if (!p) return;
    setPending((cur) => cur.map((x) => (x.clientId === clientId ? { ...x, status: 'sending', error: undefined } : x)));
    void run(clientId, p.content);
  }, [pending, run]);
  const dismiss = useCallback((clientId: string) => setPending((cur) => cur.filter((p) => p.clientId !== clientId)), []);

  const restCount = (latest.data?.length ?? 0);
  const hasOlder = !exhausted && (older.length > 0 ? true : restCount >= CHAT_PAGE);
  const loadOlder = useCallback(async () => {
    const first = older[0] ?? latest.data?.[0];
    if (!first || loadingOlder) return;
    setLoadingOlder(true);
    try {
      const page = await fetchChat(roomId, { before: first.messageId });
      for (const m of page) seen.current.add(m.messageId);
      setOlder((cur) => [...page, ...cur]);
      if (page.length < CHAT_PAGE) setExhausted(true);
    } catch { /* the button stays; try again */ } finally { setLoadingOlder(false); }
  }, [older, latest.data, loadingOlder, roomId]);

  const hideMessage = useCallback(async (id: string) => { await hide.mutateAsync({ id }); }, [hide]);

  return { messages, send, retry, dismiss, hasOlder, loadingOlder, loadOlder, hideMessage, loadError: latest.isError ? errMsg(latest.error) : undefined };
}
