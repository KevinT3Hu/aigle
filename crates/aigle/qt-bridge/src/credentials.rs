//! Credentials stay in the desktop keyring; bridge replies never contain secrets.
use anyhow::{Context, Result, ensure};

fn attributes(endpoint: &str, provider: &str) -> Result<Vec<(&'static str, String)>> {
    let endpoint = aigle_agent::agent::endpoint_base(endpoint)?;
    Ok(vec![
        ("application", "aigle-qt".into()),
        ("purpose", "agent-api-key".into()),
        (
            "provider",
            if provider.is_empty() {
                "default"
            } else {
                provider
            }
            .into(),
        ),
        ("endpoint", endpoint),
    ])
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod secret_service;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use secret_service::{save, load, remove_provider};

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "windows"
)))]
pub fn remove_provider(_endpoint: &str, _provider: &str) -> Result<()> {
    Ok(())
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "windows"
)))]
pub fn save(_endpoint: &str, _provider: &str, _key: Option<&str>) -> Result<()> {
    if _key.is_none() {
        return Ok(());
    }
    anyhow::bail!(
        "Secure credential storage is unavailable on this platform; use a session key or AIGLE_AGENT_API_KEY"
    )
}
#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "windows"
)))]
pub fn load(_endpoint: &str, _provider: &str) -> Result<Option<String>> {
    Ok(None)
}

// Provider-wide entries allow endpoint cleanup without an unencrypted key index.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn native_identity(endpoint: &str, provider: &str) -> Result<(String, String)> {
    let attrs = attributes(endpoint, provider)?;
    let provider = &attrs
        .iter()
        .find(|(name, _)| *name == "provider")
        .unwrap()
        .1;
    let endpoint = attrs
        .iter()
        .find(|(name, _)| *name == "endpoint")
        .unwrap()
        .1
        .clone();
    Ok((
        format!(
            "aigle-qt-agent-{}",
            blake3::hash(provider.as_bytes()).to_hex()
        ),
        endpoint,
    ))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native_keyring;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use native_keyring::{save, load, remove_provider};

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    #[ignore = "writes disposable entries to the native credential store"]
    fn native_keyring_round_trip() {
        let provider = format!(
            "test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let endpoint = "https://aigle-keyring-test.invalid/v1";
        let other = "https://aigle-other-test.invalid/v1";
        let result = (|| -> Result<()> {
            assert_eq!(load(endpoint, &provider)?, None);
            save(endpoint, &provider, Some("first"))?;
            save(other, &provider, Some("second"))?;
            save(endpoint, &provider, Some("replacement"))?;
            assert_eq!(load(endpoint, &provider)?.as_deref(), Some("replacement"));
            assert_eq!(load(other, &provider)?.as_deref(), Some("second"));
            save(endpoint, &provider, None)?;
            assert_eq!(load(endpoint, &provider)?, None);
            assert_eq!(load(other, &provider)?.as_deref(), Some("second"));
            remove_provider(endpoint, &provider)?;
            assert_eq!(load(other, &provider)?, None);
            remove_provider(endpoint, &provider)?;
            Ok(())
        })();
        let cleanup = remove_provider(endpoint, &provider);
        result.unwrap();
        cleanup.unwrap();
    }
    #[test]
    fn credentials_are_scoped_to_endpoint_and_application() {
        assert!(attributes("  ", "default").is_err());
        assert_eq!(
            attributes(" https://one/v1 ", "default").unwrap(),
            attributes("https://one/v1", "default").unwrap()
        );
        assert_ne!(
            attributes("https://one/v1", "default").unwrap(),
            attributes("https://two/v1", "default").unwrap()
        );
        assert_eq!(
            attributes("https://one/v1/chat/completions", "default").unwrap(),
            attributes("https://one/v1/", "default").unwrap()
        );
        assert!(attributes("file:///tmp/key", "default").is_err());
        assert!(
            attributes("https://one/v1", "default")
                .unwrap()
                .contains(&("application", "aigle-qt".into()))
        );
    }
    #[test]
    fn native_identity_normalizes_endpoints_and_isolates_providers() {
        let (service, endpoint) = native_identity("https://one/v1/chat/completions", "").unwrap();
        assert_eq!(
            (service.clone(), endpoint),
            native_identity("https://one/v1/", "default").unwrap()
        );
        let (other_endpoint_service, _) = native_identity("https://two/v1", "default").unwrap();
        assert_eq!(service, other_endpoint_service);
        assert_ne!(
            service,
            native_identity("https://one/v1", "other").unwrap().0
        );
        assert!(native_identity("file:///tmp/key", "default").is_err());
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    #[ignore = "requires an isolated Secret Service session"]
    fn isolated_keyring_round_trip() {
        assert_eq!(
            std::env::var("AIGLE_TEST_ISOLATED_KEYRING").as_deref(),
            Ok("1")
        );
        let endpoint = "https://aigle-keyring-test.invalid/v1";
        let other = "https://aigle-other-test.invalid/v1";
        save(endpoint, "default", None).unwrap();
        save(other, "default", None).unwrap();
        assert_eq!(load(endpoint, "default").unwrap(), None);
        futures::executor::block_on(async {
            let keyring = oo7::Keyring::new().await.unwrap();
            keyring.unlock().await.unwrap();
            let mut attrs = attributes(endpoint, "default").unwrap();
            attrs.retain(|(key, _)| *key != "provider");
            keyring
                .create_item("Earlier Qt key", &attrs, b"earlier-qt-key", true)
                .await
                .unwrap();
        });
        assert_eq!(
            load(endpoint, "default").unwrap().as_deref(),
            Some("earlier-qt-key")
        );
        assert_eq!(load(endpoint, "other-provider").unwrap(), None);
        save(endpoint, "default", Some("first-test-secret")).unwrap();
        assert_eq!(
            load(endpoint, "default").unwrap().as_deref(),
            Some("first-test-secret")
        );
        assert_eq!(load(other, "default").unwrap(), None);
        save(endpoint, "default", Some("replacement-test-secret")).unwrap();
        assert_eq!(
            load(endpoint, "default").unwrap().as_deref(),
            Some("replacement-test-secret")
        );
        save(endpoint, "other-provider", Some("other-provider-key")).unwrap();
        assert_eq!(
            load(endpoint, "other-provider").unwrap().as_deref(),
            Some("other-provider-key")
        );
        save(endpoint, "default", None).unwrap();
        assert_eq!(load(endpoint, "default").unwrap(), None);
        assert_eq!(
            load(endpoint, "other-provider").unwrap().as_deref(),
            Some("other-provider-key")
        );
        save(other, "other-provider", Some("old-endpoint-key")).unwrap();
        save(endpoint, "default", Some("retained-default-key")).unwrap();
        remove_provider(endpoint, "other-provider").unwrap();
        assert_eq!(load(endpoint, "other-provider").unwrap(), None);
        assert_eq!(load(other, "other-provider").unwrap(), None);
        assert_eq!(
            load(endpoint, "default").unwrap().as_deref(),
            Some("retained-default-key")
        );
        save(endpoint, "default", None).unwrap();
    }
}
