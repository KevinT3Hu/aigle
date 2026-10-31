use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum Fault {
    #[error("The session or CSRF token was not accepted")]
    Forbidden,
    #[error("Upload exceeds the configured size limit")]
    TooLarge,
    #[error("{0}")]
    Invalid(String),
    #[error("Resource not found")]
    NotFound,
    #[error("The sidecar changed; download and reconcile it before retrying")]
    Conflict,
    #[error("The work queue is full; try again later")]
    QueueFull,
    #[error("Insufficient storage space")]
    StorageFull,
    #[error("The source file changed; reimport it before downloading")]
    SourceChanged,
}
#[derive(Debug)]
pub struct ApiError(pub anyhow::Error);
pub type ApiResult<T> = Result<T, ApiError>;
impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self(error)
    }
}
impl From<Fault> for ApiError {
    fn from(error: Fault) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self.0.downcast_ref::<Fault>() {
            Some(Fault::Forbidden) => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "The session or CSRF token was not accepted".into(),
            ),
            Some(Fault::TooLarge) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                "Upload exceeds the configured size limit".into(),
            ),
            Some(Fault::Invalid(message)) => {
                (StatusCode::BAD_REQUEST, "invalid_request", message.clone())
            }
            Some(Fault::NotFound) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Resource not found".into(),
            ),
            Some(Fault::Conflict) => (
                StatusCode::PRECONDITION_FAILED,
                "edit_conflict",
                "The sidecar changed; download and reconcile it before retrying".into(),
            ),
            Some(Fault::QueueFull) => (
                StatusCode::TOO_MANY_REQUESTS,
                "queue_full",
                "The work queue is full; try again later".into(),
            ),
            Some(Fault::StorageFull) => (
                StatusCode::INSUFFICIENT_STORAGE,
                "storage_full",
                "Insufficient storage space".into(),
            ),
            Some(Fault::SourceChanged) => (
                StatusCode::CONFLICT,
                "source_changed",
                "The source file changed; reimport it before downloading".into(),
            ),
            None if self.0.chain().any(|e| {
                e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.raw_os_error() == Some(28))
            }) =>
            {
                (
                    StatusCode::INSUFFICIENT_STORAGE,
                    "storage_full",
                    "Insufficient storage space".into(),
                )
            }
            None => {
                tracing::error!(error = ?self.0, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Internal server error; see server logs".into(),
                )
            }
        };
        let mut response = (status, Json(json!({"error":message,"code":code}))).into_response();
        if status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert("retry-after", "5".parse().unwrap());
        }
        response
    }
}
pub fn invalid(message: impl Into<String>) -> anyhow::Error {
    Fault::Invalid(message.into()).into()
}
