//! Auto STT routing: direct -> server fallback, recovery, no double posting, plus the
//! language modes. Offline: fake direct / server STT.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::*;
use tokio::sync::mpsc;
use warptalk_copilot_core::{
    Channel, CopilotConfig, CopilotDeps, CopilotEngine, CopilotEvent, EngineExtras, FakeStt,
    MemorySink, SegmentRecord, StatusKind, SttRoute, SttRouting,
};
use warptalk_pipeline_runtime::{
    CommitRequest, SessionParams, SpeechToText, SttError, SttEvent, SttSegment, SttSession,
};

fn cfg() -> CopilotConfig {
    let mut c = CopilotConfig::default();
    c.me.queue_capacity_chunks = 100_000;
    c.others.queue_capacity_chunks = 100_000;
    c.stt.mode = SttRouting::Auto;
    c.stt.fallback.failure_threshold = 2;
    c.stt.fallback.retry_direct_after_ms = 300;
    c.stt.fallback.probe_timeout_ms = 1_000;
    c.engine.reconnect_backoff_ms = 20;
    c.engine.status_interval_ms = 20;
    c
}

struct Rig {
    e: CopilotEngine,
    rx: mpsc::Receiver<CopilotEvent>,
    direct: FakeStt,
    server: FakeStt,
    sink: Arc<MemorySink>,
    seen: Vec<CopilotEvent>,
}

fn rig(cfg: CopilotConfig, fail_direct_opens: u64) -> Rig {
    let direct = FakeStt::new(Duration::ZERO);
    direct.fail_next_opens(fail_direct_opens);
    let server = FakeStt::new(Duration::ZERO);
    let sink = MemorySink::new();
    let (e, rx) = CopilotEngine::with_extras(
        cfg,
        CopilotDeps {
            stt: Arc::new(direct.clone()),
            sink: sink.clone(),
            session_id: "s".into(),
        },
        EngineExtras {
            server_stt: Some(Arc::new(server.clone())),
            ..EngineExtras::default()
        },
    )
    .unwrap();
    Rig {
        e,
        rx,
        direct,
        server,
        sink,
        seen: Vec::new(),
    }
}

impl Rig {
    /// One utterance on `me` (speech then enough silence to endpoint), then wait for its segment.
    async fn utterance(&mut self) -> SegmentRecord {
        let before = segments(&self.seen).len();
        let audio = track(16_000, 0.3, &[600, -1500]);
        self.e.push_audio(Channel::Me, &audio, 16_000);
        let ev = collect_until(&mut self.rx, Duration::from_secs(5), |ev| {
            segments(ev).len() > 0
        })
        .await;
        self.seen.extend(ev);
        let all = segments(&self.seen);
        assert!(all.len() > before, "no segment arrived: {:#?}", self.seen);
        all.last().unwrap().clone()
    }

    fn statuses(&self, k: StatusKind) -> Vec<Option<String>> {
        self.seen
            .iter()
            .filter_map(|e| match e {
                CopilotEvent::Status { kind, detail, .. } if *kind == k => Some(detail.clone()),
                _ => None,
            })
            .collect()
    }
}

#[tokio::test]
async fn direct_failures_switch_to_server_then_recover_without_double_posting() {
    // Both channels open at start and share the failure budget: 3 = other(1) + me(2).
    let mut r = rig(cfg(), 3);
    // Open #1 failed at start (failure 1); wait out the backoff, the next attempt is #2.
    tokio::time::sleep(Duration::from_millis(60)).await;
    let s1 = r.utterance().await;
    assert_eq!(s1.text, "utterance 0 on me");
    assert_eq!(
        r.statuses(StatusKind::SttFallback).len(),
        1,
        "{:#?}",
        r.seen
    );
    let detail = r.statuses(StatusKind::SttFallback)[0].clone().unwrap();
    assert!(detail.starts_with("direct->server"), "{detail}");
    assert_eq!(r.server.commits(), 1);
    assert_eq!(r.direct.commits(), 0);
    let st = r.e.stats();
    assert_eq!(st.me.stt_route, SttRoute::Server);
    assert_eq!(st.me.stt_fallbacks, 1);
    assert_eq!(st.others.stt_route, SttRoute::Direct, "per channel");
    // Server-routed segments are shown but NOT posted: the server ingests them.
    assert!(
        r.sink.segments().is_empty(),
        "double posting: {:?}",
        r.sink.segments()
    );

    // After the retry period an utterance boundary probes direct and recovers.
    tokio::time::sleep(Duration::from_millis(350)).await;
    let _s2 = r.utterance().await; // still served by the server; the probe runs after it
    for _ in 0..50 {
        if r.e.stats().me.stt_route == SttRoute::Direct {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(r.e.stats().me.stt_route, SttRoute::Direct);
    r.seen
        .extend(collect_until(&mut r.rx, Duration::from_millis(100), |_| false).await);
    assert_eq!(r.statuses(StatusKind::SttRecovered).len(), 1);
    assert!(r.sink.segments().is_empty());

    // Back on direct: posted again, exactly once.
    let s3 = r.utterance().await;
    assert_eq!(s3.text, "utterance 0 on me");
    r.e.stop().await;
    let posted = r.sink.segments();
    assert_eq!(posted.len(), 1, "{posted:#?}");
    assert_eq!(posted[0].client_segment_id, s3.client_segment_id);
    assert_eq!(r.direct.commits(), 1);
    assert_eq!(r.server.commits(), 2);
}

#[tokio::test]
async fn direct_mode_never_falls_back() {
    let mut c = cfg();
    c.stt.mode = SttRouting::Direct;
    c.stt.fallback.failure_threshold = 1;
    let mut r = rig(c, 1_000);
    tokio::time::sleep(Duration::from_millis(60)).await;
    r.e.push_audio(Channel::Me, &track(16_000, 0.3, &[600, -1500]), 16_000);
    r.seen
        .extend(collect_until(&mut r.rx, Duration::from_millis(400), |_| false).await);
    assert!(r.statuses(StatusKind::SttFallback).is_empty());
    assert_eq!(r.server.sessions_opened(), 0);
    assert_eq!(r.e.stats().me.stt_route, SttRoute::Direct);
}

#[tokio::test]
async fn failures_outside_the_window_do_not_count() {
    let mut c = cfg();
    c.stt.fallback.window_ms = 30; // failures 60 ms apart never reach the threshold
    let mut r = rig(c, 3);
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(60)).await;
        r.e.push_audio(Channel::Me, &track(16_000, 0.3, &[100, -100]), 16_000);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    r.seen
        .extend(collect_until(&mut r.rx, Duration::from_millis(100), |_| false).await);
    assert!(r.statuses(StatusKind::SttFallback).is_empty());
    assert_eq!(r.server.sessions_opened(), 0);
}

#[tokio::test]
async fn server_mode_uses_only_the_server() {
    let mut c = cfg();
    c.stt.mode = SttRouting::Server;
    let mut r = rig(c, 0);
    let s = r.utterance().await;
    assert_eq!(s.text, "utterance 0 on me");
    assert_eq!(r.direct.sessions_opened(), 0);
    assert_eq!(r.e.stats().me.stt_route, SttRoute::Server);
    assert!(r.sink.segments().is_empty());
    assert!(r.statuses(StatusKind::SttFallback).is_empty());
    let c = r.statuses(StatusKind::SttConnected);
    assert_eq!(c[0].as_deref(), Some("server"));
}

#[tokio::test]
async fn server_mode_without_a_server_backend_is_a_config_error() {
    let mut c = cfg();
    c.stt.mode = SttRouting::Server;
    let res = CopilotEngine::new(
        c,
        CopilotDeps {
            stt: Arc::new(FakeStt::new(Duration::ZERO)),
            sink: MemorySink::new(),
            session_id: "s".into(),
        },
    );
    assert!(res.is_err());
}

// ------------------------------------------------------------------ language modes

/// Answers every commit with the next scripted text, labelled with the session's hint.
struct Scripted {
    texts: Mutex<Vec<String>>,
    hints: Arc<Mutex<Vec<Option<String>>>>,
}

struct ScriptedSession {
    texts: Vec<String>,
    hint: Option<String>,
    sink: mpsc::Sender<SttEvent>,
    n: usize,
}

#[async_trait]
impl SpeechToText for Scripted {
    async fn open_session(
        &self,
        params: SessionParams,
        sink: mpsc::Sender<SttEvent>,
    ) -> Result<Box<dyn SttSession>, SttError> {
        self.hints.lock().unwrap().push(params.lang_hint.clone());
        Ok(Box::new(ScriptedSession {
            texts: self.texts.lock().unwrap().clone(),
            hint: params.lang_hint,
            sink,
            n: 0,
        }))
    }
}

#[async_trait]
impl SttSession for ScriptedSession {
    async fn feed(&mut self, _: &[f32]) -> Result<(), SttError> {
        Ok(())
    }
    async fn commit(&mut self, _: CommitRequest) -> Result<(), SttError> {
        let text = self.texts[self.n % self.texts.len()].clone();
        self.n += 1;
        // Like the real provider with a hint: the hint is echoed as the language.
        let lang = self.hint.clone().unwrap_or_else(|| "auto".into());
        let _ = self
            .sink
            .send(SttEvent::Segment(SttSegment::new(text, lang, None, false)))
            .await;
        let _ = self.sink.send(SttEvent::UtteranceDone { ok: true }).await;
        Ok(())
    }
    async fn discard(&mut self) -> Result<(), SttError> {
        Ok(())
    }
    async fn close(self: Box<Self>) {}
}

const VI: &str = "Chúng ta cần hoàn thành báo cáo trước thứ sáu";
const VI_WITH_EN_TERMS: &str = "Chúng ta cần deploy lên production vào chiều nay";
const EN: &str = "Let's review the roadmap and then we can ship it";

async fn languages(json: &str) -> (Vec<String>, Vec<Option<String>>) {
    let c = CopilotConfig::from_json(json).unwrap();
    let hints = Arc::new(Mutex::new(Vec::new()));
    let stt = Scripted {
        texts: Mutex::new(vec![VI.into(), VI_WITH_EN_TERMS.into(), EN.into()]),
        hints: hints.clone(),
    };
    let sink = MemorySink::new();
    let (e, mut rx) = CopilotEngine::new(
        c,
        CopilotDeps {
            stt: Arc::new(stt),
            sink: sink.clone(),
            session_id: "s".into(),
        },
    )
    .unwrap();
    for _ in 0..3 {
        e.push_audio(Channel::Me, &track(16_000, 0.3, &[600, -1500]), 16_000);
    }
    let ev = collect_until(&mut rx, Duration::from_secs(5), |ev| {
        segments(ev).len() >= 3
    })
    .await;
    e.stop().await;
    let langs: Vec<String> = segments(&ev).into_iter().map(|s| s.language).collect();
    // Stored language is what the sink would post.
    let posted: Vec<String> = sink.segments().into_iter().map(|s| s.language).collect();
    assert_eq!(langs, posted);
    let h = hints.lock().unwrap().clone();
    (langs, h)
}

#[tokio::test]
async fn auto_hints_the_primary_and_corrects_on_strong_evidence() {
    let (langs, hints) = languages(
        r#"{"stt":{"backend":"fake"},"language":{"mode":"auto","languages":["vi","en"]},
            "me":{"endpointer":{}}}"#,
    )
    .await;
    assert_eq!(hints[0].as_deref(), Some("vi"));
    // Vietnamese stays vi (also with English terms); the English sentence becomes en.
    assert_eq!(langs, vec!["vi", "vi", "en"]);
}

#[tokio::test]
async fn multi_sends_no_hint_and_detects_every_segment() {
    let (langs, hints) =
        languages(r#"{"language":{"mode":"multi","languages":["vi","en"]}}"#).await;
    assert_eq!(hints[0], None);
    assert_eq!(langs, vec!["vi", "vi", "en"]);
}

#[tokio::test]
async fn fixed_keeps_the_single_hint_and_reported_language() {
    let (langs, hints) = languages(
        r#"{"language":{"mode":"fixed","languages":["vi","en"]},"me":{"language_hint":"vi"}}"#,
    )
    .await;
    assert_eq!(hints[0].as_deref(), Some("vi"));
    assert_eq!(langs, vec!["vi", "vi", "vi"], "reported language is kept");
}

#[test]
fn language_config_is_validated_and_defaults_to_auto() {
    let c = CopilotConfig::default();
    assert_eq!(c.language.hint(None), None);
    assert_eq!(c.stt.mode, SttRouting::Auto);
    assert!(CopilotConfig::from_json(r#"{"language":{"languages":["bad lang"]}}"#).is_err());
    assert!(CopilotConfig::from_json(r#"{"stt":{"mode":"sometimes"}}"#).is_err());
    assert!(CopilotConfig::from_json(r#"{"stt":{"fallback":{"failure_threshold":0}}}"#).is_err());
    let c = CopilotConfig::from_json(r#"{"stt":{"mode":"server"}}"#).unwrap();
    assert_eq!(c.stt.mode, SttRouting::Server);
}
