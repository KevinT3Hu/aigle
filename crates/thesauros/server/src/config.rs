use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    pub data: PathBuf,
    pub import_roots: Vec<PathBuf>,
    pub public_url: Option<String>,
    pub max_upload_bytes: u64,
    pub upload_concurrency: usize,
    pub index_workers: usize,
    pub parser_memory_bytes: u64,
    pub max_queued_jobs: u32,
    pub min_free_bytes: u64,
    pub session_ttl_seconds: u64,
    pub shutdown_seconds: u64,
    pub request_timeout_seconds: u64,
    pub token_file: Option<PathBuf>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8787".parse().unwrap(),
            data: "./thesauros-data".into(),
            import_roots: vec![],
            public_url: None,
            max_upload_bytes: 4 * 1024 * 1024 * 1024,
            upload_concurrency: 4,
            index_workers: 2,
            parser_memory_bytes: 2 * 1024 * 1024 * 1024,
            max_queued_jobs: 1000,
            min_free_bytes: 512 * 1024 * 1024,
            session_ttl_seconds: 12 * 60 * 60,
            shutdown_seconds: 330,
            request_timeout_seconds: 900,
            token_file: None,
        }
    }
}
impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut config: Self = if let Some(path) = path {
            toml::from_str(&std::fs::read_to_string(path).context("read server config")?)
                .context("parse server config")?
        } else {
            Self::default()
        };
        if let Some(value) = std::env::var_os("THESAUROS_DATA") {
            config.data = value.into();
        }
        if let Ok(value) = std::env::var("THESAUROS_LISTEN") {
            config.listen = value.parse().context("invalid THESAUROS_LISTEN")?;
        }
        if let Ok(value) = std::env::var("THESAUROS_PUBLIC_URL") {
            config.public_url = Some(value);
        }
        if let Some(value) = std::env::var_os("THESAUROS_TOKEN_FILE") {
            config.token_file = Some(value.into());
        }
        Ok(config)
    }
    pub fn validate(&mut self) -> Result<()> {
        for root in &self.import_roots {
            ensure!(
                root.is_dir(),
                "import root must be an existing directory: {}",
                root.display()
            );
            std::fs::read_dir(root)
                .with_context(|| format!("read import root {}", root.display()))?;
        }

        ensure!(
            (1..=16).contains(&self.upload_concurrency),
            "upload_concurrency must be 1 through 16"
        );
        ensure!(
            (256 * 1024 * 1024..=16 * 1024 * 1024 * 1024).contains(&self.parser_memory_bytes),
            "parser_memory_bytes must be 256 MiB through 16 GiB"
        );
        ensure!(
            (1..=8).contains(&self.index_workers),
            "index_workers must be 1 through 8"
        );
        ensure!(
            (1..=100_000).contains(&self.max_queued_jobs),
            "max_queued_jobs must be 1 through 100000"
        );
        ensure!(
            (1..=1024 * 1024 * 1024 * 1024).contains(&self.max_upload_bytes),
            "max_upload_bytes must be 1 byte through 1 TiB"
        );
        ensure!(
            (60..=30 * 86400).contains(&self.session_ttl_seconds),
            "session_ttl_seconds must be 60 seconds through 30 days"
        );
        ensure!(
            (1..=3600).contains(&self.shutdown_seconds),
            "shutdown_seconds must be 1 through 3600"
        );
        ensure!(
            (1..=86400).contains(&self.request_timeout_seconds),
            "request_timeout_seconds must be 1 through 86400"
        );
        if let Some(origin) = &self.public_url {
            let url = url::Url::parse(origin).context("invalid public_url")?;
            ensure!(
                url.scheme() == "https"
                    || (url.scheme() == "http"
                        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))),
                "public_url requires HTTPS except for localhost"
            );
            ensure!(
                url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
                    && url.path() == "/",
                "public_url must be an origin, without credentials or a path"
            );
            self.public_url = Some(url.origin().ascii_serialization());
        } else {
            ensure!(
                self.listen.ip().is_loopback(),
                "public_url is required when listening beyond loopback"
            );
        }
        Ok(())
    }
    pub fn secure_cookies(&self) -> bool {
        self.public_url
            .as_ref()
            .is_some_and(|url| url.starts_with("https://"))
    }
    pub fn token(&self) -> Result<String> {
        let token = if let Some(path) = &self.token_file {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(path).context("read token-file metadata")?;
            ensure!(
                meta.permissions().mode() & 0o077 == 0,
                "token file must not be accessible by group or other users (use chmod 600)"
            );
            std::fs::read_to_string(path)
                .context("read token file")?
                .trim()
                .to_owned()
        } else {
            std::env::var("THESAUROS_TOKEN")
                .context("Set THESAUROS_TOKEN_FILE or THESAUROS_TOKEN")?
        };
        ensure!(
            (32..=1024).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_graphic()),
            "access token must contain 32 through 1024 printable ASCII characters"
        );
        Ok(token)
    }
}
