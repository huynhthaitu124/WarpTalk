//! LiveKit media names shared by the pipeline (producer) and every client (web, macOS).
//! Single source of truth: clients mirror these literals (`dub-{lang}`, `captions`, `chat`,
//! `reactions`); the pipeline and the tests build every name through the functions below.

/// Prefix of the published dub audio tracks (`dub-{targetLang}`).
pub const DUB_TRACK_PREFIX: &str = "dub-";
/// Data-channel topic of caption messages (pipeline -> clients).
pub const CAPTIONS_TOPIC: &str = "captions";
/// Data-channel topic of chat copies (client <-> clients).
pub const CHAT_TOPIC: &str = "chat";
/// Data-channel topic of reactions (client <-> clients).
pub const REACTIONS_TOPIC: &str = "reactions";

/// Track name of the dub for `lang`: `dub-{lang}`.
pub fn dub_track_name(lang: &str) -> String {
    format!("{DUB_TRACK_PREFIX}{lang}")
}

/// The target language of a dub track name, `None` for any other track (originals).
pub fn dub_track_lang(track_name: &str) -> Option<&str> {
    track_name
        .strip_prefix(DUB_TRACK_PREFIX)
        .filter(|l| !l.is_empty())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(dub_track_name("vi"), "dub-vi");
        assert_eq!(dub_track_lang("dub-vi"), Some("vi"));
        assert_eq!(dub_track_lang(&dub_track_name("pt-BR")), Some("pt-BR"));
        assert_eq!(dub_track_lang("dub-"), None);
        assert_eq!(dub_track_lang("ai-interpreter-vi"), None);
        assert_eq!(dub_track_lang("microphone"), None);
        assert_eq!(CAPTIONS_TOPIC, "captions");
        assert_eq!(CHAT_TOPIC, "chat");
        assert_eq!(REACTIONS_TOPIC, "reactions");
    }
}
