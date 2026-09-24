//! Model prices from the models.dev catalog, cached in the data dir so a restart without network
//! still prices every execution. The cache is refreshed once a day.

use ostra_core::pricing::{self, Catalog, MODELS_DEV_URL};
use std::path::{Path, PathBuf};
use std::time::Duration;

const REFRESH_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

/// Overrides the catalog URL; an empty value turns fetching off (tests, air-gapped machines).
const URL_ENV: &str = "OSTRA_MODELS_DEV_URL";

pub fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join("models-dev.json")
}

/// Install the cached catalog, if there is one, then keep it fresh in the background.
pub fn start(data_dir: &Path) {
    let path = cache_path(data_dir);
    if let Some(c) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| Catalog::from_models_dev(&t).ok())
    {
        pricing::install(c);
    }
    let url = std::env::var(URL_ENV).unwrap_or_else(|_| MODELS_DEV_URL.to_string());
    if url.is_empty() {
        return;
    }
    tokio::spawn(async move {
        loop {
            let wait = match age(&path) {
                Some(a) if a < REFRESH_EVERY => REFRESH_EVERY - a,
                _ => match refresh(&url, &path).await {
                    Ok(()) => REFRESH_EVERY,
                    Err(e) => {
                        tracing::warn!(
                            "fetching model prices from {url} failed, keeping the cached prices: {e}"
                        );
                        RETRY_AFTER
                    }
                },
            };
            tokio::time::sleep(wait).await;
        }
    });
}

fn age(path: &Path) -> Option<Duration> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()
}

async fn refresh(url: &str, path: &Path) -> anyhow::Result<()> {
    let text = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let catalog = Catalog::from_models_dev(&text)?;
    anyhow::ensure!(!catalog.is_empty(), "the catalog lists no priced models");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &text)?;
    std::fs::rename(&tmp, path)?;
    pricing::install(catalog);
    tracing::info!("model prices refreshed from {url}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refresh_caches_what_parses_and_keeps_the_old_file_otherwise() {
        let tmp = tempfile::tempdir().unwrap();
        let path = cache_path(tmp.path());
        let good =
            r#"{"anthropic":{"models":{"claude-sonnet-5":{"cost":{"input":2,"output":10}}}}}"#;
        let app = axum::Router::new()
            .route("/good", axum::routing::get(move || async move { good }))
            .route("/bad", axum::routing::get(|| async { "{}" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        refresh(&format!("{base}/good"), &path).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), good);
        assert!(refresh(&format!("{base}/bad"), &path).await.is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            good,
            "an empty catalog does not replace the cache"
        );
    }
}
