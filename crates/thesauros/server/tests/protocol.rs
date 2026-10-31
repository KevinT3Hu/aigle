use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use thesauros::{AppState, config::Config};
use tower::ServiceExt;
const TOKEN: &str = "test-token-with-at-least-32-characters";
fn request(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap()
}
#[tokio::test]
async fn admission_is_held_until_response_bodies_are_dropped_and_health_stays_live() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path(), &[], TOKEN.into()).unwrap();
    let app = thesauros::router(state);
    let mut held = Vec::new();
    for _ in 0..128 {
        let response = app.clone().oneshot(request("/api/photos")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    assert_eq!(
        app.clone()
            .oneshot(request("/api/photos"))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.clone()
            .oneshot(request("/health/live"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    drop(held);
    assert_eq!(
        app.oneshot(request("/api/photos")).await.unwrap().status(),
        StatusCode::OK
    );
}
#[tokio::test]
async fn canonical_https_proxy_origin_uses_secure_host_cookies_and_hsts() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data: dir.path().into(),
        public_url: Some("https://photos.example.com".into()),
        ..Config::default()
    };
    let state = AppState::with_config(config, TOKEN.into()).unwrap();
    let app = thesauros::router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/actions/login")
                .header("host", "internal-upstream:8787")
                .header("origin", "https://photos.example.com")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("token={TOKEN}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with("__Host-thesauros="));
    assert!(cookie.contains("; Secure"));
    assert!(cookie.contains("HttpOnly"));
    assert_eq!(
        response.headers()["strict-transport-security"],
        "max-age=31536000"
    );
}
