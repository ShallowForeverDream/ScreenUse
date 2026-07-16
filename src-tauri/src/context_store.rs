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
    pub tab_title: Option<String>,
    pub context_title: Option<String>,
    pub context_type: Option<String>,
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
    pub app_name: Option<String>,
    pub workspace: Option<String>,
    pub active_file: Option<String>,
    pub language_id: Option<String>,
    pub git_branch: Option<String>,
    pub event_kind: Option<String>,
    pub terminal_count: u32,
    pub debug_active: Option<String>,
    pub active_terminal: Option<String>,
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

    if is_browser_app(&app)
        && is_fresh(snapshot.browser.updated_at)
        && browser_context_matches_event(event, &snapshot.browser)
    {
        let active_title = snapshot
            .browser
            .context_title
            .as_ref()
            .or(snapshot.browser.title.as_ref())
            .filter(|value| !value.trim().is_empty())
            .cloned();
        if let Some(title) = active_title {
            event.window_title = Some(title.clone());
            if !event.metadata.is_object() {
                event.metadata = json!({});
            }
            if let Some(metadata) = event.metadata.as_object_mut() {
                metadata.insert("activePageTitle".into(), Value::String(title.clone()));
                let context_type = snapshot.browser.context_type.as_deref();
                let conversation = context_type.is_some_and(|value| {
                    value.ends_with("-conversation") || value.ends_with("-new-chat")
                });
                metadata.insert(
                    "activePageSource".into(),
                    Value::String(context_type.unwrap_or("browser-extension").to_string()),
                );
                metadata.insert(
                    "activeContextType".into(),
                    Value::String(if conversation {
                        "conversation".into()
                    } else {
                        "browser-page".into()
                    }),
                );
                if conversation {
                    metadata.insert("conversationTitle".into(), Value::String(title.clone()));
                }
                if context_type == Some("chatgpt-conversation") {
                    metadata.insert("chatgptConversationTitle".into(), Value::String(title));
                }
            }
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
                "tabTitle": snapshot.browser.tab_title,
                "tabId": snapshot.browser.tab_id,
                "windowId": snapshot.browser.window_id,
                "contextTitle": snapshot.browser.context_title,
                "contextType": snapshot.browser.context_type,
                "audible": snapshot.browser.audible,
                "videoPlaying": snapshot.browser.video_playing,
            }),
        );
    }

    if is_editor_app(&app)
        && is_fresh(snapshot.editor.updated_at)
        && editor_context_matches_app(&app, snapshot.editor.app_name.as_deref())
    {
        if snapshot.editor.active_file.as_deref().is_some_and(|value| !value.trim().is_empty()) {
            event.file_path = snapshot.editor.active_file.clone();
        }
        if snapshot.editor.workspace.as_deref().is_some_and(|value| !value.trim().is_empty()) {
            event.workspace = snapshot.editor.workspace.clone();
        }
        if let Some(file_name) = snapshot
            .editor
            .active_file
            .as_deref()
            .and_then(|value| std::path::Path::new(value).file_name())
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
        {
            if !event.metadata.is_object() {
                event.metadata = json!({});
            }
            if let Some(metadata) = event.metadata.as_object_mut() {
                metadata.insert("activePageTitle".into(), Value::String(file_name.to_string()));
                metadata.insert(
                    "activePageSource".into(),
                    Value::String("vscode-extension".into()),
                );
                metadata.insert("activeContextType".into(), Value::String("editor".into()));
            }
        }
        merge_metadata(
            event,
            "editor",
            json!({
                "eventId": snapshot.editor.event_id,
                "appName": snapshot.editor.app_name,
                "languageId": snapshot.editor.language_id,
                "gitBranch": snapshot.editor.git_branch,
                "eventKind": snapshot.editor.event_kind,
                "terminalCount": snapshot.editor.terminal_count,
                "debugActive": snapshot.editor.debug_active,
                "activeTerminal": snapshot.editor.active_terminal,
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
        "tabbit", "thorium", "floorp", "waterfox", "librewolf", "duckduckgo",
    ]
    .iter()
    .any(|needle| app.contains(needle))
}

fn browser_context_matches_event(event: &RawActivityEvent, context: &BrowserContext) -> bool {
    let app = event.app.as_deref().unwrap_or_default().to_lowercase();
    let browser = context.browser.to_lowercase();
    let brand_matches = if browser.contains("edge") {
        app.contains("msedge")
    } else if browser.contains("opera") {
        app.contains("opera")
    } else if browser.contains("vivaldi") {
        app.contains("vivaldi")
    } else if browser.contains("brave") {
        app.contains("brave")
    } else if browser.contains("firefox") {
        app.contains("firefox")
    } else if browser.contains("chromium") {
        app.contains("chromium")
    } else if browser.contains("chrome") {
        app.contains("chrome")
    } else {
        true
    };
    let known_browser_process = [
        "chrome", "msedge", "firefox", "brave", "vivaldi", "opera", "chromium",
    ]
    .iter()
    .any(|needle| app.contains(needle));
    if known_browser_process && !brand_matches {
        return false;
    }
    let Some(tab_title) = context
        .tab_title
        .as_deref()
        .map(canonical_title)
        .filter(|value| !value.is_empty())
    else {
        return brand_matches;
    };
    let native_title = event
        .metadata
        .get("nativeWindowTitle")
        .and_then(Value::as_str)
        .or(event.window_title.as_deref())
        .map(canonical_title)
        .unwrap_or_default();
    let title_matches = native_title.is_empty()
        || native_title.contains(&tab_title)
        || tab_title.contains(&native_title);
    title_matches && (brand_matches || !native_title.is_empty())
}

fn canonical_title(value: &str) -> String {
    value
        .to_lowercase()
        .replace(['\r', '\n', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_editor_app(app: &str) -> bool {
    [
        "code.exe",
        "code - insiders",
        "code-insiders",
        "cursor",
        "windsurf",
        "codium",
    ]
    .iter()
    .any(|needle| app.contains(needle))
}

fn editor_context_matches_app(app: &str, app_name: Option<&str>) -> bool {
    let Some(app_name) = app_name.map(str::to_lowercase) else {
        return app.contains("code.exe");
    };
    if app.contains("cursor") {
        app_name.contains("cursor")
    } else if app.contains("windsurf") {
        app_name.contains("windsurf")
    } else if app.contains("codium") {
        app_name.contains("codium")
    } else {
        (app.contains("code.exe") || app.contains("code - insiders") || app.contains("code-insiders"))
            && (app_name.contains("visual studio code") || app_name == "code")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::InputStats;

    #[test]
    fn enriches_browser_with_the_selected_chatgpt_conversation() {
        update_browser(BrowserContext {
            event_id: "browser:1".into(),
            browser: "Google Chrome".into(),
            title: Some("ChatGPT".into()),
            tab_title: Some("自动记录时间优化 - ChatGPT".into()),
            context_title: Some("ICPC刷题网站功能需求".into()),
            context_type: Some("chatgpt-conversation".into()),
            url: Some("https://chatgpt.com/c/current-id".into()),
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
            window_title: Some("自动记录时间优化 - ChatGPT".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({
                "nativeWindowTitle": "自动记录时间优化 - ChatGPT - Google Chrome"
            }),
        };
        enrich_event(&mut event);
        assert_eq!(event.url.as_deref(), Some("https://chatgpt.com/c/current-id"));
        assert_eq!(event.window_title.as_deref(), Some("ICPC刷题网站功能需求"));
        assert_eq!(
            event.metadata["chatgptConversationTitle"].as_str(),
            Some("ICPC刷题网站功能需求")
        );
        assert_eq!(event.metadata["activeContextType"].as_str(), Some("conversation"));
        assert!(event.metadata.get("browser").is_some());
    }

    #[test]
    fn stale_browser_context_cannot_cross_apps_or_tabs() {
        let context = BrowserContext {
            browser: "Google Chrome".into(),
            tab_title: Some("ICPC 训练台".into()),
            ..Default::default()
        };
        let event = |app: &str, title: &str| RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: String::new(),
            app: Some(app.into()),
            window_title: Some(title.into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({"nativeWindowTitle": title}),
        };

        assert!(browser_context_matches_event(
            &event("chrome.exe", "ICPC 训练台 - Google Chrome"),
            &context,
        ));
        assert!(!browser_context_matches_event(
            &event("msedge.exe", "ICPC 训练台 - Microsoft Edge"),
            &context,
        ));
        assert!(!browser_context_matches_event(
            &event("chrome.exe", "论文检索 - Google Chrome"),
            &context,
        ));
    }

    #[test]
    fn editor_context_is_scoped_to_the_actual_vscode_family_app() {
        assert!(editor_context_matches_app(
            "code.exe",
            Some("Visual Studio Code"),
        ));
        assert!(editor_context_matches_app("cursor.exe", Some("Cursor")));
        assert!(!editor_context_matches_app(
            "pycharm64.exe",
            Some("Visual Studio Code"),
        ));
        assert!(!editor_context_matches_app(
            "cursor.exe",
            Some("Visual Studio Code"),
        ));
    }
}
