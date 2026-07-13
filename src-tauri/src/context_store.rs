use crate::models::RawActivityEvent;
use parking_lot::RwLock;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

const CONTEXT_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Default)]
pub struct BrowserContext {
    pub event_id: String,
    pub browser: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub tab_id: Option<i64>,
    pub window_id: Option<i64>,
    pub audible: bool,
    pub video_playing: bool,
    pub(crate) updated_at: Option<SystemTime>,
}

#[derive(Debug, Clone, Default)]
pub struct EditorContext {
    pub event_id: String,
    pub workspace: Option<String>,
    pub active_file: Option<String>,
    pub language_id: Option<String>,
    pub git_branch: Option<String>,
    pub event_kind: Option<String>,
    pub terminal_count: u32,
    pub debug_active: Option<String>,
    pub(crate) updated_at: Option<SystemTime>,
}

#[derive(Debug, Clone, Default)]
struct IntegrationContext {
    browser: BrowserContext,
    editor: EditorContext,
}

fn store() -> &'static RwLock<IntegrationContext> {
    static STORE: OnceLock<RwLock<IntegrationContext>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(IntegrationContext::default()))
}

pub fn update_browser(mut context: BrowserContext) {
    context.updated_at = Some(SystemTime::now());
    store().write().browser = context;
}

pub fn update_editor(mut context: EditorContext) {
    context.updated_at = Some(SystemTime::now());
    store().write().editor = context;
}

pub fn enrich_event(event: &mut RawActivityEvent) {
    let snapshot = store().read().clone();
    let app = event.app.as_deref().unwrap_or_default().to_lowercase();

    if is_browser_app(&app) && is_fresh(snapshot.browser.updated_at) {
        if snapshot.browser.title.as_deref().is_some_and(|value| !value.trim().is_empty()) {
            event.window_title = snapshot.browser.title.clone();
        }
        if snapshot.browser.url.as_deref().is_some_and(|value| !value.trim().is_empty()) {
            event.url = snapshot.browser.url.clone();
        }
        merge_metadata(
            event,
            "browser",
            json!({
                "eventId": snapshot.browser.event_id,
                "browser": snapshot.browser.browser,
                "tabId": snapshot.browser.tab_id,
                "windowId": snapshot.browser.window_id,
                "audible": snapshot.browser.audible,
                "videoPlaying": snapshot.browser.video_playing,
            }),
        );
    }

    if is_editor_app(&app) && is_fresh(snapshot.editor.updated_at) {
        if snapshot.editor.active_file.as_deref().is_some_and(|value| !value.trim().is_empty()) {
            event.file_path = snapshot.editor.active_file.clone();
        }
        if snapshot.editor.workspace.as_deref().is_some_and(|value| !value.trim().is_empty()) {
            event.workspace = snapshot.editor.workspace.clone();
        }
        merge_metadata(
            event,
            "editor",
            json!({
                "eventId": snapshot.editor.event_id,
                "languageId": snapshot.editor.language_id,
                "gitBranch": snapshot.editor.git_branch,
                "eventKind": snapshot.editor.event_kind,
                "terminalCount": snapshot.editor.terminal_count,
                "debugActive": snapshot.editor.debug_active,
            }),
        );
    }
}

fn merge_metadata(event: &mut RawActivityEvent, key: &str, value: Value) {
    if !event.metadata.is_object() {
        event.metadata = json!({});
    }
    if let Some(object) = event.metadata.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

fn is_fresh(updated_at: Option<SystemTime>) -> bool {
    updated_at
        .and_then(|value| value.elapsed().ok())
        .is_some_and(|elapsed| elapsed <= CONTEXT_TTL)
}

fn is_browser_app(app: &str) -> bool {
    [
        "chrome", "msedge", "brave", "vivaldi", "opera", "arc", "firefox", "chromium",
    ]
    .iter()
    .any(|needle| app.contains(needle))
}

fn is_editor_app(app: &str) -> bool {
    [
        "code.exe",
        "cursor",
        "windsurf",
        "codium",
        "devenv",
        "idea",
        "pycharm",
        "webstorm",
        "rustrover",
        "clion",
    ]
    .iter()
    .any(|needle| app.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::InputStats;

    #[test]
    fn enriches_browser_without_persisting_all_tabs() {
        update_browser(BrowserContext {
            event_id: "browser:1".into(),
            browser: "Chromium".into(),
            title: Some("ScreenUse - GitHub".into()),
            url: Some("https://github.com/ShallowForeverDream/ScreenUse".into()),
            tab_id: Some(2),
            window_id: Some(1),
            audible: false,
            video_playing: false,
            updated_at: None,
        });
        let mut event = RawActivityEvent {
            id: String::new(),
            source: "windows-foreground".into(),
            timestamp: String::new(),
            app: Some("chrome.exe".into()),
            window_title: Some("Chrome".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({}),
        };
        enrich_event(&mut event);
        assert_eq!(event.url.as_deref(), Some("https://github.com/ShallowForeverDream/ScreenUse"));
        assert!(event.metadata.get("browser").is_some());
    }
}
