//! Provider/model preferences and platform-keyring credentials.
use gpui::{App, BorrowAppContext, Global, Task};
use serde::{Deserialize, Serialize};

const LEGACY_SERVICE: &str = "aigle://agent-model-api-key";

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(skip)]
    pub api_key: String,
    #[serde(skip)]
    legacy_key: bool,
}
impl Provider {
    pub fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            id: format!(
                "{time:x}-{:x}",
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ),
            ..Default::default()
        }
    }
    pub fn service(&self) -> String {
        format!(
            "aigle://agent-provider/{}",
            blake3::hash(self.id.as_bytes())
        )
    }
    pub fn restore_credentials(&mut self, loaded: &Provider) {
        self.api_key = loaded.api_key.clone();
        self.legacy_key = loaded.legacy_key;
    }
    pub fn add_model(&mut self, model: &str) {
        let model = model.trim();
        if !model.is_empty() && !self.models.iter().any(|m| m == model) {
            self.models.push(model.into());
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(from = "StoredSettings")]
pub struct ModelSettings {
    pub providers: Vec<Provider>,
    pub active_provider: Option<String>,
    pub active_model: Option<String>,
}
impl Global for ModelSettings {}
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredSettings {
    Current {
        providers: Vec<Provider>,
        #[serde(default)]
        active_provider: Option<String>,
        #[serde(default)]
        active_model: Option<String>,
    },
    Legacy {
        #[serde(default)]
        endpoint: String,
        #[serde(default)]
        model: String,
    },
}
impl From<StoredSettings> for ModelSettings {
    fn from(stored: StoredSettings) -> Self {
        let mut settings = match stored {
            StoredSettings::Current {
                providers,
                active_provider,
                active_model,
            } => Self {
                providers,
                active_provider,
                active_model,
            },
            StoredSettings::Legacy { endpoint, model } if !endpoint.is_empty() => {
                let name = rig_reqwest::reqwest::Url::parse(&endpoint)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string))
                    .unwrap_or_else(|| "Provider".into());
                let mut provider = Provider {
                    id: "legacy".into(),
                    name,
                    endpoint,
                    ..Default::default()
                };
                provider.add_model(&model);
                Self {
                    providers: vec![provider],
                    active_provider: Some("legacy".into()),
                    active_model: Some(model),
                }
            }
            _ => Self::default(),
        };
        settings.repair_selection();
        settings
    }
}
#[derive(Clone)]
pub struct ActiveModel {
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
}
impl ModelSettings {
    pub fn load() -> Self {
        crate::config_dir()
            .and_then(|d| std::fs::read(d.join("agent.json")).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> anyhow::Result<()> {
        let dir =
            crate::config_dir().ok_or_else(|| anyhow::anyhow!("No configuration directory"))?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("agent.json"), serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
    pub fn selected_provider(&self) -> Option<&Provider> {
        self.providers
            .iter()
            .find(|p| Some(&p.id) == self.active_provider.as_ref())
    }
    pub fn active(&self) -> Option<ActiveModel> {
        let p = self.selected_provider()?;
        let model = self
            .active_model
            .as_ref()
            .filter(|m| p.models.contains(m))?;
        if p.endpoint.is_empty() {
            return None;
        }
        Some(ActiveModel {
            endpoint: p.endpoint.clone(),
            model: model.clone(),
            api_key: p.api_key.clone(),
        })
    }
    pub fn repair_selection(&mut self) {
        if self.selected_provider().is_none() {
            self.active_provider = self
                .providers
                .iter()
                .find(|p| !p.models.is_empty())
                .or(self.providers.first())
                .map(|p| p.id.clone());
        }
        let p = self.selected_provider();
        if !p.is_some_and(|p| {
            self.active_model
                .as_ref()
                .is_some_and(|m| p.models.contains(m))
        }) {
            self.active_model = p.and_then(|p| p.models.first().cloned());
        }
    }
    pub fn choose(&mut self, provider: &str, model: Option<&str>) -> anyhow::Result<()> {
        let p = self
            .providers
            .iter()
            .find(|p| p.id == provider)
            .ok_or_else(|| anyhow::anyhow!("Provider no longer exists"))?;
        let selected = match model {
            Some(m) if p.models.iter().any(|s| s == m) => Some(m.to_string()),
            Some(_) => anyhow::bail!("Model no longer exists"),
            None => p.models.first().cloned(),
        };
        self.active_provider = Some(provider.into());
        self.active_model = selected;
        Ok(())
    }
}
#[derive(Default)]
pub struct CredentialStatus {
    pub loading: bool,
    pub failed: bool,
}
impl Global for CredentialStatus {}
fn restored_key(
    endpoint: &str,
    entry: Option<(String, Vec<u8>)>,
) -> anyhow::Result<Option<String>> {
    entry
        .filter(|(provider, _)| provider == endpoint)
        .map(|(_, bytes)| String::from_utf8(bytes).map_err(anyhow::Error::from))
        .transpose()
}
pub fn init(cx: &mut App) {
    let settings = ModelSettings::load();
    let reads: Vec<_> = settings
        .providers
        .iter()
        .map(|p| {
            (
                p.clone(),
                cx.read_credentials(&p.service()),
                (p.id == "legacy").then(|| cx.read_credentials(LEGACY_SERVICE)),
            )
        })
        .collect();
    cx.set_global(CredentialStatus {
        loading: !reads.is_empty(),
        failed: false,
    });
    cx.set_global(settings);
    cx.spawn(async move |cx| {
        let mut loaded = Vec::new();
        let mut failed = false;
        for (provider, read, legacy) in reads {
            let mut result = read.await;
            let mut legacy_key = false;
            if matches!(result, Ok(None))
                && let Some(read) = legacy
            {
                result = read.await;
                legacy_key = matches!(result, Ok(Some(_)));
            }
            match result.and_then(|entry| restored_key(&provider.endpoint, entry)) {
                Ok(key) => loaded.push((provider.id, key.unwrap_or_default(), legacy_key)),
                Err(_) => failed = true,
            }
        }
        let _ = cx.update(|cx| {
            cx.update_global::<ModelSettings, _>(|settings, _| {
                for (id, key, legacy) in loaded {
                    if let Some(p) = settings.providers.iter_mut().find(|p| p.id == id) {
                        p.api_key = key;
                        p.legacy_key = legacy;
                    }
                }
            });
            cx.update_global::<CredentialStatus, _>(|s, _| {
                s.loading = false;
                s.failed = failed;
            });
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Planned operations are started sequentially, so a migration never deletes
/// the legacy key before the replacement has been stored successfully.
pub enum CredentialChange {
    Write {
        service: String,
        endpoint: String,
        key: String,
    },
    Delete {
        service: String,
    },
}
impl CredentialChange {
    pub fn start(self, cx: &App) -> Task<anyhow::Result<()>> {
        match self {
            Self::Write {
                service,
                endpoint,
                key,
            } => cx.write_credentials(&service, &endpoint, key.as_bytes()),
            Self::Delete { service } => cx.delete_credentials(&service),
        }
    }
}
pub fn credential_changes(
    next: &ModelSettings,
    previous: &ModelSettings,
    clear: &std::collections::BTreeSet<String>,
) -> Vec<CredentialChange> {
    let mut writes = Vec::new();
    let mut deletes = Vec::new();
    for provider in &next.providers {
        let old = previous.providers.iter().find(|p| p.id == provider.id);
        let changed = clear.contains(&provider.id)
            || old.is_none_or(|p| {
                p.api_key != provider.api_key || p.endpoint != provider.endpoint || p.legacy_key
            });
        if changed {
            if provider.api_key.is_empty() {
                if old.is_some_and(|p| !p.api_key.is_empty()) || clear.contains(&provider.id) {
                    deletes.push(CredentialChange::Delete {
                        service: provider.service(),
                    });
                }
            } else {
                writes.push(CredentialChange::Write {
                    service: provider.service(),
                    endpoint: provider.endpoint.clone(),
                    key: provider.api_key.clone(),
                });
            }
            if old.is_some_and(|p| p.legacy_key) {
                deletes.push(CredentialChange::Delete {
                    service: LEGACY_SERVICE.into(),
                });
            }
        }
    }
    for old in &previous.providers {
        if !next.providers.iter().any(|p| p.id == old.id) {
            deletes.push(CredentialChange::Delete {
                service: old.service(),
            });
            if old.id == "legacy" {
                deletes.push(CredentialChange::Delete {
                    service: LEGACY_SERVICE.into(),
                });
            }
        }
    }
    writes.extend(deletes);
    writes
}
pub fn clear_legacy_flags(settings: &mut ModelSettings) {
    for p in &mut settings.providers {
        p.legacy_key = false;
    }
}

pub fn fetch_models(endpoint: String, api_key: String) -> anyhow::Result<Vec<String>> {
    let base = crate::agent::endpoint_base(&endpoint)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let http = rig_reqwest::reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        let mut config = rig_core::providers::openai::OpenAIConfig::new(api_key);
        config.base_url = base;
        let list = config
            .connect(rig_reqwest::ReqwestClient::from(http))
            .list_models()
            .await?;
        let mut models: Vec<_> = list
            .data
            .into_iter()
            .map(|m| m.id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect();
        models.sort();
        models.dedup();
        Ok(models)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_updates_are_isolated_and_migrate_before_deleting_legacy_key() {
        let mut old: ModelSettings =
            serde_json::from_str(r#"{"endpoint":"https://a.test","model":"vision"}"#).unwrap();
        old.providers[0].api_key = "legacy-secret".into();
        old.providers[0].legacy_key = true;
        let changes = credential_changes(&old, &old, &Default::default());
        assert!(
            matches!(&changes[0],CredentialChange::Write {service,key,..} if service==&old.providers[0].service() && key=="legacy-secret")
        );
        assert!(
            matches!(&changes[1],CredentialChange::Delete {service} if service==LEGACY_SERVICE)
        );
        clear_legacy_flags(&mut old);
        assert!(credential_changes(&old, &old, &Default::default()).is_empty());
        let mut next = old.clone();
        let mut b = Provider::new();
        b.endpoint = "https://b.test".into();
        b.api_key = "b-secret".into();
        next.providers.push(b.clone());
        let changes = credential_changes(&next, &old, &Default::default());
        assert_eq!(changes.len(), 1);
        assert!(
            matches!(&changes[0],CredentialChange::Write {service,endpoint,..} if service==&b.service() && endpoint=="https://b.test")
        );
        let changes = credential_changes(&old, &next, &Default::default());
        assert!(matches!(&changes[0],CredentialChange::Delete {service} if service==&b.service()));
        let mut cleared = old.clone();
        cleared.providers[0].api_key.clear();
        assert!(
            matches!(&credential_changes(&cleared,&old,&Default::default())[0],CredentialChange::Delete {service} if service==&old.providers[0].service())
        );
    }
    #[test]
    fn discovers_models_using_rig_with_provider_endpoint_and_key() {
        use std::{
            io::{BufRead, BufReader, Write},
            net::TcpListener,
            time::Duration,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => panic!("model discovery request: {e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                headers.push(line);
            }
            assert!(headers[0].starts_with("GET /v1/models "));
            assert!(headers.iter().any(|s| {
                s.to_ascii_lowercase()
                    .starts_with("authorization: bearer test-key")
            }));
            let body = r#"{"object":"list","data":[{"id":"vision-b"},{"id":"vision-a"},{"id":"vision-b"}],"has_more":false}"#;
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let models = fetch_models(
            format!("http://{address}/v1/chat/completions"),
            "test-key".into(),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(models, vec!["vision-a", "vision-b"]);
    }
    #[test]
    fn migrates_legacy_and_keeps_keys_out_of_json() {
        let mut settings: ModelSettings =
            serde_json::from_str(r#"{"endpoint":"https://example.com/v1","model":"vision"}"#)
                .unwrap();
        assert_eq!(settings.active().unwrap().model, "vision");
        settings.providers[0].api_key = "secret-value".into();
        let saved = serde_json::to_string(&settings).unwrap();
        assert!(!saved.contains("secret-value"));
        assert!(!saved.contains("api_key"));
        let restored: ModelSettings = serde_json::from_str(&saved).unwrap();
        assert_eq!(
            restored.active().unwrap().endpoint,
            "https://example.com/v1"
        );
        assert!(restored.active().unwrap().api_key.is_empty());
    }
    #[test]
    fn provider_selection_never_mixes_models_or_credentials() {
        let mut a = Provider::new();
        a.endpoint = "https://a.test".into();
        a.api_key = "a-key".into();
        a.add_model("one");
        a.add_model("one");
        a.add_model("two");
        let mut b = Provider::new();
        b.endpoint = "https://b.test".into();
        b.api_key = "b-key".into();
        b.add_model("three");
        assert_ne!(a.service(), b.service());
        let mut settings = ModelSettings {
            providers: vec![a.clone(), b.clone()],
            ..Default::default()
        };
        settings.choose(&a.id, Some("two")).unwrap();
        assert_eq!(settings.active().unwrap().api_key, "a-key");
        settings.choose(&b.id, None).unwrap();
        assert_eq!(settings.active().unwrap().model, "three");
        assert_eq!(settings.active().unwrap().api_key, "b-key");
        assert!(settings.choose(&b.id, Some("one")).is_err());
        settings.providers.retain(|p| p.id != b.id);
        settings.repair_selection();
        assert_eq!(settings.active().unwrap().model, "one");
    }
    #[test]
    fn restores_keys_only_for_their_provider_endpoint() {
        let saved = Some(("https://a.test".into(), b"key".to_vec()));
        assert_eq!(
            restored_key("https://a.test", saved.clone())
                .unwrap()
                .as_deref(),
            Some("key")
        );
        assert_eq!(restored_key("https://b.test", saved).unwrap(), None);
        assert!(
            restored_key(
                "https://a.test",
                Some(("https://a.test".into(), vec![0xff]))
            )
            .is_err()
        );
    }
}
