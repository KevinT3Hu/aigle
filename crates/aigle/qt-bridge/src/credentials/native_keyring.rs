use super::*;

mod native {
    use super::*;
    pub(super) fn entry(endpoint: &str, provider: &str) -> Result<(keyring::Entry, String)> {
        let (service, endpoint) = native_identity(endpoint, provider)?;
        Ok((
            keyring::Entry::new(&service, "endpoint-keys")
                .context("Cannot open native credential store")?,
            endpoint,
        ))
    }
    pub(super) fn read(
        entry: &keyring::Entry,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        match entry.get_password() {
            Ok(value) => {
                serde_json::from_str(&value).context("Invalid stored provider credentials")
            }
            Err(keyring::Error::NoEntry) => Ok(Default::default()),
            Err(error) => Err(error).context("Cannot read native credentials"),
        }
    }
    pub(super) fn delete(entry: &keyring::Entry) -> Result<()> {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error).context("Cannot remove native credentials"),
        }
    }
}

pub fn save(endpoint: &str, provider: &str, key: Option<&str>) -> Result<()> {
    if let Some(key) = key {
        ensure!(!key.is_empty(), "API key required");
    }
    let (entry, endpoint) = native::entry(endpoint, provider)?;
    let mut keys = native::read(&entry)?;
    match key {
        Some(key) => {
            keys.insert(endpoint, key.to_owned());
        }
        None => {
            keys.remove(&endpoint);
        }
    }
    if keys.is_empty() {
        native::delete(&entry)
    } else {
        entry
            .set_password(&serde_json::to_string(&keys)?)
            .context("Cannot store native credentials")
    }
}

pub fn load(endpoint: &str, provider: &str) -> Result<Option<String>> {
    let (entry, endpoint) = native::entry(endpoint, provider)?;
    Ok(native::read(&entry)?.remove(&endpoint))
}

pub fn remove_provider(endpoint: &str, provider: &str) -> Result<()> {
    native::delete(&native::entry(endpoint, provider)?.0)
}
