//! Key validation before storing (byo-llm.md section 6.4): SSRF guard on customer endpoints and
//! a minimal authenticated call through `LlmProvider::validate`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use url::{Host, Url};
use warptalk_assistant::application::ports::{KeyCheckError, KeyValidator};
use warptalk_assistant::domain::byo::LlmProviderKind;
use warptalk_llm::{LlmError, ProviderConfig, ProviderId, ProviderRegistry};

const VALIDATE_TIMEOUT: Duration = Duration::from_secs(15);
pub const ENV_ALLOW_INSECURE_BASE_URL: &str = "AI_ALLOW_INSECURE_BASE_URL";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseUrlPolicy {
    /// `https` only; hosts resolving to private, loopback, link-local or metadata addresses are
    /// refused.
    Strict,
    /// Any http(s) URL, including loopback. Local development and tests only.
    AllowInsecure,
}

impl BaseUrlPolicy {
    /// `AI_ALLOW_INSECURE_BASE_URL=true` relaxes the guard; default is strict.
    pub fn from_env() -> Self {
        match std::env::var(ENV_ALLOW_INSECURE_BASE_URL)
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("1" | "true" | "TRUE" | "yes") => Self::AllowInsecure,
            _ => Self::Strict,
        }
    }
}

pub struct LlmKeyValidator {
    policy: BaseUrlPolicy,
}

impl LlmKeyValidator {
    pub fn new(policy: BaseUrlPolicy) -> Self {
        Self { policy }
    }
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        // 100.64.0.0/10 carrier-grade NAT, 0.0.0.0/8
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
        || o[0] == 0)
}

fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let seg0 = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (seg0 & 0xfe00) == 0xfc00 // unique local
        || (seg0 & 0xffc0) == 0xfe80) // link-local
}

fn bad(msg: &str) -> KeyCheckError {
    KeyCheckError::BadBaseUrl(msg.to_owned())
}

#[async_trait]
impl KeyValidator for LlmKeyValidator {
    async fn check_base_url(&self, base_url: &str) -> Result<(), KeyCheckError> {
        let url = Url::parse(base_url).map_err(|_| bad("baseUrl is not a valid URL"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(bad("baseUrl must use http or https"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(bad("baseUrl must not contain credentials"));
        }
        let host = url.host().ok_or_else(|| bad("baseUrl has no host"))?;
        if self.policy == BaseUrlPolicy::AllowInsecure {
            return Ok(());
        }
        if url.scheme() != "https" {
            return Err(bad("baseUrl must use https"));
        }
        let denied = || bad("baseUrl must not point to a private or local address");
        match host {
            Host::Ipv4(ip) => public_v4(ip).then_some(()).ok_or_else(denied),
            Host::Ipv6(ip) => public_v6(ip).then_some(()).ok_or_else(denied),
            Host::Domain(name) => {
                if name.eq_ignore_ascii_case("localhost") || name.ends_with(".localhost") {
                    return Err(denied());
                }
                let port = url.port_or_known_default().unwrap_or(443);
                let addrs = tokio::net::lookup_host((name, port))
                    .await
                    .map_err(|_| bad("baseUrl host does not resolve"))?;
                let mut any = false;
                for a in addrs {
                    any = true;
                    if !is_public(a.ip()) {
                        return Err(denied());
                    }
                }
                any.then_some(())
                    .ok_or_else(|| bad("baseUrl host does not resolve"))
            }
        }
    }

    async fn validate(
        &self,
        provider: LlmProviderKind,
        base_url: Option<&str>,
        key: &SecretString,
    ) -> Result<(), KeyCheckError> {
        let kind = match provider {
            LlmProviderKind::OpenAi => ProviderId::OpenAi,
            LlmProviderKind::Anthropic => ProviderId::Anthropic,
            LlmProviderKind::Gemini => ProviderId::Gemini,
            LlmProviderKind::OpenAiCompatible => ProviderId::OpenAiCompatible,
        };
        let mut cfg = ProviderConfig::new(kind, key.expose_secret());
        cfg.base_url = base_url.map(str::to_owned);
        cfg.max_retries = 0;
        cfg.timeout_ms = u64::try_from(VALIDATE_TIMEOUT.as_millis()).unwrap_or(15_000);
        let p =
            ProviderRegistry::build(cfg).map_err(|e| KeyCheckError::Unreachable(e.to_string()))?;
        match tokio::time::timeout(VALIDATE_TIMEOUT, p.validate()).await {
            Err(_) => Err(KeyCheckError::Unreachable("validation timed out".into())),
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(match e {
                LlmError::Auth(m) | LlmError::InvalidRequest(m) => KeyCheckError::Rejected(m),
                LlmError::QuotaOrBilling(m) => KeyCheckError::Account(m),
                other => KeyCheckError::Unreachable(other.to_string()),
            }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn strict_policy_blocks_local_and_plain_http() {
        let v = LlmKeyValidator::new(BaseUrlPolicy::Strict);
        for u in [
            "http://example.com",
            "https://127.0.0.1",
            "https://169.254.169.254/latest",
            "https://10.0.0.5",
            "https://localhost:8443",
            "https://[::1]",
            "https://user:pw@example.com",
            "ftp://example.com",
            "not a url",
        ] {
            assert!(v.check_base_url(u).await.is_err(), "{u}");
        }
        assert!(v.check_base_url("https://93.184.216.34/v1").await.is_ok());
    }

    #[tokio::test]
    async fn insecure_policy_allows_loopback() {
        let v = LlmKeyValidator::new(BaseUrlPolicy::AllowInsecure);
        assert!(v.check_base_url("http://127.0.0.1:1234").await.is_ok());
        assert!(v.check_base_url("ftp://x").await.is_err());
    }
}
