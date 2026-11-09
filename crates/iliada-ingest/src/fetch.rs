//! Size-capped, hashing downloads (HTTPS or local files).

use std::path::Path;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct Fetcher {
    client: reqwest::Client,
    max_bytes: u64,
    allow_http: bool,
}

#[derive(Debug)]
pub struct Fetched {
    pub bytes: Vec<u8>,
    pub sha256: String,
}

impl Fetcher {
    pub fn new(max_bytes: u64, allow_http: bool) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("iliada-ingest/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(20))
            .build()?;
        Ok(Self { client, max_bytes, allow_http })
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Fetch `src`, which is an `https://` URL, a `file://` URL, or a local path.
    pub async fn fetch(&self, src: &str) -> Result<Fetched> {
        if let Some(path) = local_path(src) {
            let meta = tokio::fs::metadata(&path).await.with_context(|| format!("cannot stat {path}"))?;
            if meta.len() > self.max_bytes {
                bail!("{path} is {} bytes, over the {} byte limit", meta.len(), self.max_bytes);
            }
            let bytes = tokio::fs::read(&path).await?;
            let sha256 = hex::encode(Sha256::digest(&bytes));
            return Ok(Fetched { bytes, sha256 });
        }
        self.check_scheme(src)?;
        let resp = self.client.get(src).send().await?.error_for_status().with_context(|| format!("GET {src}"))?;
        if let Some(len) = resp.content_length()
            && len > self.max_bytes
        {
            bail!("{src} is {len} bytes, over the {} byte limit", self.max_bytes);
        }
        let mut hasher = Sha256::new();
        let mut bytes = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if (bytes.len() + chunk.len()) as u64 > self.max_bytes {
                bail!("{src} exceeded the {} byte limit while downloading", self.max_bytes);
            }
            hasher.update(&chunk);
            bytes.extend_from_slice(&chunk);
        }
        Ok(Fetched { bytes, sha256: hex::encode(hasher.finalize()) })
    }

    /// A cheap change detector for watch mode: the `ETag` or `Last-Modified`
    /// header from a HEAD request.
    pub async fn validator(&self, url: &str) -> Result<Option<String>> {
        self.check_scheme(url)?;
        let resp = self.client.head(url).send().await?.error_for_status()?;
        let h = resp.headers();
        Ok(h.get(reqwest::header::ETAG)
            .or_else(|| h.get(reqwest::header::LAST_MODIFIED))
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned))
    }

    fn check_scheme(&self, url: &str) -> Result<()> {
        if url.starts_with("https://") || (self.allow_http && url.starts_with("http://")) {
            Ok(())
        } else {
            bail!("refusing to fetch {url:?}: only https:// (or --allow-http) and local files are allowed")
        }
    }
}

fn local_path(src: &str) -> Option<String> {
    if let Some(p) = src.strip_prefix("file://") {
        return Some(p.to_owned());
    }
    (!src.contains("://") && Path::new(src).exists()).then(|| src.to_owned())
}
