//! HTTP client for the radio-browser.info station database.
//!
//! Resolves curated station UUIDs and free-text searches to `Station`
//! records that can be passed to the streamer. The mirror set comes from
//! [`super::discovery`]; on a 5xx or transport error from the first mirror,
//! the client tries the next one in priority order until 3 have failed.
//! After that we surface a clear error and let the picker decide whether
//! to retry later.
//!
//! **Sanitisation (SEC-004 / D20):** every free-text field that lands in a
//! `Station` is passed through `clitunes_core::sanitize` at parse time so
//! that hostile metadata cannot inject terminal escape sequences into the
//! TUI. The sanitiser strips C0 (except \t \n \r), C1, ESC and DEL.

use anyhow::{anyhow, Context, Result};
use clitunes_core::{sanitize, Station, StationUuid};
use serde::Deserialize;
use std::time::Duration;
use tracing::{debug, warn};

use super::discovery::Mirror;

/// User-Agent string sent with every radio-browser request. Identifies us
/// to mirror operators so they can debug abuse without guessing.
pub const USER_AGENT: &str = concat!(
    "clitunes/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/vxcozy/clitunes)"
);

/// Per-request timeout. Three seconds is enough for healthy mirrors over
/// any reasonable connection; slow mirrors will fail and the next one will
/// be tried.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Cap the number of mirrors we try per call. Beyond 3 the user is better
/// served by an error than by a long pause.
pub const MAX_MIRRORS_TRIED: usize = 3;

#[derive(Clone)]
pub struct StationDb {
    client: reqwest::Client,
    mirrors: Vec<Mirror>,
}

impl StationDb {
    pub fn new(mirrors: Vec<Mirror>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            // radio-browser frequently 302s; reqwest follows by default but
            // we make the limit explicit so a redirect loop fails fast.
            .redirect(reqwest::redirect::Policy::limited(5))
            .gzip(true)
            .build()
            .context("build reqwest client for station db")?;
        Ok(Self { client, mirrors })
    }

    /// Look up a station by its radio-browser UUID. Tries up to
    /// [`MAX_MIRRORS_TRIED`] mirrors in priority order before giving up.
    pub async fn get_station_by_uuid(&self, uuid: &str) -> Result<Station> {
        let path = format!("/json/stations/byuuid/{}", uuid);
        let raws: Vec<RawStation> = self.fetch_json(&path).await?;
        let raw = raws
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("station not found; the curated list may be out of date"))?;
        Ok(raw.into_station())
    }

    /// Free-text search for the picker. Returns up to ~50 stations.
    pub async fn get_station_by_name(&self, name: &str) -> Result<Vec<Station>> {
        // radio-browser supports `byname` for partial matches.
        let encoded = url_encode_path_segment(name);
        let path = format!("/json/stations/byname/{}", encoded);
        let raws: Vec<RawStation> = self.fetch_json(&path).await?;
        Ok(raws.into_iter().map(RawStation::into_station).collect())
    }

    async fn fetch_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let mut last_err: Option<anyhow::Error> = None;
        for mirror in self.mirrors.iter().take(MAX_MIRRORS_TRIED) {
            let url = format!("{}{}", mirror.https_base(), path);
            debug!(%url, "station db request");
            match self.client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    return resp
                        .json::<T>()
                        .await
                        .with_context(|| format!("parse json from {}", url));
                }
                Ok(resp) => {
                    let status = resp.status();
                    warn!(%url, %status, "mirror returned non-success");
                    last_err = Some(anyhow!("{} returned {}", mirror.host, status));
                }
                Err(e) => {
                    warn!(%url, error = %e, "mirror request failed");
                    last_err = Some(anyhow!("{}: {}", mirror.host, e));
                }
            }
        }
        Err(last_err
            .unwrap_or_else(|| anyhow!("could not reach radio-browser.info; check your network")))
    }
}

/// Wire-format station as it comes back from radio-browser. Field names
/// match the radio-browser JSON shape; the `into_station` conversion does
/// the sanitisation pass and copies into the `clitunes-core` `Station`.
#[derive(Debug, Deserialize)]
struct RawStation {
    #[serde(default)]
    stationuuid: String,
    #[serde(default)]
    name: String,
    #[serde(default, alias = "url_resolved")]
    url_resolved: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    tags: Option<String>,
    #[serde(default)]
    bitrate: Option<u32>,
    #[serde(default)]
    codec: Option<String>,
}

impl RawStation {
    fn into_station(self) -> Station {
        // url_resolved is the post-redirect direct stream URL; some entries
        // only have the unresolved `url` field, fall back to that.
        let url = if !self.url_resolved.is_empty() {
            self.url_resolved
        } else {
            self.url
        };

        Station {
            uuid: StationUuid::new(sanitize(&self.stationuuid)),
            name: sanitize(&self.name),
            // URL is *not* sanitised the same way; we keep it byte-clean
            // because reqwest will reject malformed URLs anyway, and
            // stripping bytes from a URL would silently corrupt it. The
            // URL is never displayed to the user.
            url_resolved: url,
            country: self.country.as_deref().map(sanitize),
            language: self.language.as_deref().map(sanitize),
            tags: self
                .tags
                .as_deref()
                .map(|t| {
                    t.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(sanitize)
                        .collect()
                })
                .unwrap_or_default(),
            bitrate: self.bitrate,
            codec: self.codec.as_deref().map(sanitize),
        }
    }
}

/// Minimal percent-encoder for path segments. radio-browser's `byname`
/// endpoint accepts UTF-8 station names but spaces and `/` need escaping.
/// We avoid pulling `percent-encoding` for one call site.
fn url_encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_station_into_station_sanitises_name() {
        let raw = RawStation {
            stationuuid: "abc-123".into(),
            name: "\x1b[2JEvil Radio".into(),
            url_resolved: "https://example.com/stream".into(),
            url: "".into(),
            country: Some("\x1b]8;;evil\x1b\\Germany".into()),
            language: Some("German".into()),
            tags: Some("\x1bdance,electronic".into()),
            bitrate: Some(128),
            codec: Some("MP3".into()),
        };
        let st = raw.into_station();
        assert!(!st.name.contains('\x1b'));
        assert!(st.name.contains("Evil Radio"));
        assert!(!st.country.as_ref().unwrap().contains('\x1b'));
        assert!(st.tags.iter().all(|t| !t.contains('\x1b')));
        assert_eq!(st.bitrate, Some(128));
        assert_eq!(st.url_resolved, "https://example.com/stream");
    }

    #[test]
    fn raw_station_falls_back_to_url_when_resolved_empty() {
        let raw = RawStation {
            stationuuid: "abc".into(),
            name: "Foo".into(),
            url_resolved: "".into(),
            url: "https://example.com/stream.mp3".into(),
            country: None,
            language: None,
            tags: None,
            bitrate: None,
            codec: None,
        };
        assert_eq!(
            raw.into_station().url_resolved,
            "https://example.com/stream.mp3"
        );
    }

    #[test]
    fn raw_station_tags_split_and_trim() {
        let raw = RawStation {
            stationuuid: "x".into(),
            name: "x".into(),
            url_resolved: "x".into(),
            url: "".into(),
            country: None,
            language: None,
            tags: Some(" pop , rock ,, jazz".into()),
            bitrate: None,
            codec: None,
        };
        let st = raw.into_station();
        assert_eq!(st.tags, vec!["pop", "rock", "jazz"]);
    }

    #[test]
    fn url_encode_path_segment_handles_spaces() {
        assert_eq!(url_encode_path_segment("BBC Radio 6"), "BBC%20Radio%206");
    }

    #[test]
    fn url_encode_path_segment_handles_unicode() {
        // 'é' = 0xC3 0xA9 in UTF-8.
        assert_eq!(url_encode_path_segment("café"), "caf%C3%A9");
    }
}
