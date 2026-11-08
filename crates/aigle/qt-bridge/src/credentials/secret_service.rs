use super::*;

// The first Qt credential implementation had no provider attribute. Only the
// default profile may read/delete those entries; never match another profile.
async fn legacy_items(
    keyring: &oo7::Keyring,
    attributes: &[(&str, String)],
) -> Result<Vec<oo7::Item>> {
    let attributes: Vec<_> = attributes
        .iter()
        .filter(|(key, _)| *key != "provider")
        .cloned()
        .collect();
    let mut legacy = Vec::new();
    for item in keyring.search_items(&attributes).await? {
        if !item.attributes().await?.contains_key("provider") {
            legacy.push(item);
        }
    }
    Ok(legacy)
}

pub fn save(endpoint: &str, provider: &str, key: Option<&str>) -> Result<()> {
    let attributes = attributes(endpoint, provider)?;
    if let Some(key) = key {
        ensure!(!key.is_empty(), "API key required");
    }
    futures::executor::block_on(async {
        let keyring = oo7::Keyring::new()
            .await
            .context("Cannot open desktop keyring")?;
        keyring
            .unlock()
            .await
            .context("Cannot unlock desktop keyring")?;
        match key {
            Some(key) => keyring
                .create_item("Aigle Qt agent API key", &attributes, key.as_bytes(), true)
                .await
                .context("Cannot store API key")?,
            None => {
                keyring
                    .delete(&attributes)
                    .await
                    .context("Cannot remove API key")?;
                if provider.is_empty() || provider == "default" {
                    for item in legacy_items(&keyring, &attributes).await? {
                        item.delete()
                            .await
                            .context("Cannot remove earlier Qt API key")?;
                    }
                }
            }
        }
        Ok(())
    })
}

pub fn load(endpoint: &str, provider: &str) -> Result<Option<String>> {
    let attributes = attributes(endpoint, provider)?;
    futures::executor::block_on(async {
        let keyring = oo7::Keyring::new()
            .await
            .context("Cannot open desktop keyring")?;
        keyring
            .unlock()
            .await
            .context("Cannot unlock desktop keyring")?;
        let mut items = keyring
            .search_items(&attributes)
            .await
            .context("Cannot find API key")?;
        if items.is_empty() && (provider.is_empty() || provider == "default") {
            items = legacy_items(&keyring, &attributes).await?;
        }
        if let Some(item) = items.into_iter().next() {
            item.unlock().await.context("Cannot unlock API key")?;
            let secret = item.secret().await.context("Cannot read API key")?;
            return Ok(Some(
                String::from_utf8(secret.to_vec()).context("Invalid stored API key encoding")?,
            ));
        }
        Ok(None)
    })
}

pub fn remove_provider(endpoint: &str, provider: &str) -> Result<()> {
    let mut attrs = attributes(endpoint, provider)?;
    attrs.retain(|(key, _)| *key != "endpoint");
    futures::executor::block_on(async {
        let keyring = oo7::Keyring::new()
            .await
            .context("Cannot open desktop keyring")?;
        keyring
            .unlock()
            .await
            .context("Cannot unlock desktop keyring")?;
        keyring
            .delete(&attrs)
            .await
            .context("Cannot remove provider API keys")?;
        if provider.is_empty() || provider == "default" {
            for item in legacy_items(&keyring, &attributes(endpoint, provider)?).await? {
                item.delete()
                    .await
                    .context("Cannot remove earlier Qt API key")?;
            }
        }
        Ok(())
    })
}
