#![allow(dead_code)]

use anyhow::{Context, Result};
use crates_index::GitIndex;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub name: String,
    pub latest: Option<String>,
    pub latest_stable: Option<String>,
}

#[derive(Clone)]
pub struct CratesIoClient {
    index: Arc<Mutex<GitIndex>>,
    http: Client,
    cache: Arc<Mutex<HashMap<String, VersionInfo>>>,
    limiter: Arc<Semaphore>,
}

impl CratesIoClient {
    pub fn new() -> Result<Self> {
        let index = GitIndex::new_cargo_default()?;
        let http = Client::builder().user_agent("cargo-upkeep").build()?;

        Ok(Self {
            index: Arc::new(Mutex::new(index)),
            http,
            cache: Arc::new(Mutex::new(HashMap::new())),
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
            let info = self.fetch_one(&name, allow_prerelease).await?;
            results.insert(name.clone(), info.clone());
            let mut cache = self.cache.lock().await;
            cache.insert(name, info);
        }

        Ok(results)
    }

    async fn fetch_one(&self, name: &str, allow_prerelease: bool) -> Result<VersionInfo> {
        if let Some(info) = self.fetch_from_index(name, allow_prerelease).await? {
            return Ok(info);
        }

        self.fetch_from_api(name, allow_prerelease).await
    }

    async fn fetch_from_index(
        &self,
        name: &str,
        allow_prerelease: bool,
    ) -> Result<Option<VersionInfo>> {
        let index = self.index.lock().await;
        let krate = match index.crate_(name) {
            Some(krate) => krate,
            None => return Ok(None),
        };

        let mut latest: Option<Version> = None;
        let mut latest_stable: Option<Version> = None;

        for version in krate.versions() {
            if version.is_yanked() {
                continue;
            }

            let parsed = match Version::parse(version.version()) {
                Ok(parsed) => parsed,
                Err(_) => continue,
            };

            if allow_prerelease && latest.as_ref().map_or(true, |current| parsed > *current) {
                latest = Some(parsed.clone());
            }

            if parsed.pre.is_empty()
                && latest_stable
                    .as_ref()
                    .map_or(true, |current| parsed > *current)
            {
                latest_stable = Some(parsed);
            }
        }

        let latest = latest.map(|v| v.to_string());
        let latest_stable = latest_stable.map(|v| v.to_string());

        Ok(Some(VersionInfo {
            name: name.to_string(),
            latest: if allow_prerelease {
                latest
            } else {
                latest_stable.clone()
            },
            latest_stable,
        }))
    }

    async fn fetch_from_api(&self, name: &str, allow_prerelease: bool) -> Result<VersionInfo> {
        let _permit = self.limiter.acquire().await?;
        let url = format!("https://crates.io/api/v1/crates/{name}");
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("failed to fetch crate info from {url}"))?;
        let payload: CratesIoResponse = response
            .error_for_status()
            .with_context(|| format!("HTTP error fetching {name} from crates.io"))?
            .json()
            .await
            .with_context(|| format!("failed to parse JSON response for {name}"))?;
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
