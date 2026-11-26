#![allow(dead_code, clippy::unwrap_used)]

use std::time::Duration;

use tokio::sync::mpsc;
use warptalk_copilot_core::CopilotEvent;

pub fn tone(rate: u32, ms: u64, amp: f32) -> Vec<f32> {
    let n = (u64::from(rate) * ms / 1000) as usize;
    (0..n)
        .map(|i| amp * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin())
        .collect()
}

pub fn silence(rate: u32, ms: u64) -> Vec<f32> {
    vec![0.0; (u64::from(rate) * ms / 1000) as usize]
}

/// Concatenates `(speech_ms or silence_ms)` pieces: positive = tone, negative = silence.
pub fn track(rate: u32, amp: f32, parts: &[i64]) -> Vec<f32> {
    let mut out = Vec::new();
    for &p in parts {
        if p >= 0 {
            out.extend(tone(rate, p as u64, amp));
        } else {
            out.extend(silence(rate, (-p) as u64));
        }
    }
    out
}

pub async fn collect_until<F: Fn(&[CopilotEvent]) -> bool>(
    rx: &mut mpsc::Receiver<CopilotEvent>,
    timeout: Duration,
    done: F,
) -> Vec<CopilotEvent> {
    let mut all = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;
    while !done(&all) {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(e)) => all.push(e),
            _ => break,
        }
    }
    all
}

pub fn segments(ev: &[CopilotEvent]) -> Vec<warptalk_copilot_core::SegmentRecord> {
    ev.iter()
        .filter_map(|e| match e {
            CopilotEvent::Segment { segment } => Some(segment.clone()),
            _ => None,
        })
        .collect()
}
