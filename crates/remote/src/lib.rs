//! Checkout immutable RAWs, edit them locally in Aigle, and synchronize XMP sidecars.
use anyhow::{Context, Result, bail};
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub struct Remote {
    base: String,
    token: String,
    http: Client,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Photo {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub captured: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub rating: i8,
    pub revision: String,
    pub error: Option<String>,
}
#[derive(Deserialize, Serialize)]
pub struct Page {
    pub photos: Vec<Photo>,
    pub limit: u32,
    pub offset: u32,
}
#[derive(Serialize, Deserialize)]
struct Checkout {
    server: String,
    id: String,
    name: String,
    revision: String,
}

fn checked(response: Response) -> Result<Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text()?;
    if status == StatusCode::PRECONDITION_FAILED {
        bail!(
            "Edit conflict: server sidecar changed. Preserve your local XMP and reconcile it with the server before retrying. {body}"
        );
    }
    bail!("Server returned {status}: {body}")
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().context("file has no parent")?)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    Ok(())
}
fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".."
}
impl Remote {
    pub fn new(base: &str, token: String) -> Result<Self> {
        let url = reqwest::Url::parse(base)?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
            "expected an HTTP(S) server URL"
        );
        anyhow::ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "server URL must not contain credentials, query, or fragment"
        );
        Ok(Self {
            base: base.trim_end_matches('/').into(),
            token,
            http: Client::builder()
                .timeout(Duration::from_secs(600))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }
    fn url(&self, path: &str) -> String {
        format!("{}/api/{path}", self.base)
    }
    pub fn list(&self, search: &str, limit: u32, offset: u32) -> Result<Page> {
        let response = self
            .http
            .get(self.url("photos"))
            .bearer_auth(&self.token)
            .query(&[
                ("q", search.to_string()),
                ("limit", limit.to_string()),
                ("offset", offset.to_string()),
            ])
            .send()?;
        Ok(checked(response)?.json()?)
    }
    pub fn photo(&self, id: &str) -> Result<Photo> {
        anyhow::ensure!(
            id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid photo ID"
        );
        Ok(checked(
            self.http
                .get(self.url(&format!("photos/{id}")))
                .bearer_auth(&self.token)
                .send()?,
        )?
        .json()?)
    }
    /// Refuses existing checkout directories so local edits cannot be overwritten.
    pub fn checkout(&self, id: &str, cache: &Path) -> Result<PathBuf> {
        let photo = self.photo(id)?;
        anyhow::ensure!(valid_name(&photo.name), "invalid filename from server");
        fs::create_dir_all(cache)?;
        let folder = cache.join(id);
        fs::create_dir(&folder)
            .context("checkout directory already exists; use it or choose another cache")?;
        let result = (|| {
            let raw = folder.join(&photo.name);
            let mut response = checked(
                self.http
                    .get(self.url(&format!("photos/{id}/raw")))
                    .bearer_auth(&self.token)
                    .send()?,
            )?;
            let mut tmp = tempfile::NamedTempFile::new_in(&folder)?;
            let mut h = blake3::Hasher::new();
            let mut buf = [0; 128 * 1024];
            loop {
                let n = response.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                tmp.write_all(&buf[..n])?;
                h.update(&buf[..n]);
            }
            anyhow::ensure!(
                h.finalize().to_hex().as_str() == id,
                "RAW checksum mismatch"
            );
            tmp.persist(&raw)?;
            let response = self
                .http
                .get(self.url(&format!("photos/{id}/sidecar")))
                .bearer_auth(&self.token)
                .send()?;
            let revision = if response.status() == StatusCode::NOT_FOUND {
                "none".to_owned()
            } else {
                let response = checked(response)?;
                let rev = response
                    .headers()
                    .get("ETag")
                    .context("missing sidecar ETag")?
                    .to_str()?
                    .trim_matches('"')
                    .to_owned();
                atomic(
                    &folder.join(format!("{}.xmp", photo.name)),
                    &response.bytes()?,
                )?;
                rev
            };
            let state = Checkout {
                server: self.base.clone(),
                id: id.to_owned(),
                name: photo.name,
                revision,
            };
            atomic(
                &folder.join(".thesauros.json"),
                &serde_json::to_vec_pretty(&state)?,
            )?;
            Ok(raw)
        })();
        // Keep partial downloads for inspection, never remove a directory with user edits.
        result
    }
    /// Open an existing checkout without replacing local work, or download it once.
    pub fn checkout_or_open(&self, id: &str, cache: &Path) -> Result<PathBuf> {
        let photo = self.photo(id)?;
        let folder = cache.join(id);
        let manifest = folder.join(".thesauros.json");
        if !manifest.exists() {
            return self.checkout(id, cache);
        }
        let state: Checkout = serde_json::from_slice(&fs::read(manifest)?)?;
        anyhow::ensure!(
            state.server == self.base && state.id == photo.id && valid_name(&state.name),
            "checkout identity mismatch"
        );
        let raw = folder.join(state.name);
        anyhow::ensure!(raw.is_file(), "cached RAW is missing");
        Ok(raw)
    }

    pub fn upload(&self, path: &Path) -> Result<Photo> {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("expected UTF-8 filename")?;
        let encoded: String = name.bytes().map(|c| format!("%{c:02X}")).collect();
        let response = self
            .http
            .post(self.url("upload"))
            .bearer_auth(&self.token)
            .header("X-Filename", encoded)
            .body(fs::File::open(path)?)
            .send()?;
        Ok(checked(response)?.json()?)
    }
    pub fn import(&self, path: &str) -> Result<serde_json::Value> {
        Ok(checked(
            self.http
                .post(self.url("import"))
                .bearer_auth(&self.token)
                .json(&serde_json::json!({"path":path}))
                .send()?,
        )?
        .json()?)
    }
    pub fn push(&self, folder: &Path) -> Result<()> {
        let manifest = folder.join(".thesauros.json");
        let mut state: Checkout = serde_json::from_slice(&fs::read(&manifest)?)?;
        anyhow::ensure!(
            state.server == self.base,
            "checkout belongs to another server"
        );
        anyhow::ensure!(valid_name(&state.name), "invalid checkout filename");
        self.photo(&state.id)?;
        let response = checked(
            self.http
                .put(self.url(&format!("photos/{}/sidecar", state.id)))
                .bearer_auth(&self.token)
                .header("If-Match", format!("\"{}\"", state.revision))
                .header("Content-Type", "application/rdf+xml")
                .body(fs::read(folder.join(format!("{}.xmp", state.name)))?)
                .send()?,
        )?;
        state.revision = response.json::<serde_json::Value>()?["revision"]
            .as_str()
            .context("missing revision")?
            .into();
        atomic(&manifest, &serde_json::to_vec_pretty(&state)?)?;
        Ok(())
    }
}

/// Read the server recorded in a checkout; the token is supplied separately and never persisted.
pub fn checkout_server(folder: &Path) -> Result<String> {
    let state: Checkout = serde_json::from_slice(&fs::read(folder.join(".thesauros.json"))?)?;
    Ok(state.server)
}
