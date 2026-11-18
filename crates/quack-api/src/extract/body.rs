//! The JSON body of a write (§14.1, §14.8): `Content-Type:
//! application/json` (else `415`), at most 256 KiB (else `413`), and JSON
//! that matches the route's request type (else `400 invalid_json`).

use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::{StatusCode, header};
use quack_text::api::ApiCode;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// A request body parsed as `T`.
#[derive(Debug)]
pub(crate) struct Body<T>(pub T);

fn is_json(content_type: &str) -> bool {
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    essence.eq_ignore_ascii_case("application/json")
}

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Body<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        let json = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(is_json);
        if !json {
            return Err(ApiError::api(ApiCode::UnsupportedMediaType));
        }
        let bytes = Bytes::from_request(req, state).await.map_err(|rejection| {
            if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                ApiError::api(ApiCode::BodyTooLarge)
            } else {
                ApiError::api(ApiCode::InvalidJson)
            }
        })?;
        serde_json::from_slice(&bytes)
            .map(Self)
            .map_err(|_| ApiError::api(ApiCode::InvalidJson))
    }
}

#[cfg(test)]
mod tests {
    use super::is_json;

    #[test]
    fn content_types() {
        assert!(is_json("application/json"));
        assert!(is_json("Application/JSON; charset=utf-8"));
        assert!(!is_json("text/plain"));
        assert!(!is_json("application/jsonp"));
    }
}
