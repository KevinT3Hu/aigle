use crate::{
    AppState,
    error::{ApiResult, Fault, invalid},
    library::{Job, Photo, Search, validate_name},
};
use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::{
    path::{Path as FsPath, PathBuf},
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/photos", get(list))
        .route("/api/photos/{id}", get(detail))
        .route("/api/photos/{id}/raw", get(raw))
        .route("/api/photos/{id}/preview", get(preview))
        .route("/api/photos/{id}/sidecar", get(sidecar).put(write_sidecar))
        .route("/api/photos/{id}/reindex", post(reindex))
        .route("/api/upload", post(upload))
        .route("/api/import", post(import))
        .route("/api/jobs", get(jobs))
        .route("/api/jobs/{id}", get(job))
        .route("/api/jobs/{id}/cancel", post(cancel))
        .route("/api/jobs/{id}/retry", post(retry))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Router::new()
        .merge(api)
        .route(
            "/health/live",
            get(|| async { Json(json!({"status":"ok"})) }),
        )
        .route("/health/ready", get(ready))
        .with_state(state)
}
async fn authenticate(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if !state.bearer(request.headers()) {
        let allowed = state.login_limiter.lock().allow();
        return (if allowed{StatusCode::UNAUTHORIZED}else{StatusCode::TOO_MANY_REQUESTS},[("www-authenticate","Bearer"),("retry-after","60")],Json(json!({"error":if allowed{"Bearer token required"}else{"Too many authentication failures"},"code":"unauthorized"}))).into_response();
    }
    next.run(request).await
}
async fn ready(State(state): State<AppState>) -> Response {
    match state.call(|lib| lib.ready()).await {
        Ok(()) => (StatusCode::OK, Json(json!({"status":"ready"}))).into_response(),
        Err(error) => {
            tracing::warn!(error=?error,"readiness check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"status":"unavailable"})),
            )
                .into_response()
        }
    }
}
async fn list(
    State(state): State<AppState>,
    Query(search): Query<Search>,
) -> ApiResult<Json<serde_json::Value>> {
    let limit = search.limit();
    let offset = search.offset();
    let photos = state.call(move |lib| lib.list(&search)).await?;
    Ok(Json(json!({"photos":photos,"limit":limit,"offset":offset})))
}
async fn detail(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Photo>> {
    Ok(Json(state.call(move |lib| lib.photo(&id)).await?))
}
async fn raw(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> ApiResult<Response> {
    download(state, id, false, request).await
}
async fn preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> ApiResult<Response> {
    download(state, id, true, request).await
}
pub async fn download(
    state: AppState,
    id: String,
    preview: bool,
    mut request: Request,
) -> ApiResult<Response> {
    let (photo, path, etag) = state
        .call(move |lib| {
            let p = lib.photo(&id)?;
            let path = if preview {
                let path = lib.root.join("previews").join(format!("{id}.jpg"));
                if !path.is_file() {
                    return Err(Fault::NotFound.into());
                }
                path
            } else {
                lib.check_source(&p)?;
                p.path.clone()
            };
            let etag = if preview {
                format!("\"{}\"", crate::library::hash(&path)?)
            } else {
                format!("\"{}\"", p.id)
            };
            Ok((p, path, etag))
        })
        .await?;
    if request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|v| v.trim().trim_start_matches("W/") == etag || v.trim() == "*")
        })
    {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
    }
    if request.headers().contains_key(header::IF_NONE_MATCH) {
        request.headers_mut().remove(header::IF_MODIFIED_SINCE);
    }
    if request
        .headers()
        .get(header::IF_RANGE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v != etag)
    {
        request.headers_mut().remove(header::RANGE);
    }
    // ServeFile handles byte ranges, HEAD, If-Modified-Since, and bounded streaming.
    let response = tower_http::services::ServeFile::new(path)
        .oneshot(request)
        .await
        .unwrap();
    let mut response = response.map(Body::new);
    response
        .headers_mut()
        .insert(header::ETAG, etag.parse().unwrap());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        if preview {
            "image/jpeg"
        } else {
            "application/octet-stream"
        }
        .parse()
        .unwrap(),
    );
    if !preview {
        let encoded =
            percent_encoding::utf8_percent_encode(&photo.name, percent_encoding::NON_ALPHANUMERIC)
                .to_string();
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"photo.raw\"; filename*=UTF-8''{encoded}")
                .parse()
                .unwrap(),
        );
    }
    Ok(response)
}
async fn sidecar(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let (bytes, rev) = state.call(move |lib| lib.read_sidecar(&id)).await?;
    Ok(match bytes {
        Some(bytes) => (
            [
                (header::CONTENT_TYPE, "application/rdf+xml".to_string()),
                (header::ETAG, format!("\"{rev}\"")),
            ],
            bytes,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            [(header::ETAG, "\"none\"")],
            Json(json!({"error":"No sidecar","code":"no_sidecar"})),
        )
            .into_response(),
    })
}
async fn write_sidecar(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    bytes: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    let expected = headers
        .get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let rev = state
        .call(move |lib| lib.write_sidecar(&id, &expected, &bytes))
        .await?;
    Ok(Json(json!({"revision":rev})))
}
#[derive(Deserialize)]
struct Import {
    path: PathBuf,
}
async fn import(
    State(state): State<AppState>,
    Json(input): Json<Import>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let job = state.call(move |lib| lib.queue_import(&input.path)).await?;
    state.notify();
    Ok((StatusCode::ACCEPTED, Json(json!({"job":job}))))
}
async fn reindex(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    let job = state.call(move |lib| lib.reindex(&id)).await?;
    state.notify();
    Ok((StatusCode::ACCEPTED, Json(job)))
}
async fn jobs(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(json!({"jobs":state.call(|lib|lib.jobs()).await?})))
}
async fn job(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Job>> {
    Ok(Json(state.call(move |lib| lib.job(&id)).await?))
}
async fn cancel(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Job>> {
    Ok(Json(state.call(move |lib| lib.cancel(&id)).await?))
}
async fn retry(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Job>> {
    let job = state.call(move |lib| lib.retry(&id)).await?;
    state.notify();
    Ok(Json(job))
}

/// Stream a single file to a same-filesystem temporary file and hash it incrementally.
pub async fn spool<S, E>(
    state: &AppState,
    name: &str,
    stream: S,
) -> Result<(tempfile::NamedTempFile, String, u64)>
where
    S: Stream<Item = std::result::Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    validate_name(name)?;
    let suffix = format!(
        ".{}",
        FsPath::new(name).extension().unwrap().to_string_lossy()
    );
    let temp = tempfile::Builder::new()
        .prefix("upload-")
        .suffix(&suffix)
        .tempfile_in(state.library.root.join("tmp"))?;
    let mut file = tokio::fs::File::from_std(temp.reopen()?);
    let mut stream = stream;
    let mut received = 0u64;
    let mut hasher = blake3::Hasher::new();
    loop {
        let next = tokio::time::timeout(Duration::from_secs(120), stream.next())
            .await
            .map_err(|_| invalid("Upload timed out"))?;
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|_| invalid("Incomplete upload"))?;
        received = received
            .checked_add(chunk.len() as u64)
            .context("upload size overflow")?;
        if received > state.config.max_upload_bytes {
            return Err(Fault::TooLarge.into());
        }
        file.write_all(&chunk).await?;
        hasher.update(&chunk);
    }
    if received == 0 {
        return Err(invalid("Empty RAW upload"));
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    Ok((temp, hasher.finalize().to_hex().to_string(), received))
}
async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<Photo>)> {
    let _permit = state
        .uploads
        .clone()
        .try_acquire_owned()
        .map_err(|_| anyhow::Error::from(Fault::QueueFull))?;
    let name = headers
        .get("x-filename")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| invalid("X-Filename required"))?;
    let name = percent_encoding::percent_decode_str(name)
        .decode_utf8()
        .map_err(|_| invalid("Invalid UTF-8 filename"))?
        .into_owned();
    validate_name(&name)?;
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| invalid("Content-Length required"))?;
    if declared == 0 || declared > state.config.max_upload_bytes {
        return Err(Fault::TooLarge.into());
    }
    state.call(|lib| lib.check_queue()).await?;
    let _reservation = state.reserve(declared)?;
    let (temp, id, received) = tokio::time::timeout(
        Duration::from_secs(900),
        spool(&state, &name, body.into_data_stream()),
    )
    .await
    .map_err(|_| invalid("Upload exceeded 15 minutes"))??;
    if received != declared {
        return Err(invalid("Content-Length mismatch").into());
    }
    let photo = state
        .call(move |lib| lib.accept_upload(temp, &name, &id))
        .await?;
    state.notify();
    Ok((StatusCode::CREATED, Json(photo)))
}
