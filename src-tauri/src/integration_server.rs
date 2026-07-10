use crate::db::{now, AppDb};
use crate::models::{InputStats, RawActivityEvent};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use tiny_http::{Header, Method, Response, Server};
use uuid::Uuid;

pub fn start_local_ingest_server(db: Arc<AppDb>) -> Result<()> {
    std::thread::Builder::new().name("screenuse-local-ingest".into()).spawn(move || {
        let server = match Server::http("127.0.0.1:51247") {
            Ok(server) => server,
            Err(err) => {
                eprintln!("ScreenUse local ingest server disabled: {err}");
                return;
            }
        };
        for mut request in server.incoming_requests() {
            let method = request.method().clone();
            let path = request.url().to_string();
            if method == Method::Options {
                let _ = request.respond(cors(Response::from_string("").with_status_code(204)));
                continue;
            }
            if method != Method::Post {
                let _ = request.respond(cors(Response::from_string("method not allowed").with_status_code(405)));
                continue;
            }
            let mut body = String::new();
            let result = request.as_reader().read_to_string(&mut body)
                .map_err(anyhow::Error::from)
                .and_then(|_| ingest_payload(&db, &path, &body));
            match result {
                Ok(()) => { let _ = request.respond(cors(Response::from_string("ok"))); }
                Err(err) => { let _ = request.respond(cors(Response::from_string(err.to_string()).with_status_code(500))); }
            }
        }
    })?;
    Ok(())
}

fn ingest_payload(db: &AppDb, path: &str, body: &str) -> Result<()> {
    let value: Value = serde_json::from_str(body)?;
    let event = if path.contains("/browser/tabs") { browser_event(value) } else if path.contains("/vscode/activity") { vscode_event(value) } else { generic_event(value) };
    db.ingest_raw_event(event)
}

fn browser_event(value: Value) -> RawActivityEvent {
    let mut active_title = None;
    let mut active_url = None;
    let mut tab_count = 0;
    if let Some(windows) = value.get("windows").and_then(|v| v.as_array()) {
        for win in windows {
            if let Some(tabs) = win.get("tabs").and_then(|v| v.as_array()) {
                tab_count += tabs.len();
                for tab in tabs {
                    if tab.get("active").and_then(|v| v.as_bool()).unwrap_or(false) {
                        active_title = tab.get("title").and_then(|v| v.as_str()).map(ToOwned::to_owned);
                        active_url = tab.get("url").and_then(|v| v.as_str()).map(ToOwned::to_owned);
                    }
                }
            }
        }
    }
    RawActivityEvent {
        id: Uuid::new_v4().to_string(), source: "browser-extension".into(), timestamp: now(), app: Some("Chromium".into()), window_title: active_title, url: active_url, file_path: None, workspace: None, input_stats: InputStats::default(), metadata: json!({ "tabCount": tab_count, "raw": value }),
    }
}

fn vscode_event(value: Value) -> RawActivityEvent {
    let workspace = value.get("workspace").and_then(|v| v.as_array()).and_then(|arr| arr.first()).and_then(|w| w.get("path")).and_then(|v| v.as_str()).map(ToOwned::to_owned);
    let file_path = value.get("activeFile").and_then(|v| v.as_str()).map(ToOwned::to_owned);
    let title = file_path.as_ref().map(|p| format!("VS Code - {}", p)).or_else(|| Some("VS Code".into()));
    RawActivityEvent {
        id: Uuid::new_v4().to_string(), source: "vscode-extension".into(), timestamp: now(), app: Some("VS Code".into()), window_title: title, url: None, file_path, workspace, input_stats: InputStats::default(), metadata: value,
    }
}

fn generic_event(value: Value) -> RawActivityEvent {
    RawActivityEvent { id: Uuid::new_v4().to_string(), source: "local-http".into(), timestamp: now(), app: None, window_title: None, url: None, file_path: None, workspace: None, input_stats: InputStats::default(), metadata: value }
}

fn cors(mut response: Response<std::io::Cursor<Vec<u8>>>) -> Response<std::io::Cursor<Vec<u8>>> {
    for (name, value) in [
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Headers", "content-type"),
        ("Access-Control-Allow-Methods", "POST, OPTIONS"),
    ] {
        if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) { response.add_header(header); }
    }
    response
}
