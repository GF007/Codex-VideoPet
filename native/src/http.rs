use crate::{
    core::{sanitize_label, CodexEventRequest, VALID_STATES},
    runtime::{safe_source, RuntimeData, SetStateRequest, SetWindowRequest, API_ADDRESS},
};
use serde::de::DeserializeOwned;
use std::{io::Read, sync::Arc, thread, time::Duration};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const MAX_BODY_BYTES: usize = 16 * 1024;

pub fn start(runtime: Arc<RuntimeData>) -> Result<thread::JoinHandle<()>, String> {
    let server = Server::http(API_ADDRESS).map_err(|error| error.to_string())?;
    let worker_runtime = runtime.clone();
    let result = thread::Builder::new()
        .name("codex-pet-http".to_owned())
        .spawn(move || {
            while !worker_runtime
                .shutdown
                .load(std::sync::atomic::Ordering::Acquire)
            {
                match server.recv_timeout(Duration::from_millis(250)) {
                    Ok(Some(request)) => handle_http(request, worker_runtime.clone()),
                    Ok(None) => continue,
                    Err(error) => {
                        worker_runtime
                            .set_http_status(false, Some(sanitize_label(&error.to_string(), 160)));
                        break;
                    }
                }
            }
        });
    match result {
        Ok(handle) => {
            runtime.set_http_status(true, None);
            crate::hook_bridge::release_launch_lock();
            Ok(handle)
        }
        Err(error) => {
            runtime.set_http_status(
                false,
                Some(sanitize_label(
                    &format!("unable to start HTTP server: {error}"),
                    160,
                )),
            );
            Err(format!("unable to start HTTP server: {error}"))
        }
    }
}

fn handle_http(mut request: Request, runtime: Arc<RuntimeData>) {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or_default();
    let response = if method == Method::Post && !is_authorized(&request, &runtime) {
        json_response(
            401,
            serde_json::json!({ "ok": false, "error": "unauthorized" }),
        )
    } else {
        match (method, path) {
            (Method::Get, "/health") => json_response(200, serde_json::json!({ "ok": true })),
            (Method::Get, "/v1/state") => json_response(200, runtime.diagnostics_json()),
            (Method::Get, "/v1/sessions") => json_response(200, runtime.sessions_json()),
            (Method::Get, "/v1/video-config") => json_response(200, runtime.video_config_json()),
            (Method::Get, "/v1/states") => {
                json_response(200, serde_json::json!({ "states": VALID_STATES }))
            }
            (Method::Post, "/v1/codex-event") => {
                match read_json_body::<CodexEventRequest>(&mut request) {
                    Ok(payload) => match runtime.apply_codex_event(payload) {
                        Ok(outcome) => json_response(
                            200,
                            serde_json::json!({
                                "ok": true,
                                "state": outcome.state,
                                "sessions": outcome.sessions
                            }),
                        ),
                        Err(error) => {
                            json_response(422, serde_json::json!({ "ok": false, "error": error }))
                        }
                    },
                    Err(error) => {
                        json_response(400, serde_json::json!({ "ok": false, "error": error }))
                    }
                }
            }
            (Method::Post, "/v1/state") | (Method::Post, "/v1/codex-state") => {
                match read_json_body::<SetStateRequest>(&mut request) {
                    Ok(payload) => {
                        let (state, source, ttl_ms) = safe_source(payload);
                        match runtime.transition_state(&state, &source, ttl_ms) {
                            Ok(snapshot) => json_response(
                                200,
                                serde_json::json!({ "ok": true, "state": snapshot }),
                            ),
                            Err(error) => json_response(
                                422,
                                serde_json::json!({ "ok": false, "error": error }),
                            ),
                        }
                    }
                    Err(error) => {
                        json_response(400, serde_json::json!({ "ok": false, "error": error }))
                    }
                }
            }
            (Method::Post, "/v1/window") => {
                match read_json_body::<SetWindowRequest>(&mut request) {
                    Ok(payload) => match runtime.update_window(payload) {
                        Ok(window) => {
                            json_response(200, serde_json::json!({ "ok": true, "window": window }))
                        }
                        Err(error) => {
                            json_response(500, serde_json::json!({ "ok": false, "error": error }))
                        }
                    },
                    Err(error) => {
                        json_response(400, serde_json::json!({ "ok": false, "error": error }))
                    }
                }
            }
            _ => json_response(
                404,
                serde_json::json!({ "ok": false, "error": "not found" }),
            ),
        }
    };
    let _ = request.respond(response);
}

fn json_response(status: u16, value: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let header = Header::from_bytes("Content-Type", "application/json; charset=utf-8")
        .expect("static content type");
    Response::from_data(serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec()))
        .with_status_code(StatusCode(status))
        .with_header(header)
}

fn read_json_body<T: DeserializeOwned>(request: &mut Request) -> Result<T, String> {
    let length = request
        .body_length()
        .ok_or_else(|| "content-length is required".to_owned())?;
    if length > MAX_BODY_BYTES {
        return Err("request body too large".to_owned());
    }
    let mut body = String::new();
    request
        .as_reader()
        .take((MAX_BODY_BYTES + 1) as u64)
        .read_to_string(&mut body)
        .map_err(|_| "failed to read request body".to_owned())?;
    if body.len() > MAX_BODY_BYTES {
        return Err("request body too large".to_owned());
    }
    serde_json::from_str(&body).map_err(|_| "invalid JSON body".to_owned())
}

fn is_authorized(request: &Request, runtime: &RuntimeData) -> bool {
    let Some(expected) = runtime.api_token.as_deref() else {
        return true;
    };
    let expected = format!("Bearer {expected}");
    request.headers().iter().any(|header| {
        header.field.equiv("Authorization") && header.value.as_str() == expected.as_str()
    })
}
