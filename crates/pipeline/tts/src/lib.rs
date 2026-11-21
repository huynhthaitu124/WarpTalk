//! Text-to-speech stage: [`Translation`](warptalk_mt::Translation)s in, streamed
//! [`DubChunk`]s out. See [`stage`] for the latency design decisions.
//!
//! Building blocks: ports ([`Synthesizer`], [`TtsCache`], [`ConsentChecker`], [`VoiceCloner`],
//! [`VoiceCatalogProvider`], [`VoiceProfiles`]); the vendor adapters over `warptalk-cartesia`
//! ([`cartesia_adapter`]); [`SlotGate`]; delivery math ([`delivery`]); voice selection and the
//! progressive clone ladder ([`voice`]); caches ([`cache`]).
//!
//! Config: [`TtsConfig`] stored as JSON in the `pipeline` settings namespace, key `tts`; the
//! output sample rate (24000 or 44100) and warm pool size are in `CartesiaConfig`.

pub mod cache;
pub mod cartesia_adapter;
mod config;
pub mod delivery;
mod error;
pub mod gate;
mod ports;
pub mod stage;
pub mod testing;
mod types;
pub mod voice;

pub use cache::{InMemoryTtsCache, RedisTtsCache, cache_key};
pub use cartesia_adapter::{CartesiaCatalog, CartesiaSynthesizer, CartesiaVoiceCloner};
pub use config::{
    CacheConfig, CatchUpConfig, CloneConfig, ContextConfig, IsochronyConfig, QualityConfig,
    RetryConfig, SpeedConfig, TtsConfig,
};
pub use error::TtsError;
pub use gate::{SlotGate, SlotPermit};
pub use ports::{
    CachedAudio, CatalogVoice, CloneRequest, ConsentChecker, ContextParams, GenConfig, NoConsent,
    NoProfiles, SpeakStats, SynthContext, Synthesizer, TtsCache, VoiceCatalogProvider, VoiceCloner,
    VoiceProfiles,
};
pub use stage::{
    STAGE_TTS_FIRST_AUDIO, STAGE_TTS_SLOT_WAIT, STAGE_TTS_SYNTHESIS, TtsDeps, TtsOutputs, TtsStage,
    TtsStageHandle, give_up_reason,
};
pub use types::{DubChunk, RouteTier, TtsEvent, TtsStamps, VoiceInfo, VoiceMode, VoiceSource};
pub use voice::{CloneLadder, ClonedVoice, Resolved, VoiceResolver, assess_sample};
