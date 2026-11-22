#![allow(clippy::unwrap_used)]
//! SimulatedTransport -> RoomWorker -> FakeStt -> SttOnlyStage round trip (no Redis).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::watch;
use uuid::Uuid;
use warptalk_contracts::events::{AudioRoute, AudioRoutesSnapshot, RouteStatus};
use warptalk_kernel::{ParticipantId, RoomId};
use warptalk_pipeline::downstream::CAPTION_TOPIC;
use warptalk_pipeline::{
    MemoryResultSink, RoomWorker, RoomWorkerDeps, RoutesView, SimParticipant, SimulatedTransport,
    SttOnlyStage,
};
use warptalk_pipeline_runtime::PipelineConfig;
use warptalk_pipeline_runtime::testing::{FakeStt, energy_vad_factory};

fn view(speakers: &[ParticipantId], active: bool) -> RoutesView {
    RoutesView::from_snapshot(&AudioRoutesSnapshot {
        generated_at: Utc::now(),
        room_status: "Active".into(),
        translation_active: active,
        room_languages: vec!["en".into(), "vi".into()],
        routes: speakers
            .iter()
            .map(|s| AudioRoute {
                route_id: Uuid::now_v7(),
                source_participant_id: *s,
                target_participant_id: ParticipantId::new(),
                source_user_id: None,
                target_user_id: None,
                source_language: "en".into(),
                target_language: "vi".into(),
                voice_clone_enabled: false,
                status: RouteStatus::Broadcasting,
            })
            .collect(),
    })
}

#[tokio::test]
async fn simulated_speech_becomes_captions_and_stt_results() {
    let room = RoomId::new();
    let pid = ParticipantId::new();
    let transport = Arc::new(SimulatedTransport::with_participants(vec![
        SimParticipant::synthetic(pid.to_string(), 1500, 1500).utterances(1),
    ]));
    let results = Arc::new(MemoryResultSink::default());
    let (_tx, cfg) = watch::channel(PipelineConfig::default());
    let worker = RoomWorker::start(RoomWorkerDeps {
        room,
        transport: transport.clone(),
        stt: Arc::new(FakeStt::default()),
        vad_factory: energy_vad_factory(),
        downstream: Arc::new(SttOnlyStage),
        results: results.clone(),
        config: cfg,
        flash_override: None,
        identity: "agent".into(),
        initial_routes: view(&[pid], true),
    })
    .await
    .unwrap();
    wait(|| worker.speakers() == vec![pid.to_string()]).await;
    tokio::time::sleep(Duration::from_millis(3500)).await;
    worker.stop().await;
    assert!(worker.is_finished());

    let r = results.results();
    assert!(r.len() >= 2, "early + final expected, got {}", r.len());
    assert!(
        r.iter()
            .all(|x| x.meeting_id == room && x.speaker_id == pid.to_string())
    );
    assert!(r.iter().any(|x| x.is_final_chunk));
    let cap = transport.capture(room).unwrap();
    assert_eq!(cap.identity(), "agent");
    assert_eq!(cap.json_on(CAPTION_TOPIC).len(), r.len());
    assert!(cap.is_closed());
}

#[tokio::test]
async fn routes_change_adds_and_removes_speakers() {
    let room = RoomId::new();
    let (a, b) = (ParticipantId::new(), ParticipantId::new());
    let transport = Arc::new(SimulatedTransport::with_participants(vec![
        SimParticipant::synthetic(a.to_string(), 1000, 1000),
        SimParticipant::synthetic(b.to_string(), 1000, 1000),
    ]));
    let (_tx, cfg) = watch::channel(PipelineConfig::default());
    let worker = RoomWorker::start(RoomWorkerDeps {
        room,
        transport,
        stt: Arc::new(FakeStt::default()),
        vad_factory: energy_vad_factory(),
        downstream: Arc::new(SttOnlyStage),
        results: Arc::new(MemoryResultSink::default()),
        config: cfg,
        flash_override: None,
        identity: "agent".into(),
        initial_routes: view(&[a], true),
    })
    .await
    .unwrap();
    wait(|| worker.speakers() == vec![a.to_string()]).await;
    worker.set_routes(view(&[a, b], true));
    wait(|| worker.speakers().len() == 2).await;
    worker.set_routes(view(&[b], true));
    wait(|| worker.speakers() == vec![b.to_string()]).await;
    worker.set_routes(view(&[b], false));
    wait(|| worker.speakers().is_empty()).await;
    worker.stop().await;
}

async fn wait(cond: impl Fn() -> bool) {
    for _ in 0..100 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("condition not reached");
}

/// The meeting module mints LiveKit tokens with identity = user id while the routes are keyed by
/// participant id: audio must still reach the speaker of its route.
#[tokio::test]
async fn audio_by_user_identity_reaches_speaker_keyed_by_participant() {
    let room = RoomId::new();
    let (pid, uid) = (ParticipantId::new(), warptalk_kernel::UserId::new());
    let transport = Arc::new(SimulatedTransport::with_participants(vec![
        SimParticipant::synthetic(uid.to_string(), 1500, 1500).utterances(1),
    ]));
    let results = Arc::new(MemoryResultSink::default());
    let (_tx, cfg) = watch::channel(PipelineConfig::default());
    // the snapshot names the user behind the participant
    let snap = AudioRoutesSnapshot {
        generated_at: Utc::now(),
        room_status: "Active".into(),
        translation_active: true,
        room_languages: vec!["en".into(), "vi".into()],
        routes: vec![AudioRoute {
            route_id: Uuid::now_v7(),
            source_participant_id: pid,
            target_participant_id: ParticipantId::new(),
            source_user_id: Some(uid),
            target_user_id: None,
            source_language: "en".into(),
            target_language: "vi".into(),
            voice_clone_enabled: false,
            status: RouteStatus::Broadcasting,
        }],
    };
    let v = RoutesView::from_snapshot(&snap);
    let worker = RoomWorker::start(RoomWorkerDeps {
        room,
        transport,
        stt: Arc::new(FakeStt::default()),
        vad_factory: energy_vad_factory(),
        downstream: Arc::new(SttOnlyStage),
        results: results.clone(),
        config: cfg,
        flash_override: None,
        identity: "agent".into(),
        initial_routes: v,
    })
    .await
    .unwrap();
    wait(|| worker.speakers() == vec![pid.to_string()]).await;
    tokio::time::sleep(Duration::from_millis(3500)).await;
    worker.stop().await;
    let r = results.results();
    assert!(
        !r.is_empty(),
        "no transcripts: audio of identity {uid} was dropped"
    );
    assert!(r.iter().all(|x| x.speaker_id == pid.to_string()));
}
