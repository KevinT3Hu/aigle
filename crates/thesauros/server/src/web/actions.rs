use crate::{
    AppState,
    auth::Session,
    error::{ApiError, ApiResult, Fault, invalid},
    library::validate_name,
};
use axum::{
    Form, Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;

pub fn router(state: AppState) -> Router {
    let forms = Router::new()
        .route("/actions/login", post(login))
        .route("/actions/logout", post(logout))
        .route("/actions/rating", post(rating))
        .route("/actions/import", post(import))
        .route("/actions/reindex", post(reindex))
        .route("/actions/job", post(job))
        .layer(DefaultBodyLimit::max(64 * 1024));
    let uploads =
        Router::new()
            .route("/actions/upload", post(upload))
            .layer(DefaultBodyLimit::max(
                state
                    .config
                    .max_upload_bytes
                    .saturating_mul(4)
                    .saturating_add(1024 * 1024)
                    .min(usize::MAX as u64) as usize,
            ));
    Router::new()
        .merge(forms)
        .merge(uploads)
        .route("/media/{id}/{kind}", get(media))
        .route("/assets/{name}", get(asset))
        .with_state(state)
}
fn denied() -> ApiError {
    ApiError(Fault::Forbidden.into())
}
async fn require(state: &AppState, headers: &HeaderMap, csrf: &str) -> ApiResult<Session> {
    if !state.origin_allowed(headers) {
        return Err(denied());
    }
    let current = state.session(headers).await?.ok_or_else(denied)?;
    if !state.csrf(&current, csrf) {
        return Err(denied());
    }
    Ok(current)
}
async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<crate::auth::Login>,
) -> ApiResult<Response> {
    if !state.origin_allowed(&headers) {
        return Ok((
            StatusCode::FORBIDDEN,
            "Cross-origin sign-in is not permitted",
        )
            .into_response());
    }
    if !state.login_limiter.lock().allow() {
        return Ok((
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "60")],
            "Too many sign-in attempts; try again in a minute",
        )
            .into_response());
    }
    if form.token.len() > 1024 || !state.check_token(&form.token) {
        return Ok(Redirect::to("/?login=failed").into_response());
    }
    let cookie = state.create_session().await?;
    Ok(([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response())
}
#[derive(Deserialize)]
struct Csrf {
    csrf: String,
}
async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<Csrf>,
) -> ApiResult<Response> {
    let current = require(&state, &headers, &form.csrf).await?;
    state
        .call(move |lib| {
            lib.db()?
                .execute("DELETE FROM sessions WHERE hash=?", [current.hash])?;
            Ok(())
        })
        .await?;
    Ok((
        [(header::SET_COOKIE, state.clear_cookie())],
        Redirect::to("/"),
    )
        .into_response())
}
#[derive(Deserialize)]
struct Rating {
    csrf: String,
    id: String,
    revision: String,
    rating: i8,
}
fn success(headers: &HeaderMap, message: String) -> Response {
    if headers.get("hx-request").is_some_and(|h| h == "true") {
        (
            [(
                "hx-trigger-after-settle",
                r#"{"libraryChanged":null,"jobsChanged":null}"#,
            )],
            Html(format!("<p>{message}</p>")),
        )
            .into_response()
    } else {
        Redirect::to("/").into_response()
    }
}
async fn rating(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<Rating>,
) -> ApiResult<Response> {
    require(&state, &headers, &form.csrf).await?;
    state
        .call(move |lib| lib.rate(&form.id, &format!("\"{}\"", form.revision), form.rating))
        .await?;
    Ok(success(&headers, "Rating saved.".into()))
}
#[derive(Deserialize)]
struct Import {
    csrf: String,
    path: std::path::PathBuf,
}
async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<Import>,
) -> ApiResult<Response> {
    require(&state, &headers, &form.csrf).await?;
    state.call(move |lib| lib.queue_import(&form.path)).await?;
    state.notify();
    Ok(success(&headers, "Import queued.".into()))
}
#[derive(Deserialize)]
struct PhotoAction {
    csrf: String,
    id: String,
}
async fn reindex(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<PhotoAction>,
) -> ApiResult<Response> {
    require(&state, &headers, &form.csrf).await?;
    state.call(move |lib| lib.reindex(&form.id)).await?;
    state.notify();
    Ok(success(&headers, "Reindex queued.".into()))
}
#[derive(Deserialize)]
struct JobAction {
    csrf: String,
    id: String,
    operation: String,
}
async fn job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<JobAction>,
) -> ApiResult<Response> {
    require(&state, &headers, &form.csrf).await?;
    let updated = state
        .call(move |lib| match form.operation.as_str() {
            "cancel" => lib.cancel(&form.id),
            "retry" => lib.retry(&form.id),
            _ => Err(invalid("Invalid job action")),
        })
        .await?;
    state.notify();
    let message = match updated.state.as_str() {
        "queued" => "Job queued.",
        "cancelled" => "Job cancelled.",
        _ if updated.cancel_requested => "Cancellation requested.",
        _ => "Job already finished.",
    };
    Ok(success(&headers, message.into()))
}
async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<Response> {
    if !state.origin_allowed(&headers) {
        return Err(denied());
    }
    let current = state.session(&headers).await?.ok_or_else(denied)?;
    let _permit = state
        .uploads
        .clone()
        .try_acquire_owned()
        .map_err(|_| anyhow::Error::from(Fault::QueueFull))?;
    let total = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| invalid("Content-Length required"))?;
    state.call(|lib| lib.check_queue()).await?;
    let _reservation = state.reserve(total)?;
    let mut accepted = 0;
    let mut rejected = 0;
    let mut validated = false;
    let mut count = 0;
    while let Some(field) =
        tokio::time::timeout(std::time::Duration::from_secs(120), multipart.next_field())
            .await
            .map_err(|_| invalid("Upload timed out"))?
            .map_err(|_| invalid("Invalid multipart upload"))?
    {
        if field.name() == Some("csrf") {
            let token = field
                .text()
                .await
                .map_err(|_| invalid("Invalid CSRF field"))?;
            if !state.csrf(&current, &token) {
                return Err(denied());
            }
            validated = true;
            continue;
        }
        if !validated {
            return Err(denied());
        }
        if field.name() != Some("files") {
            return Err(invalid("Unexpected upload field").into());
        }
        count += 1;
        if count > 100 {
            return Err(invalid("At most 100 files per batch").into());
        }
        let name = field.file_name().unwrap_or("").to_owned();
        if validate_name(&name).is_err() {
            rejected += 1;
            continue;
        }
        let (temp, id, _) = crate::api::spool(&state, &name, field).await?;
        state
            .call(move |lib| lib.accept_upload(temp, &name, &id))
            .await?;
        accepted += 1;
        state.notify();
    }
    if !validated {
        return Err(denied());
    }
    let mut message = format!(
        "{accepted} {} queued.",
        if accepted == 1 { "file" } else { "files" }
    );
    if rejected > 0 {
        message.push_str(&format!(
            " {rejected} {} rejected: invalid RAW filename.",
            if rejected == 1 { "file" } else { "files" }
        ));
    }
    Ok(success(&headers, message))
}
async fn media(
    State(state): State<AppState>,
    Path((id, kind)): Path<(String, String)>,
    request: Request,
) -> ApiResult<Response> {
    if state.session(request.headers()).await?.is_none() {
        return Ok((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Session expired; sign in again"})),
        )
            .into_response());
    }
    let preview = match kind.as_str() {
        "raw" => false,
        "preview" => true,
        _ => return Err(Fault::NotFound.into()),
    };
    crate::api::download(state, id, preview, request).await
}
async fn asset(Path(name): Path<String>) -> Response {
    let (bytes, mime): (&'static [u8], &str) =
        if Some(name.as_str()) == super::CSS_URL.strip_prefix("/assets/") {
            (
                include_bytes!("../../assets/app.css"),
                "text/css; charset=utf-8",
            )
        } else if Some(name.as_str()) == super::JS_URL.strip_prefix("/assets/") {
            (
                include_bytes!("../../assets/app.js"),
                "text/javascript; charset=utf-8",
            )
        } else if Some(name.as_str()) == super::ICON_URL.strip_prefix("/assets/") {
            (include_bytes!("../../assets/icon.svg"), "image/svg+xml")
        } else if name == "htmx-2.0.11.min.js" {
            (
                include_bytes!("../../assets/htmx.min.js"),
                "text/javascript; charset=utf-8",
            )
        } else {
            return StatusCode::NOT_FOUND.into_response();
        };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        Body::from(bytes),
    )
        .into_response()
}
