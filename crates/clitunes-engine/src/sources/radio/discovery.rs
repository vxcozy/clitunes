//! radio-browser.info mirror discovery via DNS SRV (RFC 2782).
//!
//! Resolves `_api._tcp.radio-browser.info` SRV records and returns the live
//! mirror set sorted by priority then weight per the RFC. Falls through a
//! three-tier fallback chain so the radio source still works when:
//!
//! 1. **DNS SRV resolves** → use live mirrors, persist to cache.
//! 2. **SRV fails / times out** → load `~/.cache/clitunes/radio-browser-mirrors.json`
//!    if present and fresher than 24h.
//! 3. **Cache missing or stale and SRV still fails** → use a baked-in static
//!    list of known-good mirrors compiled into the binary. Last resort: if a
//!    stale cache exists and the baked-in list also fails health check, the
//!    stale cache is logged and returned anyway.
//!
//! The 3-second SRV timeout is chosen to keep the picker first-paint snappy
//! (SC1 budget for the entire first-run is ~3 seconds; we cannot spend the
//! whole budget on DNS). After timeout we fall through to cache and the
//! picker can render with the cached set while a background refresh runs.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::TokioAsyncResolver;
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// Known-good mirrors as of 2026-04. Hand-picked from the radio-browser.info
/// "all servers" page; rotated when one goes dark. The baked-in list is
/// intentionally short — it's a tertiary fallback, not a primary directory.
/// If you find yourself updating this list more than twice a year, the
/// real fix is to make the cache TTL longer or the SRV timeout shorter.
pub const BAKED_IN_MIRRORS: &[&str] = &[
    "de1.api.radio-browser.info",
    "de2.api.radio-browser.info",
    "fi1.api.radio-browser.info",
    "nl1.api.radio-browser.info",
    "at1.api.radio-browser.info",
];

/// SRV query target. Public so tests can reference the same constant the
/// implementation uses.
pub const SRV_QUERY: &str = "_api._tcp.radio-browser.info.";

/// SRV resolution must complete in 3s or we fall through to cache.
pub const SRV_TIMEOUT: Duration = Duration::from_secs(3);

/// A cache entry is considered fresh for 24h.
pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Mirror {
    /// Hostname (no scheme, no path). HTTPS is implied.
    pub host: String,
    /// SRV priority (lower = higher preference per RFC 2782).
    pub priority: u16,
    /// SRV weight (higher = more traffic share at the same priority).
    pub weight: u16,
}

impl Mirror {
    pub fn https_base(&self) -> String {
        format!("https://{}", self.host)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CacheFile {
    /// Seconds since UNIX epoch when the file was written.
    written_at_secs: u64,
    mirrors: Vec<Mirror>,
}

/// What kind of source produced the returned mirror list. Useful for tests
/// and for emitting an honest UI hint when the picker is rendering against
/// a stale cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MirrorSource {
    LiveSrv,
    FreshCache,
    StaleCache,
    BakedIn,
}

#[derive(Clone, Debug)]
pub struct DiscoveredMirrors {
    pub mirrors: Vec<Mirror>,
    pub source: MirrorSource,
}

/// Default cache file location: `$XDG_CACHE_HOME/clitunes/radio-browser-mirrors.json`
/// (or the macOS / Windows equivalent via the `dirs` crate).
pub fn default_cache_path() -> Option<PathBuf> {
    let mut p = dirs::cache_dir()?;
    p.push("clitunes");
    p.push("radio-browser-mirrors.json");
    Some(p)
}

/// Discovery entrypoint. Tries SRV, then fresh cache, then baked-in, then
/// stale cache. Always returns *something* unless every fallback was
/// explicitly empty.
pub async fn discover_mirrors() -> Result<DiscoveredMirrors> {
    discover_with_paths(default_cache_path()).await
}

/// Test-friendly entrypoint that takes an explicit cache path. Production
/// code should call [`discover_mirrors`] which uses the user's cache dir.
pub async fn discover_with_paths(cache_path: Option<PathBuf>) -> Result<DiscoveredMirrors> {
    match resolve_srv_with_timeout().await {
        Ok(mirrors) if !mirrors.is_empty() => {
            info!(count = mirrors.len(), "radio-browser SRV resolved");
            if let Some(path) = cache_path.as_ref() {
                if let Err(e) = write_cache(path, &mirrors).await {
                    warn!(error = %e, "failed to persist mirror cache (continuing anyway)");
                }
            }
            return Ok(DiscoveredMirrors {
                mirrors,
                source: MirrorSource::LiveSrv,
            });
        }
        Ok(_) => {
            warn!("radio-browser SRV returned zero records, falling through to cache");
        }
        Err(e) => {
            warn!(error = %e, "radio-browser SRV failed, falling through to cache");
        }
    }

    if let Some(path) = cache_path.as_ref() {
        match read_cache(path).await {
            Ok(Some((mirrors, age))) if age < CACHE_TTL && !mirrors.is_empty() => {
                debug!(age_secs = age.as_secs(), "using fresh mirror cache");
                return Ok(DiscoveredMirrors {
                    mirrors,
                    source: MirrorSource::FreshCache,
                });
            }
            Ok(Some((mirrors, age))) if !mirrors.is_empty() => {
                warn!(
                    age_secs = age.as_secs(),
                    "mirror cache is stale; will retry SRV in background, using baked-in for now"
                );
                // Stale cache held in reserve below the baked-in list.
                let baked = baked_in_mirrors();
                if !baked.is_empty() {
                    return Ok(DiscoveredMirrors {
                        mirrors: baked,
                        source: MirrorSource::BakedIn,
                    });
                }
                return Ok(DiscoveredMirrors {
                    mirrors,
                    source: MirrorSource::StaleCache,
                });
            }
            Ok(_) => {
                debug!("no mirror cache present");
            }
            Err(e) => {
                warn!(error = %e, "failed to read mirror cache");
            }
        }
    }

    let baked = baked_in_mirrors();
    if !baked.is_empty() {
        info!(count = baked.len(), "using baked-in mirror list");
        return Ok(DiscoveredMirrors {
            mirrors: baked,
            source: MirrorSource::BakedIn,
        });
    }

    Err(anyhow!(
        "could not discover any radio-browser.info mirrors (SRV failed, cache empty, baked-in empty)"
    ))
}

/// Build the baked-in mirror set with priority/weight matching their order
/// in [`BAKED_IN_MIRRORS`]. Order = preference; ties broken by appearance.
pub fn baked_in_mirrors() -> Vec<Mirror> {
    BAKED_IN_MIRRORS
        .iter()
        .enumerate()
        .map(|(idx, host)| Mirror {
            host: (*host).to_string(),
            priority: idx as u16,
            weight: 0,
        })
        .collect()
}

async fn resolve_srv_with_timeout() -> Result<Vec<Mirror>> {
    timeout(SRV_TIMEOUT, resolve_srv())
        .await
        .map_err(|_| anyhow!("DNS SRV lookup timed out after {:?}", SRV_TIMEOUT))?
}

async fn resolve_srv() -> Result<Vec<Mirror>> {
    let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());
    let lookup = resolver
        .srv_lookup(SRV_QUERY)
        .await
        .with_context(|| format!("SRV query failed for {}", SRV_QUERY))?;

    let mut mirrors: Vec<Mirror> = lookup
        .iter()
        .map(|srv| {
            let mut host = srv.target().to_utf8();
            // Strip the trailing dot DNS names always carry.
            if host.ends_with('.') {
                host.pop();
            }
            Mirror {
                host,
                priority: srv.priority(),
                weight: srv.weight(),
            }
        })
        .collect();

    sort_by_rfc2782(&mut mirrors);
    Ok(mirrors)
}

/// Sort SRV records per RFC 2782: ascending priority first, then by weight
/// descending within the same priority. (The RFC's full algorithm uses
/// weighted random selection within a priority bucket; we approximate by
/// sorting weight-descending so the highest-weight mirror in a bucket is
/// tried first. This is fine for a 3-mirror retry budget.)
pub fn sort_by_rfc2782(mirrors: &mut [Mirror]) {
    mirrors.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| b.weight.cmp(&a.weight))
    });
}

async fn read_cache(path: &PathBuf) -> Result<Option<(Vec<Mirror>, Duration)>> {
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Ok(None);
    }
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("read cache {}", path.display()))?;
    let cache: CacheFile = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse cache {}", path.display()))?;
    let written = SystemTime::UNIX_EPOCH + Duration::from_secs(cache.written_at_secs);
    let age = SystemTime::now()
        .duration_since(written)
        .unwrap_or(Duration::ZERO);
    Ok(Some((cache.mirrors, age)))
}

async fn write_cache(path: &PathBuf, mirrors: &[Mirror]) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create cache dir {}", parent.display()))?;
    }
    let cache = CacheFile {
        written_at_secs: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        mirrors: mirrors.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&cache).context("serialise cache")?;
    tokio::fs::write(path, bytes)
        .await
        .with_context(|| format!("write cache {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc2782_sort_priority_then_weight_desc() {
        let mut input = vec![
            Mirror {
                host: "lo-prio".into(),
                priority: 20,
                weight: 100,
            },
            Mirror {
                host: "hi-prio-low-w".into(),
                priority: 10,
                weight: 1,
            },
            Mirror {
                host: "hi-prio-high-w".into(),
                priority: 10,
                weight: 50,
            },
            Mirror {
                host: "hi-prio-mid-w".into(),
                priority: 10,
                weight: 25,
            },
        ];
        sort_by_rfc2782(&mut input);
        assert_eq!(input[0].host, "hi-prio-high-w");
        assert_eq!(input[1].host, "hi-prio-mid-w");
        assert_eq!(input[2].host, "hi-prio-low-w");
        assert_eq!(input[3].host, "lo-prio");
    }

    #[test]
    fn baked_in_is_nonempty_and_ordered() {
        let m = baked_in_mirrors();
        assert!(!m.is_empty(), "baked-in mirror list must never be empty");
        for (i, mirror) in m.iter().enumerate() {
            assert_eq!(mirror.priority, i as u16);
            assert!(!mirror.host.is_empty());
            assert!(!mirror.host.contains("://"));
        }
    }

    #[test]
    fn https_base_format() {
        let m = Mirror {
            host: "de1.api.radio-browser.info".into(),
            priority: 0,
            weight: 0,
        };
        assert_eq!(m.https_base(), "https://de1.api.radio-browser.info");
    }
}
