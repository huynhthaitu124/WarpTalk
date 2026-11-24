//! Ephemeral STT token: provider trait, HTTP implementation and a refreshing cache.
//!
//! `POST {base}/api/v1/copilot/sessions/{id}/stt-token` (bearer) ->
//! `{provider, model, token, expiresAt, websocketUrl?}`. The platform API key never leaves the
//! server; this token is short-lived and only good for the STT socket.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::error::CopilotError;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SttToken {
    pub provider: String,
    pub model: String,
    pub token: String,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub websocket_url: Option<String>,
}

impl std::fmt::Debug for SttToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SttToken")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

#[async_trait]
pub trait SttTokenProvider: Send + Sync {
    async fn fetch(&self) -> Result<SttToken, CopilotError>;
}

pub struct HttpSttTokenProvider {
    client: reqwest::Client,
    url: String,
    bearer: std::sync::Mutex<String>,
}

impl HttpSttTokenProvider {
    pub fn new(
        base_url: &str,
        session_id: &str,
        bearer: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, CopilotError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| CopilotError::Token(e.to_string()))?;
        Ok(Self {
            client,
            url: format!(
                "{}/api/v1/copilot/sessions/{}/stt-token",
                base_url.trim_end_matches('/'),
                session_id
            ),
            bearer: std::sync::Mutex::new(bearer.into()),
        })
    }

    pub fn set_bearer(&self, bearer: impl Into<String>) {
        *self
            .bearer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = bearer.into();
    }
}

#[async_trait]
impl SttTokenProvider for HttpSttTokenProvider {
    async fn fetch(&self) -> Result<SttToken, CopilotError> {
        let bearer = self
            .bearer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let resp = self
            .client
            .post(&self.url)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| CopilotError::Token(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(CopilotError::Token(format!("http {}", status.as_u16())));
        }
        resp.json::<SttToken>()
            .await
            .map_err(|e| CopilotError::Token(format!("bad token response: {e}")))
    }
}

type NowFn = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Caches the current token and refreshes it `margin` before expiry.
pub struct TokenManager {
    provider: Arc<dyn SttTokenProvider>,
    margin: chrono::Duration,
    reuse: bool,
    now: NowFn,
    cached: Mutex<Option<SttToken>>,
}

impl TokenManager {
    pub fn new(provider: Arc<dyn SttTokenProvider>, margin: Duration, reuse: bool) -> Arc<Self> {
        Self::with_clock(provider, margin, reuse, Arc::new(Utc::now))
    }

    /// Injectable clock (tests).
    pub fn with_clock(
        provider: Arc<dyn SttTokenProvider>,
        margin: Duration,
        reuse: bool,
        now: NowFn,
    ) -> Arc<Self> {
        Arc::new(Self {
            provider,
            margin: chrono::Duration::from_std(margin).unwrap_or(chrono::Duration::seconds(60)),
            reuse,
            now,
            cached: Mutex::new(None),
        })
    }

    fn fresh(&self, t: &SttToken) -> bool {
        t.expires_at - self.margin > (self.now)()
    }

    /// A token valid for at least `margin` more; fetches when none / near expiry.
    pub async fn get(&self) -> Result<SttToken, CopilotError> {
        let mut g = self.cached.lock().await;
        if self.reuse
            && let Some(t) = g.as_ref().filter(|t| self.fresh(t))
        {
            return Ok(t.clone());
        }
        let t = self.provider.fetch().await?;
        *g = Some(t.clone());
        Ok(t)
    }

    /// Time until the cached token should be refreshed (zero if none/stale).
    pub async fn time_to_refresh(&self) -> Duration {
        let g = self.cached.lock().await;
        match g.as_ref() {
            Some(t) => (t.expires_at - self.margin - (self.now)())
                .to_std()
                .unwrap_or(Duration::ZERO),
            None => Duration::ZERO,
        }
    }

    /// Background task keeping the cache warm: sleeps until `expiry - margin`, then refetches
    /// (retrying every `retry` on failure). Abort the handle to stop it.
    pub fn spawn_refresher(self: &Arc<Self>, retry: Duration) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                let wait = me.time_to_refresh().await;
                if wait > Duration::ZERO {
                    tokio::time::sleep(wait).await;
                    continue;
                }
                let r = {
                    let mut g = me.cached.lock().await;
                    match me.provider.fetch().await {
                        Ok(t) => {
                            *g = Some(t);
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                };
                if r.is_err() {
                    tokio::time::sleep(retry).await;
                }
            }
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Counting {
        n: AtomicU64,
        ttl_s: i64,
        now: NowFn,
    }

    #[async_trait]
    impl SttTokenProvider for Counting {
        async fn fetch(&self) -> Result<SttToken, CopilotError> {
            let n = self.n.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(SttToken {
                provider: "openai".into(),
                model: "m".into(),
                token: format!("tok-{n}"),
                expires_at: (self.now)() + chrono::Duration::seconds(self.ttl_s),
                websocket_url: None,
            })
        }
    }

    #[tokio::test]
    async fn refreshes_before_expiry_and_reuses_otherwise() {
        let offset = Arc::new(std::sync::atomic::AtomicI64::new(0));
        let o = offset.clone();
        let now: NowFn =
            Arc::new(move || Utc::now() + chrono::Duration::seconds(o.load(Ordering::SeqCst)));
        let prov = Arc::new(Counting {
            n: AtomicU64::new(0),
            ttl_s: 300,
            now: now.clone(),
        });
        let m = TokenManager::with_clock(prov.clone(), Duration::from_secs(60), true, now);
        assert_eq!(m.get().await.unwrap().token, "tok-1");
        assert_eq!(m.get().await.unwrap().token, "tok-1");
        offset.store(200, Ordering::SeqCst); // 100 s left > margin
        assert_eq!(m.get().await.unwrap().token, "tok-1");
        offset.store(245, Ordering::SeqCst); // 55 s left < margin 60: refresh
        assert_eq!(m.get().await.unwrap().token, "tok-2");
        assert_eq!(prov.n.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn no_reuse_fetches_every_time() {
        let now: NowFn = Arc::new(Utc::now);
        let prov = Arc::new(Counting {
            n: AtomicU64::new(0),
            ttl_s: 300,
            now: now.clone(),
        });
        let m = TokenManager::with_clock(prov.clone(), Duration::from_secs(60), false, now);
        m.get().await.unwrap();
        m.get().await.unwrap();
        assert_eq!(prov.n.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn background_refresher_renews_ahead_of_expiry() {
        let now: NowFn = Arc::new(Utc::now);
        // ttl 2 s, margin 1 s: first refresh due after ~1 s.
        let prov = Arc::new(Counting {
            n: AtomicU64::new(0),
            ttl_s: 2,
            now: now.clone(),
        });
        let m = TokenManager::with_clock(prov.clone(), Duration::from_secs(1), true, now.clone());
        m.get().await.unwrap();
        let h = m.spawn_refresher(Duration::from_millis(50));
        tokio::time::sleep(Duration::from_millis(1600)).await;
        h.abort();
        assert!(
            prov.n.load(Ordering::SeqCst) >= 2,
            "token was not refreshed"
        );
        // Whatever is cached must not be expired.
        let t = m.get().await.unwrap();
        assert!(t.expires_at > now());
    }
}
