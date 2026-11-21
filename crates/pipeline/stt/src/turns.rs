//! Pure, synchronous transcript logic of one session: no IO, fully unit-testable.
//!
//! Model (verified against the upstream implementation, `stt_worker/model.py`
//! `_transcribe_via_session`):
//! - `gpt-live-transcribe` transcribes audio **as it is appended**, so deltas arrive while the
//!   speaker is still talking, tagged with the conversation item they belong to.
//! - A turn's item id is only learned from `input_audio_buffer.committed`. Deltas are therefore
//!   buffered per item and attributed to a turn at that point; deltas of items that are never
//!   committed (a cleared turn) are never emitted.
//! - **Early sentences**: at `committed` everything transcribed so far is already in the buffer,
//!   so complete sentences go out immediately (zero wait) and further ones as deltas arrive,
//!   instead of waiting for `completed` (commit -> completed p50 774 ms upstream).
//! - Early output is **append-only**; `completed` carries the whole transcript, of which only the
//!   remainder after the flushed prefix is emitted (whitespace-blind prefix match: Japanese has
//!   no space after `。`, and numbers cut at a dot come back whole).
//! - A revision inside the flushed prefix cannot be retracted (it was already translated), so the
//!   remainder is dropped rather than risk duplicate output.
//! - Turns are emitted strictly in commit order; events of a later item wait for earlier turns.
//! - The logprob floor applies to the remainder only: per-token logprobs exist only on
//!   `completed`, so early sentences are exempt.
//! - Empty / control-only text is never emitted; the same sentence repeated three times in a row
//!   (the model getting stuck on noise) stops early emission for the turn.

use std::collections::{HashMap, VecDeque};

use tracing::{debug, warn};

use crate::config::LogprobFilter;
use crate::protocol::{ErrorBody, ServerEvent, TokenLogprob};
use crate::sentence::split_stream;

/// What the session should forward to the runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Segment {
        text: String,
        lang: String,
        confidence: Option<f32>,
        is_early: bool,
    },
    Done {
        ok: bool,
    },
}

/// Removes control characters, collapses whitespace; `None` when nothing is left.
pub fn clean_text(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in s.chars() {
        if c.is_whitespace() || c.is_control() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Cheap script-based guess used only when no language hint exists. `"und"` when unsure.
pub fn guess_language(text: &str) -> &'static str {
    let mut vi = false;
    for c in text.chars() {
        match c {
            '\u{3040}'..='\u{30FF}' => return "ja",
            '\u{AC00}'..='\u{D7AF}' => return "ko",
            'ă' | 'â' | 'đ' | 'ê' | 'ô' | 'ơ' | 'ư' | 'Ă' | 'Â' | 'Đ' | 'Ê' | 'Ô' | 'Ơ' | 'Ư'
            | '\u{1EA0}'..='\u{1EF9}' => vi = true,
            _ => {}
        }
    }
    if vi {
        "vi"
    } else if text.chars().any(|c| c.is_ascii_alphabetic()) {
        "en"
    } else {
        "und"
    }
}

/// Whitespace-blind prefix removal. `None` when `flushed` is not a prefix of `final_text`.
pub fn after_flushed_prefix(final_text: &str, flushed: &str) -> Option<String> {
    let mut rest = final_text.char_indices().peekable();
    for ch in flushed.chars().filter(|c| !c.is_whitespace()) {
        loop {
            match rest.peek() {
                Some(&(_, c)) if c.is_whitespace() => {
                    rest.next();
                }
                Some(&(_, c)) if c == ch => {
                    rest.next();
                    break;
                }
                _ => return None,
            }
        }
    }
    let start = rest.peek().map_or(final_text.len(), |&(i, _)| i);
    Some(final_text[start..].trim().to_string())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Remainder {
    Text(String),
    Empty,
    /// The model revised text that was already flushed.
    Mismatch,
}

/// Append-only early-sentence emitter for one turn.
#[derive(Debug, Default)]
pub struct EarlyEmitter {
    tail: String,
    flushed: String,
    last: Option<String>,
    repeats: u8,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RepetitionLoop;

impl EarlyEmitter {
    /// Adds a delta; returns sentences that became complete.
    pub fn push_delta(&mut self, delta: &str) -> Result<Vec<String>, RepetitionLoop> {
        self.tail.push_str(delta);
        let (sentences, tail) = split_stream(&self.tail, false);
        self.tail = tail;
        let mut out = Vec::new();
        for s in sentences {
            let Some(clean) = clean_text(&s) else {
                continue;
            };
            let norm = clean.to_lowercase();
            if self.last.as_deref() == Some(norm.as_str()) {
                self.repeats += 1;
            } else {
                self.repeats = 0;
            }
            self.last = Some(norm);
            if self.repeats >= 2 {
                return Err(RepetitionLoop);
            }
            if !self.flushed.is_empty() {
                self.flushed.push(' ');
            }
            self.flushed.push_str(&clean);
            out.push(clean);
        }
        Ok(out)
    }

    pub fn flushed(&self) -> &str {
        &self.flushed
    }

    /// What `final_text` adds beyond what was flushed.
    pub fn finalize(&self, final_text: &str) -> Remainder {
        if self.flushed.is_empty() {
            return clean_text(final_text).map_or(Remainder::Empty, Remainder::Text);
        }
        match after_flushed_prefix(final_text, &self.flushed) {
            Some(rest) => clean_text(&rest).map_or(Remainder::Empty, Remainder::Text),
            None => Remainder::Mismatch,
        }
    }
}

#[derive(Debug, Default)]
struct ItemBuf {
    deltas: String,
    completed: Option<(String, Option<Vec<TokenLogprob>>)>,
    failed: Option<Option<ErrorBody>>,
}

#[derive(Debug)]
struct BoundTurn {
    item_id: String,
    lang_hint: Option<String>,
    emitter: EarlyEmitter,
    poisoned: bool,
}

/// Bounded memory for items that never get committed.
const MAX_ITEMS: usize = 16;

pub struct TurnMachine {
    filter: LogprobFilter,
    /// Commits issued, awaiting `input_audio_buffer.committed` (language hint of each).
    pending: VecDeque<Option<String>>,
    bound: VecDeque<BoundTurn>,
    items: HashMap<String, ItemBuf>,
    item_order: VecDeque<String>,
}

impl TurnMachine {
    pub fn new(filter: LogprobFilter) -> Self {
        Self {
            filter,
            pending: VecDeque::new(),
            bound: VecDeque::new(),
            items: HashMap::new(),
            item_order: VecDeque::new(),
        }
    }

    /// Call (before sending the commit) with the speaker's language hint.
    pub fn expect_commit(&mut self, lang_hint: Option<String>) {
        self.pending.push_back(lang_hint);
    }

    /// Commits whose outcome is still unknown.
    pub fn outstanding(&self) -> usize {
        self.pending.len() + self.bound.len()
    }

    /// Connection lost / timed out: fail every outstanding commit.
    pub fn fail_all(&mut self) -> Vec<Out> {
        let n = self.outstanding();
        self.pending.clear();
        self.bound.clear();
        self.items.clear();
        self.item_order.clear();
        (0..n).map(|_| Out::Done { ok: false }).collect()
    }

    pub fn on_event(&mut self, ev: ServerEvent) -> Vec<Out> {
        let mut out = Vec::new();
        match ev {
            ServerEvent::Committed { item_id, .. } => {
                if let Some(lang_hint) = self.pending.pop_front() {
                    self.touch(&item_id);
                    self.bound.push_back(BoundTurn {
                        item_id,
                        lang_hint,
                        emitter: EarlyEmitter::default(),
                        poisoned: false,
                    });
                }
            }
            ServerEvent::Delta { item_id, delta } => {
                self.touch(&item_id);
                if let Some(b) = self.items.get_mut(&item_id) {
                    b.deltas.push_str(&delta);
                }
            }
            ServerEvent::Completed {
                item_id,
                transcript,
                logprobs,
            } => {
                self.touch(&item_id);
                if let Some(b) = self.items.get_mut(&item_id) {
                    b.completed = Some((transcript, logprobs));
                }
            }
            ServerEvent::Failed { item_id, error } => {
                self.touch(&item_id);
                if let Some(b) = self.items.get_mut(&item_id) {
                    b.failed = Some(error);
                }
            }
            ServerEvent::Error { error } => {
                if error.is_commit_empty() && self.pending.pop_front().is_some() {
                    out.push(Out::Done { ok: false });
                } else {
                    warn!(code = ?error.code, message = ?error.message, "realtime error event");
                }
            }
            ServerEvent::SessionCreated {} | ServerEvent::SessionUpdated {} | ServerEvent::Other => {}
        }
        self.pump(&mut out);
        out
    }

    fn touch(&mut self, item_id: &str) {
        if !self.items.contains_key(item_id) {
            self.items.insert(item_id.to_string(), ItemBuf::default());
            self.item_order.push_back(item_id.to_string());
            while self.item_order.len() > MAX_ITEMS {
                // Never evict an item a bound turn is waiting on.
                let Some(old) = self.item_order.pop_front() else {
                    break;
                };
                if self.bound.iter().any(|b| b.item_id == old) {
                    self.item_order.push_back(old);
                    break;
                }
                self.items.remove(&old);
            }
        }
    }

    fn pump(&mut self, out: &mut Vec<Out>) {
        loop {
            let Some(front) = self.bound.front_mut() else {
                return;
            };
            let Some(buf) = self.items.get_mut(&front.item_id) else {
                return;
            };
            let lang_for = |hint: &Option<String>, text: &str| {
                hint.clone().unwrap_or_else(|| guess_language(text).to_string())
            };
            if !buf.deltas.is_empty() {
                let d = std::mem::take(&mut buf.deltas);
                if !front.poisoned {
                    match front.emitter.push_delta(&d) {
                        Ok(sentences) => {
                            for s in sentences {
                                out.push(Out::Segment {
                                    lang: lang_for(&front.lang_hint, &s),
                                    text: s,
                                    confidence: None,
                                    is_early: true,
                                });
                            }
                        }
                        Err(RepetitionLoop) => {
                            warn!("stt repetition loop; stopping early output for this turn");
                            front.poisoned = true;
                        }
                    }
                }
            }
            if buf.failed.take().is_some() {
                let id = front.item_id.clone();
                self.finish_front(&id, out, false);
                continue;
            }
            let Some((transcript, logprobs)) = buf.completed.take() else {
                return;
            };
            let ok = !front.poisoned;
            if !front.poisoned {
                match front.emitter.finalize(&transcript) {
                    Remainder::Text(text) => {
                        let lang = lang_for(&front.lang_hint, &text);
                        let avg = average_logprob(logprobs.as_deref());
                        if self.filter.keeps(&lang, avg) {
                            out.push(Out::Segment {
                                text,
                                lang,
                                confidence: avg,
                                is_early: false,
                            });
                        } else {
                            debug!(?avg, %lang, "dropped low-confidence transcript");
                        }
                    }
                    Remainder::Empty => {}
                    Remainder::Mismatch => warn!(
                        flushed = front.emitter.flushed(),
                        "final transcript revised flushed text; dropping the remainder"
                    ),
                }
            }
            let id = front.item_id.clone();
            self.finish_front(&id, out, ok);
        }
    }

    fn finish_front(&mut self, item_id: &str, out: &mut Vec<Out>, ok: bool) {
        self.bound.pop_front();
        self.items.remove(item_id);
        self.item_order.retain(|i| i != item_id);
        out.push(Out::Done { ok });
    }
}

/// Mean token logprob; `None` when the model supplied none.
pub fn average_logprob(lp: Option<&[TokenLogprob]>) -> Option<f32> {
    let lp = lp?;
    if lp.is_empty() {
        return None;
    }
    Some((lp.iter().map(|t| t.logprob).sum::<f64>() / lp.len() as f64) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> TurnMachine {
        TurnMachine::new(LogprobFilter {
            default_floor: -0.7,
            by_language: HashMap::from([("ja".to_string(), -1.2)]),
        })
    }
    fn delta(item: &str, d: &str) -> ServerEvent {
        ServerEvent::Delta {
            item_id: item.into(),
            delta: d.into(),
        }
    }
    fn committed(item: &str) -> ServerEvent {
        ServerEvent::Committed {
            item_id: item.into(),
            previous_item_id: None,
        }
    }
    fn completed(item: &str, text: &str, lp: Option<f64>) -> ServerEvent {
        ServerEvent::Completed {
            item_id: item.into(),
            transcript: text.into(),
            logprobs: lp.map(|l| {
                vec![TokenLogprob {
                    token: "x".into(),
                    logprob: l,
                }]
            }),
        }
    }
    fn seg(text: &str, early: bool) -> Out {
        Out::Segment {
            text: text.into(),
            lang: "en".into(),
            confidence: None,
            is_early: early,
        }
    }

    #[test]
    fn deltas_before_commit_flush_at_commit() {
        let mut m = machine();
        m.expect_commit(Some("en".into()));
        assert!(m.on_event(delta("i1", "Hello there. How are")).is_empty());
        // Commit attributes the buffered text: first sentence goes out with zero wait.
        assert_eq!(m.on_event(committed("i1")), vec![seg("Hello there.", true)]);
        assert_eq!(
            m.on_event(delta("i1", " you? I am")),
            vec![seg("How are you?", true)]
        );
    }

    #[test]
    fn completed_carries_only_the_remainder() {
        let mut m = machine();
        m.expect_commit(Some("en".into()));
        m.on_event(committed("i1"));
        m.on_event(delta("i1", "Hello there. How are you? I am fine."));
        let out = m.on_event(completed("i1", "Hello there. How are you? I am fine.", None));
        assert_eq!(out, vec![Out::Done { ok: true }]); // all flushed early, nothing left
        assert_eq!(m.outstanding(), 0);

        m.expect_commit(Some("en".into()));
        m.on_event(committed("i2"));
        m.on_event(delta("i2", "One. Two and"));
        let out = m.on_event(completed("i2", "One. Two and three.", Some(-0.1)));
        assert_eq!(
            out,
            vec![
                Out::Segment {
                    text: "Two and three.".into(),
                    lang: "en".into(),
                    confidence: Some(-0.1),
                    is_early: false
                },
                Out::Done { ok: true }
            ]
        );
    }

    #[test]
    fn whitespace_blind_prefix_for_japanese() {
        assert_eq!(
            after_flushed_prefix("すみません。それは本当", "すみません。"),
            Some("それは本当".into())
        );
        assert_eq!(after_flushed_prefix("0.2.27 ok", "0. 2. 27"), Some("ok".into()));
        assert_eq!(after_flushed_prefix("different", "Hello"), None);
    }

    #[test]
    fn revision_of_flushed_text_drops_remainder() {
        let mut m = machine();
        m.expect_commit(Some("en".into()));
        m.on_event(committed("i1"));
        m.on_event(delta("i1", "Hello there. More"));
        let out = m.on_event(completed("i1", "Hi there. More words.", None));
        assert_eq!(out, vec![Out::Done { ok: true }]);
    }

    #[test]
    fn low_logprob_remainder_filtered_with_language_override() {
        let mut m = machine();
        m.expect_commit(Some("vi".into()));
        m.on_event(committed("i1"));
        let out = m.on_event(completed("i1", "Xin chào", Some(-1.0)));
        assert_eq!(out, vec![Out::Done { ok: true }]);

        m.expect_commit(Some("ja".into()));
        m.on_event(committed("i2"));
        let out = m.on_event(completed("i2", "はい", Some(-1.0)));
        assert!(matches!(out[0], Out::Segment { .. })); // -1.0 >= ja floor -1.2
    }

    #[test]
    fn empty_and_control_text_deduped() {
        let mut m = machine();
        m.expect_commit(None);
        m.on_event(committed("i1"));
        let out = m.on_event(completed("i1", " \u{0}\u{7} \n", None));
        assert_eq!(out, vec![Out::Done { ok: true }]);
        assert_eq!(clean_text("a\u{0}b\n  c"), Some("a b c".into()));
        assert_eq!(clean_text(" \t"), None);
    }

    #[test]
    fn later_item_waits_for_earlier_turn() {
        let mut m = machine();
        m.expect_commit(Some("en".into()));
        m.expect_commit(Some("en".into()));
        m.on_event(committed("a"));
        m.on_event(committed("b"));
        // b's events arrive first: nothing may be emitted ahead of a.
        assert!(m.on_event(delta("b", "Second one. rest")).is_empty());
        assert!(m.on_event(completed("b", "Second one. rest", None)).is_empty());
        let out = m.on_event(completed("a", "First.", None));
        assert_eq!(
            out,
            vec![
                seg("First.", false),
                Out::Done { ok: true },
                seg("Second one.", true),
                seg("rest", false),
                Out::Done { ok: true },
            ]
        );
    }

    #[test]
    fn uncommitted_item_deltas_never_emitted() {
        let mut m = machine();
        m.on_event(delta("ghost", "Stale words. here"));
        m.expect_commit(Some("en".into()));
        m.on_event(committed("real"));
        let out = m.on_event(delta("real", "Fresh. text"));
        assert_eq!(out, vec![seg("Fresh.", true)]);
    }

    #[test]
    fn repetition_loop_stops_early_output() {
        let mut m = machine();
        m.expect_commit(Some("en".into()));
        m.on_event(committed("i"));
        let mut early = 0;
        for _ in 0..5 {
            early += m
                .on_event(delta("i", "Thank you. "))
                .iter()
                .filter(|o| matches!(o, Out::Segment { .. }))
                .count();
        }
        assert_eq!(early, 2);
        let out = m.on_event(completed("i", "Thank you.", None));
        assert_eq!(out, vec![Out::Done { ok: false }]);
    }

    #[test]
    fn failed_and_connection_loss() {
        let mut m = machine();
        m.expect_commit(None);
        m.on_event(committed("i"));
        let out = m.on_event(ServerEvent::Failed {
            item_id: "i".into(),
            error: None,
        });
        assert_eq!(out, vec![Out::Done { ok: false }]);

        m.expect_commit(None);
        m.expect_commit(None);
        assert_eq!(m.fail_all(), vec![Out::Done { ok: false }; 2]);
    }

    #[test]
    fn empty_commit_error_fails_the_pending_commit() {
        let mut m = machine();
        m.expect_commit(None);
        let out = m.on_event(ServerEvent::Error {
            error: ErrorBody {
                code: Some(crate::protocol::ERR_COMMIT_EMPTY.into()),
                ..ErrorBody::default()
            },
        });
        assert_eq!(out, vec![Out::Done { ok: false }]);
        assert_eq!(m.outstanding(), 0);
    }

    #[test]
    fn language_guess() {
        assert_eq!(guess_language("すみません"), "ja");
        assert_eq!(guess_language("Xin chào các bạn, đây là"), "vi");
        assert_eq!(guess_language("hello"), "en");
        assert_eq!(guess_language("123"), "und");
    }
}
