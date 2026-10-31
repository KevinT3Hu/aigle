use std::{fs, os::unix::fs::PermissionsExt};
use thesauros::config::Config;
#[test]
fn validates_origin_and_resource_bounds_and_rejects_unknown_keys() {
    assert!(toml::from_str::<Config>("index_workres = 2").is_err());
    let mut config = Config {
        listen: "0.0.0.0:8787".parse().unwrap(),
        ..Config::default()
    };
    assert!(config.validate().is_err());
    for url in [
        "http://photos.example.com",
        "https://user:secret@photos.example.com",
        "https://photos.example.com/path",
        "https://photos.example.com/?secret=1",
    ] {
        config.public_url = Some(url.into());
        assert!(config.validate().is_err(), "{url}");
    }
    config.public_url = Some("https://photos.example.com/".into());
    config.validate().unwrap();
    assert_eq!(
        config.public_url.as_deref(),
        Some("https://photos.example.com")
    );
    assert!(config.secure_cookies());
    config.index_workers = 0;
    assert!(config.validate().is_err());
}
#[test]
fn credential_file_permissions_and_secret_shape_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    fs::write(&path, "test-token-with-at-least-32-characters\n").unwrap();
    let config = Config {
        token_file: Some(path.clone()),
        ..Config::default()
    };
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(config.token().is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        config.token().unwrap(),
        "test-token-with-at-least-32-characters"
    );
    fs::write(path, "short").unwrap();
    assert!(config.token().is_err());
}
