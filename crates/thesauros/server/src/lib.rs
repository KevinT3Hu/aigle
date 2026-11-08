mod api;
#[cfg(windows)]
mod windows_permissions;
pub mod auth;
pub mod config;
pub mod error;
pub mod library;
pub mod web;
pub mod workers;

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use config::Config;
use futures::StreamExt;
use library::Library;
use parking_lot::Mutex;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio_util::sync::CancellationToken;
use topcoat::router::tower::{TowerRoute, TowerService};
use tracing::Instrument;

#[derive(Clone)]
pub struct AppState {
    pub library: Library,
    pub config: Arc<Config>,
    credential_hash: [u8; 32],
    pub uploads: Arc<tokio::sync::Semaphore>,
    calls: Arc<tokio::sync::Semaphore>,
    requests: Arc<tokio::sync::Semaphore>,
    pub reserved: Arc<AtomicU64>,
    pub login_limiter: Arc<Mutex<auth::LoginLimiter>>,
    pub stopping: CancellationToken,
    pub wake: Arc<tokio::sync::Notify>,
}
impl AppState {
    pub fn open(data: &std::path::Path, imports: &[PathBuf], token: String) -> Result<Self> {
        Self::with_config(
            Config {
                data: data.into(),
                import_roots: imports.to_vec(),
                ..Config::default()
            },
            token,
        )
    }
    pub fn with_config(mut config: Config, token: String) -> Result<Self> {
        config.validate()?;
        anyhow::ensure!(
            (32..=1024).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_graphic()),
            "access token must contain at least 32 characters"
        );
        let library = Library::with_config(config)?;
        let stopping = library.stopping.clone();
        Ok(Self {
            config: library.config.clone(),
            uploads: Arc::new(tokio::sync::Semaphore::new(
                library.config.upload_concurrency,
            )),
            library,
            credential_hash: *blake3::hash(token.as_bytes()).as_bytes(),
            calls: Arc::new(tokio::sync::Semaphore::new(32)),
            requests: Arc::new(tokio::sync::Semaphore::new(128)),
            reserved: Arc::new(AtomicU64::new(0)),
            login_limiter: Arc::new(Mutex::new(auth::LoginLimiter::default())),
            stopping,
            wake: Arc::new(tokio::sync::Notify::new()),
        })
    }
    pub async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Library) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self.calls.clone().acquire_owned().await?;
        let library = self.library.clone();
        let span = tracing::Span::current();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            span.in_scope(|| f(&library))
        })
        .await
        .context("catalog task failed")?
    }
    pub fn notify(&self) {
        self.wake.notify_waiters();
    }
}
/// Topcoat owns page routing and rendering. Tower adapters keep the JSON API
/// and streaming multipart/file handlers interoperable with Aigle's existing client.
pub fn router(state: AppState) -> Router {
    let api = api::router(state.clone());
    let actions = web::actions(state.clone());
    let mut builder = topcoat::router::Router::builder()
        .app_context(state.clone())
        .origin_policy(
            topcoat::router::OriginPolicy::new()
                .trust_origins(state.config.public_url.iter().cloned()),
        );
    if let Some(origin) = state.config.public_url.as_deref() {
        builder = builder.base_url(origin);
    }
    let topcoat = builder
        .page(web::home)
        .page(web::gallery_page)
        .page(web::jobs_page)
        .page(web::filters_page)
        .route(TowerRoute::any("/api/{*rest}", api.clone()))
        .route(TowerRoute::any("/actions/{*rest}", actions.clone()))
        .route(TowerRoute::any("/media/{*rest}", actions.clone()))
        .route(TowerRoute::any("/assets/{*rest}", actions.clone()))
        .route(TowerRoute::any("/health/{*rest}", api))
        .build();
    Router::new()
        .fallback_service(TowerService::new(topcoat))
        .layer(middleware::from_fn_with_state(state.clone(), operational))
        .with_state(state)
}
async fn operational(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    if state.stopping.is_cancelled() {
        return (StatusCode::SERVICE_UNAVAILABLE, "Server shutting down").into_response();
    }
    let permit = if request.uri().path().starts_with("/health/") {
        None
    } else {
        Some(match state.requests.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("retry-after", "2")],
                    "Server busy",
                )
                    .into_response();
            }
        })
    };
    let id = library::secret()[..24].to_owned();
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let start = Instant::now();
    request
        .headers_mut()
        .insert("x-request-id", id.parse().unwrap());
    let span = tracing::info_span!("http",request_id=%id,%method,%path);
    let mut response = match tokio::time::timeout(
        std::time::Duration::from_secs(state.config.request_timeout_seconds),
        next.run(request).instrument(span),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => (StatusCode::REQUEST_TIMEOUT, "Request timed out").into_response(),
    };
    let headers = response.headers_mut();
    headers.insert("x-request-id", id.parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("referrer-policy", "same-origin".parse().unwrap());
    headers.insert(
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()".parse().unwrap(),
    );
    headers.insert("content-security-policy","default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'".parse().unwrap());
    if state.config.secure_cookies() {
        headers.insert(
            "strict-transport-security",
            "max-age=31536000".parse().unwrap(),
        );
    }
    if !path.starts_with("/assets/") {
        headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    }
    tracing::info!(request_id=%id,%method,%path,status=response.status().as_u16(),duration_ms=start.elapsed().as_millis() as u64,"http request");
    let (parts, body) = response.into_parts();
    let stream = body.into_data_stream().map(move |chunk| {
        let _keep = &permit;
        chunk
    });
    Response::from_parts(parts, axum::body::Body::from_stream(stream))
}
pub struct Reservation {
    state: AppState,
    bytes: u64,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.state.reserved.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
impl AppState {
    pub fn reserve(&self, bytes: u64) -> Result<Reservation> {
        let free = fs2::available_space(&self.library.root)?;
        let result = self
            .reserved
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|total| total.saturating_add(self.config.min_free_bytes) <= free)
            });
        if result.is_err() {
            return Err(error::Fault::StorageFull.into());
        }
        Ok(Reservation {
            state: self.clone(),
            bytes,
        })
    }
}
pub fn worker_ops() -> Vec<(&'static str, aigle_isolate::Handler)> {
    let mut ops = aigle_library::WORKER_OPS.to_vec();
    ops.push(("server.inspect", |body, _| {
        let path: PathBuf = serde_json::from_slice(body)
            .map_err(|e| aigle_isolate::Failure::Failed(e.to_string()))?;
        let data =
            library::inspect(&path).map_err(|e| aigle_isolate::Failure::Failed(e.to_string()))?;
        aigle_isolate::Reply::new(&data)
    }));
    ops.push(("server.validate_xmp", |body, _| {
        let packet: String = serde_json::from_slice(body)
            .map_err(|e| aigle_isolate::Failure::Failed(e.to_string()))?;
        let rating = library::validate_xmp(&packet)
            .map_err(|e| aigle_isolate::Failure::Failed(e.to_string()))?;
        aigle_isolate::Reply::new(&rating)
    }));
    ops.push(("server.rate_xmp", |body, _| {
        let (packet, rating): (Option<String>, i8) = serde_json::from_slice(body)
            .map_err(|e| aigle_isolate::Failure::Failed(e.to_string()))?;
        let updated = library::update_rating(packet.as_deref(), rating)
            .map_err(|e| aigle_isolate::Failure::Failed(e.to_string()))?;
        aigle_isolate::Reply::new(&updated)
    }));
    ops
}
