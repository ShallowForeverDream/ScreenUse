use crate::context_store::{self, BrowserContext, EditorContext};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::io::Read;
use tiny_http::{Header, Method, Response, Server};

const MAX_BODY_BYTES: u64 = 256 * 1024;

pub fn start_local_ingest_server() -> Result<()> {
    std::thread::Builder::new()
        .name("screenuse-local-context".into())
        .spawn(move || {
            let server = match Server::http("127.0.0.1:51247") {
                Ok(server) => server,
                Err(error) => {
                    eprintln!("ScreenUse local context server disabled: {error}");
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
                if method == Method::Get && path == "/health" {
                    let _ = request.respond(cors(Response::from_string("ok")));
                    continue;
                }
                if method != Method::Post {
                    let _ = request.respond(cors(Response::from_string("method not allowed").with_status_code(405)));
                    continue;
                }

                let mut body = String::new();
                let result = request
                    .as_reader()
                    .take(MAX_BODY_BYTES)
                    .read_to_string(&mut body)
                    .map_err(anyhow::Error::from)
                    .and_then(|_| ingest_payload(&path, &body));
                match result {
                    Ok(()) => {
                        let _ = request.respond(cors(Response::from_string("ok")));
                    }
                    Err(error) => {
                        let _ = request.respond(cors(Response::from_string(error.to_string()).with_status_code(400)));
                    }
                }
            }
        })?;
    Ok(())
}

fn ingest_payload(path: &str, body: &str) -> Result<()> {
    let value: Value = serde_json::from_str(body)?;
    if path.contains("/browser/tabs") {
        context_store::update_browser(browser_context(&value));
        Ok(())
    } else if path.contains("/vscode/activity") {
        context_store::update_editor(editor_context(&value));
        Ok(())
    } else {
        Err(anyhow!("unknown integration endpoint"))
    }
}

fn browser_context(value: &Value) -> BrowserContext {
    let mut title = value.get("title").and_then(Value::as_str).map(ToOwned::to_owned);
    let context_title = value
        .get("contextTitle")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned);
    let context_type = value
        .get("contextType")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned);
    if context_title.is_some() {
        title.clone_from(&context_title);
    }
    let mut url = value.get("url").and_then(Value::as_str).map(ToOwned::to_owned);
    let mut tab_id = value.get("tabId").and_then(Value::as_i64);
    let mut window_id = value.get("windowId").and_then(Value::as_i64);
    let mut audible = value.get("audible").and_then(Value::as_bool).unwrap_or(false);
    let mut video_playing = value.get("videoPlaying").and_then(Value::as_bool).unwrap_or(false);

    // Backward-compatible parsing for the v0.1 extension payload. Only the
    // focused window's active tab is retained; the full tab list is discarded.
    if title.is_none() && url.is_none() {
        let windows = value.get("windows").and_then(Value::as_array);
        let focused = windows
            .and_then(|items| items.iter().find(|window| window.get("focused").and_then(Value::as_bool).unwrap_or(false)))
            .or_else(|| windows.and_then(|items| items.first()));
        if let Some(window) = focused {
            window_id = window.get("id").and_then(Value::as_i64);
            if let Some(tab) = window
                .get("tabs")
                .and_then(Value::as_array)
                .and_then(|tabs| tabs.iter().find(|tab| tab.get("active").and_then(Value::as_bool).unwrap_or(false)))
            {
                title = tab.get("title").and_then(Value::as_str).map(ToOwned::to_owned);
                url = tab.get("url").and_then(Value::as_str).map(ToOwned::to_owned);
                tab_id = tab.get("id").and_then(Value::as_i64);
                audible = tab.get("audible").and_then(Value::as_bool).unwrap_or(false);
                video_playing = tab.get("videoPlaying").and_then(Value::as_bool).unwrap_or(false);
            }
        }
    }

    let browser = value
        .get("browser")
        .and_then(Value::as_str)
        .unwrap_or("Chromium")
        .to_string();
    let event_id = value
        .get("eventId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{}:{}:{}", window_id.unwrap_or_default(), tab_id.unwrap_or_default(), url.as_deref().unwrap_or_default()));

    BrowserContext {
        event_id: cap(&event_id, 200),
        browser: cap(&browser, 80),
        title: title.map(|value| cap(&value, 320)),
        context_title: context_title.map(|value| cap(&value, 320)),
        context_type: context_type.map(|value| cap(&value, 80)),
        url: url.map(|value| cap(&value, 1200)),
        tab_id,
        window_id,
        audible,
        video_playing,
        ..Default::default()
    }
}

fn editor_context(value: &Value) -> EditorContext {
    let workspace = value
        .get("workspace")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("path").or_else(|| item.get("name")))
        .and_then(Value::as_str)
        .map(|value| cap(value, 900));
    let active_file = value
        .get("activeFile")
        .and_then(Value::as_str)
        .map(|value| cap(value, 900));
    let event_id = value
        .get("eventId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{}:{}", workspace.as_deref().unwrap_or_default(), active_file.as_deref().unwrap_or_default()));

    EditorContext {
        event_id: cap(&event_id, 200),
        workspace,
        active_file,
        language_id: value.get("languageId").and_then(Value::as_str).map(|value| cap(value, 80)),
        git_branch: value.get("gitBranch").and_then(Value::as_str).map(|value| cap(value, 160)),
        event_kind: value.get("eventKind").and_then(Value::as_str).map(|value| cap(value, 80)),
        terminal_count: value.get("terminalCount").and_then(Value::as_u64).unwrap_or_default().min(u32::MAX as u64) as u32,
        debug_active: value.get("debugActive").and_then(Value::as_str).map(|value| cap(value, 160)),
        ..Default::default()
    }
}

fn cap(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn cors(mut response: Response<std::io::Cursor<Vec<u8>>>) -> Response<std::io::Cursor<Vec<u8>>> {
    for (name, value) in [
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Headers", "content-type"),
        ("Access-Control-Allow-Methods", "GET, POST, OPTIONS"),
    ] {
        if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response.add_header(header);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_only_active_tab_from_legacy_payload() {
        let context = browser_context(&json!({
            "windows": [{
                "id": 1,
                "focused": true,
                "tabs": [
                    {"id": 2, "active": false, "title": "Hidden", "url": "https://example.com"},
                    {"id": 3, "active": true, "title": "ScreenUse", "url": "https://github.com/ShallowForeverDream/ScreenUse"}
                ]
            }]
        }));
        assert_eq!(context.tab_id, Some(3));
        assert_eq!(context.title.as_deref(), Some("ScreenUse"));
    }

    #[test]
    fn keeps_the_selected_chatgpt_conversation_title() {
        let context = browser_context(&json!({
            "title": "ICPC刷题网站功能需求",
            "tabTitle": "ChatGPT",
            "contextTitle": "ICPC刷题网站功能需求",
            "contextType": "chatgpt-conversation",
            "url": "https://chatgpt.com/c/current-id",
            "tabId": 3,
            "windowId": 1
        }));
        assert_eq!(context.title.as_deref(), Some("ICPC刷题网站功能需求"));
        assert_eq!(context.context_title.as_deref(), Some("ICPC刷题网站功能需求"));
        assert_eq!(context.context_type.as_deref(), Some("chatgpt-conversation"));
    }
}
