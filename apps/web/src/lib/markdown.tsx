// Small, safe markdown renderer for assistant answers: headings, paragraphs, bullet/numbered lists, **bold**, *italic*,
// `code` and http(s)/mailto links. Everything becomes React elements (text is never interpreted as HTML), links open
// in a new tab with rel=noopener, and citation tags such as [T3] / [D2] are handed to `renderTag`.
import type { ReactNode } from 'react';
import type { Citation } from './ask';

const TAG_GROUP = '\\[[TtDd]\\d{1,3}(?:[,;\\s]+[TtDd]\\d{1,3})*\\]';
const TAG = new RegExp(`^${TAG_GROUP}$`);
const TOKENS = new RegExp([
  String.raw`\*\*[^*\n]+\*\*`,
  '`[^`\\n]+`',
  TAG_GROUP,
  String.raw`\[[^\]\n]+\]\((?:https?:\/\/|mailto:)[^)\s]+\)`,
  String.raw`(?<![*\w])\*[^*\s][^*\n]*\*(?![*\w])`,
  String.raw`(?<!\w)_[^_\s][^_\n]*_(?!\w)`,
].join('|'));

export type TagRenderer = (tag: string, kind: 'T' | 'D', n: number) => ReactNode;

function inline(text: string, renderTag: TagRenderer | undefined, key: string): ReactNode[] {
  const out: ReactNode[] = [];
  let rest = text;
  let i = 0;
  while (rest) {
    const m = TOKENS.exec(rest);
    if (!m) { out.push(rest); break; }
    if (m.index > 0) out.push(rest.slice(0, m.index));
    const tok = m[0];
    const k = `${key}-${i++}`;
    if (TAG.test(tok)) {
      const parts = [...tok.matchAll(/([TtDd])(\d{1,3})/g)];
      out.push(<span key={k}>{renderTag ? parts.map((p, n) => <span key={n}>{renderTag(`[${p[0]}]`, p[1]!.toUpperCase() as 'T' | 'D', Number(p[2]))}</span>) : tok}</span>);
    } else if (tok.startsWith('**')) out.push(<strong key={k} className="font-semibold">{inline(tok.slice(2, -2), renderTag, k)}</strong>);
    else if (tok.startsWith('`')) out.push(<code key={k} className="rounded-xs bg-surface-3 px-1 font-mono text-[0.9em]">{tok.slice(1, -1)}</code>);
    else if (tok.startsWith('[')) {
      const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(tok);
      out.push(link ? <a key={k} href={link[2]} target="_blank" rel="noopener noreferrer">{link[1]}</a> : tok);
    } else out.push(<em key={k}>{inline(tok.slice(1, -1), renderTag, k)}</em>);
    rest = rest.slice(m.index + tok.length);
  }
  return out;
}

/** Block-level pass over the answer text. */
export function Markdown({ text, renderTag, className }: { text: string; renderTag?: TagRenderer; className?: string }) {
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  const blocks: ReactNode[] = [];
  let para: string[] = [];
  let list: { ordered: boolean; items: string[] } | null = null;
  const flushPara = () => {
    if (!para.length) return;
    const k = `p${blocks.length}`;
    blocks.push(<p key={k} className="break-words">{para.flatMap((l, n) => [...(n ? [<br key={`${k}-b${n}`} />] : []), ...inline(l, renderTag, `${k}-${n}`)])}</p>);
    para = [];
  };
  const flushList = () => {
    if (!list) return;
    const k = `l${blocks.length}`;
    const Tag = list.ordered ? 'ol' : 'ul';
    blocks.push(<Tag key={k} className={list.ordered ? 'list-decimal pl-5' : 'list-disc pl-5'}>{list.items.map((it, n) => <li key={n} className="break-words">{inline(it, renderTag, `${k}-${n}`)}</li>)}</Tag>);
    list = null;
  };
  for (const raw of lines) {
    const line = raw.trimEnd();
    const h = /^(#{1,6})\s+(.*)$/.exec(line);
    const ul = /^\s*[-*•]\s+(.*)$/.exec(line);
    const ol = /^\s*\d+[.)]\s+(.*)$/.exec(line);
    if (h) {
      flushPara(); flushList();
      const k = `h${blocks.length}`;
      blocks.push(<h4 key={k} className="mt-1 text-body font-semibold text-ink">{inline(h[2]!, renderTag, k)}</h4>);
    } else if (ul || ol) {
      flushPara();
      const ordered = !!ol;
      if (list && list.ordered !== ordered) flushList();
      list ??= { ordered, items: [] };
      list.items.push((ul ?? ol)![1]!);
    } else if (!line.trim()) { flushPara(); flushList(); }
    else { flushList(); para.push(line); }
  }
  flushPara(); flushList();
  return <div className={className ?? 'flex flex-col gap-2'}>{blocks}</div>;
}

/**
 * Maps `[T3]` / `[D2]` tags to citations. The backend lists citations in order of first use and drops tags that point
 * nowhere, so tags are matched to the citation list in order, by kind.
 */
export function tagCitations(text: string, citations: Citation[]): Map<string, Citation> {
  const map = new Map<string, Citation>();
  let next = 0;
  for (const g of text.matchAll(new RegExp(TAG_GROUP, 'g'))) for (const m of g[0].matchAll(/([TtDd])(\d{1,3})/g)) {
    const kind = m[1]!.toUpperCase();
    const id = `${kind}${m[2]}`;
    if (map.has(id)) continue;
    const c = citations[next];
    if (c && (kind === 'T') === (c.kind === 'transcript')) { map.set(id, c); next++; }
  }
  return map;
}
