//! `copilot-cli`: streams WAV files through the copilot engine at real-time pace and posts
//! the resulting segments.
//!
//! ```text
//! copilot-cli [--me a.wav] [--others b.wav] [--server URL --session ID --token BEARER]
//!             [--fake-stt] [--realtime] [--fast] [--config cfg.json]
//! ```
//!
//! * `--fake-stt`: deterministic STT ("utterance N on <channel>"), no network for STT.
//! * `--realtime`: pace the audio at wall-clock speed (this is the default; the flag is
//!   accepted for explicitness). `--fast` pushes as quickly as possible (CI).
//! * Without `--server` segments are only printed (in-memory sink).
//! * Without `--fake-stt`, STT is OpenAI Realtime with an ephemeral token fetched from
//!   `{server}/api/v1/copilot/sessions/{session}/stt-token`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow_free::{CliError, Result};
use warptalk_copilot_core::sink::{HttpSegmentSink, MemorySink, SegmentSink};
use warptalk_copilot_core::wav::read_wav;
use warptalk_copilot_core::{
    Channel, CopilotConfig, CopilotDeps, CopilotEngine, CopilotEvent, EphemeralRealtimeStt,
    FakeStt, HttpSttTokenProvider, TokenManager,
};
use warptalk_pipeline_runtime::SpeechToText;

mod anyhow_free {
    #[derive(Debug)]
    pub struct CliError(pub String);
    impl std::fmt::Display for CliError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl<E: std::error::Error> From<E> for CliError {
        fn from(e: E) -> Self {
            CliError(e.to_string())
        }
    }
    pub type Result<T> = std::result::Result<T, CliError>;
}

#[derive(Default)]
struct Args {
    me: Option<PathBuf>,
    others: Option<PathBuf>,
    server: Option<String>,
    session: Option<String>,
    token: Option<String>,
    config: Option<PathBuf>,
    fake_stt: bool,
    fast: bool,
}

fn parse_args() -> Result<Args> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |name: &str| {
            it.next()
                .ok_or_else(|| CliError(format!("{name} needs a value")))
        };
        match arg.as_str() {
            "--me" => a.me = Some(val("--me")?.into()),
            "--others" => a.others = Some(val("--others")?.into()),
            "--server" => a.server = Some(val("--server")?),
            "--session" => a.session = Some(val("--session")?),
            "--token" => a.token = Some(val("--token")?),
            "--config" => a.config = Some(val("--config")?.into()),
            "--fake-stt" => a.fake_stt = true,
            "--realtime" => a.fast = false,
            "--fast" => a.fast = true,
            "-h" | "--help" => {
                println!(
                    "usage: copilot-cli [--me f.wav] [--others f.wav] [--server URL --session ID \
                     --token BEARER] [--fake-stt] [--realtime|--fast] [--config cfg.json]"
                );
                std::process::exit(0);
            }
            other => return Err(CliError(format!("unknown argument {other}"))),
        }
    }
    if a.me.is_none() && a.others.is_none() {
        return Err(CliError("give at least one of --me / --others".into()));
    }
    Ok(a)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args = parse_args()?;
    let cfg = match &args.config {
        Some(p) => CopilotConfig::from_json(&std::fs::read_to_string(p)?)?,
        None => CopilotConfig::default(),
    };
    let session = args.session.clone().unwrap_or_else(|| "local".into());

    let sink: Arc<dyn SegmentSink> = match (&args.server, &args.token) {
        (Some(server), Some(token)) => {
            HttpSegmentSink::spawn(cfg.sink.clone(), server, &session, token.clone())?
        }
        (Some(_), None) => return Err(CliError("--server needs --token".into())),
        _ => MemorySink::new(),
    };
    let stt: Arc<dyn SpeechToText> = if args.fake_stt {
        Arc::new(FakeStt::new(Duration::from_millis(cfg.stt.fake_latency_ms)))
    } else {
        let (Some(server), Some(token)) = (&args.server, &args.token) else {
            return Err(CliError(
                "real STT needs --server, --session and --token (or use --fake-stt)".into(),
            ));
        };
        let prov = Arc::new(HttpSttTokenProvider::new(
            server,
            &session,
            token.clone(),
            Duration::from_millis(cfg.stt.open_timeout_ms),
        )?);
        let mgr = TokenManager::new(
            prov,
            Duration::from_secs(cfg.stt.token_refresh_margin_s),
            cfg.stt.reuse_token,
        );
        mgr.spawn_refresher(Duration::from_secs(5));
        Arc::new(EphemeralRealtimeStt::new(cfg.stt.clone(), mgr))
    };

    let (engine, mut rx) = CopilotEngine::new(
        cfg,
        CopilotDeps {
            stt,
            sink,
            session_id: session,
        },
    )?;
    let printer = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                CopilotEvent::Segment { segment: s } => println!(
                    "[{} {}-{} ms]{} {}",
                    s.channel.as_str(),
                    s.start_ms,
                    s.end_ms,
                    if s.likely_echo { " (echo?)" } else { "" },
                    s.text
                ),
                CopilotEvent::Error { code, message, .. } => eprintln!("error {code}: {message}"),
                _ => {}
            }
        }
    });

    let me = args.me.as_deref().map(read_wav).transpose()?;
    let others = args.others.as_deref().map(read_wav).transpose()?;
    const CHUNK_MS: usize = 20;
    let total_ms = [&me, &others]
        .iter()
        .filter_map(|w| w.as_ref())
        .map(|w| w.samples.len() * 1000 / w.sample_rate as usize)
        .max()
        .unwrap_or(0);
    let mut tick = tokio::time::interval(Duration::from_millis(CHUNK_MS as u64));
    let mut t = 0;
    while t < total_ms {
        if !args.fast {
            tick.tick().await;
        } else if (t / CHUNK_MS).is_multiple_of(50) {
            tokio::task::yield_now().await;
        }
        for (ch, wav) in [(Channel::Me, &me), (Channel::Others, &others)] {
            if let Some(w) = wav {
                let per = w.sample_rate as usize * CHUNK_MS / 1000;
                let from = (t / CHUNK_MS) * per;
                if from < w.samples.len() {
                    let to = (from + per).min(w.samples.len());
                    engine.push_audio(ch, &w.samples[from..to], w.sample_rate);
                }
            }
        }
        t += CHUNK_MS;
    }
    engine.stop().await;
    let st = engine.stats();
    drop(engine);
    let _ = printer.await;
    eprintln!(
        "done: me {} seg / others {} seg; dropped chunks me={} others={}; sink delivered {} (buffered {})",
        st.me.segments,
        st.others.segments,
        st.me.chunks_dropped,
        st.others.chunks_dropped,
        st.sink.delivered,
        st.sink.buffered
    );
    Ok(())
}
