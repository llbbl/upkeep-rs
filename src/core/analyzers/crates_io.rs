use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::{sleep, Duration};

use crate::core::error::{ErrorCode, Result, UpkeepError};
#[derive(Debug, Clone)]
pub struct VersionInfo {
    /// The crate name (kept for debugging and future use).
    #[allow(dead_code)]
    pub name: String,
    pub latest: Option<String>,
    /// The latest stable version (kept for future prerelease filtering).
    #[allow(dead_code)]
    pub latest_stable: Option<String>,
}

#[derive(Clone)]
pub struct CratesIoClient {
    http: Client,
    cache: Arc<Mutex<HashMap<String, VersionInfo>>>,
    limiter: Arc<Semaphore>,
}

impl CratesIoClient {
    pub fn new() -> Result<Self> {
        let http = Client::builder().user_agent("cargo-upkeep").build()?;

        Ok(Self {
            http,
            cache: Arc::new(Mutex::new(HashMap::new())),
            // crates.io rate limit: 1 request per second
            limiter: Arc::new(Semaphore::new(1)),
        })
    }

    pub async fn fetch_latest_versions(
        &self,
        names: &[String],
        allow_prerelease: bool,
    ) -> Result<HashMap<String, VersionInfo>> {
        let mut results = HashMap::new();
        let mut pending = Vec::new();

        {
            let cache = self.cache.lock().await;
            for name in names {
                if let Some(info) = cache.get(name) {
                    results.insert(name.clone(), info.clone());
                } else {
                    pending.push(name.clone());
                }
            }
        }

        for name in pending {
            // Acquire semaphore first to serialize API access
            let _permit = self.limiter.acquire().await?;

            // Re-check cache after acquiring semaphore to avoid TOCTOU race condition:
            // Another task may have populated the cache while we were waiting
            {
                let cache = self.cache.lock().await;
                if let Some(info) = cache.get(&name) {
                    results.insert(name.clone(), info.clone());
                    continue;
                }
            }

            let info = self.fetch_from_api_inner(&name, allow_prerelease).await?;
            results.insert(name.clone(), info.clone());
            let mut cache = self.cache.lock().await;
            cache.insert(name, info);
        }

        Ok(results)
    }

    /// Internal helper that fetches from API. Caller must hold the semaphore permit.
    async fn fetch_from_api_inner(
        &self,
        name: &str,
        allow_prerelease: bool,
    ) -> Result<VersionInfo> {
        // Rate limit: wait 1 second before making the request
        // This ensures we don't exceed crates.io rate limits (1 req/sec)
        sleep(Duration::from_secs(1)).await;

        let url = format!("https://crates.io/api/v1/crates/{name}");
        let response = self.http.get(&url).send().await.map_err(|err| {
            UpkeepError::context(
                ErrorCode::Http,
                format!("failed to fetch crate info from {url}"),
                err,
            )
        })?;
        let payload: CratesIoResponse = response
            .error_for_status()
            .map_err(|err| {
                UpkeepError::context(
                    ErrorCode::Http,
                    format!("HTTP error fetching {name} from crates.io"),
                    err,
                )
            })?
            .json()
            .await
            .map_err(|err| {
                UpkeepError::context(
                    ErrorCode::Json,
                    format!("failed to parse JSON response for {name}"),
                    err,
                )
            })?;

        let max_version = payload.krate.max_version;
        let max_stable_version = payload.krate.max_stable_version;

        // Determine the version to recommend based on prerelease preference
        let selected = if allow_prerelease {
            // When prereleases are allowed, prefer max_version (which includes prereleases),
            // falling back to max_stable_version if max_version is somehow missing
            max_version.clone().or_else(|| max_stable_version.clone())
        } else {
            // When prereleases are not allowed, prefer max_stable_version.
            // If no stable version exists (crate only has prereleases), fall back to
            // the prerelease version rather than returning None - this allows users
            // to see that an update exists, even if it's a prerelease.
            max_stable_version.clone().or_else(|| max_version.clone())
        };

        Ok(VersionInfo {
            name: name.to_string(),
            latest: selected,
            latest_stable: max_stable_version,
        })
    }
}

#[derive(Debug, Deserialize)]
struct CratesIoResponse {
    #[serde(rename = "crate")]
    krate: CratesIoCrate,
}

#[derive(Debug, Deserialize)]
struct CratesIoCrate {
    max_version: Option<String>,
    max_stable_version: Option<String>,
}
