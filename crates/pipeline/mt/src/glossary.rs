//! Glossary: precedence merge, relevance selection, prompt rendering.
//!
//! Only terms that OCCUR in the source are sent (spec 08 section 4 item 15: the whole glossary
//! on every realtime request adds tokens and steers short ambiguous speech toward unrelated
//! terms). Two kinds of hit, ported from upstream `_select_relevant_glossary_terms`:
//! - [`MatchKind::Exact`]: the term is literally in the text (word boundaries, spaces and
//!   hyphens equivalent). A terminology instruction.
//! - [`MatchKind::Possible`]: nothing matched literally but part of the text SOUNDS like the
//!   term (STT heard "Codex" as "cô đích"). A suspicion offered to the model, never a fact, and
//!   the only glossary evidence that opens the ASR-repair clause.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlossaryTerm {
    pub source: String,
    /// Empty or equal to `source` (case-insensitively) means "keep as written, do not translate".
    pub target: String,
}

impl GlossaryTerm {
    pub fn new(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
        }
    }

    fn is_keep(&self) -> bool {
        self.target.trim().is_empty() || self.target.trim().eq_ignore_ascii_case(self.source.trim())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Exact,
    Possible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedTerm {
    pub term: GlossaryTerm,
    pub kind: MatchKind,
}

/// Glossary of one room: workspace terms override global ones with the same source term
/// (case-insensitive). Called per segment, so implementations should cache.
#[async_trait]
pub trait GlossaryProvider: Send + Sync {
    async fn terms(&self, room: &str) -> Arc<Vec<GlossaryTerm>>;
}

/// Precedence merge: workspace > global.
pub fn merge_glossaries(global: &[GlossaryTerm], workspace: &[GlossaryTerm]) -> Vec<GlossaryTerm> {
    let mut out: Vec<GlossaryTerm> = workspace.to_vec();
    for g in global {
        let key = g.source.trim().to_lowercase();
        if !out.iter().any(|t| t.source.trim().to_lowercase() == key) {
            out.push(g.clone());
        }
    }
    out
}

#[derive(Default)]
struct MemState {
    global: Vec<GlossaryTerm>,
    workspace_terms: HashMap<String, Vec<GlossaryTerm>>,
    room_workspace: HashMap<String, String>,
    merged: HashMap<String, Arc<Vec<GlossaryTerm>>>,
}

/// In-memory [`GlossaryProvider`] for tests and single-node setups.
#[derive(Default)]
pub struct InMemoryGlossary {
    state: RwLock<MemState>,
}

impl InMemoryGlossary {
    pub fn new() -> Self {
        Self::default()
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, MemState> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set_global(&self, terms: Vec<GlossaryTerm>) {
        let mut s = self.write();
        s.global = terms;
        s.merged.clear();
    }

    pub fn set_workspace(&self, workspace: &str, terms: Vec<GlossaryTerm>) {
        let mut s = self.write();
        s.workspace_terms.insert(workspace.into(), terms);
        s.merged.clear();
    }

    pub fn bind_room(&self, room: &str, workspace: &str) {
        let mut s = self.write();
        s.room_workspace.insert(room.into(), workspace.into());
        s.merged.remove(room);
    }
}

#[async_trait]
impl GlossaryProvider for InMemoryGlossary {
    async fn terms(&self, room: &str) -> Arc<Vec<GlossaryTerm>> {
        if let Some(m) = self
            .state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .merged
            .get(room)
        {
            return m.clone();
        }
        let mut s = self.write();
        let ws = s
            .room_workspace
            .get(room)
            .and_then(|w| s.workspace_terms.get(w))
            .cloned()
            .unwrap_or_default();
        let merged = Arc::new(merge_glossaries(&s.global, &ws));
        s.merged.insert(room.into(), merged.clone());
        merged
    }
}

// ---- matching ---------------------------------------------------------------------------

/// Folds a char to roughly what it sounds like: lowercase, Latin diacritics and the Vietnamese
/// tone marks dropped, `đ` mapped by hand (no Unicode normalisation crate in the workspace, so
/// this is a table; it covers Latin-1, Latin Extended-A and the Vietnamese block).
fn fold_char(c: char) -> Option<char> {
    let lc = c.to_lowercase().next().unwrap_or(c);
    const GROUPS: &[(&str, char)] = &[
        ("àáảãạăằắẳẵặâầấẩẫậäåāąá", 'a'),
        ("èéẻẽẹêềếểễệëēęě", 'e'),
        ("ìíỉĩịïīîı", 'i'),
        ("òóỏõọôồốổỗộơờớởỡợöøōő", 'o'),
        ("ùúủũụưừứửữựüūůűû", 'u'),
        ("ỳýỷỹỵÿ", 'y'),
        ("đďð", 'd'),
        ("çćč", 'c'),
        ("ñńň", 'n'),
        ("śšş", 's'),
        ("źżž", 'z'),
        ("łľ", 'l'),
        ("ř", 'r'),
        ("ť", 't'),
    ];
    for (set, to) in GROUPS {
        if set.contains(lc) {
            return Some(*to);
        }
    }
    // Combining marks and separators carry no sound.
    if ('\u{0300}'..='\u{036f}').contains(&lc)
        || lc.is_whitespace()
        || matches!(lc, '_' | '-' | '.')
    {
        return None;
    }
    Some(lc)
}

pub(crate) fn skeleton(s: &str) -> Vec<char> {
    s.chars().filter_map(fold_char).collect()
}

/// Ratcliff/Obershelp similarity (Python `difflib.SequenceMatcher.ratio`), over char slices.
fn ratio(a: &[char], b: &[char]) -> f32 {
    fn matches(a: &[char], b: &[char]) -> usize {
        if a.is_empty() || b.is_empty() {
            return 0;
        }
        // Longest common substring.
        let (mut best, mut ai, mut bi) = (0usize, 0usize, 0usize);
        let mut prev = vec![0usize; b.len() + 1];
        for i in 1..=a.len() {
            let mut cur = vec![0usize; b.len() + 1];
            for j in 1..=b.len() {
                if a[i - 1] == b[j - 1] {
                    cur[j] = prev[j - 1] + 1;
                    if cur[j] > best {
                        best = cur[j];
                        ai = i - best;
                        bi = j - best;
                    }
                }
            }
            prev = cur;
        }
        if best == 0 {
            return 0;
        }
        best + matches(&a[..ai], &b[..bi]) + matches(&a[ai + best..], &b[bi + best..])
    }
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    2.0 * matches(a, b) as f32 / total as f32
}

/// Measured upstream on real Vietnamese sentences: 0.65 sits in the gap for terms of seven or
/// more skeleton characters; below that no threshold separates a mishearing from chance, so
/// short terms are excluded rather than guessed at.
const MISHEARD_SIMILARITY: f32 = 0.65;
const MIN_MISHEARD_SKELETON: usize = 7;

fn sounds_like(source: &str, text: &str) -> bool {
    let needle = skeleton(source);
    if needle.len() < MIN_MISHEARD_SKELETON {
        return false;
    }
    let hay = skeleton(text);
    if hay.is_empty() {
        return false;
    }
    if hay.windows(needle.len()).any(|w| w == needle.as_slice()) {
        return true;
    }
    let lo = MIN_MISHEARD_SKELETON.max(needle.len().saturating_sub(2));
    for width in lo..=needle.len() + 2 {
        if width > hay.len() {
            break;
        }
        for w in hay.windows(width) {
            if ratio(&needle, w) >= MISHEARD_SIMILARITY {
                return true;
            }
        }
    }
    false
}

/// Normalises separators (`[\s_-]+` -> one space) and lowercases.
fn collapse(s: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    let mut sep = false;
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_whitespace() || c == '_' || c == '-' {
            sep = true;
            continue;
        }
        if sep && !out.is_empty() {
            out.push(' ');
        }
        sep = false;
        out.push(c);
    }
    out
}

/// Literal occurrence with word boundaries (`(?<!\w)term(?!\w)`).
fn contains_exact(haystack: &str, term: &str) -> bool {
    let needle = collapse(term);
    if needle.is_empty() {
        return false;
    }
    let hay = collapse(haystack);
    if hay.len() < needle.len() {
        return false;
    }
    (0..=hay.len() - needle.len()).any(|i| {
        hay[i..i + needle.len()] == needle[..]
            && (i == 0 || !hay[i - 1].is_alphanumeric())
            && hay
                .get(i + needle.len())
                .is_none_or(|c| !c.is_alphanumeric())
    })
}

/// Terms worth sending with `text`, tagged by how they matched, capped at `limit`. Exact hits
/// are collected first and never lose their slot to a suspicion. Empty when nothing occurs.
pub fn select_terms(text: &str, terms: &[GlossaryTerm], limit: usize) -> Vec<SelectedTerm> {
    if terms.is_empty() || text.trim().is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut exact = Vec::new();
    let mut possible = Vec::new();
    for t in terms {
        if t.source.trim().is_empty() {
            continue;
        }
        if contains_exact(text, &t.source) {
            exact.push(SelectedTerm {
                term: t.clone(),
                kind: MatchKind::Exact,
            });
        } else if sounds_like(&t.source, text) {
            possible.push(SelectedTerm {
                term: t.clone(),
                kind: MatchKind::Possible,
            });
        }
    }
    exact.extend(possible);
    exact.truncate(limit);
    exact
}

/// True when any selected term is only a suspicion: evidence of a mishearing. An exact hit is a
/// terminology instruction, not a sign the recogniser slipped, so it does not open repair.
pub fn has_suspected_mishearing(selected: &[SelectedTerm]) -> bool {
    selected.iter().any(|s| s.kind == MatchKind::Possible)
}

/// System-prompt addendum. Empty when there is nothing to say.
pub fn render_block(selected: &[SelectedTerm]) -> String {
    let (mut keep, mut map, mut heard) = (Vec::new(), Vec::new(), Vec::new());
    for s in selected {
        let source = s.term.source.trim();
        if source.is_empty() {
            continue;
        }
        if s.kind == MatchKind::Possible {
            heard.push(if s.term.is_keep() {
                format!("- \"{source}\"")
            } else {
                format!("- \"{source}\" (translate as \"{}\")", s.term.target.trim())
            });
        } else if s.term.is_keep() {
            keep.push(format!("- \"{source}\""));
        } else {
            map.push(format!("- \"{source}\" -> \"{}\"", s.term.target.trim()));
        }
    }
    if keep.is_empty() && map.is_empty() && heard.is_empty() {
        return String::new();
    }
    let mut sections = vec![
        "\n\nThis workspace has a glossary. Apply it exactly, overriding your own default \
         translation choice whenever one of these terms appears:"
            .to_string(),
    ];
    if !keep.is_empty() {
        sections.push(format!(
            "Keep these terms exactly as written in the source - do not translate them:\n{}",
            keep.join("\n")
        ));
    }
    if !map.is_empty() {
        sections.push(format!(
            "Use these exact translations instead of a generic one:\n{}",
            map.join("\n")
        ));
    }
    if !heard.is_empty() {
        sections.push(format!(
            "None of these next terms appear literally, but part of the utterance SOUNDS like \
             one of them, which usually means speech recognition mangled it. Read it as the \
             term ONLY if that makes the sentence coherent. If reading it that way would not \
             make sense here, translate what was actually said and change nothing:\n{}",
            heard.join("\n")
        ));
    }
    sections.join("\n\n")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn t(s: &str, g: &str) -> GlossaryTerm {
        GlossaryTerm::new(s, g)
    }

    #[test]
    fn only_terms_present_in_source_are_selected() {
        let terms = vec![
            t("WarpTalk", ""),
            t("staging", "moi truong staging"),
            t("Kubernetes", ""),
        ];
        let sel = select_terms(
            "Deploy it to staging, then tell WarpTalk users.",
            &terms,
            12,
        );
        let names: Vec<_> = sel.iter().map(|s| s.term.source.as_str()).collect();
        assert_eq!(names, ["WarpTalk", "staging"]);
        assert!(sel.iter().all(|s| s.kind == MatchKind::Exact));
        assert!(select_terms("Hello world", &terms, 12).is_empty());
        assert_eq!(render_block(&[]), "");
    }

    #[test]
    fn word_boundaries_and_hyphen_equivalence() {
        let terms = vec![t("UI", ""), t("marketing plan", "")];
        assert!(select_terms("We will build it", &terms, 12).is_empty());
        let sel = select_terms("Review the marketing-plan and the UI.", &terms, 12);
        assert_eq!(sel.len(), 2);
    }

    #[test]
    fn possible_match_for_misheard_term_and_gate() {
        let terms = vec![t("Kubernetes", "")];
        // Diacritic-insensitive containment and a near miss both count as "sounds like".
        let sel = select_terms("chúng ta chạy kubernetis trên cụm", &terms, 12);
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].kind, MatchKind::Possible);
        assert!(has_suspected_mishearing(&sel));
        assert!(render_block(&sel).contains("SOUNDS like"));
        let exact = select_terms("we use Kubernetes", &terms, 12);
        assert!(!has_suspected_mishearing(&exact));
        // Short terms are never guessed at.
        assert!(select_terms("hom nay troi dep qua", &[t("Codex", "")], 12).is_empty());
    }

    #[test]
    fn limit_keeps_exact_before_possible() {
        let terms = vec![t("Kubernetes", ""), t("alpha", ""), t("beta", "")];
        let sel = select_terms("alpha and beta on kubernetis", &terms, 2);
        assert_eq!(sel.len(), 2);
        assert!(sel.iter().all(|s| s.kind == MatchKind::Exact));
    }

    #[test]
    fn render_distinguishes_keep_and_map() {
        let sel = select_terms(
            "sprint and staging",
            &[t("sprint", "sprint"), t("staging", "thu nghiem")],
            12,
        );
        let b = render_block(&sel);
        assert!(b.contains("do not translate them:\n- \"sprint\""));
        assert!(b.contains("\"staging\" -> \"thu nghiem\""));
    }

    #[tokio::test]
    async fn workspace_overrides_global() {
        let g = InMemoryGlossary::new();
        g.set_global(vec![t("sprint", "chay nuoc rut"), t("deploy", "")]);
        g.set_workspace("ws1", vec![t("Sprint", "sprint")]);
        g.bind_room("room1", "ws1");
        let terms = g.terms("room1").await;
        let sprint: Vec<_> = terms
            .iter()
            .filter(|x| x.source.eq_ignore_ascii_case("sprint"))
            .collect();
        assert_eq!(sprint.len(), 1);
        assert_eq!(sprint[0].target, "sprint");
        assert!(terms.iter().any(|x| x.source == "deploy"));
        // A room with no workspace sees the global terms only.
        assert_eq!(g.terms("other").await.len(), 2);
    }

    #[test]
    fn ratio_matches_difflib_on_known_pair() {
        let a: Vec<char> = "abcd".chars().collect();
        let b: Vec<char> = "bcde".chars().collect();
        assert!((ratio(&a, &b) - 0.75).abs() < 1e-6);
    }
}
