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
    base_url: String,
    rate_limit_delay: Duration,
}

impl CratesIoClient {
    pub fn new() -> Result<Self> {
        let http = Client::builder().user_agent("cargo-upkeep").build()?;

        Ok(Self {
            http,
            cache: Arc::new(Mutex::new(HashMap::new())),
            // crates.io rate limit: 1 request per second
            limiter: Arc::new(Semaphore::new(1)),
            base_url: "https://crates.io/api/v1".to_string(),
            rate_limit_delay: Duration::from_secs(1),
        })
    }

    #[cfg(test)]
    fn new_with_base_url(base_url: String, rate_limit_delay: Duration) -> Result<Self> {
        let http = Client::builder().user_agent("cargo-upkeep").build()?;

        Ok(Self {
            http,
            cache: Arc::new(Mutex::new(HashMap::new())),
            limiter: Arc::new(Semaphore::new(1)),
            base_url,
            rate_limit_delay,
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
        sleep(self.rate_limit_delay).await;

        let url = format!("{}/crates/{name}", self.base_url);
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

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::GET;
    use httpmock::MockServer;
    use serde_json::json;

    fn test_client(base_url: String) -> CratesIoClient {
        CratesIoClient::new_with_base_url(base_url, Duration::from_secs(0))
            .expect("client")
    }

    #[tokio::test]
    async fn fetch_latest_versions_prefers_prerelease_when_allowed() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/crates/serde");
            then.status(200).json_body(json!({
                "crate": {
                    "max_version": "2.0.0-beta.1",
                    "max_stable_version": "1.0.190"
                }
            }));
        });

        let client = test_client(server.url(""));
        let result = client
            .fetch_latest_versions(&vec!["serde".to_string()], true)
            .await
            .expect("fetch");

        let info = result.get("serde").expect("serde info");
        assert_eq!(info.latest.as_deref(), Some("2.0.0-beta.1"));
        assert_eq!(info.latest_stable.as_deref(), Some("1.0.190"));
        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn fetch_latest_versions_prefers_stable_when_prerelease_not_allowed() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/crates/tokio");
            then.status(200).json_body(json!({
                "crate": {
                    "max_version": "2.0.0-beta.1",
                    "max_stable_version": "1.35.1"
                }
            }));
        });

        let client = test_client(server.url(""));
        let result = client
            .fetch_latest_versions(&vec!["tokio".to_string()], false)
            .await
            .expect("fetch");

        let info = result.get("tokio").expect("tokio info");
        assert_eq!(info.latest.as_deref(), Some("1.35.1"));
        assert_eq!(info.latest_stable.as_deref(), Some("1.35.1"));
        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn fetch_latest_versions_falls_back_when_versions_missing() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/crates/empty");
            then.status(200).json_body(json!({
                "crate": {
                    "max_version": null,
                    "max_stable_version": null
                }
            }));
        });

        let client = test_client(server.url(""));
        let result = client
            .fetch_latest_versions(&vec!["empty".to_string()], true)
            .await
            .expect("fetch");

        let info = result.get("empty").expect("empty info");
        assert!(info.latest.is_none());
        assert!(info.latest_stable.is_none());
        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn fetch_latest_versions_uses_cache() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/crates/cached");
            then.status(200).json_body(json!({
                "crate": {
                    "max_version": "1.2.3",
                    "max_stable_version": "1.2.3"
                }
            }));
        });

        let client = test_client(server.url(""));
        let names = vec!["cached".to_string()];

        let first = client.fetch_latest_versions(&names, false).await.unwrap();
        assert_eq!(first.get("cached").unwrap().latest.as_deref(), Some("1.2.3"));

        let second = client.fetch_latest_versions(&names, false).await.unwrap();
        assert_eq!(second.get("cached").unwrap().latest.as_deref(), Some("1.2.3"));

        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn fetch_latest_versions_handles_404_response() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/crates/nonexistent");
            then.status(404).body("Not Found");
        });

        let client = test_client(server.url(""));
        let result = client
            .fetch_latest_versions(&vec!["nonexistent".to_string()], false)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code(), crate::core::error::ErrorCode::Http);
        assert!(err.to_string().contains("HTTP error"));
        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn fetch_latest_versions_handles_500_response() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/crates/broken");
            then.status(500).body("Internal Server Error");
        });

        let client = test_client(server.url(""));
        let result = client
            .fetch_latest_versions(&vec!["broken".to_string()], false)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code(), crate::core::error::ErrorCode::Http);
        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn fetch_latest_versions_handles_invalid_json() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/crates/badjson");
            then.status(200).body("not valid json");
        });

        let client = test_client(server.url(""));
        let result = client
            .fetch_latest_versions(&vec!["badjson".to_string()], false)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code(), crate::core::error::ErrorCode::Json);
        assert!(err.to_string().contains("failed to parse JSON"));
        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn fetch_latest_versions_handles_network_error() {
        // Use a port that is not listening to simulate network error
        let client = test_client("http://127.0.0.1:1".to_string());
        let result = client
            .fetch_latest_versions(&vec!["anypackage".to_string()], false)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code(), crate::core::error::ErrorCode::Http);
        assert!(err.to_string().contains("failed to fetch crate info"));
    }
}
