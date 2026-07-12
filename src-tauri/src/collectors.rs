use crate::classification;
use crate::context_store;
use crate::db::{now, AppDb};
use crate::models::{InputStats, RawActivityEvent};
use anyhow::{anyhow, Result};
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
    signature: String,
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

const SWITCH_CONFIRM_OBSERVATIONS: u8 = 2;

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
                sanitize_event(&mut event);
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
                }
                let signature = context_signature(&event, settings.idle_threshold_seconds);
                if active.is_none() {
                    pending = None;
                    match open_context(&collector, &db, event, signature) {
                        Ok(context) => active = Some(context),
                        Err(error) => collector.set_error(error),
                    }
                } else {
                    let active_signature = active.as_ref().map(|current| current.signature.clone()).unwrap_or_default();
                    if active_signature == signature {
                        pending = None;
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
                        let immediate = active_signature == "idle" || signature == "idle";
                        let ready = if immediate {
                            pending = Some(PendingContext {
                                signature: signature.clone(),
                                first_event: event.clone(),
                                observations: SWITCH_CONFIRM_OBSERVATIONS,
                            });
                            true
                        } else {
                            observe_pending_context(&mut pending, &signature, &event)
                        };

                        if ready {
                            let Some(mut next) = pending.take() else { continue; };
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

                let idle = active
                    .as_ref()
                    .map(|current| current.event.input_stats.idle_seconds >= settings.idle_threshold_seconds as u64)
                    .unwrap_or(false);
                let poll_seconds = if idle {
                    settings.poll_interval_seconds.max(10)
                } else {
                    settings.poll_interval_seconds
                };
                sleep(Duration::from_secs(poll_seconds as u64)).await;
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
    pending
        .as_ref()
        .is_some_and(|candidate| candidate.observations >= SWITCH_CONFIRM_OBSERVATIONS)
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
    classification::ingest_event(db, &event)?;
    collector.clear_error();
    *collector.last_event_at.lock() = Some(event.timestamp.clone());
    let observed_at = Instant::now();
    Ok(ActiveContext {
        id: event.id.clone(),
        signature,
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
    classification::ingest_event(db, &event)?;
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
    classification::ingest_event(db, &event)?;
    classification::finalize_context(db, &event)?;
    *collector.last_event_at.lock() = Some(event.timestamp);
    Ok(())
}

fn is_unexpected_observation_gap(elapsed: Duration, poll_interval_seconds: u32) -> bool {
    let expected = u64::from(poll_interval_seconds.max(10));
    elapsed > Duration::from_secs(expected.saturating_mul(4).max(60))
}

fn context_signature(event: &RawActivityEvent, idle_threshold_seconds: u32) -> String {
    if event.input_stats.idle_seconds >= idle_threshold_seconds as u64 {
        return "idle".into();
    }
    format!(
        "{}|{}|{}|{}|{}",
        event.app.as_deref().unwrap_or_default().to_lowercase(),
        event.window_title.as_deref().unwrap_or_default(),
        event.url.as_deref().unwrap_or_default(),
        event.file_path.as_deref().unwrap_or_default(),
        event.workspace.as_deref().unwrap_or_default(),
    )
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
        let title = String::from_utf16_lossy(&buffer[..copied.max(0) as usize]);

        let mut pid = 0u32;
        let _ = GetWindowThreadProcessId(window, Some(&mut pid));
        let executable = process_image_path(pid).ok();
        let app = executable
            .as_ref()
            .and_then(|path| PathBuf::from(path).file_name().map(|name| name.to_string_lossy().to_string()))
            .unwrap_or_else(|| format!("pid:{pid}"));

        let mut last_input = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        let idle_seconds = if GetLastInputInfo(&mut last_input).as_bool() {
            GetTickCount().saturating_sub(last_input.dwTime) as u64 / 1000
        } else {
            0
        };

        Ok(RawActivityEvent {
            id: String::new(),
            source: "windows-foreground".into(),
            timestamp: now(),
            app: Some(app),
            window_title: Some(title),
            url: None,
            file_path: executable,
            workspace: None,
            input_stats: InputStats {
                idle_seconds,
                ..Default::default()
            },
            metadata: json!({ "pid": pid, "platform": "windows", "capture": "metadata-only" }),
        })
    }
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
    fn detects_suspend_sized_observation_gaps() {
        assert!(!is_unexpected_observation_gap(Duration::from_secs(12), 2));
        assert!(!is_unexpected_observation_gap(Duration::from_secs(60), 15));
        assert!(is_unexpected_observation_gap(Duration::from_secs(61), 2));
        assert!(is_unexpected_observation_gap(Duration::from_secs(61), 15));
    }

    #[test]
    fn requires_two_consecutive_observations_before_switching() {
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
        assert!(observe_pending_context(
            &mut pending,
            "search",
            &event("Google Scholar", "2026-07-12T10:00:20Z"),
        ));
        assert_eq!(pending.as_ref().map(|candidate| candidate.observations), Some(2));
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
}
