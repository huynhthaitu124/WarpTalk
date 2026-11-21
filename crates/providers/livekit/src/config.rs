use std::fmt;

/// Connection settings for a LiveKit server.
#[derive(Clone)]
pub struct LiveKitConfig {
    /// Public server URL handed to clients (`wss://...`); `https://` equivalent is used for Twirp.
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
}

impl fmt::Debug for LiveKitConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveKitConfig")
            .field("url", &self.url)
            .field("api_key", &self.api_key)
            .field("api_secret", &"***")
            .finish()
    }
}

impl LiveKitConfig {
    /// HTTP(S) base URL for server API calls (`wss` -> `https`, `ws` -> `http`, no trailing slash).
    pub fn http_url(&self) -> String {
        let u = self.url.trim_end_matches('/');
        if let Some(rest) = u.strip_prefix("wss://") {
            format!("https://{rest}")
        } else if let Some(rest) = u.strip_prefix("ws://") {
            format!("http://{rest}")
        } else {
            u.to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_secret() {
        let c = LiveKitConfig {
            url: "wss://lk.example".into(),
            api_key: "key".into(),
            api_secret: "supersecret".into(),
        };
        let d = format!("{c:?}");
        assert!(!d.contains("supersecret"));
        assert!(d.contains("key"));
        assert_eq!(c.http_url(), "https://lk.example");
    }
}
