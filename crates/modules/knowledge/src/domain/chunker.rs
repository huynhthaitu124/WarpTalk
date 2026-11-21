//! Structure-aware chunking.
//!
//! Documents: pages -> markdown headings / paragraphs / code fences -> sentences, packed up to a
//! token budget with sentence-level overlap. A chunk never crosses a page or heading boundary
//! (so `page` and `heading_path` are exact) and is never cut mid-sentence; the only exception is
//! a single sentence larger than the hard budget, which is split at word boundaries.
//!
//! Transcripts: consecutive segments are grouped into time windows (and token budget) with
//! segment-level overlap; consecutive segments of one speaker share a `[Speaker]` line.

use super::text::{estimate_tokens, split_oversized, split_sentences};

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkingParams {
    /// Soft budget: chunks are closed once adding a sentence would pass it.
    pub target_tokens: usize,
    /// Hard budget for any single chunk.
    pub max_tokens: usize,
    /// Sentences (or segments) repeated at the start of the next chunk.
    pub overlap_tokens: usize,
    /// Hard character cap per chunk.
    pub char_limit: usize,
    pub transcript_window_ms: u64,
    /// A silence longer than this starts a new transcript chunk (no overlap carried).
    pub transcript_gap_ms: u64,
}

impl Default for ChunkingParams {
    fn default() -> Self {
        Self {
            target_tokens: 300,
            max_tokens: 450,
            overlap_tokens: 40,
            char_limit: 2000,
            transcript_window_ms: 60_000,
            transcript_gap_ms: 20_000,
        }
    }
}

/// Text of one page. `page` is 1-based; `None` for sources without pages.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PageText {
    #[serde(default)]
    pub page: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SegmentInput {
    pub segment_id: String,
    #[serde(default)]
    pub speaker: Option<String>,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// A chunk before embedding.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChunkDraft {
    pub text: String,
    pub page: Option<u32>,
    pub heading_path: Vec<String>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub segment_ids: Vec<String>,
    pub speakers: Vec<String>,
}

#[derive(Debug, Clone)]
struct Unit {
    text: String,
    tokens: usize,
    page: Option<u32>,
    heading_path: Vec<String>,
    /// Starts a new paragraph (joined with a blank line instead of a space).
    para_start: bool,
    /// A page or heading change precedes this unit: close the current chunk first.
    boundary: bool,
}

fn heading_of(line: &str) -> Option<(usize, String)> {
    let t = line.trim();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &t[hashes..];
        if rest.starts_with(' ') {
            let title = rest.trim().trim_end_matches('#').trim();
            if !title.is_empty() {
                return Some((hashes, title.to_owned()));
            }
        }
    }
    None
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

enum Block {
    Heading(usize, String),
    Paragraph(String),
    Code(String),
}

fn parse_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut code: Option<Vec<&str>> = None;
    let flush_para = |para: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            blocks.push(Block::Paragraph(para.join("\n")));
            para.clear();
        }
    };
    for line in text.lines() {
        if let Some(buf) = code.as_mut() {
            buf.push(line);
            if is_fence(line) && buf.len() > 1 {
                blocks.push(Block::Code(buf.join("\n")));
                code = None;
            }
            continue;
        }
        if is_fence(line) {
            flush_para(&mut para, &mut blocks);
            code = Some(vec![line]);
        } else if let Some((level, title)) = heading_of(line) {
            flush_para(&mut para, &mut blocks);
            blocks.push(Block::Heading(level, title));
        } else if line.trim().is_empty() {
            flush_para(&mut para, &mut blocks);
        } else {
            para.push(line.trim());
        }
    }
    if let Some(buf) = code {
        blocks.push(Block::Code(buf.join("\n")));
    }
    flush_para(&mut para, &mut blocks);
    blocks
}

fn push_units(
    out: &mut Vec<Unit>,
    pieces: Vec<String>,
    page: Option<u32>,
    heading_path: &[String],
    boundary: &mut bool,
    first_para_start: bool,
) {
    for (i, piece) in pieces.into_iter().enumerate() {
        let tokens = estimate_tokens(&piece);
        out.push(Unit {
            text: piece,
            tokens,
            page,
            heading_path: heading_path.to_vec(),
            para_start: first_para_start && i == 0,
            boundary: std::mem::take(boundary),
        });
    }
}

fn build_units(pages: &[PageText], p: &ChunkingParams) -> Vec<Unit> {
    let mut units = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut boundary = false;
    let mut first_page = true;
    for page in pages {
        if !first_page {
            boundary = true;
        }
        first_page = false;
        for block in parse_blocks(&page.text) {
            let path: Vec<String> = stack.iter().map(|(_, t)| t.clone()).collect();
            match block {
                Block::Heading(level, title) => {
                    while stack.last().is_some_and(|(l, _)| *l >= level) {
                        stack.pop();
                    }
                    stack.push((level, title));
                    boundary = true;
                }
                Block::Paragraph(text) => {
                    let mut pieces = Vec::new();
                    for s in split_sentences(&text) {
                        if estimate_tokens(&s) > p.max_tokens || s.chars().count() > p.char_limit {
                            pieces.extend(split_oversized(&s, p.max_tokens, p.char_limit));
                        } else {
                            pieces.push(s);
                        }
                    }
                    push_units(&mut units, pieces, page.page, &path, &mut boundary, true);
                }
                Block::Code(text) => {
                    // Code stays whole unless it exceeds the budget; then split by lines.
                    let mut pieces = Vec::new();
                    let mut cur = String::new();
                    for line in text.lines() {
                        let next_len = cur.chars().count() + line.chars().count() + 1;
                        if !cur.is_empty()
                            && (estimate_tokens(&cur) + estimate_tokens(line) > p.max_tokens
                                || next_len > p.char_limit)
                        {
                            pieces.push(std::mem::take(&mut cur));
                        }
                        if !cur.is_empty() {
                            cur.push('\n');
                        }
                        cur.push_str(line);
                    }
                    if !cur.is_empty() {
                        pieces.push(cur);
                    }
                    let pieces = pieces
                        .into_iter()
                        .flat_map(|s| {
                            if estimate_tokens(&s) > p.max_tokens || s.chars().count() > p.char_limit
                            {
                                split_oversized(&s, p.max_tokens, p.char_limit)
                            } else {
                                vec![s]
                            }
                        })
                        .collect();
                    push_units(&mut units, pieces, page.page, &path, &mut boundary, true);
                }
            }
        }
    }
    units
}

fn join_units(units: &[Unit]) -> String {
    let mut s = String::new();
    for (i, u) in units.iter().enumerate() {
        if i > 0 {
            s.push_str(if u.para_start { "\n\n" } else { " " });
        }
        s.push_str(&u.text);
    }
    s
}

/// Chunk a document (pages in order).
pub fn chunk_document(pages: &[PageText], p: &ChunkingParams) -> Vec<ChunkDraft> {
    let units = build_units(pages, p);
    let mut chunks = Vec::new();
    let mut cur: Vec<Unit> = Vec::new();
    let mut cur_tokens = 0usize;
    let mut cur_chars = 0usize;
    let min_fill = p.target_tokens / 3;

    let emit = |cur: &[Unit], chunks: &mut Vec<ChunkDraft>| {
        let Some(first) = cur.first() else { return };
        let text = join_units(cur);
        if text.trim().is_empty() {
            return;
        }
        chunks.push(ChunkDraft {
            text,
            page: first.page,
            heading_path: first.heading_path.clone(),
            ..ChunkDraft::default()
        });
    };

    for u in units {
        let u_chars = u.text.chars().count() + 1;
        let structural = u.boundary && !cur.is_empty();
        let over_target = cur_tokens + u.tokens > p.target_tokens
            && !(cur_tokens < min_fill && cur_tokens + u.tokens <= p.max_tokens);
        let over_chars = cur_chars + u_chars > p.char_limit;
        if !cur.is_empty() && (structural || over_target || over_chars) {
            emit(&cur, &mut chunks);
            if structural {
                cur.clear();
            } else {
                // carry whole trailing sentences as overlap
                let mut keep = 0usize;
                let mut sum = 0usize;
                for unit in cur.iter().rev() {
                    if sum + unit.tokens > p.overlap_tokens {
                        break;
                    }
                    sum += unit.tokens;
                    keep += 1;
                }
                let start = cur.len() - keep;
                cur = cur.split_off(start);
                if let Some(first) = cur.first_mut() {
                    first.para_start = false;
                }
            }
            cur_tokens = cur.iter().map(|x| x.tokens).sum();
            cur_chars = cur.iter().map(|x| x.text.chars().count() + 1).sum();
            // the overlap must leave room for the incoming unit
            while !cur.is_empty()
                && (cur_tokens + u.tokens > p.max_tokens || cur_chars + u_chars > p.char_limit)
            {
                let gone = cur.remove(0);
                cur_tokens -= gone.tokens;
                cur_chars -= gone.text.chars().count() + 1;
            }
        }
        cur_tokens += u.tokens;
        cur_chars += u_chars;
        cur.push(u);
    }
    emit(&cur, &mut chunks);
    chunks
}

#[derive(Debug, Clone)]
struct Piece {
    segment_id: String,
    speaker: Option<String>,
    start: u64,
    end: u64,
    text: String,
    tokens: usize,
}

fn render_pieces(pieces: &[Piece]) -> String {
    let mut lines: Vec<(Option<&str>, String)> = Vec::new();
    for p in pieces {
        match lines.last_mut() {
            Some((sp, line)) if *sp == p.speaker.as_deref() => {
                line.push(' ');
                line.push_str(&p.text);
            }
            _ => lines.push((p.speaker.as_deref(), p.text.clone())),
        }
    }
    lines
        .into_iter()
        .map(|(sp, line)| match sp {
            Some(s) => format!("[{s}] {line}"),
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn transcript_draft(pieces: &[Piece]) -> Option<ChunkDraft> {
    let first = pieces.first()?;
    let mut segment_ids: Vec<String> = Vec::new();
    let mut speakers: Vec<String> = Vec::new();
    for p in pieces {
        if !segment_ids.contains(&p.segment_id) {
            segment_ids.push(p.segment_id.clone());
        }
        if let Some(s) = &p.speaker
            && !speakers.contains(s)
        {
            speakers.push(s.clone());
        }
    }
    Some(ChunkDraft {
        text: render_pieces(pieces),
        page: None,
        heading_path: Vec::new(),
        start_ms: Some(first.start),
        end_ms: pieces.iter().map(|p| p.end).max(),
        segment_ids,
        speakers,
    })
}

/// Chunk transcript segments (any order; sorted by start time).
pub fn chunk_transcript(segments: &[SegmentInput], p: &ChunkingParams) -> Vec<ChunkDraft> {
    let mut segs: Vec<&SegmentInput> = segments
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .collect();
    segs.sort_by_key(|s| (s.start_ms, s.end_ms));

    let mut pieces: Vec<Piece> = Vec::new();
    for s in segs {
        let text = s.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let tokens = estimate_tokens(&text);
        let parts: Vec<String> = if tokens > p.max_tokens || text.chars().count() > p.char_limit {
            let mut v = Vec::new();
            for sent in split_sentences(&text) {
                if estimate_tokens(&sent) > p.max_tokens || sent.chars().count() > p.char_limit {
                    v.extend(split_oversized(&sent, p.max_tokens, p.char_limit));
                } else {
                    v.push(sent);
                }
            }
            v
        } else {
            vec![text]
        };
        for part in parts {
            pieces.push(Piece {
                segment_id: s.segment_id.clone(),
                speaker: s.speaker.clone(),
                start: s.start_ms,
                end: s.end_ms.max(s.start_ms),
                tokens: estimate_tokens(&part),
                text: part,
            });
        }
    }

    let mut chunks = Vec::new();
    let mut cur: Vec<Piece> = Vec::new();
    for piece in pieces {
        if let Some(first) = cur.first() {
            let last_end = cur.iter().map(|x| x.end).max().unwrap_or(first.end);
            let cur_tokens: usize = cur.iter().map(|x| x.tokens).sum();
            let gap = piece.start.saturating_sub(last_end) > p.transcript_gap_ms;
            let window = piece.end.saturating_sub(first.start) > p.transcript_window_ms;
            let size = cur_tokens + piece.tokens > p.target_tokens
                || render_pieces(&cur).chars().count() + piece.text.chars().count()
                    > p.char_limit;
            if gap || window || size {
                chunks.extend(transcript_draft(&cur));
                if gap {
                    cur.clear();
                } else {
                    let mut keep = 0;
                    let mut sum = 0;
                    for x in cur.iter().rev() {
                        if sum + x.tokens > p.overlap_tokens {
                            break;
                        }
                        sum += x.tokens;
                        keep += 1;
                    }
                    let at = cur.len() - keep;
                    cur = cur.split_off(at);
                }
            }
        }
        cur.push(piece);
    }
    chunks.extend(transcript_draft(&cur));
    chunks
}

/// Convention for text extractors: pages are separated by a form feed (`\x0c`, what `pdftotext`
/// emits) or by a line `<<page N>>`. Returns the pages with 1-based numbers.
pub fn split_pages_by_markers(text: &str) -> Vec<PageText> {
    let mut pages: Vec<PageText> = Vec::new();
    let mut cur = String::new();
    let mut number: u32 = 1;
    let mut flush = |cur: &mut String, number: u32, pages: &mut Vec<PageText>| {
        if !cur.trim().is_empty() {
            pages.push(PageText {
                page: Some(number),
                text: std::mem::take(cur),
            });
        } else {
            cur.clear();
        }
    };
    for line in text.split_inclusive('\n') {
        let marker = line
            .trim()
            .strip_prefix("<<page ")
            .and_then(|r| r.strip_suffix(">>"))
            .and_then(|n| n.trim().parse::<u32>().ok());
        if let Some(n) = marker {
            flush(&mut cur, number, &mut pages);
            number = n;
            continue;
        }
        let mut rest = line;
        while let Some(pos) = rest.find('\u{000C}') {
            cur.push_str(&rest[..pos]);
            flush(&mut cur, number, &mut pages);
            number += 1;
            rest = &rest[pos + 1..];
        }
        cur.push_str(rest);
    }
    flush(&mut cur, number, &mut pages);
    pages
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn params(target: usize, max: usize, overlap: usize) -> ChunkingParams {
        ChunkingParams {
            target_tokens: target,
            max_tokens: max,
            overlap_tokens: overlap,
            ..ChunkingParams::default()
        }
    }

    fn sentence(n: usize) -> String {
        format!("Sentence number {n} talks about the quarterly budget plan.")
    }

    #[test]
    fn headings_pages_and_sentences_are_respected() {
        let pages = vec![
            PageText {
                page: Some(1),
                text: format!("# Intro\n\n{}\n\n## Scope\n\n{}", sentence(1), sentence(2)),
            },
            PageText {
                page: Some(2),
                text: sentence(3),
            },
        ];
        let chunks = chunk_document(&pages, &params(300, 450, 0));
        assert_eq!(chunks.len(), 3, "{chunks:#?}");
        assert_eq!(chunks[0].heading_path, ["Intro"]);
        assert_eq!(chunks[0].page, Some(1));
        assert_eq!(chunks[1].heading_path, ["Intro", "Scope"]);
        assert_eq!(chunks[1].text, sentence(2));
        assert_eq!(chunks[2].page, Some(2));
        // page 2 inherits the heading path of the open section
        assert_eq!(chunks[2].heading_path, ["Intro", "Scope"]);
    }

    #[test]
    fn never_splits_mid_sentence_and_overlaps_whole_sentences() {
        let text = (1..=40).map(sentence).collect::<Vec<_>>().join(" ");
        let pages = vec![PageText {
            page: Some(7),
            text,
        }];
        let p = params(60, 90, 25);
        let chunks = chunk_document(&pages, &p);
        assert!(chunks.len() > 4);
        for c in &chunks {
            assert!(estimate_tokens(&c.text) <= p.max_tokens, "{}", c.text);
            assert_eq!(c.page, Some(7));
            // every chunk is a run of whole sentences
            for s in split_sentences(&c.text) {
                assert!(s.starts_with("Sentence number ") && s.ends_with("plan."), "{s}");
            }
        }
        // overlap: the first sentence of chunk n+1 appears at the end of chunk n
        for w in chunks.windows(2) {
            let first_next = split_sentences(&w[1].text)[0].clone();
            assert!(w[0].text.contains(&first_next), "no overlap: {first_next}");
        }
        // all sentences are covered in order
        let all: String = chunks.iter().map(|c| c.text.clone()).collect::<Vec<_>>().join(" ");
        for n in 1..=40 {
            assert!(all.contains(&sentence(n)));
        }
    }

    #[test]
    fn oversized_sentence_is_split_at_words() {
        let long = format!("{}.", "alpha ".repeat(300).trim());
        let chunks = chunk_document(
            &[PageText {
                page: None,
                text: long,
            }],
            &params(50, 80, 0),
        );
        assert!(chunks.len() > 2);
        assert!(chunks.iter().all(|c| estimate_tokens(&c.text) <= 80));
    }

    #[test]
    fn code_fences_stay_whole() {
        let text = "Run this:\n\n```\nline one.\nline two. still code\n```\n\nDone.";
        let chunks = chunk_document(
            &[PageText {
                page: None,
                text: text.into(),
            }],
            &params(300, 450, 0),
        );
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("line one.\nline two. still code"));
    }

    fn seg(id: &str, speaker: &str, start: u64, end: u64, text: &str) -> SegmentInput {
        SegmentInput {
            segment_id: id.into(),
            speaker: Some(speaker.into()),
            start_ms: start,
            end_ms: end,
            text: text.into(),
        }
    }

    #[test]
    fn transcript_groups_by_window_with_timestamps_and_segment_ids() {
        let segs = vec![
            seg("s3", "Bob", 25_000, 30_000, "We should ship Friday."),
            seg("s1", "Ann", 0, 4_000, "Welcome everyone."),
            seg("s2", "Ann", 4_000, 9_000, "Let us review the roadmap."),
            seg("s4", "Ann", 70_000, 75_000, "Great, then Friday it is."),
            seg("s5", "Bob", 400_000, 405_000, "Next meeting topic."),
        ];
        let p = ChunkingParams {
            overlap_tokens: 0,
            ..ChunkingParams::default()
        };
        let chunks = chunk_transcript(&segs, &p);
        assert_eq!(chunks.len(), 3, "{chunks:#?}");
        assert_eq!(chunks[0].segment_ids, ["s1", "s2", "s3"]);
        assert_eq!(chunks[0].start_ms, Some(0));
        assert_eq!(chunks[0].end_ms, Some(30_000));
        assert_eq!(chunks[0].speakers, ["Ann", "Bob"]);
        assert_eq!(
            chunks[0].text,
            "[Ann] Welcome everyone. Let us review the roadmap.\n[Bob] We should ship Friday."
        );
        assert_eq!(chunks[1].segment_ids, ["s4"]);
        // a long silence starts a fresh chunk
        assert_eq!(chunks[2].start_ms, Some(400_000));
    }

    #[test]
    fn transcript_overlap_repeats_trailing_segment() {
        let segs: Vec<_> = (0..12u64)
            .map(|i| seg(&format!("s{i}"), "Ann", i * 10_000, i * 10_000 + 9_000, "ok."))
            .collect();
        let p = ChunkingParams {
            transcript_window_ms: 30_000,
            overlap_tokens: 5,
            ..ChunkingParams::default()
        };
        let chunks = chunk_transcript(&segs, &p);
        assert!(chunks.len() >= 3);
        for w in chunks.windows(2) {
            let last = w[0].segment_ids.last().unwrap();
            assert!(w[1].segment_ids.contains(last), "{w:#?}");
        }
    }

    #[test]
    fn page_markers_split_pages() {
        let pages = split_pages_by_markers("one\n\u{c}two\n\u{c}three");
        assert_eq!(
            pages.iter().map(|p| (p.page, p.text.trim())).collect::<Vec<_>>(),
            [(Some(1), "one"), (Some(2), "two"), (Some(3), "three")]
        );
        let pages = split_pages_by_markers("<<page 4>>\nalpha\n<<page 9>>\nbeta\n");
        assert_eq!(
            pages.iter().map(|p| (p.page, p.text.trim())).collect::<Vec<_>>(),
            [(Some(4), "alpha"), (Some(9), "beta")]
        );
    }
}
