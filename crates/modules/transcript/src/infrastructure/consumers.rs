//! Stream handlers for `transcript-persistence` (spec 12 §6.1).

use std::sync::Arc;

use async_trait::async_trait;
use warptalk_bus::{HandlerError, StreamHandler, StreamMessage};
use warptalk_contracts::events::{SttResult, TranslateResult, TtsResult};

use crate::application::ingest::{IngestError, IngestService};

fn map_err(e: IngestError) -> HandlerError {
    match e {
        IngestError::Invalid(m) => HandlerError::Fatal(m),
        IngestError::Transient(m) => HandlerError::Retry(m),
    }
}

fn decode<T: serde::de::DeserializeOwned>(msg: &StreamMessage) -> Result<T, HandlerError> {
    msg.json::<T>()
        .map_err(|e| HandlerError::Fatal(e.to_string()))
}

macro_rules! handler {
    ($name:ident, $ty:ty, $method:ident) => {
        pub struct $name {
            ingest: Arc<IngestService>,
        }

        impl $name {
            pub fn new(ingest: Arc<IngestService>) -> Self {
                Self { ingest }
            }
        }

        #[async_trait]
        impl StreamHandler for $name {
            async fn handle(&self, msg: &StreamMessage) -> Result<(), HandlerError> {
                let value: $ty = decode(msg)?;
                self.ingest
                    .$method(value)
                    .await
                    .map(|_| ())
                    .map_err(map_err)
            }
        }
    };
}

handler!(SttResultsHandler, SttResult, ingest_stt);
handler!(TranslateResultsHandler, TranslateResult, ingest_translation);
handler!(TtsResultsHandler, TtsResult, ingest_tts);
