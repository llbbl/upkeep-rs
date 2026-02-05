#![allow(dead_code)]

use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::{sleep, Duration};

use crate::core::error::{ErrorCode, Result, UpkeepError};
#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub name: String,
    pub latest: Option<String>,
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
            let info = self.fetch_from_api(&name, allow_prerelease).await?;
            results.insert(name.clone(), info.clone());
            let mut cache = self.cache.lock().await;
            cache.insert(name, info);
        }

        Ok(results)
    }

    async fn fetch_from_api(&self, name: &str, allow_prerelease: bool) -> Result<VersionInfo> {
        let _permit = self.limiter.acquire().await?;
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

        // Rate limit: wait 1 second between requests
        sleep(Duration::from_secs(1)).await;

        let mut latest = payload.krate.max_version;
        let mut latest_stable = payload.krate.max_stable_version;

        if !allow_prerelease {
            if let Some(version) = latest.as_ref() {
                if !is_stable(version) {
                    latest = None;
                }
            }
        }

        if latest_stable.is_none() && !allow_prerelease {
            latest_stable = latest.clone();
        }

        let selected = if allow_prerelease {
            latest.clone().or_else(|| latest_stable.clone())
        } else {
            latest_stable.clone()
        };

        Ok(VersionInfo {
            name: name.to_string(),
            latest: selected,
            latest_stable,
        })
    }
}

fn is_stable(version: &str) -> bool {
    match Version::parse(version) {
        Ok(parsed) => parsed.pre.is_empty(),
        Err(_) => false, // Conservative: don't recommend unparseable versions as stable
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
