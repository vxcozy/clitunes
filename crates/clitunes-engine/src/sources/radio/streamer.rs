//! HTTP streamer for radio station URLs.
//!
//! Opens the resolved stream URL with `Icy-MetaData: 1` so the server
//! interleaves ICY metadata chunks (parsed by Unit 6, not here). Returns
//! the response headers plus an async byte stream that downstream code
//! (Unit 7's symphonia decoder) consumes. Reconnects on transient network
//! errors with exponential backoff: 1s, 2s, 4s, 8s; gives up after 4 tries
//! and surfaces a `source_error`-shaped result.
//!
//! Unit 5 is purely network: this module returns bytes, it does not decode
//! and it does not parse ICY metadata.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use clitunes_core::sanitize;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use super::station_db::USER_AGENT;

/// Backoff schedule for transient stream-open failures. Aligned with the
/// Unit 5 spec: 1s, 2s, 4s, 8s, give up.
pub const BACKOFF_SCHEDULE: &[Duration] = &[
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];

/// Connect timeout for the initial GET. Streams that take longer than this
/// to start sending bytes are considered dead.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Header value the streamer sends to ask for ICY metadata interleaving.
pub const ICY_METADATA_HEADER: &str = "Icy-MetaData";

/// What the server told us about its stream. Every free-text `Icy-*`
/// header is passed through [`clitunes_core::sanitize`] at construction
/// time so any downstream code can trust the contents — Icecast operators
/// control these strings and they reach the now-playing display. See
/// plan decision D20.
#[derive(Clone, Debug, Default)]
pub struct StreamHeaders {
    pub content_type: Option<String>,
    pub icy_name: Option<String>,
    pub icy_genre: Option<String>,
    pub icy_description: Option<String>,
    pub icy_br: Option<String>,
    pub icy_metaint: Option<usize>,
}

impl StreamHeaders {
    fn from_response(resp: &reqwest::Response) -> Self {
        let headers = resp.headers();
        let get_sanitized = |name: &str| -> Option<String> {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(sanitize)
                .filter(|s| !s.is_empty())
        };
        // content-type is parsed by the decoder, not rendered, so it
        // doesn't need sanitization — but `Icy-*` fields all do.
        let content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        StreamHeaders {
            content_type,
            icy_name: get_sanitized("icy-name"),
            icy_genre: get_sanitized("icy-genre"),
            icy_description: get_sanitized("icy-description"),
            icy_br: get_sanitized("icy-br"),
            icy_metaint: headers
                .get("icy-metaint")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<usize>().ok()),
        }
    }
}

/// Result of a successful stream open: the headers and an async byte stream
/// the caller pumps until exhaustion or stop signal.
pub struct OpenedStream {
    pub url: String,
    pub headers: StreamHeaders,
    pub bytes: BoxStream<'static, Result<Bytes, reqwest::Error>>,
}

#[derive(Clone)]
pub struct RadioStreamer {
    client: reqwest::Client,
}

impl Default for RadioStreamer {
    fn default() -> Self {
        Self::new().expect("default reqwest client")
    }
}

impl RadioStreamer {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            // No top-level read timeout: this is a long-lived stream.
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .context("build reqwest client for streamer")?;
        Ok(Self { client })
    }

    /// Open a stream URL with retries. Returns the headers and the byte
    /// stream on success; surfaces a clear error if all retries fail.
    pub async fn open(&self, url: &str) -> Result<OpenedStream> {
        let mut last_err: Option<anyhow::Error> = None;
        for (attempt, backoff) in std::iter::once(&Duration::ZERO)
            .chain(BACKOFF_SCHEDULE.iter())
            .enumerate()
        {
            if !backoff.is_zero() {
                debug!(attempt, secs = backoff.as_secs(), "stream open backoff");
                sleep(*backoff).await;
            }
            match self.try_open_once(url).await {
                Ok(stream) => {
                    if attempt > 0 {
                        info!(attempt, %url, "stream re-opened after retry");
                    }
                    return Ok(stream);
                }
                Err(e) => {
                    warn!(attempt, %url, error = %e, "stream open failed");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            anyhow!(
                "could not open stream after {} attempts",
                BACKOFF_SCHEDULE.len() + 1
            )
        }))
    }

    async fn try_open_once(&self, url: &str) -> Result<OpenedStream> {
        let resp = self
            .client
            .get(url)
            .header(ICY_METADATA_HEADER, "1")
            .send()
            .await
            .with_context(|| format!("GET {}", url))?;

        if !resp.status().is_success() {
            return Err(anyhow!("upstream returned HTTP {}", resp.status()));
        }

        let final_url = resp.url().to_string();
        let headers = StreamHeaders::from_response(&resp);
        debug!(?headers, %final_url, "stream open ok");

        Ok(OpenedStream {
            url: final_url,
            headers,
            bytes: resp.bytes_stream().boxed(),
        })
    }
}

/// Drain the byte stream until `max_bytes` have been read or the stream
/// ends. Useful for the manual verification harness from the bead spec
/// ("print the first 10kB of bytes received") and for tests that want to
/// confirm bytes flowed without standing up a full decoder.
pub async fn drain_at_most(stream: &mut OpenedStream, max_bytes: usize) -> Result<usize> {
    let mut total = 0usize;
    while total < max_bytes {
        match stream.bytes.next().await {
            Some(Ok(chunk)) => total += chunk.len(),
            Some(Err(e)) => return Err(anyhow!("stream chunk error: {}", e)),
            None => break,
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_schedule_is_exponential() {
        assert_eq!(BACKOFF_SCHEDULE.len(), 4);
        assert_eq!(BACKOFF_SCHEDULE[0], Duration::from_secs(1));
        assert_eq!(BACKOFF_SCHEDULE[1], Duration::from_secs(2));
        assert_eq!(BACKOFF_SCHEDULE[2], Duration::from_secs(4));
        assert_eq!(BACKOFF_SCHEDULE[3], Duration::from_secs(8));
    }

    #[test]
    fn streamer_constructs() {
        let _ = RadioStreamer::new().expect("client builds");
    }
}
