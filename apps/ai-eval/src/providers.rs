//! Provider selection. Keys come from the environment and are never logged or serialized.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use warptalk_llm::{LlmProvider, ProviderConfig, ProviderId, ProviderRegistry};

use crate::dataset::Meeting;
use crate::fake::FakeProvider;

/// A provider choice made on the command line.
#[derive(Clone)]
pub enum ProviderSpec {
    /// Deterministic scripted answers (needs no keys, no network).
    Fake,
    /// The fake, but it obeys prompt injections (to prove the injection metric catches them).
    FakeNaive,
    Real {
        name: String,
        provider: Arc<dyn LlmProvider>,
    },
}

impl ProviderSpec {
    pub fn name(&self) -> &str {
        match self {
            Self::Fake => "fake",
            Self::FakeNaive => "fake-naive",
            Self::Real { name, .. } => name,
        }
    }

    pub fn is_fake(&self) -> bool {
        matches!(self, Self::Fake | Self::FakeNaive)
    }

    /// The provider to use for one meeting (the fake is scripted per meeting).
    pub fn for_meeting(&self, m: &Meeting) -> Arc<dyn LlmProvider> {
        match self {
            Self::Fake => Arc::new(FakeProvider::new(m)),
            Self::FakeNaive => Arc::new(FakeProvider::naive(m)),
            Self::Real { provider, .. } => provider.clone(),
        }
    }

    /// The provider to use as judge.
    pub fn for_judge(&self) -> Arc<dyn LlmProvider> {
        match self {
            Self::Fake | Self::FakeNaive => Arc::new(FakeProvider::judge_only()),
            Self::Real { provider, .. } => provider.clone(),
        }
    }
}

fn env_key(vars: &[&str]) -> Result<String> {
    for v in vars {
        if let Ok(k) = std::env::var(v)
            && !k.trim().is_empty()
        {
            return Ok(k);
        }
    }
    bail!("set {} to use this provider", vars.join(" or "))
}

/// `fake | fake-naive | openai | anthropic | gemini`.
pub fn build(name: &str) -> Result<ProviderSpec> {
    let (kind, vars): (ProviderId, &[&str]) = match name {
        "fake" => return Ok(ProviderSpec::Fake),
        "fake-naive" => return Ok(ProviderSpec::FakeNaive),
        "openai" => (ProviderId::OpenAi, &["OPENAI_API_KEY"]),
        "anthropic" => (ProviderId::Anthropic, &["ANTHROPIC_API_KEY"]),
        "gemini" => (ProviderId::Gemini, &["GEMINI_API_KEY", "GOOGLE_API_KEY"]),
        other => bail!("unknown provider `{other}` (fake, openai, anthropic, gemini)"),
    };
    let key = env_key(vars)?;
    let mut cfg = ProviderConfig::new(kind, key);
    cfg.timeout_ms = 90_000;
    let provider = ProviderRegistry::build(cfg)
        .map_err(|e| anyhow!("building the {name} provider failed: {e}"))?;
    Ok(ProviderSpec::Real {
        name: name.to_owned(),
        provider,
    })
}
