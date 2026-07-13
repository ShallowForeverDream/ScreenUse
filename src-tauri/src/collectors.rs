use crate::classification;
use crate::context_store;
use crate::db::{now, AppDb};
use crate::models::{InputStats, RawActivityEvent};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use parking_lot::Mutex;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{sleep, Duration, Instant};
use uuid::Uuid;

pub trait CollectorAdapter {
    fn start(&self, db: Arc<AppDb>) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn health(&self) -> CollectorHealth;
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectorHealth {
    pub running: bool,
    pub last_event_at: Option<String>,
    pub last_error: Option<String>,
}

pub struct DesktopCollector {
    running: AtomicBool,
    last_event_at: Mutex<Option<String>>,
    last_error: Mutex<Option<String>>,
}

#[derive(Debug, Clone)]
struct ActiveContext {
    id: String,
    session_id: String,
    signature: String,
    started_at: String,
    event: RawActivityEvent,
    last_observed_at: Instant,
    last_emitted_at: Instant,
}

#[derive(Debug, Clone)]
struct PendingContext {
    signature: String,
    first_event: RawActivityEvent,
    observations: u8,
}

const SWITCH_CONFIRM_SECONDS: i64 = 5;

impl DesktopCollector {
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            last_event_at: Mutex::new(None),
            last_error: Mutex::new(None),
        }
    }

    fn set_error(&self, error: impl ToString) {
        *self.last_error.lock() = Some(error.to_string());
    }

    fn clear_error(&self) {
        *self.last_error.lock() = None;
    }
}

impl CollectorAdapter for Arc<DesktopCollector> {
    fn start(&self, db: Arc<AppDb>) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let collector = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut active: Option<ActiveContext> = None;
            let mut pending: Option<PendingContext> = None;
            let mut task_view_started_at: Option<String> = None;
            let mut settings = db.get_settings().unwrap_or_default().normalized();
            let mut settings_loaded_at = Instant::now();

            loop {
                if !collector.running.load(Ordering::SeqCst) {
                    if let Some(previous) = active.take() {
                        if let Err(error) = close_context(&collector, &db, previous) {
                            collector.set_error(error);
                        }
                    }
                    break;
                }

                if settings_loaded_at.elapsed() >= Duration::from_secs(60) {
                    if let Ok(latest) = db.get_settings() {
                        settings = latest.normalized();
                    }
                    settings_loaded_at = Instant::now();
                }

                let mut event = match capture_foreground_event() {
                    Ok(event) => event,
                    Err(error) => {
                        collector.set_error(error);
                        sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };
                context_store::enrich_event(&mut event);
                if settings.passive_content_counts_as_active
                    && event.input_stats.idle_seconds >= settings.idle_threshold_seconds as u64
                    && is_passive_attention(&event)
                {
                    let input_idle_seconds = event.input_stats.idle_seconds;
                    mark_metadata(
                        &mut event,
                        "inputIdleSeconds",
                        serde_json::Value::from(input_idle_seconds),
                    );
                    mark_metadata(&mut event, "passiveAttention", serde_json::Value::Bool(true));
                    event.input_stats.idle_seconds = 0;
                }
                sanitize_event(&mut event);

                // Task View is a transition surface, not a standalone activity. Keep its
                // first timestamp and assign that interval to the context selected next.
                if is_windows_task_view(&event) {
                    task_view_started_at.get_or_insert_with(|| event.timestamp.clone());
                    pending = None;
                    if let Some(current) = active.as_mut() {
                        current.last_observed_at = Instant::now();
                    }
                    sleep(Duration::from_secs(settings.poll_interval_seconds as u64)).await;
                    continue;
                }

                let observation_gap = active.as_ref().is_some_and(|current| {
                    is_unexpected_observation_gap(current.last_observed_at.elapsed(), settings.poll_interval_seconds)
                });
                if observation_gap {
                    if let Some(previous) = active.take() {
                        let ended_at = previous.event.timestamp.clone();
                        if let Err(error) = close_context_at(&collector, &db, previous, ended_at) {
                            collector.set_error(error);
                        }
                    }
                    pending = None;
                    task_view_started_at = None;
                }
                let signature = context_signature(&event, settings.idle_threshold_seconds);
                if active.is_none() {
                    pending = None;
                    if let Some(boundary) = task_view_started_at.take() {
                        event.timestamp = boundary;
                        mark_metadata(&mut event, "taskViewHandoff", serde_json::Value::Bool(true));
                    }
                    match open_context(&collector, &db, event, signature) {
                        Ok(context) => active = Some(context),
                        Err(error) => collector.set_error(error),
                    }
                } else {
                    let active_signature = active.as_ref().map(|current| current.signature.clone()).unwrap_or_default();
                    if active_signature == signature {
                        pending = None;
                        task_view_started_at = None;
                        if let Some(current) = active.as_mut() {
                            current.last_observed_at = Instant::now();
                            let heartbeat_due = current.last_emitted_at.elapsed()
                                >= Duration::from_secs(settings.heartbeat_seconds as u64);
                            if heartbeat_due {
                                if let Err(error) = heartbeat_context(&collector, &db, current, event) {
                                    collector.set_error(error);
                                }
                            } else {
                                current.event = event;
                            }
                        }
                    } else {
                        if let Some(current) = active.as_mut() {
                            current.last_observed_at = Instant::now();
                        }
                        let mut transition_event = event.clone();
                        if signature == "idle" {
                            if let Some(current) = active.as_ref() {
                                transition_event.timestamp = idle_boundary_at(&event, &current.started_at);
                                mark_metadata(
                                    &mut transition_event,
                                    "idleBoundaryBackdated",
                                    serde_json::Value::Bool(true),
                                );
                            }
                        } else if pending
                            .as_ref()
                            .map_or(true, |candidate| candidate.signature != signature)
                        {
                            if let Some(boundary) = task_view_started_at.as_ref() {
                                transition_event.timestamp = boundary.clone();
                                mark_metadata(
                                    &mut transition_event,
                                    "taskViewHandoff",
                                    serde_json::Value::Bool(true),
                                );
                            }
                        }
                        let immediate = active_signature == "idle" || signature == "idle";
                        let ready = if immediate {
                            pending = Some(PendingContext {
                                signature: signature.clone(),
                                first_event: transition_event,
                                observations: 1,
                            });
                            true
                        } else {
                            observe_pending_context(&mut pending, &signature, &transition_event)
                        };

                        if ready {
                            let Some(mut next) = pending.take() else { continue; };
                            task_view_started_at = None;
                            mark_metadata(
                                &mut next.first_event,
                                "switchConfirmedAfterObservations",
                                serde_json::Value::from(next.observations),
                            );
                            let boundary = next.first_event.timestamp.clone();
                            if let Some(previous) = active.take() {
                                if let Err(error) = close_context_at(&collector, &db, previous, boundary.clone()) {
                                    collector.set_error(error);
                                }
                            }
                            match open_context(&collector, &db, next.first_event, next.signature) {
                                Ok(mut context) => {
                                    if event.timestamp > boundary {
                                        if let Err(error) = heartbeat_context(&collector, &db, &mut context, event) {
                                            collector.set_error(error);
                                        }
                                    }
                                    active = Some(context);
                                }
                                Err(error) => collector.set_error(error),
                            }
                        }
                    }
                }

                sleep(Duration::from_secs(settings.poll_interval_seconds as u64)).await;
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn health(&self) -> CollectorHealth {
        CollectorHealth {
            running: self.running.load(Ordering::SeqCst),
            last_event_at: self.last_event_at.lock().clone(),
            last_error: self.last_error.lock().clone(),
        }
    }

}

fn observe_pending_context(
    pending: &mut Option<PendingContext>,
    signature: &str,
    event: &RawActivityEvent,
) -> bool {
    match pending {
        Some(candidate) if candidate.signature == signature => {
            candidate.observations = candidate.observations.saturating_add(1);
        }
        _ => {
            *pending = Some(PendingContext {
                signature: signature.to_string(),
                first_event: event.clone(),
                observations: 1,
            });
        }
    }
    pending.as_ref().is_some_and(|candidate| {
        elapsed_seconds(&candidate.first_event.timestamp, &event.timestamp)
            .is_some_and(|seconds| seconds >= SWITCH_CONFIRM_SECONDS)
    })
}

fn open_context(
    collector: &DesktopCollector,
    db: &AppDb,
    mut event: RawActivityEvent,
    signature: String,
) -> Result<ActiveContext> {
    event.id = Uuid::new_v4().to_string();
    if event.timestamp.is_empty() {
        event.timestamp = now();
    }
    mark_metadata(&mut event, "contextStart", serde_json::Value::Bool(true));
    let session = classification::ingest_event(db, &event)?
        .context("collector context did not create a work session")?;
    collector.clear_error();
    *collector.last_event_at.lock() = Some(event.timestamp.clone());
    let observed_at = Instant::now();
    let started_at = event.timestamp.clone();
    Ok(ActiveContext {
        id: event.id.clone(),
        session_id: session.id,
        signature,
        started_at,
        event,
        last_observed_at: observed_at,
        last_emitted_at: observed_at,
    })
}

fn heartbeat_context(
    collector: &DesktopCollector,
    db: &AppDb,
    current: &mut ActiveContext,
    mut event: RawActivityEvent,
) -> Result<()> {
    event.id = current.id.clone();
    mark_metadata(&mut event, "heartbeat", serde_json::Value::Bool(true));
    db.heartbeat_raw_event(&event, &current.session_id)?;
    collector.clear_error();
    *collector.last_event_at.lock() = Some(event.timestamp.clone());
    current.event = event;
    current.last_observed_at = Instant::now();
    current.last_emitted_at = Instant::now();
    Ok(())
}

fn close_context(collector: &DesktopCollector, db: &AppDb, previous: ActiveContext) -> Result<()> {
    close_context_at(collector, db, previous, now())
}

fn close_context_at(collector: &DesktopCollector, db: &AppDb, previous: ActiveContext, ended_at: String) -> Result<()> {
    let mut event = previous.event;
    event.id = previous.id;
    event.timestamp = ended_at;
    mark_metadata(&mut event, "contextEnd", serde_json::Value::Bool(true));
    db.heartbeat_raw_event(&event, &previous.session_id)?;
    classification::finalize_context(db, &event, &previous.session_id)?;
    *collector.last_event_at.lock() = Some(event.timestamp);
    Ok(())
}

fn is_unexpected_observation_gap(elapsed: Duration, poll_interval_seconds: u32) -> bool {
    let expected = u64::from(poll_interval_seconds.max(1));
    elapsed > Duration::from_secs(expected.saturating_mul(4).max(60))
}

fn context_signature(event: &RawActivityEvent, idle_threshold_seconds: u32) -> String {
    if event.input_stats.idle_seconds >= idle_threshold_seconds as u64 {
        return "idle".into();
    }
    let app = event.app.as_deref().unwrap_or_default().to_lowercase();
    let window_title = signature_window_title(&app, event.window_title.as_deref());
    format!(
        "{}|{}|{}|{}|{}",
        app,
        window_title,
        event.url.as_deref().unwrap_or_default(),
        event.file_path.as_deref().unwrap_or_default(),
        event.workspace.as_deref().unwrap_or_default(),
    )
}

fn elapsed_seconds(started_at: &str, ended_at: &str) -> Option<i64> {
    let started = DateTime::parse_from_rfc3339(started_at).ok()?;
    let ended = DateTime::parse_from_rfc3339(ended_at).ok()?;
    Some((ended - started).num_seconds())
}

fn idle_boundary_at(event: &RawActivityEvent, active_started_at: &str) -> String {
    let observed = DateTime::parse_from_rfc3339(&event.timestamp)
        .map(|value| value.with_timezone(&Utc));
    let active_started = DateTime::parse_from_rfc3339(active_started_at)
        .map(|value| value.with_timezone(&Utc));
    match (observed, active_started) {
        (Ok(observed), Ok(active_started)) => {
            let candidate = observed - ChronoDuration::seconds(event.input_stats.idle_seconds as i64);
            candidate
                .max(active_started)
                .to_rfc3339_opts(SecondsFormat::Secs, true)
        }
        _ => event.timestamp.clone(),
    }
}

fn is_windows_task_view(event: &RawActivityEvent) -> bool {
    let app = event.app.as_deref().unwrap_or_default().trim().to_lowercase();
    let title = event.window_title.as_deref().unwrap_or_default().trim().to_lowercase();
    let shell_app = [
        "explorer.exe",
        "shellexperiencehost.exe",
        "applicationframehost.exe",
        "taskhostw.exe",
    ]
    .contains(&app.as_str());
    shell_app
        && (matches!(title.as_str(), "任务视图" | "task view")
            || title.contains("multitaskingviewframe"))
}

fn signature_window_title<'a>(app: &str, title: Option<&'a str>) -> &'a str {
    let title = title.unwrap_or_default();
    if (app == "qq" || app == "qq.exe")
        && matches!(title.trim(), "" | "QQ" | "图片查看器")
    {
        "qq-main"
    } else {
        title
    }
}

fn sanitize_event(event: &mut RawActivityEvent) {
    event.app = event.app.take().map(|value| cap(&value, 160));
    event.window_title = event.window_title.take().map(|value| cap(&value, 320));
    event.url = event.url.take().map(|value| cap(&value, 1200));
    event.file_path = event.file_path.take().map(|value| cap(&value, 900));
    event.workspace = event.workspace.take().map(|value| cap(&value, 900));
}

fn mark_metadata(event: &mut RawActivityEvent, key: &str, value: serde_json::Value) {
    if !event.metadata.is_object() {
        event.metadata = json!({});
    }
    if let Some(object) = event.metadata.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

fn cap(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        value.chars().take(max_chars).collect()
    }
}

fn is_passive_attention(event: &RawActivityEvent) -> bool {
    let app = event.app.as_deref().unwrap_or_default().to_lowercase();
    let browser_video_playing = event
        .metadata
        .get("browser")
        .and_then(|browser| browser.get("videoPlaying"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let video_player = [
        "vlc",
        "mpv",
        "potplayer",
        "wmplayer",
        "media player",
        "mpc-hc",
        "mpc-be",
        "smplayer",
    ]
    .iter()
    .any(|needle| app.contains(needle));
    browser_video_playing || video_player
}

#[cfg(windows)]
unsafe fn process_image_path(pid: u32) -> Result<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, BOOL};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid)?;
    let mut buffer = vec![0u16; 32768];
    let mut size = buffer.len() as u32;
    let result = QueryFullProcessImageNameW(
        handle,
        PROCESS_NAME_FORMAT(0),
        PWSTR(buffer.as_mut_ptr()),
        &mut size,
    );
    let _ = CloseHandle(handle);
    result?;
    Ok(String::from_utf16_lossy(&buffer[..size as usize]))
}

#[cfg(windows)]
fn capture_foreground_event() -> Result<RawActivityEvent> {
    use std::path::PathBuf;
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    };

    unsafe {
        let window = GetForegroundWindow();
        if window.0.is_null() {
            return Err(anyhow!("no foreground window"));
        }
        let length = GetWindowTextLengthW(window).max(0) as usize;
        let mut buffer = vec![0u16; length + 1];
        let copied = if buffer.is_empty() {
            0
        } else {
            GetWindowTextW(window, &mut buffer)
        };
        let native_title = String::from_utf16_lossy(&buffer[..copied.max(0) as usize]);

        let mut pid = 0u32;
        let _ = GetWindowThreadProcessId(window, Some(&mut pid));
        let executable = process_image_path(pid).ok();
        let app = executable
            .as_ref()
            .and_then(|path| PathBuf::from(path).file_name().map(|name| name.to_string_lossy().to_string()))
            .unwrap_or_else(|| format!("pid:{pid}"));
        let page_context = active_page_context(window, &app, &native_title);
        let title = page_context
            .as_ref()
            .map(|context| context.title.clone())
            .unwrap_or_else(|| native_title.clone());

        let mut last_input = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        let idle_seconds = if GetLastInputInfo(&mut last_input).as_bool() {
            GetTickCount().saturating_sub(last_input.dwTime) as u64 / 1000
        } else {
            0
        };

        let mut metadata = json!({ "pid": pid, "platform": "windows", "capture": "metadata-only" });
        let workspace = page_context.as_ref().and_then(|context| context.project.clone());
        if let Some(context) = page_context {
            metadata["nativeWindowTitle"] = serde_json::Value::String(native_title);
            metadata["activePageTitle"] = serde_json::Value::String(context.title.clone());
            metadata["activePageSource"] = serde_json::Value::String(context.source.into());
            if context.source == "chatgpt-conversation" {
                metadata["chatgptConversationTitle"] = serde_json::Value::String(context.title);
            }
            if let Some(project) = context.project {
                metadata["chatgptProject"] = serde_json::Value::String(project);
            }
        }

        Ok(RawActivityEvent {
            id: String::new(),
            source: "windows-foreground".into(),
            timestamp: now(),
            app: Some(app),
            window_title: Some(title),
            url: None,
            file_path: executable,
            workspace,
            input_stats: InputStats {
                idle_seconds,
                ..Default::default()
            },
            metadata,
        })
    }
}

#[cfg(windows)]
#[derive(Debug, Clone)]
struct ActivePageContext {
    title: String,
    project: Option<String>,
    source: &'static str,
}

#[cfg(windows)]
fn active_page_context(
    window: windows::Win32::Foundation::HWND,
    app: &str,
    native_title: &str,
) -> Option<ActivePageContext> {
    if is_chat_workspace_app(app) {
        return chatgpt_selected_context(window).ok().flatten();
    }
    if is_document_app(app) {
        let native_document = clean_document_title(native_title, app);
        let selected_tab = if native_document.is_none() {
            selected_document_tab(window).ok().flatten()
        } else {
            None
        };
        let visible_wps_title = if native_document.is_none()
            && selected_tab.is_none()
            && is_wps_app(app)
        {
            visible_wps_document_title()
        } else {
            None
        };
        let (title, source) = native_document
            .map(|title| (title, "document-window-title"))
            .or_else(|| selected_tab.map(|title| (title, "selected-document-tab")))
            .or_else(|| visible_wps_title.map(|title| (title, "wps-visible-window")))?;
        return Some(ActivePageContext { title, project: None, source });
    }
    None
}

#[cfg(windows)]
fn is_chat_workspace_app(app: &str) -> bool {
    ["chatgpt.exe", "codex.exe"]
        .iter()
        .any(|name| app.eq_ignore_ascii_case(name))
}

#[cfg(windows)]
fn is_wps_app(app: &str) -> bool {
    ["wps.exe", "et.exe", "wpp.exe", "wpspdf.exe"]
        .iter()
        .any(|name| app.eq_ignore_ascii_case(name))
}

#[cfg(windows)]
fn is_document_app(app: &str) -> bool {
    is_wps_app(app)
        || [
            "winword.exe",
            "excel.exe",
            "powerpnt.exe",
            "onenote.exe",
            "outlook.exe",
            "acrord32.exe",
            "acrobat.exe",
            "typora.exe",
            "obsidian.exe",
            "notepad.exe",
            "notepad++.exe",
        ]
        .iter()
        .any(|name| app.eq_ignore_ascii_case(name))
}

#[cfg(windows)]
fn chatgpt_selected_context(
    window: windows::Win32::Foundation::HWND,
) -> Result<Option<ActivePageContext>> {
    use windows::core::VARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope_Descendants,
        UIA_NamePropertyId, UIA_TextControlTypeId,
    };

    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> windows::core::Result<Option<ActivePageContext>> {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = automation.ElementFromHandle(window)?;
            let mut action: Option<IUIAutomationElement> = None;
            for label in [
                "任务操作",
                "对话操作",
                "聊天操作",
                "会话操作",
                "切换置顶摘要",
                "Task options",
                "Chat options",
                "Conversation options",
                "Toggle pinned summary",
            ] {
                let value = VARIANT::from(label);
                let condition = automation.CreatePropertyCondition(UIA_NamePropertyId, &value)?;
                if let Ok(element) = root.FindFirst(TreeScope_Descendants, &condition) {
                    action = Some(element);
                    break;
                }
            }
            let Some(action) = action else { return Ok(None); };
            let walker = automation.ControlViewWalker()?;
            let parent = walker.GetParentElement(&action)?;
            let mut child = walker.GetFirstChildElement(&parent).ok();
            let mut title = None;
            let mut project = None;
            for _ in 0..10 {
                let Some(element) = child else { break; };
                let name = element.CurrentName().map(|value| value.to_string()).unwrap_or_default();
                if let Some(value) = name
                    .strip_prefix("项目：")
                    .or_else(|| name.strip_prefix("Project: "))
                    .or_else(|| name.strip_prefix("Project："))
                {
                    let value = value.trim();
                    if !value.is_empty() {
                        project = Some(value.to_string());
                    }
                } else if element.CurrentControlType().ok() == Some(UIA_TextControlTypeId)
                    && valid_chatgpt_title(&name)
                {
                    title = Some(name.trim().to_string());
                }
                child = walker.GetNextSiblingElement(&element).ok();
            }
            Ok(title.map(|title| ActivePageContext {
                title,
                project,
                source: "chatgpt-conversation",
            }))
        })();
        if initialized {
            CoUninitialize();
        }
        result.map_err(Into::into)
    }
}

#[cfg(windows)]
fn selected_document_tab(
    window: windows::Win32::Foundation::HWND,
) -> Result<Option<String>> {
    use windows::core::VARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationSelectionItemPattern, TreeScope_Descendants,
        UIA_ControlTypePropertyId, UIA_SelectionItemPatternId, UIA_TabItemControlTypeId,
    };

    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> windows::core::Result<Option<String>> {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = automation.ElementFromHandle(window)?;
            let value = VARIANT::from(UIA_TabItemControlTypeId.0);
            let condition =
                automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &value)?;
            let elements = root.FindAll(TreeScope_Descendants, &condition)?;
            for index in 0..elements.Length()? {
                let element = elements.GetElement(index)?;
                let selected: IUIAutomationSelectionItemPattern = match element
                    .GetCurrentPatternAs(UIA_SelectionItemPatternId)
                {
                    Ok(pattern) => pattern,
                    Err(_) => continue,
                };
                if selected.CurrentIsSelected()?.as_bool() {
                    let name = element.CurrentName()?.to_string();
                    if let Some(title) = valid_selected_document_title(&name) {
                        return Ok(Some(title));
                    }
                }
            }
            Ok(None)
        })();
        if initialized {
            CoUninitialize();
        }
        result.map_err(Into::into)
    }
}

#[cfg(windows)]
fn clean_document_title(native_title: &str, app: &str) -> Option<String> {
    let mut title = native_title.replace(['\r', '\n', '\t'], " ").trim().to_string();
    let suffixes: &[&str] = if is_wps_app(app) {
        &[" - WPS Office", " – WPS Office", " — WPS Office", " - WPS"]
    } else {
        &[
            " - Microsoft Word",
            " - Word",
            " - Microsoft Excel",
            " - Excel",
            " - Microsoft PowerPoint",
            " - PowerPoint",
            " - Microsoft OneNote",
            " - OneNote",
            " - Outlook",
            " - Adobe Acrobat Reader",
            " - Adobe Acrobat",
            " - Acrobat Reader",
            " - Typora",
            " - Obsidian",
            " - Notepad++",
            " - Notepad",
        ]
    };
    for suffix in suffixes {
        if let Some(stripped) = title.strip_suffix(suffix) {
            title = stripped.trim().to_string();
            break;
        }
    }
    valid_document_title(&title)
}

#[cfg(windows)]
fn valid_document_title(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > 220
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "wps" | "wps office" | "word" | "microsoft word" | "excel" | "microsoft excel"
                | "powerpoint" | "microsoft powerpoint" | "onenote" | "microsoft onenote"
                | "outlook" | "adobe acrobat" | "acrobat reader" | "typora" | "obsidian"
                | "notepad" | "notepad++"
        )
    {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(windows)]
fn valid_selected_document_title(value: &str) -> Option<String> {
    let title = valid_document_title(value)?;
    let normalized = title.to_ascii_lowercase();
    [
        ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx", ".pdf", ".txt", ".md",
        ".csv", ".rtf",
    ]
    .iter()
    .any(|extension| normalized.contains(extension))
    .then_some(title)
}

#[cfg(windows)]
fn visible_wps_document_title() -> Option<String> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible,
    };

    struct Search {
        title: Option<String>,
    }

    unsafe extern "system" fn visit(window: HWND, state: LPARAM) -> BOOL {
        let search = unsafe { &mut *(state.0 as *mut Search) };
        if !unsafe { IsWindowVisible(window) }.as_bool() {
            return BOOL(1);
        }
        let mut pid = 0u32;
        let _ = unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        let app = unsafe { process_image_path(pid) }.ok().and_then(|path| {
            std::path::Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        });
        let Some(app) = app.filter(|app| is_wps_app(app)) else {
            return BOOL(1);
        };
        let length = unsafe { GetWindowTextLengthW(window) }.max(0) as usize;
        if length == 0 {
            return BOOL(1);
        }
        let mut buffer = vec![0u16; length + 1];
        let copied = unsafe { GetWindowTextW(window, &mut buffer) }.max(0) as usize;
        let native_title = String::from_utf16_lossy(&buffer[..copied]);
        if let Some(title) = clean_document_title(&native_title, &app)
            .and_then(|title| valid_selected_document_title(&title))
        {
            search.title = Some(title);
            return BOOL(0);
        }
        BOOL(1)
    }

    let mut search = Search { title: None };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM((&mut search as *mut Search) as isize));
    }
    search.title
}

#[cfg(windows)]
fn valid_chatgpt_title(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.chars().count() <= 220
        && !matches!(
            value,
            "ChatGPT"
                | "Codex"
                | "任务操作"
                | "对话操作"
                | "聊天操作"
                | "会话操作"
                | "Task options"
                | "Chat options"
                | "Conversation options"
        )
}

#[cfg(not(windows))]
fn capture_foreground_event() -> Result<RawActivityEvent> {
    Err(anyhow!("foreground metadata collector is currently implemented for Windows"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_contexts_share_one_signature() {
        let mut event = RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: String::new(),
            app: Some("one.exe".into()),
            window_title: Some("One".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats { idle_seconds: 300, ..Default::default() },
            metadata: json!({}),
        };
        let first = context_signature(&event, 180);
        event.app = Some("two.exe".into());
        assert_eq!(first, context_signature(&event, 180));
    }

    #[test]
    fn qq_main_window_and_image_viewer_share_one_signature() {
        let mut event = RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: String::new(),
            app: Some("QQ.exe".into()),
            window_title: Some("QQ".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({}),
        };
        let main = context_signature(&event, 180);
        event.window_title = Some("图片查看器".into());
        assert_eq!(main, context_signature(&event, 180));

        event.window_title = Some("QQ设置".into());
        assert_ne!(main, context_signature(&event, 180));
    }

    #[test]
    fn detects_suspend_sized_observation_gaps() {
        assert!(!is_unexpected_observation_gap(Duration::from_secs(12), 2));
        assert!(!is_unexpected_observation_gap(Duration::from_secs(60), 15));
        assert!(is_unexpected_observation_gap(Duration::from_secs(61), 2));
        assert!(is_unexpected_observation_gap(Duration::from_secs(61), 15));
    }

    #[test]
    fn requires_five_continuous_seconds_before_switching() {
        let event = |title: &str, timestamp: &str| RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: timestamp.into(),
            app: Some("chrome.exe".into()),
            window_title: Some(title.into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({}),
        };
        let mut pending = None;
        assert!(!observe_pending_context(
            &mut pending,
            "search",
            &event("Google Scholar", "2026-07-12T10:00:10Z"),
        ));
        assert_eq!(
            pending.as_ref().map(|candidate| candidate.first_event.timestamp.as_str()),
            Some("2026-07-12T10:00:10Z"),
        );
        assert!(!observe_pending_context(
            &mut pending,
            "search",
            &event("Google Scholar", "2026-07-12T10:00:14Z"),
        ));
        assert!(observe_pending_context(
            &mut pending,
            "search",
            &event("Google Scholar", "2026-07-12T10:00:15Z"),
        ));
        assert_eq!(pending.as_ref().map(|candidate| candidate.observations), Some(3));
        assert_eq!(
            pending.as_ref().map(|candidate| candidate.first_event.timestamp.as_str()),
            Some("2026-07-12T10:00:10Z"),
        );
    }

    #[test]
    fn a_different_transient_context_restarts_switch_confirmation() {
        let mut pending = Some(PendingContext {
            signature: "loading".into(),
            first_event: RawActivityEvent {
                id: String::new(),
                source: "test".into(),
                timestamp: "2026-07-12T10:00:10Z".into(),
                app: Some("chrome.exe".into()),
                window_title: Some("Loading".into()),
                url: None,
                file_path: None,
                workspace: None,
                input_stats: InputStats::default(),
                metadata: json!({}),
            },
            observations: 1,
        });
        let search = RawActivityEvent {
            timestamp: "2026-07-12T10:00:20Z".into(),
            window_title: Some("Google Scholar".into()),
            ..pending.as_ref().unwrap().first_event.clone()
        };
        assert!(!observe_pending_context(&mut pending, "search", &search));
        let candidate = pending.expect("new candidate");
        assert_eq!(candidate.signature, "search");
        assert_eq!(candidate.observations, 1);
        assert_eq!(candidate.first_event.timestamp, "2026-07-12T10:00:20Z");
    }

    #[test]
    fn only_confirmed_video_playback_counts_as_passive_attention() {
        let event = RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: String::new(),
            app: Some("msedge.exe".into()),
            window_title: Some("系统设计课程 - Bilibili".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats { idle_seconds: 900, ..Default::default() },
            metadata: json!({ "browser": { "videoPlaying": true } }),
        };
        assert!(is_passive_attention(&event));
        assert!(!is_passive_attention(&RawActivityEvent {
            app: Some("Zoom.exe".into()),
            window_title: Some("Weekly sync".into()),
            metadata: json!({}),
            ..event.clone()
        }));
        assert!(is_passive_attention(&RawActivityEvent {
            app: Some("vlc.exe".into()),
            window_title: Some("recording.mp4".into()),
            metadata: json!({}),
            ..event.clone()
        }));
        assert!(!is_passive_attention(&RawActivityEvent {
            app: Some("notepad.exe".into()),
            window_title: Some("notes.txt".into()),
            metadata: json!({}),
            ..event
        }));
    }

    #[test]
    fn idle_boundary_is_backdated_to_last_input_but_not_before_context_start() {
        let event = RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: "2026-07-12T10:05:00Z".into(),
            app: Some("QQ.exe".into()),
            window_title: Some("QQ".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats { idle_seconds: 180, ..Default::default() },
            metadata: json!({}),
        };
        assert_eq!(idle_boundary_at(&event, "2026-07-12T10:00:00Z"), "2026-07-12T10:02:00Z");
        assert_eq!(idle_boundary_at(&event, "2026-07-12T10:04:00Z"), "2026-07-12T10:04:00Z");
    }

    #[test]
    fn windows_task_view_is_a_handoff_surface() {
        let event = RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: "2026-07-12T10:00:00Z".into(),
            app: Some("explorer.exe".into()),
            window_title: Some("任务视图".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({}),
        };
        assert!(is_windows_task_view(&event));
        assert!(!is_windows_task_view(&RawActivityEvent {
            app: Some("chrome.exe".into()),
            ..event
        }));
    }

    #[cfg(windows)]
    #[test]
    fn chatgpt_title_filter_rejects_only_generic_chrome() {
        assert!(valid_chatgpt_title("自动记录时间优化"));
        assert!(!valid_chatgpt_title("ChatGPT"));
        assert!(!valid_chatgpt_title("  "));
    }

    #[cfg(windows)]
    #[test]
    fn office_titles_are_reduced_to_the_active_document() {
        assert_eq!(
            clean_document_title("刘雨薇_课程成绩.xlsx - WPS Office", "wps.exe"),
            Some("刘雨薇_课程成绩.xlsx".into())
        );
        assert_eq!(
            clean_document_title("ICPC 训练计划.docx - Microsoft Word", "WINWORD.EXE"),
            Some("ICPC 训练计划.docx".into())
        );
        assert_eq!(clean_document_title("WPS Office", "et.exe"), None);
        assert_eq!(valid_selected_document_title("开始"), None);
        assert_eq!(
            valid_selected_document_title("ICPC 训练计划.docx"),
            Some("ICPC 训练计划.docx".into())
        );
    }

    #[cfg(windows)]
    #[test]
    fn supported_page_apps_cover_chat_and_office_tools() {
        assert!(is_chat_workspace_app("ChatGPT.exe"));
        assert!(is_chat_workspace_app("codex.exe"));
        assert!(is_document_app("wps.exe"));
        assert!(is_document_app("EXCEL.EXE"));
        assert!(!is_document_app("steam.exe"));
    }
}
