//! Plain JSON and byte requests for the server's newer endpoints (the Gallery, phone backups),
//! whose answers the apps read into their own types.
use std::io::Read;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::api::{map_err, Api, ApiError, Fetch};
use crate::manage::{Counted, Progress};

/// Like `map_err`, but a 409 keeps the server's message ("Phone backups are off", "There is
/// already an album called…"), which these endpoints use for things to tell the person.
fn explain(err: ureq::Error) -> ApiError {
    match err {
        ureq::Error::Status(409, response) => {
            let body = response.into_json::<Value>().unwrap_or_default();
            match body["error"].as_str() {
                Some(message) => ApiError::Other(message.to_string()),
                None => ApiError::Conflict,
            }
        }
        other => map_err(other),
    }
}

fn read_json<T: DeserializeOwned>(response: ureq::Response) -> Result<T, ApiError> {
    response.into_json().map_err(|err| ApiError::Other(format!("The server's answer was not understood: {err}")))
}

impl Api {
    /// GETs `path` with `query` and reads the JSON answer.
    pub fn get_json<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T, ApiError> {
        let mut request = self.request("GET", path);
        for (key, value) in query {
            request = request.query(key, value);
        }
        match request.clone().call() {
            Err(ureq::Error::Transport(transport)) if transport.kind() == ureq::ErrorKind::Io => read_json(request.call().map_err(explain)?),
            other => read_json(other.map_err(explain)?),
        }
    }

    /// Sends JSON with `method` (POST, PATCH, DELETE) and reads the JSON answer.
    pub fn send_json<T: DeserializeOwned>(&self, method: &str, path: &str, query: &[(&str, &str)], body: Value) -> Result<T, ApiError> {
        let build = || {
            let mut request = self.request(method, path);
            for (key, value) in query {
                request = request.query(key, value);
            }
            request
        };
        // A dead pooled connection means the request never arrived, so sending again is safe.
        let response = match build().send_json(&body) {
            Err(ureq::Error::Transport(transport)) if transport.kind() == ureq::ErrorKind::Io => build().send_json(&body),
            other => other,
        };
        read_json(response.map_err(explain)?)
    }

    /// GETs raw bytes (at most `limit`), such as a face picture.
    pub fn get_bytes(&self, path: &str, query: &[(&str, &str)], limit: u64) -> Result<Vec<u8>, ApiError> {
        let mut request = self.request("GET", path);
        for (key, value) in query {
            request = request.query(key, value);
        }
        let mut bytes = Vec::new();
        request
            .fetch()?
            .into_reader()
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|err| ApiError::Other(err.to_string()))?;
        Ok(bytes)
    }

    /// POSTs a file's bytes as the body (a phone backup, or a picture to search with) and reads
    /// the JSON answer. `progress` sees the bytes sent and can cancel.
    pub fn post_stream<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        body: impl Read,
        size: Option<u64>,
        progress: Progress,
    ) -> Result<T, ApiError> {
        let mut request = self.request("POST", path).set("Content-Type", "application/octet-stream");
        for (key, value) in query {
            request = request.query(key, value);
        }
        if let Some(size) = size {
            request = request.set("Content-Length", &size.to_string());
        }
        let cancelled = std::cell::Cell::new(false);
        let watch = |done: u64| {
            let go_on = progress(done);
            cancelled.set(!go_on);
            go_on
        };
        match request.send(Counted::new(body, &watch)) {
            Ok(response) => read_json(response),
            Err(_) if cancelled.get() => Err(ApiError::Cancelled),
            Err(err) => Err(explain(err)),
        }
    }
}
