//! Exercises the facade the way the UniFFI layer does (listener thread, push, stop), offline:
//! fake STT, sink pointed at a closed port. Runs without the `ffi` feature too (the listener
//! trait is plain Rust); with it, the scaffolding is compiled into the same crate.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::track;
use warptalk_copilot_core::{
    Channel, CopilotError, CopilotEvent, CopilotEventListener, CopilotHandle, PushStatus,
    StartParams,
};

#[derive(Default)]
struct Collect(Mutex<Vec<CopilotEvent>>);

impl CopilotEventListener for Collect {
    fn on_event(&self, event: CopilotEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[test]
fn listener_receives_segments_and_stop_is_clean() {
    let h = CopilotHandle::new(r#"{"stt":{"backend":"fake"}}"#.into()).unwrap();
    assert!(matches!(h.stats(), Err(CopilotError::NotStarted)));
    assert!(matches!(h.stop(), Err(CopilotError::NotStarted)));
    let l = Arc::new(Collect::default());
    h.set_listener(l.clone());
    h.set_listener(l.clone()); // replacing must not spawn a second pump
    h.start(StartParams {
        base_url: "http://127.0.0.1:1".into(),
        session_id: "s".into(),
        bearer: "b".into(),
    })
    .unwrap();
    h.update_bearer("b2".into());
    assert!(h.update_endpointer(Channel::Me, "{".into()).is_err());

    let audio = track(16_000, 0.3, &[-300, 1000, -2000]);
    let t0 = Instant::now();
    for c in audio.chunks(320) {
        let st = h.push_audio(Channel::Me, c.to_vec(), 16_000);
        assert!(matches!(
            st,
            PushStatus::Accepted | PushStatus::DroppedOldest
        ));
    }
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "push_audio must not block"
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = l.0.lock().unwrap().iter().any(
            |e| matches!(e, CopilotEvent::Segment { segment } if segment.channel == Channel::Me),
        );
        if got {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no segment delivered to listener"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(h.stats().unwrap().me.chunks_pushed > 0);
    h.stop().unwrap();
    assert_eq!(
        h.push_audio(Channel::Me, vec![0.0; 320], 16_000),
        PushStatus::Closed
    );
}
