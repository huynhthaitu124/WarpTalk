#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use redis::aio::ConnectionManager;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use warptalk_bus::{HandlerError, StreamHandler, StreamMessage, xadd_json};
use warptalk_config::JwtConfig;
use warptalk_contracts::events::{SttResult, TranslateResult, TtsResult};
use warptalk_contracts::{keys, streams};
use warptalk_kernel::{AppResult, RoomId, SystemClock, UserId, WorkspaceId};
use warptalk_transcript::application::ports::{
    AlwaysSaveRoomPolicy, InMemoryReadAccess, InMemoryRooms, RoomPolicy, StaticAiPolicy,
};
use warptalk_transcript::infrastructure::consumers::{
    SttResultsHandler, TranslateResultsHandler, TtsResultsHandler,
};
use warptalk_transcript::infrastructure::redis_adapters::paused_flag_key;
use warptalk_transcript::{MIGRATIONS, TranscriptDeps, TranscriptModule};
use warptalk_web::JwtKeys;

struct NeverSave;

#[async_trait]
impl RoomPolicy for NeverSave {
    async fn save_transcript(&self, _: RoomId) -> AppResult<bool> {
        Ok(false)
    }
}

struct Ctx {
    pool: PgPool,
    redis: ConnectionManager,
    router: Router,
    keys: JwtKeys,
    access: Arc<InMemoryReadAccess>,
    stt: SttResultsHandler,
    tr: TranslateResultsHandler,
    tts: TtsResultsHandler,
    ws: WorkspaceId,
    room: RoomId,
    host: UserId,
}

struct Opts {
    retrieval: bool,
    save: bool,
}

const DEFAULT: Opts = Opts {
    retrieval: false,
    save: true,
};

async fn ctx(o: Opts) -> Option<Ctx> {
    let (Ok(db), Ok(redis_url)) = (std::env::var("DATABASE_URL"), std::env::var("REDIS_URL"))
    else {
        println!("DATABASE_URL / REDIS_URL unset; skipping");
        return None;
    };
    let pool = warptalk_db::connect(&db, 5).await.unwrap();
    warptalk_db::run_migrations(&pool, &[MIGRATIONS])
        .await
        .unwrap();
    let redis = warptalk_bus::connect(&redis_url).await.unwrap();
    let keys = JwtKeys::from_config(&JwtConfig {
        secret: "test-secret-test-secret-test-secret-1234".into(),
        issuer: "test".into(),
        audience: "test".into(),
        access_ttl_seconds: 900,
        refresh_ttl_seconds: 3600,
    });
    let rooms = Arc::new(InMemoryRooms::default());
    let access = Arc::new(InMemoryReadAccess::default());
    let room_policy: Arc<dyn RoomPolicy> = if o.save {
        Arc::new(AlwaysSaveRoomPolicy)
    } else {
        Arc::new(NeverSave)
    };
    let module = TranscriptModule::new(TranscriptDeps {
        pool: pool.clone(),
        redis: redis.clone(),
        jwt: keys.clone(),
        clock: Arc::new(SystemClock),
        rooms: rooms.clone(),
        room_policy,
        workspace_ai: Arc::new(StaticAiPolicy {
            retrieval: o.retrieval,
            external_llm: true,
        }),
        host: rooms.clone(),
        read_access: access.clone(),
    });
    let ingest = module.ingest();
    let (ws, room, host) = (WorkspaceId::new(), RoomId::new(), UserId::new());
    rooms.set(room, ws, host);
    Some(Ctx {
        router: module.router(),
        pool,
        redis,
        keys,
        access,
        stt: SttResultsHandler::new(ingest.clone()),
        tr: TranslateResultsHandler::new(ingest.clone()),
        tts: TtsResultsHandler::new(ingest),
        ws,
        room,
        host,
    })
}

fn stt(room: RoomId, seg: Uuid, text: &str, is_final: bool) -> SttResult {
    SttResult {
        segment_id: seg.to_string(),
        meeting_id: room,
        speaker_id: Uuid::now_v7().to_string(),
        text: text.into(),
        language: "vi".into(),
        start_ms: 1000,
        end_ms: 2500,
        anchor_ms: None,
        confidence: Some(0.93),
        chunk_index: None,
        is_final_chunk: is_final,
        timestamp_ms: None,
        clean_flags: None,
        far_speaker_name: None,
        far_speaker_source: None,
        far_speaker_confidence: None,
    }
}

fn translation(room: RoomId, seg: &str, lang: &str, text: &str) -> TranslateResult {
    TranslateResult {
        segment_id: seg.into(),
        target_lang: lang.into(),
        translated_text: text.into(),
        translator_model: Some("test-mt".into()),
        latency_ms: Some(120),
        is_retranslated: false,
        previous_translation_content_id: None,
        source_stt_confidence: Some(0.93),
        meeting_id: Some(room),
    }
}

impl Ctx {
    /// XADD the value to a fresh stream via `xadd_json`, read it back, and run the handler on it.
    async fn deliver<T: serde::Serialize>(
        &self,
        stream_base: &str,
        value: &T,
        handler: &dyn StreamHandler,
    ) -> Result<(), HandlerError> {
        let msg = self.push(stream_base, value).await;
        handler.handle(&msg).await
    }

    async fn push<T: serde::Serialize>(&self, stream_base: &str, value: &T) -> StreamMessage {
        let stream = format!("{stream_base}:test-{}", Uuid::new_v4());
        let mut conn = self.redis.clone();
        let id = xadd_json(&mut conn, &stream, value, 100).await.unwrap();
        let entries: Vec<(String, HashMap<String, String>)> = redis::cmd("XRANGE")
            .arg(&stream)
            .arg("-")
            .arg("+")
            .query_async(&mut conn)
            .await
            .unwrap();
        let _: i64 = redis::cmd("EXPIRE")
            .arg(&stream)
            .arg(60)
            .query_async(&mut conn)
            .await
            .unwrap();
        let (eid, fields) = entries.into_iter().next().unwrap();
        assert_eq!(eid, id);
        StreamMessage {
            stream,
            id: eid,
            fields,
        }
    }

    async fn count(&self, sql: &str, id: Uuid) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn transcript_id(&self) -> Uuid {
        sqlx::query_scalar(
            "SELECT id FROM transcript.transcripts WHERE translation_room_id = $1 AND is_current",
        )
        .bind(self.room.as_uuid())
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        user: UserId,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let token = self
            .keys
            .issue_access(user, Uuid::new_v4(), false, Utc::now())
            .unwrap()
            .0;
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"));
        let body = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = self
            .router
            .clone()
            .oneshot(b.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn embedding_requests(&self) -> Vec<HashMap<String, String>> {
        let mut conn = self.redis.clone();
        let entries: Vec<(String, HashMap<String, String>)> = redis::cmd("XREVRANGE")
            .arg(streams::EMBEDDING_INDEX_REQUESTS)
            .arg("+")
            .arg("-")
            .arg("COUNT")
            .arg(500)
            .query_async(&mut conn)
            .await
            .unwrap();
        let ws = self.ws.to_string();
        entries
            .into_iter()
            .map(|(_, f)| f)
            .filter(|f| f.get("workspace_id") == Some(&ws))
            .collect()
    }
}

#[tokio::test]
async fn ingest_flow_is_idempotent() {
    let Some(c) = ctx(DEFAULT).await else { return };
    let seg = Uuid::now_v7();
    let r = stt(c.room, seg, "xin chao moi nguoi", false);

    c.deliver(streams::STT_RESULTS, &r, &c.stt).await.unwrap();
    c.deliver(streams::STT_RESULTS, &r, &c.stt).await.unwrap(); // redelivery
    let tid = c.transcript_id().await;
    let (total, last, status): (i32, i32, String) = sqlx::query_as(
        "SELECT total_segments, last_sequence_order, status FROM transcript.transcripts WHERE id = $1",
    )
    .bind(tid)
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!((total, last, status.as_str()), (1, 1, "RECORDING"));
    assert_eq!(
        c.count(
            "SELECT count(*) FROM transcript.transcript_segments WHERE id = $1",
            seg
        )
        .await,
        1
    );

    // Second segment advances the sequence.
    let seg2 = Uuid::now_v7();
    c.deliver(
        streams::STT_RESULTS,
        &stt(c.room, seg2, "hai", false),
        &c.stt,
    )
    .await
    .unwrap();
    let order: i32 = sqlx::query_scalar(
        "SELECT sequence_order FROM transcript.transcript_segments WHERE id = $1",
    )
    .bind(seg2)
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(order, 2);

    // Translation, with the worker's "-c{idx}" chunk suffix; twice.
    let t = translation(c.room, &format!("{seg}-c0"), "en", "hello everyone");
    c.deliver(streams::TRANSLATE_RESULTS, &t, &c.tr)
        .await
        .unwrap();
    c.deliver(streams::TRANSLATE_RESULTS, &t, &c.tr)
        .await
        .unwrap();
    assert_eq!(
        c.count(
            "SELECT count(*) FROM transcript.segment_translation_links WHERE segment_id = $1 AND is_current",
            seg
        )
        .await,
        1
    );
    // A different translation replaces the current link; the old link stays as history.
    let t2 = translation(c.room, &seg.to_string(), "en", "hi all");
    c.deliver(streams::TRANSLATE_RESULTS, &t2, &c.tr)
        .await
        .unwrap();
    assert_eq!(
        c.count(
            "SELECT count(*) FROM transcript.segment_translation_links WHERE segment_id = $1",
            seg
        )
        .await,
        2
    );

    // TTS metadata, twice.
    let tts = TtsResult {
        segment_id: seg.to_string(),
        target_lang: "en".into(),
        voice_type: Some("clone".into()),
        provider_voice_id: Some("voice-1".into()),
        clone_provider: Some("cartesia".into()),
        anchor_provider: None,
        duration_ms: Some(900),
        meeting_id: Some(c.room),
    };
    c.deliver(streams::TTS_RESULTS, &tts, &c.tts).await.unwrap();
    c.deliver(streams::TTS_RESULTS, &tts, &c.tts).await.unwrap();
    let (n, vt): (i64, String) = sqlx::query_as(
        "SELECT count(*), max(voice_type) FROM transcript.audio_dubbings d \
         JOIN transcript.segment_translation_links l ON l.translation_content_id = d.translation_content_id \
         WHERE l.segment_id = $1 AND l.is_current",
    )
    .bind(seg)
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!((n, vt.as_str()), (1, "cloned"));

    // REST read: ordered segments, translation for ?lang=en only on the translated one.
    let user = UserId::new();
    c.access.allow(user, c.room);
    let (st, body) = c
        .call(
            Method::GET,
            &format!("/transcripts/{}?lang=en", c.room),
            user,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let segs = body["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0]["id"], seg.to_string());
    assert_eq!(segs[0]["translation"]["text"], "hi all");
    assert!(segs[1]["translation"].is_null());
    // Strangers are refused; the host is always admitted.
    let (st, _) = c
        .call(
            Method::GET,
            &format!("/transcripts/{}", c.room),
            UserId::new(),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, body) = c
        .call(
            Method::GET,
            &format!("/transcripts/{}", c.room),
            c.host,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(body["segments"][0]["translation"].is_null());
}

#[tokio::test]
async fn markers_empty_and_ephemeral_rooms_are_acked_not_stored() {
    let Some(c) = ctx(DEFAULT).await else { return };
    let mut marker = SttResult::meeting_end(Uuid::now_v7().to_string(), c.room, 5);
    marker.speaker_id = "system".into();
    c.deliver(streams::STT_RESULTS, &marker, &c.stt)
        .await
        .unwrap();
    // Marker text with a normal language is still a control marker.
    let mut m2 = stt(c.room, Uuid::now_v7(), "__MEETING_END__", false);
    m2.language = "en".into();
    c.deliver(streams::STT_RESULTS, &m2, &c.stt).await.unwrap();
    // Trailing empty final marker.
    c.deliver(
        streams::STT_RESULTS,
        &stt(c.room, Uuid::now_v7(), "  ", true),
        &c.stt,
    )
    .await
    .unwrap();
    assert_eq!(
        c.count(
            "SELECT count(*) FROM transcript.transcripts WHERE translation_room_id = $1",
            c.room.as_uuid()
        )
        .await,
        0
    );

    // SaveTranscript=false room: acked, nothing persisted (also for translations).
    let Some(e) = ctx(Opts {
        retrieval: false,
        save: false,
    })
    .await
    else {
        return;
    };
    let seg = Uuid::now_v7();
    e.deliver(
        streams::STT_RESULTS,
        &stt(e.room, seg, "private", false),
        &e.stt,
    )
    .await
    .unwrap();
    e.deliver(
        streams::TRANSLATE_RESULTS,
        &translation(e.room, &seg.to_string(), "en", "private"),
        &e.tr,
    )
    .await
    .unwrap();
    assert_eq!(
        e.count(
            "SELECT count(*) FROM transcript.transcripts WHERE translation_room_id = $1",
            e.room.as_uuid()
        )
        .await,
        0
    );
}

#[tokio::test]
async fn paused_segment_is_skipped_and_its_translation_is_an_intentional_skip() {
    let Some(c) = ctx(DEFAULT).await else { return };
    let mut conn = c.redis.clone();
    let flag = paused_flag_key(c.room);
    let _: () = redis::cmd("SET")
        .arg(&flag)
        .arg(1)
        .arg("EX")
        .arg(60)
        .query_async(&mut conn)
        .await
        .unwrap();
    let seg = Uuid::now_v7();
    c.deliver(
        streams::STT_RESULTS,
        &stt(c.room, seg, "paused words", false),
        &c.stt,
    )
    .await
    .unwrap();
    assert_eq!(
        c.count(
            "SELECT count(*) FROM transcript.transcript_segments WHERE id = $1",
            seg
        )
        .await,
        0
    );
    let remembered: bool = redis::cmd("SISMEMBER")
        .arg(keys::transcript_paused_segments(c.room))
        .arg(seg.to_string())
        .query_async(&mut conn)
        .await
        .unwrap();
    assert!(remembered);
    let ttl: i64 = redis::cmd("TTL")
        .arg(keys::transcript_paused_segments(c.room))
        .query_async(&mut conn)
        .await
        .unwrap();
    assert!(ttl > 0 && ttl <= 1800);

    // Later translation / TTS for the skipped segment: acked, nothing stored.
    c.deliver(
        streams::TRANSLATE_RESULTS,
        &translation(c.room, &format!("{seg}-c0"), "en", "paused words"),
        &c.tr,
    )
    .await
    .unwrap();
    let tts = TtsResult {
        segment_id: seg.to_string(),
        target_lang: "en".into(),
        voice_type: None,
        provider_voice_id: None,
        clone_provider: None,
        anchor_provider: None,
        duration_ms: None,
        meeting_id: Some(c.room),
    };
    c.deliver(streams::TTS_RESULTS, &tts, &c.tts).await.unwrap();

    // A translation for an unknown (not skipped) segment is retried; garbage is fatal.
    let unknown = translation(c.room, &Uuid::now_v7().to_string(), "en", "late");
    let err = c
        .deliver(streams::TRANSLATE_RESULTS, &unknown, &c.tr)
        .await
        .unwrap_err();
    assert!(matches!(err, HandlerError::Retry(_)));
    let bad = translation(c.room, "not-a-uuid", "en", "x");
    let err = c
        .deliver(streams::TRANSLATE_RESULTS, &bad, &c.tr)
        .await
        .unwrap_err();
    assert!(matches!(err, HandlerError::Fatal(_)));
    let undecodable = c.push(streams::STT_RESULTS, &json!({ "nope": 1 })).await;
    assert!(matches!(
        c.stt.handle(&undecodable).await.unwrap_err(),
        HandlerError::Fatal(_)
    ));

    // After resume, segments persist again.
    let _: i64 = redis::cmd("DEL")
        .arg(&flag)
        .query_async(&mut conn)
        .await
        .unwrap();
    let seg2 = Uuid::now_v7();
    c.deliver(
        streams::STT_RESULTS,
        &stt(c.room, seg2, "after resume", false),
        &c.stt,
    )
    .await
    .unwrap();
    assert_eq!(
        c.count(
            "SELECT count(*) FROM transcript.transcript_segments WHERE id = $1",
            seg2
        )
        .await,
        1
    );
}

#[tokio::test]
async fn corrections_keep_history_and_finalize_locks() {
    let Some(c) = ctx(DEFAULT).await else { return };
    let seg = Uuid::now_v7();
    c.deliver(
        streams::STT_RESULTS,
        &stt(c.room, seg, "original text", false),
        &c.stt,
    )
    .await
    .unwrap();
    c.deliver(
        streams::TRANSLATE_RESULTS,
        &translation(c.room, &seg.to_string(), "en", "original"),
        &c.tr,
    )
    .await
    .unwrap();
    let tid = c.transcript_id().await;
    let user = UserId::new();
    c.access.allow(user, c.room);
    let uri = format!("/transcripts/{tid}/corrections");

    let (st, first) = c
        .call(
            Method::POST,
            &uri,
            user,
            Some(json!({ "segmentId": seg, "text": "first fix", "reason": "typo" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{first}");
    assert_eq!(first["originalText"], "original text");
    let (st, _) = c
        .call(
            Method::POST,
            &uri,
            user,
            Some(json!({ "segmentId": seg, "text": "second fix" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, list) = c
        .call(Method::GET, &format!("{uri}?segmentId={seg}"), user, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["correctedText"], "first fix");
    assert_eq!(list[0]["reason"], "typo");
    assert_eq!(list[1]["originalText"], "first fix");
    assert_eq!(list[1]["correctedText"], "second fix");

    let (text, corrected): (String, bool) = sqlx::query_as(
        "SELECT original_text, is_corrected FROM transcript.transcript_segments WHERE id = $1",
    )
    .bind(seg)
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!((text.as_str(), corrected), ("second fix", true));
    let stale: bool = sqlx::query_scalar(
        "SELECT is_stale FROM transcript.segment_translation_links WHERE segment_id = $1 AND is_current",
    )
    .bind(seg)
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert!(stale);

    // Validation and scoping.
    let (st, _) = c
        .call(
            Method::POST,
            &uri,
            user,
            Some(json!({ "segmentId": seg, "text": "  " })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = c
        .call(
            Method::POST,
            &uri,
            user,
            Some(json!({ "segmentId": Uuid::now_v7(), "text": "x" })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = c
        .call(
            Method::POST,
            &uri,
            UserId::new(),
            Some(json!({ "segmentId": seg, "text": "x" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Finalize: host only, idempotent, then locked.
    let fin = format!("/transcripts/{tid}/finalize");
    let (st, _) = c.call(Method::POST, &fin, user, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = c.call(Method::POST, &fin, c.host, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = c.call(Method::POST, &fin, c.host, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (status, active, at): (String, bool, Option<chrono::DateTime<Utc>>) = sqlx::query_as(
        "SELECT status, is_active, finalized_at FROM transcript.transcripts WHERE id = $1",
    )
    .bind(tid)
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!(status, "FINALIZED");
    assert!(!active && at.is_some());
    let (st, body) = c
        .call(
            Method::POST,
            &uri,
            user,
            Some(json!({ "segmentId": seg, "text": "too late" })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "TRANSCRIPT_LOCKED");
    // History is intact.
    let n: i64 = c
        .count(
            "SELECT count(*) FROM transcript.transcript_corrections WHERE segment_id = $1",
            seg,
        )
        .await;
    assert_eq!(n, 2);
}

#[tokio::test]
async fn embedding_request_only_when_policy_allows() {
    // Policy denies: nothing published.
    let Some(off) = ctx(DEFAULT).await else {
        return;
    };
    off.deliver(
        streams::STT_RESULTS,
        &stt(off.room, Uuid::now_v7(), "no index", true),
        &off.stt,
    )
    .await
    .unwrap();
    assert!(off.embedding_requests().await.is_empty());

    // Policy allows: one entry for the final segment, none for a non-final one.
    let Some(on) = ctx(Opts {
        retrieval: true,
        save: true,
    })
    .await
    else {
        return;
    };
    on.deliver(
        streams::STT_RESULTS,
        &stt(on.room, Uuid::now_v7(), "partial", false),
        &on.stt,
    )
    .await
    .unwrap();
    assert!(on.embedding_requests().await.is_empty());
    let seg = Uuid::now_v7();
    let r = stt(on.room, seg, "index me", true);
    on.deliver(streams::STT_RESULTS, &r, &on.stt).await.unwrap();
    on.deliver(streams::STT_RESULTS, &r, &on.stt).await.unwrap(); // redelivery: no duplicate
    let reqs = on.embedding_requests().await;
    assert_eq!(reqs.len(), 1);
    let f = &reqs[0];
    let tid = on.transcript_id().await.to_string();
    assert_eq!(f["collection_id"], format!("workspace_{}", on.ws));
    assert_eq!(f["source_type"], "transcript");
    assert_eq!(f["source_id"], tid);
    assert_eq!(f["external_llm_allowed"], "true");
    assert_eq!(f["ai_retrieval_allowed"], "true");
    assert_eq!(f["retention_state"], "active");
    assert_eq!(f["deletion_state"], "active");
    assert!(f["timestamp_ms"].parse::<i64>().unwrap() > 0);
    assert!(Uuid::parse_str(&f["job_id"]).is_ok());
    let chunks: Value = serde_json::from_str(&f["chunks_json"]).unwrap();
    assert_eq!(chunks[0]["id"], seg.to_string());
    assert!(chunks[0]["text"].as_str().unwrap().ends_with("] index me"));
    assert_eq!(chunks[0]["metadata"]["transcript_id"], tid);
    assert_eq!(chunks[0]["metadata"]["start_ms"], 1000);
}

#[tokio::test]
async fn head_chain_links_previous_transcript() {
    let Some(c) = ctx(DEFAULT).await else { return };
    c.deliver(
        streams::STT_RESULTS,
        &stt(c.room, Uuid::now_v7(), "one", false),
        &c.stt,
    )
    .await
    .unwrap();
    let first = c.transcript_id().await;
    // A new session: the old head is demoted out of band; ingestion creates a linked v2.
    sqlx::query("UPDATE transcript.transcripts SET is_current = false WHERE id = $1")
        .bind(first)
        .execute(&c.pool)
        .await
        .unwrap();
    c.deliver(
        streams::STT_RESULTS,
        &stt(c.room, Uuid::now_v7(), "two", false),
        &c.stt,
    )
    .await
    .unwrap();
    let (version, prev): (i32, Option<Uuid>) = sqlx::query_as(
        "SELECT version, previous_transcript_id FROM transcript.transcripts \
         WHERE translation_room_id = $1 AND is_current",
    )
    .bind(c.room.as_uuid())
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert_eq!((version, prev), (2, Some(first)));
}

#[tokio::test]
async fn malformed_input_is_invalid_request() {
    let Some(c) = ctx(Opts {
        retrieval: false,
        save: false,
    })
    .await
    else {
        return;
    };
    let token = c
        .keys
        .issue_access(UserId::new(), Uuid::new_v4(), false, Utc::now())
        .unwrap()
        .0;
    for (uri, body) in [
        ("/transcripts/not-a-uuid/corrections", "{}"),
        (
            &format!("/transcripts/{}/corrections", Uuid::new_v4()),
            "{bad",
        ),
    ] {
        let req = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = c.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(v["code"], "INVALID_REQUEST", "{uri} {v}");
    }
}

#[tokio::test]
async fn multi_sentence_translation_is_stored_in_full_and_in_order() {
    let Some(c) = ctx(DEFAULT).await else { return };
    let seg = Uuid::now_v7();
    c.deliver(
        streams::STT_RESULTS,
        &stt(c.room, seg, "Mot. Hai. Ba.", true),
        &c.stt,
    )
    .await
    .unwrap();
    // The wire id of the pipeline's per-sentence results: `{segment}-{lang}-c{n}`. The second
    // sentence is delivered first (redelivery and reordering must not matter).
    let id = |n: u32| format!("{seg}-en-c{n}");
    for (n, text) in [(1, "Two."), (0, "One."), (2, "Three."), (1, "Two.")] {
        c.deliver(
            streams::TRANSLATE_RESULTS,
            &translation(c.room, &id(n), "en", text),
            &c.tr,
        )
        .await
        .unwrap();
    }
    let current: Vec<String> = sqlx::query_scalar(
        "SELECT c.translated_text FROM transcript.segment_translation_links l \
         JOIN transcript.translation_contents c ON c.id = l.translation_content_id \
         WHERE l.segment_id = $1 AND l.is_current",
    )
    .bind(seg)
    .fetch_all(&c.pool)
    .await
    .unwrap();
    assert_eq!(
        current,
        vec!["One. Two. Three."],
        "one current link, all sentences"
    );

    // Dubbing metadata attaches to the combined translation.
    let tts = TtsResult {
        segment_id: id(2),
        target_lang: "en".into(),
        voice_type: Some("default".into()),
        provider_voice_id: Some("v".into()),
        clone_provider: None,
        anchor_provider: None,
        duration_ms: Some(500),
        meeting_id: Some(c.room),
    };
    c.deliver(streams::TTS_RESULTS, &tts, &c.tts).await.unwrap();

    let user = UserId::new();
    c.access.allow(user, c.room);
    let (st, body) = c
        .call(
            Method::GET,
            &format!("/transcripts/{}?lang=en", c.room),
            user,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        body["segments"][0]["translation"]["text"],
        "One. Two. Three."
    );

    // A whole-segment translation (no chunk suffix, e.g. a retranslation) replaces the pieces.
    c.deliver(
        streams::TRANSLATE_RESULTS,
        &translation(c.room, &seg.to_string(), "en", "One two three."),
        &c.tr,
    )
    .await
    .unwrap();
    let (_, body) = c
        .call(
            Method::GET,
            &format!("/transcripts/{}?lang=en", c.room),
            user,
            None,
        )
        .await;
    assert_eq!(body["segments"][0]["translation"]["text"], "One two three.");

    // Languages written without spaces are joined without one.
    let seg2 = Uuid::now_v7();
    c.deliver(
        streams::STT_RESULTS,
        &stt(c.room, seg2, "Mot. Hai.", true),
        &c.stt,
    )
    .await
    .unwrap();
    for (n, text) in [(0, "一。"), (1, "二。")] {
        c.deliver(
            streams::TRANSLATE_RESULTS,
            &translation(c.room, &format!("{seg2}-ja-c{n}"), "ja", text),
            &c.tr,
        )
        .await
        .unwrap();
    }
    let (_, body) = c
        .call(
            Method::GET,
            &format!("/transcripts/{}?lang=ja", c.room),
            user,
            None,
        )
        .await;
    assert_eq!(body["segments"][1]["translation"]["text"], "一。二。");
}

#[tokio::test]
async fn stt_offsets_confidence_and_names() {
    let Some(c) = ctx(DEFAULT).await else { return };
    let now_ms = Utc::now().timestamp_millis();
    let pool = c.pool.clone();
    let row = |seg: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (Option<i32>, Option<i32>, Option<f64>, String, bool)>(
                "SELECT start_time_ms, end_time_ms, confidence::float8, speaker_name, is_final \
             FROM transcript.transcript_segments WHERE id = $1",
            )
            .bind(seg)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    // Anchored (pipeline) offsets are stored as they are; the name replaces the participant id.
    let a = Uuid::now_v7();
    let mut r = stt(c.room, a, "hello", true);
    r.start_ms = 61_000;
    r.end_ms = 63_500;
    r.anchor_ms = Some(now_ms - 100_000);
    r.confidence = Some(0.4966);
    r.far_speaker_name = Some("Ann Lee".into());
    r.far_speaker_source = Some("participant".into());
    r.far_speaker_confidence = Some(1.0);
    c.deliver(streams::STT_RESULTS, &r, &c.stt).await.unwrap();
    let (s, e, conf, name, fin) = row(a).await;
    assert_eq!((s, e), (Some(61_000), Some(63_500)));
    assert!((conf.unwrap() - 0.4966).abs() < 1e-4);
    assert_eq!((name.as_str(), fin), ("Ann Lee", true));

    // Legacy epoch milliseconds (no anchor) are taken relative to the transcript start.
    let b = Uuid::now_v7();
    let mut r = stt(c.room, b, "epoch", true);
    r.start_ms = now_ms + 4_000;
    r.end_ms = now_ms + 6_000;
    c.deliver(streams::STT_RESULTS, &r, &c.stt).await.unwrap();
    let (s, e, _, name, _) = row(b).await;
    let (s, e) = (s.unwrap(), e.unwrap());
    assert!((3_000..=7_000).contains(&s) && e > s, "{s} {e}");
    assert_eq!(
        name, r.speaker_id,
        "no name known: the participant id is all there is"
    );

    // Values that cannot be an offset are logged and stored as NULL, never clamped.
    let d = Uuid::now_v7();
    let mut r = stt(c.room, d, "overflow", true);
    r.start_ms = i64::MAX;
    r.end_ms = -5;
    r.anchor_ms = Some(now_ms);
    r.confidence = Some(-0.7); // an average logprob is not a probability: unknown
    c.deliver(streams::STT_RESULTS, &r, &c.stt).await.unwrap();
    let (s, e, conf, _, _) = row(d).await;
    assert_eq!((s, e, conf), (None, None, None));
    let tid = c.transcript_id().await;
    let dur: i32 =
        sqlx::query_scalar("SELECT total_duration_ms FROM transcript.transcripts WHERE id = $1")
            .bind(tid)
            .fetch_one(&c.pool)
            .await
            .unwrap();
    assert!(dur >= 63_500 && dur < 100_000, "{dur}");
}
