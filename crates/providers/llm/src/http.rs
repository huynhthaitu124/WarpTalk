//! Shared HTTP plumbing: client, auth headers, error classification, SSE driver, JSON helpers.

use std::collections::VecDeque;
use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use futures::stream::BoxStream;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Url};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

use crate::config::ProviderConfig;
use crate::error::LlmError;
use crate::model::{StreamEvent, Usage};

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// One shared connection pool; key / base_url / headers travel with each request.
fn shared_client() -> Result<reqwest::Client, LlmError> {
    if let Some(c) = CLIENT.get() {
        return Ok(c.clone());
    }
    let c = reqwest::Client::builder()
        // Never follow redirects: a redirect could forward credentials to another host.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .user_agent("warptalk-llm")
        .build()
        .map_err(|e| LlmError::Transport(e.without_url().to_string()))?;
    Ok(CLIENT.get_or_init(|| c).clone())
}

pub(crate) fn map_reqwest(e: reqwest::Error) -> LlmError {
    let e = e.without_url();
    if e.is_timeout() {
        LlmError::Timeout
    } else if e.is_decode() {
        LlmError::Decode(e.to_string())
    } else {
        LlmError::Transport(e.to_string())
    }
}

#[derive(Clone, Copy)]
pub(crate) enum AuthStyle {
    Bearer,
    Header(&'static str),
}

pub(crate) struct HttpCore {
    client: reqwest::Client,
    base: Url,
    api_key: SecretString,
    headers: HeaderMap,
    auth: Option<(HeaderName, HeaderValue)>,
}

impl fmt::Debug for HttpCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpCore")
            .field("base", &self.base.as_str())
            .field("api_key", &"[REDACTED]")
            .field(
                "headers",
                &self.headers.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl HttpCore {
    pub fn new(
        cfg: &ProviderConfig,
        default_base: &str,
        auth: AuthStyle,
        static_headers: &[(&'static str, &'static str)],
    ) -> Result<Self, LlmError> {
        let base_str = cfg.base_url.as_deref().unwrap_or(default_base);
        let base = Url::parse(base_str)
            .map_err(|e| LlmError::InvalidRequest(format!("invalid base_url: {e}")))?;
        if base.cannot_be_a_base() {
            return Err(LlmError::InvalidRequest("invalid base_url".into()));
        }
        let mut headers = HeaderMap::new();
        for (k, v) in static_headers {
            headers.insert(HeaderName::from_static(k), HeaderValue::from_static(v));
        }
        let mut custom_auth = false;
        for (k, v) in &cfg.default_headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| LlmError::InvalidRequest(format!("invalid header name `{k}`")))?;
            let mut value = HeaderValue::from_str(v)
                .map_err(|_| LlmError::InvalidRequest(format!("invalid value for header `{k}`")))?;
            value.set_sensitive(true);
            if matches!(
                name.as_str(),
                "authorization" | "api-key" | "x-api-key" | "x-goog-api-key"
            ) {
                custom_auth = true;
            }
            headers.insert(name, value);
        }
        let key = cfg.api_key.expose_secret();
        // Azure / Vertex style gateways bring their own auth header: do not add ours.
        let auth = if custom_auth || key.is_empty() {
            None
        } else {
            let (name, raw) = match auth {
                AuthStyle::Bearer => (AUTHORIZATION, format!("Bearer {key}")),
                AuthStyle::Header(h) => (HeaderName::from_static(h), key.to_string()),
            };
            let mut value = HeaderValue::from_str(&raw)
                .map_err(|_| LlmError::InvalidRequest("api key has invalid characters".into()))?;
            value.set_sensitive(true);
            Some((name, value))
        };
        Ok(Self {
            client: shared_client()?,
            base,
            api_key: cfg.api_key.clone(),
            headers,
            auth,
        })
    }

    /// `base` + path segments, preserving any query on the base (e.g. Azure `api-version`).
    pub fn url(&self, segments: &[&str]) -> Result<Url, LlmError> {
        let mut u = self.base.clone();
        u.path_segments_mut()
            .map_err(|_| LlmError::InvalidRequest("invalid base_url".into()))?
            .pop_if_empty()
            .extend(segments);
        Ok(u)
    }

    pub async fn send(
        &self,
        method: Method,
        url: Url,
        body: Option<&Value>,
        sse: bool,
    ) -> Result<reqwest::Response, LlmError> {
        let mut rb = self
            .client
            .request(method, url)
            .headers(self.headers.clone());
        if let Some((n, v)) = &self.auth {
            rb = rb.header(n.clone(), v.clone());
        }
        if sse {
            rb = rb.header(ACCEPT, "text/event-stream");
        }
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let resp = rb.send().await.map_err(map_reqwest)?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let mut text = resp.text().await.unwrap_or_default();
        let key = self.api_key.expose_secret();
        if !key.is_empty() {
            text = text.replace(key, "[REDACTED]");
        }
        Err(classify_http_error(status, &headers, &text))
    }

    pub async fn post_json(
        &self,
        url: Url,
        body: &Value,
    ) -> Result<(Value, Option<String>), LlmError> {
        let resp = self.send(Method::POST, url, Some(body), false).await?;
        let rid = request_id(resp.headers());
        let v = resp
            .json::<Value>()
            .await
            .map_err(|e| LlmError::Decode(e.without_url().to_string()))?;
        Ok((v, rid))
    }

    pub async fn post_sse(&self, url: Url, body: &Value) -> Result<reqwest::Response, LlmError> {
        self.send(Method::POST, url, Some(body), true).await
    }

    pub async fn get_ok(&self, url: Url) -> Result<(), LlmError> {
        self.send(Method::GET, url, None, false).await.map(|_| ())
    }
}

pub(crate) fn request_id(h: &HeaderMap) -> Option<String> {
    ["request-id", "x-request-id"]
        .iter()
        .find_map(|n| h.get(*n))
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

fn parse_retry_after(h: &HeaderMap) -> Option<Duration> {
    if let Some(ms) = h
        .get("retry-after-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
    {
        return Duration::try_from_secs_f64(ms / 1000.0).ok();
    }
    h.get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .and_then(|s| Duration::try_from_secs_f64(s).ok())
}

const QUOTA_CODES: [&str; 7] = [
    "credit_balance_exhausted",
    "organization_spend_limit_exceeded",
    "project_spend_limit_exceeded",
    "enforced_spend_limit_reached",
    "insufficient_quota",
    "billing_error",
    "billing_not_active",
];

fn truncate(s: &str) -> String {
    s.chars().take(500).collect()
}

/// Classifies by provider error codes first and HTTP status second: a 429 can mean either
/// "slow down" or "out of money".
pub(crate) fn classify_http_error(status: u16, headers: &HeaderMap, body: &str) -> LlmError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    classify(status, parse_retry_after(headers), &v, body)
}

fn classify(status: u16, retry_after: Option<Duration>, v: &Value, raw: &str) -> LlmError {
    let err = v.get("error").unwrap_or(v);
    let mut tags = String::new();
    let mut push = |x: Option<&Value>| {
        match x {
            Some(Value::String(s)) => tags.push_str(s),
            Some(Value::Number(n)) => tags.push_str(&n.to_string()),
            _ => {}
        }
        tags.push(' ');
    };
    push(err.get("type"));
    push(err.get("code"));
    push(err.get("status"));
    push(err.get("error_code"));
    push(err.pointer("/details/error_code"));
    // Gemini: error.details is an array of {reason: ...}.
    if let Some(arr) = err.get("details").and_then(Value::as_array) {
        for d in arr {
            push(d.get("reason"));
        }
    }
    let tags = tags.to_lowercase();
    let msg = err
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| raw.to_owned());
    let low = msg.to_lowercase();
    let msg = truncate(&msg);

    if status == 402 || QUOTA_CODES.iter().any(|c| tags.contains(c)) || low.contains("spend limit")
    {
        return LlmError::QuotaOrBilling(msg);
    }
    // UNVERIFIED: Gemini reports a bad key as HTTP 400 with reason API_KEY_INVALID.
    if tags.contains("api_key_invalid") || low.contains("api key not valid") {
        return LlmError::Auth(msg);
    }
    if tags.contains("content_filter") || tags.contains("content_policy") {
        return LlmError::ContentFiltered(msg);
    }
    if status == 413
        || tags.contains("context_length_exceeded")
        || tags.contains("request_too_large")
        || low.contains("context length")
        || low.contains("context window")
        || low.contains("prompt is too long")
        || low.contains("maximum context")
        || low.contains("too many tokens")
        || low.contains("exceeds the maximum number of tokens")
        || (low.contains("input token count") && low.contains("exceeds"))
    {
        return LlmError::ContextTooLong(msg);
    }
    match status {
        401 | 403 => LlmError::Auth(msg),
        408 => LlmError::Timeout,
        429 => LlmError::RateLimited {
            retry_after,
            message: msg,
        },
        409 | 500..=599 => LlmError::Overloaded(msg),
        _ => LlmError::InvalidRequest(msg),
    }
}

/// Error object delivered inside an SSE stream (HTTP status already 200).
pub(crate) fn classify_stream_error(v: &Value) -> LlmError {
    let err = v.get("error").unwrap_or(v);
    let ty = err
        .get("type")
        .or_else(|| err.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let status = match ty {
        "overloaded_error" | "server_is_overloaded" | "service_unavailable_error" => 529,
        "rate_limit_error" | "rate_limit_exceeded" => 429,
        "authentication_error" => 401,
        "permission_error" => 403,
        "billing_error" => 402,
        "invalid_request_error" => 400,
        "timeout_error" => 504,
        _ => 500,
    };
    classify(status, None, v, &v.to_string())
}

/// Stateful per-provider SSE interpreter.
pub(crate) trait SseParser: Send + 'static {
    fn event(&mut self, name: &str, data: &str) -> Result<Vec<StreamEvent>, LlmError>;
    /// Called once when the byte stream ends.
    fn finish(&mut self) -> Result<Vec<StreamEvent>, LlmError> {
        Ok(Vec::new())
    }
}

struct SseState<P> {
    src: BoxStream<'static, Result<eventsource_stream::Event, String>>,
    parser: P,
    queue: VecDeque<StreamEvent>,
    finished: bool,
}

pub(crate) fn sse_stream<P: SseParser>(
    resp: reqwest::Response,
    parser: P,
) -> BoxStream<'static, Result<StreamEvent, LlmError>> {
    let src = resp
        .bytes_stream()
        .eventsource()
        .map(|r| r.map_err(|e| e.to_string()))
        .boxed();
    let st = SseState {
        src,
        parser,
        queue: VecDeque::new(),
        finished: false,
    };
    futures::stream::unfold(st, |mut st| async move {
        loop {
            if let Some(ev) = st.queue.pop_front() {
                return Some((Ok(ev), st));
            }
            if st.finished {
                return None;
            }
            match st.src.next().await {
                None => {
                    st.finished = true;
                    match st.parser.finish() {
                        Ok(v) => st.queue.extend(v),
                        Err(e) => return Some((Err(e), st)),
                    }
                }
                Some(Err(e)) => {
                    st.finished = true;
                    return Some((Err(LlmError::Transport(e)), st));
                }
                Some(Ok(ev)) => match st.parser.event(&ev.event, &ev.data) {
                    Ok(v) => st.queue.extend(v),
                    Err(e) => {
                        st.finished = true;
                        return Some((Err(e), st));
                    }
                },
            }
        }
    })
    .boxed()
}

pub(crate) fn parse_json(data: &str) -> Result<Value, LlmError> {
    serde_json::from_str(data).map_err(|e| LlmError::Decode(format!("bad SSE payload: {e}")))
}

pub(crate) fn u32_at(v: &Value, ptr: &str) -> u32 {
    v.pointer(ptr)
        .and_then(Value::as_u64)
        .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

/// OpenAI-shaped usage (Responses and Chat Completions). Input counts include cached tokens
/// there, so uncached = input - cached.
pub(crate) fn openai_usage(
    u: &Value,
    input: &str,
    output: &str,
    in_det: &str,
    out_det: &str,
) -> Usage {
    let total_in = u32_at(u, &format!("/{input}"));
    // UNVERIFIED: OpenAI reports cached tokens as `<input>_details.cached_tokens`, included in the input count.
    let cached = u32_at(u, &format!("/{in_det}/cached_tokens"));
    Usage {
        input_uncached: total_in.saturating_sub(cached),
        cache_read: cached,
        cache_write: 0,
        output: u32_at(u, &format!("/{output}")),
        reasoning: u32_at(u, &format!("/{out_det}/reasoning_tokens")),
    }
}

/// Splits a text-only view of parts (used for system / user string contents).
pub(crate) fn join_text(parts: &[crate::model::Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            crate::model::Part::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn tool_args(args_json: &str, id: &str) -> Result<Value, LlmError> {
    if args_json.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_str(args_json).map_err(|e| {
        LlmError::InvalidRequest(format!("tool call {id}: args_json is not valid JSON: {e}"))
    })
}
