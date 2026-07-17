use crate::classification;
use crate::context_store;
use crate::db::{now, AppDb};
use crate::models::{InputStats, RawActivityEvent};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use parking_lot::Mutex;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;
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
    pub manual_away: bool,
    pub last_event_at: Option<String>,
    pub last_error: Option<String>,
}

pub struct DesktopCollector {
    running: AtomicBool,
    generation: AtomicU64,
    lifecycle: Mutex<()>,
    wake: Notify,
    manual_away: AtomicBool,
    manual_away_started_at: Mutex<Option<String>>,
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

#[derive(Debug, Clone)]
struct ManualAwayReturn {
    first_event: RawActivityEvent,
    latest_event: RawActivityEvent,
}

#[derive(Debug, Clone)]
struct TransitionHandoff {
    started_at: String,
    source: &'static str,
}

const SWITCH_CONFIRM_SECONDS: i64 = 5;
const MANUAL_AWAY_RETURN_SECONDS: i64 = 5;
const MANUAL_AWAY_POLL_SECONDS: u32 = 1;

impl DesktopCollector {
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            lifecycle: Mutex::new(()),
            wake: Notify::new(),
            manual_away: AtomicBool::new(false),
            manual_away_started_at: Mutex::new(None),
            last_event_at: Mutex::new(None),
            last_error: Mutex::new(None),
        }
    }

    fn set_error(&self, error: impl ToString) {
        *self.last_error.lock() = Some(error.to_string());
    }

    pub fn report_error(&self, error: impl ToString) {
        self.set_error(error);
    }

    fn clear_error(&self) {
        *self.last_error.lock() = None;
    }

    pub fn begin_manual_away(&self) {
        if !self.manual_away.swap(true, Ordering::SeqCst) {
            *self.manual_away_started_at.lock() = Some(now());
        }
        self.wake.notify_one();
    }

    pub fn cancel_manual_away(&self) {
        self.manual_away.store(false, Ordering::SeqCst);
        *self.manual_away_started_at.lock() = None;
        self.wake.notify_one();
    }

    pub fn is_manual_away(&self) -> bool {
        self.manual_away.load(Ordering::SeqCst)
    }

    async fn wait_for_next_poll(&self, seconds: u64) {
        tokio::select! {
            _ = sleep(Duration::from_secs(seconds.max(1))) => {}
            _ = self.wake.notified() => {}
        }
    }
}

impl CollectorAdapter for Arc<DesktopCollector> {
    fn start(&self, db: Arc<AppDb>) -> Result<()> {
        let _lifecycle = self.lifecycle.lock();
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;

        let collector = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut active: Option<ActiveContext> = None;
            let mut pending: Option<PendingContext> = None;
            let mut manual_away_return: Option<ManualAwayReturn> = None;
            let mut handoff: Option<TransitionHandoff> = None;
            let mut settings = db.get_settings().unwrap_or_default().normalized();
            let mut settings_loaded_at = Instant::now();

            loop {
                if !collector.running.load(Ordering::SeqCst)
                    || collector.generation.load(Ordering::SeqCst) != generation
                {
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
                        collector.wait_for_next_poll(5).await;
                        continue;
                    }
                };
                context_store::enrich_event(&mut event);
                let manual_away = collector.is_manual_away();
                if !manual_away {
                    manual_away_return = None;
                    let passive_attention = settings
                        .passive_content_counts_as_active
                        .then(|| passive_attention_reason(&event))
                        .flatten();
                    if let Some(passive_attention_reason) = passive_attention.filter(|_| {
                        event.input_stats.idle_seconds >= settings.idle_threshold_seconds as u64
                    }) {
                        let input_idle_seconds = event.input_stats.idle_seconds;
                        mark_metadata(
                            &mut event,
                            "inputIdleSeconds",
                            serde_json::Value::from(input_idle_seconds),
                        );
                        mark_metadata(
                            &mut event,
                            "passiveAttention",
                            serde_json::Value::Bool(true),
                        );
                        mark_metadata(
                            &mut event,
                            "passiveAttentionReason",
                            serde_json::Value::String(passive_attention_reason.into()),
                        );
                        event.input_stats.idle_seconds = 0;
                    }
                }
                sanitize_event(&mut event);

                if manual_away {
                    if observe_manual_away_return(
                        &mut manual_away_return,
                        &event,
                        MANUAL_AWAY_POLL_SECONDS,
                    ) {
                        let Some(candidate) = manual_away_return.take() else {
                            continue;
                        };
                        collector.cancel_manual_away();
                        pending = None;
                        handoff = None;

                        let boundary = candidate.first_event.timestamp.clone();
                        let mut start_event = candidate.latest_event.clone();
                        start_event.timestamp = boundary.clone();
                        mark_metadata(
                            &mut start_event,
                            "manualAwayReturn",
                            serde_json::Value::Bool(true),
                        );
                        mark_metadata(
                            &mut start_event,
                            "manualAwayReturnConfirmedSeconds",
                            serde_json::Value::from(MANUAL_AWAY_RETURN_SECONDS),
                        );
                        let signature =
                            context_signature(&start_event, settings.idle_threshold_seconds);
                        if let Some(previous) = active.take() {
                            if let Err(error) =
                                close_context_at(&collector, &db, previous, boundary.clone())
                            {
                                collector.set_error(error);
                            }
                        }
                        match open_context(&collector, &db, start_event, signature) {
                            Ok(mut context) => {
                                if candidate.latest_event.timestamp > boundary {
                                    let mut heartbeat_event = candidate.latest_event;
                                    mark_metadata(
                                        &mut heartbeat_event,
                                        "manualAwayReturn",
                                        serde_json::Value::Bool(true),
                                    );
                                    if let Err(error) = heartbeat_context(
                                        &collector,
                                        &db,
                                        &mut context,
                                        heartbeat_event,
                                    ) {
                                        collector.set_error(error);
                                    }
                                }
                                active = Some(context);
                            }
                            Err(error) => collector.set_error(error),
                        }
                        collector
                            .wait_for_next_poll(u64::from(MANUAL_AWAY_POLL_SECONDS))
                            .await;
                        continue;
                    }

                    let manual_started_at = collector.manual_away_started_at.lock().take();
                    force_manual_away_event(
                        &mut event,
                        settings.idle_threshold_seconds,
                        manual_started_at.as_deref(),
                    );
                }

                // Transition surfaces are not standalone activities. Keep their first
                // timestamp and assign that interval to the semantic context selected next.
                let handoff_source = if is_windows_task_view(&event) {
                    Some("windows-task-view")
                } else if is_incomplete_chat_workspace_handoff(active.as_ref(), &event) {
                    Some("chat-workspace-loading")
                } else {
                    None
                };
                if let Some(source) = handoff_source {
                    handoff.get_or_insert_with(|| TransitionHandoff {
                        started_at: event.timestamp.clone(),
                        source,
                    });
                    pending = None;
                    if let Some(current) = active.as_mut() {
                        current.last_observed_at = Instant::now();
                    }
                    collector
                        .wait_for_next_poll(if manual_away {
                            u64::from(MANUAL_AWAY_POLL_SECONDS)
                        } else {
                            settings.poll_interval_seconds as u64
                        })
                        .await;
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
                    handoff = None;
                }
                let signature = context_signature(&event, settings.idle_threshold_seconds);
                if active.is_none() {
                    pending = None;
                    if let Some(boundary) = handoff.take() {
                        apply_transition_handoff(&mut event, &boundary);
                    }
                    match open_context(&collector, &db, event, signature) {
                        Ok(context) => active = Some(context),
                        Err(error) => collector.set_error(error),
                    }
                } else {
                    let active_signature = active.as_ref().map(|current| current.signature.clone()).unwrap_or_default();
                    if should_inherit_active_context(&active_signature, &signature, &event) {
                        pending = None;
                        handoff = None;
                        if let Some(current) = active.as_mut() {
                            current.last_observed_at = Instant::now();
                            let inherited_event =
                                inherit_active_context_event(&current.event, event);
                            let heartbeat_due = current.last_emitted_at.elapsed()
                                >= Duration::from_secs(settings.heartbeat_seconds as u64);
                            if heartbeat_due {
                                if let Err(error) = heartbeat_context(
                                    &collector,
                                    &db,
                                    current,
                                    inherited_event,
                                ) {
                                    collector.set_error(error);
                                }
                            } else {
                                current.event = inherited_event;
                            }
                        }
                    } else if active_signature == signature {
                        pending = None;
                        handoff = None;
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
                            if let Some(boundary) = handoff.as_ref() {
                                apply_transition_handoff(&mut transition_event, boundary);
                            }
                        }
                        // Enter idle immediately because its boundary is backdated. Leaving
                        // idle still needs five continuous seconds, preventing tiny blocks.
                        let immediate = signature == "idle";
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
                            handoff = None;
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

                collector
                    .wait_for_next_poll(if manual_away {
                        u64::from(MANUAL_AWAY_POLL_SECONDS)
                    } else {
                        settings.poll_interval_seconds as u64
                    })
                    .await;
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        let _lifecycle = self.lifecycle.lock();
        self.running.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.wake.notify_one();
        Ok(())
    }

    fn health(&self) -> CollectorHealth {
        CollectorHealth {
            running: self.running.load(Ordering::SeqCst),
            manual_away: self.is_manual_away(),
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

fn observe_manual_away_return(
    candidate: &mut Option<ManualAwayReturn>,
    event: &RawActivityEvent,
    poll_interval_seconds: u32,
) -> bool {
    let input_idle_milliseconds = event
        .metadata
        .get("inputIdleMilliseconds")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| event.input_stats.idle_seconds.saturating_mul(1000));
    let recent_input_limit = u64::from(poll_interval_seconds.max(1))
        .saturating_mul(1000)
        .saturating_add(250);
    if input_idle_milliseconds > recent_input_limit {
        *candidate = None;
        return false;
    }

    match candidate {
        Some(current) => current.latest_event = event.clone(),
        None => {
            *candidate = Some(ManualAwayReturn {
                first_event: event.clone(),
                latest_event: event.clone(),
            });
        }
    }
    candidate.as_ref().is_some_and(|current| {
        elapsed_seconds(
            &current.first_event.timestamp,
            &current.latest_event.timestamp,
        )
        .is_some_and(|seconds| seconds >= MANUAL_AWAY_RETURN_SECONDS)
    })
}

fn force_manual_away_event(
    event: &mut RawActivityEvent,
    idle_threshold_seconds: u32,
    started_at: Option<&str>,
) {
    let input_idle_seconds = event.input_stats.idle_seconds;
    mark_metadata(
        event,
        "inputIdleSeconds",
        serde_json::Value::from(input_idle_seconds),
    );
    mark_metadata(event, "manualAway", serde_json::Value::Bool(true));
    if let Some(started_at) = started_at.filter(|value| !value.trim().is_empty()) {
        event.timestamp = started_at.to_string();
        mark_metadata(
            event,
            "manualAwayStartedAt",
            serde_json::Value::String(started_at.to_string()),
        );
    }
    event.input_stats.idle_seconds = u64::from(idle_threshold_seconds.max(1));
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
    if let Some(session) = classification::finalize_context(db, &event, &previous.session_id)? {
        db.absorb_short_auto_session(&session.id)?;
    }
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
    if matches!(app.as_str(), "searchhost.exe" | "searchapp.exe")
        && matches!(title.as_str(), "" | "search" | "搜索")
    {
        return true;
    }
    if app == "startmenuexperiencehost.exe"
        && matches!(title.as_str(), "" | "start" | "开始")
    {
        return true;
    }
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

fn is_incomplete_chat_workspace_handoff(
    active: Option<&ActiveContext>,
    event: &RawActivityEvent,
) -> bool {
    let Some(active) = active else { return false; };
    let app = event
        .app
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let active_app = active
        .event
        .app
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if app != active_app || !matches!(app.as_str(), "chatgpt" | "chatgpt.exe" | "codex" | "codex.exe") {
        return false;
    }
    let active_has_page = active
        .event
        .metadata
        .get("activePageTitle")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let observed_has_page = event
        .metadata
        .get("activePageTitle")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let title = event
        .window_title
        .as_deref()
        .unwrap_or_default()
        .trim();
    active_has_page
        && !observed_has_page
        && matches!(title.to_ascii_lowercase().as_str(), "" | "chatgpt" | "codex")
}

fn apply_transition_handoff(event: &mut RawActivityEvent, handoff: &TransitionHandoff) {
    event.timestamp.clone_from(&handoff.started_at);
    mark_metadata(
        event,
        "contextHandoff",
        serde_json::Value::String(handoff.source.into()),
    );
    if handoff.source == "windows-task-view" {
        mark_metadata(event, "taskViewHandoff", serde_json::Value::Bool(true));
    }
}

fn should_inherit_active_context(
    active_signature: &str,
    observed_signature: &str,
    event: &RawActivityEvent,
) -> bool {
    !active_signature.is_empty()
        && active_signature != "idle"
        && observed_signature != "idle"
        && (is_task_overlay_app(event)
            || is_chat_auxiliary_overlay(active_signature, event))
}

fn inherit_active_context_event(
    current: &RawActivityEvent,
    observed: RawActivityEvent,
) -> RawActivityEvent {
    let mut inherited = current.clone();
    inherited.timestamp = observed.timestamp;
    inherited.input_stats = observed.input_stats;
    mark_metadata(
        &mut inherited,
        "transientOverlay",
        json!({
            "app": observed.app,
            "title": observed.window_title,
        }),
    );
    inherited
}

fn is_task_overlay_app(event: &RawActivityEvent) -> bool {
    let app = event
        .app
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let app = app.trim_end_matches(".exe");
    if matches!(
        app,
        "snipaste"
            | "snippingtool"
            | "screenclippinghost"
            | "qqscreenshot"
            | "qqscreenclip"
            | "qqsc"
            | "sharex"
            | "greenshot"
            | "picpick"
            | "lightshot"
            | "flameshot"
    ) {
        return true;
    }
    let title = event
        .window_title
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    is_screenshot_overlay_title(&title)
}

fn is_screenshot_overlay_title(title: &str) -> bool {
    let compact = title
        .trim()
        .to_lowercase()
        .chars()
        .filter(|character| {
            !character.is_whitespace()
                && !matches!(character, '·' | '•' | '-' | '—' | '_' | ':' | '：')
        })
        .collect::<String>();
    let compact = ["qq", "微信", "wechat", "weixin", "钉钉", "dingtalk"]
        .iter()
        .find_map(|prefix| compact.strip_prefix(prefix))
        .unwrap_or(&compact);
    matches!(
        compact,
        "截图"
            | "qq截图"
            | "截屏"
            | "屏幕截图"
            | "屏幕截取"
            | "截图工具"
            | "截图编辑"
            | "screenshot"
            | "screencapture"
            | "screenclipping"
            | "snippingtool"
            | "snippersnipaste"
    )
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

fn passive_attention_reason(event: &RawActivityEvent) -> Option<&'static str> {
    let app = event.app.as_deref().unwrap_or_default().to_lowercase();
    let title = event.window_title.as_deref().unwrap_or_default().trim().to_lowercase();
    let url = event.url.as_deref().unwrap_or_default().to_lowercase();
    let browser_video_playing = event
        .metadata
        .get("browser")
        .and_then(|browser| browser.get("videoPlaying"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if browser_video_playing {
        return Some("browser-video-playing");
    }

    let local_media_playing = event
        .metadata
        .get("mediaPlaying")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if local_media_playing {
        return Some("media-player-foreground");
    }

    let meeting_controls_active = event
        .metadata
        .get("meetingActive")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if meeting_controls_active {
        return Some("meeting-app-foreground");
    }

    let meeting_marker = [
        "会议",
        "meeting",
        "webinar",
        "通话",
        "正在共享",
        "共享屏幕",
        "screen sharing",
        "飞书妙记",
    ]
    .iter()
    .any(|needle| title.contains(needle));
    let dedicated_meeting_app = [
        "wemeetapp",
        "voovmeeting",
        "tencentmeeting",
        "zoom.exe",
        "ciscocollabhost",
        "webexhost",
        "webexmta",
    ]
    .iter()
    .any(|needle| app.contains(needle));
    if dedicated_meeting_app && meeting_marker {
        return Some("meeting-app-foreground");
    }

    let collaboration_app = [
        "teams",
        "ms-teams",
        "feishu",
        "lark",
        "dingtalk",
        "wecom",
        "wxwork",
        "skype",
        "qq",
        "weixin",
        "wechat",
        "slack",
        "discord",
    ]
    .iter()
    .any(|needle| app.contains(needle));
    if collaboration_app && meeting_marker {
        return Some("collaboration-meeting-foreground");
    }

    let meeting_url = [
        "meet.google.com",
        "meeting.tencent.com",
        "voovmeeting.com",
        "zoom.us/wc/",
        "teams.microsoft.com/l/meetup",
        "vc.feishu.cn",
    ]
    .iter()
    .any(|needle| url.contains(needle));
    let browser_app = [
        "chrome",
        "msedge",
        "firefox",
        "brave",
        "opera",
        "vivaldi",
        "arc.exe",
        "tabbit",
    ]
    .iter()
    .any(|needle| app.contains(needle));
    let meeting_browser_title = browser_app
        && [
            "google meet",
            "腾讯会议",
            "voov meeting",
            "zoom meeting",
            "microsoft teams meeting",
            "飞书会议",
            "钉钉会议",
            "cisco webex",
        ]
        .iter()
        .any(|needle| title.contains(needle));
    if meeting_url || meeting_browser_title {
        return Some("browser-meeting-foreground");
    }

    None
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
fn hosted_window_process(
    window: windows::Win32::Foundation::HWND,
    host_pid: u32,
) -> Option<(u32, String)> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, GetWindowThreadProcessId,
    };

    struct SearchState {
        host_pid: u32,
        result: Option<(u32, String)>,
    }

    unsafe extern "system" fn visit(child: HWND, parameter: LPARAM) -> BOOL {
        let state = &mut *(parameter.0 as *mut SearchState);
        let mut pid = 0u32;
        let _ = GetWindowThreadProcessId(child, Some(&mut pid));
        if pid == 0 || pid == state.host_pid {
            return BOOL(1);
        }
        let Ok(path) = process_image_path(pid) else {
            return BOOL(1);
        };
        if !is_hosted_app_candidate(&path) {
            return BOOL(1);
        }
        state.result = Some((pid, path));
        BOOL(0)
    }

    let mut state = SearchState {
        host_pid,
        result: None,
    };
    unsafe {
        let _ = EnumChildWindows(
            window,
            Some(visit),
            LPARAM((&mut state as *mut SearchState) as isize),
        );
    }
    state.result
}

#[cfg(windows)]
fn is_hosted_app_candidate(path: &str) -> bool {
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path)
        .to_lowercase();
    ![
        "applicationframehost.exe",
        "shellexperiencehost.exe",
        "startmenuexperiencehost.exe",
        "searchhost.exe",
        "textinputhost.exe",
        "runtimebroker.exe",
        "dwm.exe",
        "ctfmon.exe",
    ]
    .contains(&name.as_str())
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
        let mut app = executable
            .as_ref()
            .and_then(|path| PathBuf::from(path).file_name().map(|name| name.to_string_lossy().to_string()))
            .unwrap_or_else(|| format!("pid:{pid}"));
        if app.eq_ignore_ascii_case("ApplicationFrameHost.exe") {
            if let Some((hosted_pid, hosted_path)) = hosted_window_process(window, pid) {
                pid = hosted_pid;
                app = PathBuf::from(&hosted_path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| format!("pid:{hosted_pid}"));
            }
        }
        let page_context = active_page_context(window, &app, &native_title);
        let title = page_context
            .as_ref()
            .map(|context| context.title.clone())
            .unwrap_or_else(|| native_title.clone());

        let mut last_input = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        let idle_milliseconds = if GetLastInputInfo(&mut last_input).as_bool() {
            GetTickCount().wrapping_sub(last_input.dwTime) as u64
        } else {
            0
        };
        let idle_seconds = idle_milliseconds / 1000;

        let mut metadata = json!({ "pid": pid, "platform": "windows", "capture": "metadata-only" });
        metadata["inputIdleMilliseconds"] = serde_json::Value::from(idle_milliseconds);
        if is_media_app(&app)
            || (is_chat_client_app(&app) && looks_like_media_window(&native_title))
        {
            if let Some(playing) = foreground_media_playing(window).ok().flatten() {
                metadata["mediaPlaying"] = serde_json::Value::Bool(playing);
            }
        }
        if (is_meeting_app(&app) || is_chat_client_app(&app))
            && foreground_meeting_active(window).unwrap_or(false)
        {
            metadata["meetingActive"] = serde_json::Value::Bool(true);
        }
        let workspace = page_context
            .as_ref()
            .and_then(|context| context.workspace.clone());
        if let Some(context) = page_context {
            metadata["nativeWindowTitle"] = serde_json::Value::String(native_title);
            metadata["activePageTitle"] = serde_json::Value::String(context.title.clone());
            metadata["activePageSource"] = serde_json::Value::String(context.source.into());
            metadata["activeContextType"] = serde_json::Value::String(context.kind.into());
            if context.kind == "conversation" {
                metadata["conversationTitle"] =
                    serde_json::Value::String(context.title.clone());
            }
            if is_openai_conversation_source(context.source) {
                metadata["chatgptConversationTitle"] = serde_json::Value::String(
                    raw_openai_conversation_title(
                        context.source,
                        &context.title,
                        context.workspace.as_deref(),
                    ),
                );
            }
            if context.source == "qq-conversation-header" {
                metadata["qqConversationTitle"] =
                    serde_json::Value::String(context.title.clone());
            }
            if is_openai_conversation_source(context.source) {
                if let Some(project) = context.workspace {
                    metadata["chatgptProject"] = serde_json::Value::String(project);
                }
            }
        }

        Ok(RawActivityEvent {
            id: String::new(),
            source: "windows-foreground".into(),
            timestamp: now(),
            app: Some(app),
            window_title: Some(title),
            url: None,
            // The process executable identifies the app, not the document being
            // worked on. Real files are supplied by editor/document integrations.
            file_path: None,
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
    workspace: Option<String>,
    source: &'static str,
    kind: &'static str,
}

#[cfg(windows)]
fn foreground_media_playing(
    window: windows::Win32::Foundation::HWND,
) -> Result<Option<bool>> {
    foreground_accessible_state(
        window,
        &[
            "暂停",
            "暂停播放",
            "Pause",
            "Pause playback",
            "Pause video",
        ],
        &[
            "播放",
            "继续播放",
            "Play",
            "Play video",
            "Resume playback",
        ],
    )
}

#[cfg(windows)]
fn foreground_meeting_active(window: windows::Win32::Foundation::HWND) -> Result<bool> {
    Ok(foreground_accessible_state(
        window,
        &[
            "离开会议",
            "结束会议",
            "结束通话",
            "挂断",
            "Leave meeting",
            "End meeting",
            "End call",
            "Hang up",
        ],
        &[],
    )?
    .unwrap_or(false))
}

#[cfg(windows)]
fn foreground_accessible_state(
    window: windows::Win32::Foundation::HWND,
    positive_names: &[&str],
    negative_names: &[&str],
) -> Result<Option<bool>> {
    use windows::core::VARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_ButtonControlTypeId,
        UIA_ControlTypePropertyId,
    };

    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> windows::core::Result<Option<bool>> {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = automation.ElementFromHandle(window)?;
            let condition = automation.CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_ButtonControlTypeId.0),
            )?;
            let buttons = root.FindAll(TreeScope_Descendants, &condition)?;
            for index in 0..buttons.Length()? {
                let name = buttons
                    .GetElement(index)?
                    .CurrentName()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                for (names, state) in [(positive_names, true), (negative_names, false)] {
                    if names.iter().any(|candidate| name.eq_ignore_ascii_case(candidate)) {
                        return Ok(Some(state));
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

fn is_chat_auxiliary_overlay(active_signature: &str, event: &RawActivityEvent) -> bool {
    let active_app = active_signature
        .split('|')
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let active_app = active_app.trim_end_matches(".exe");
    let app = event
        .app
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let app = app.trim_end_matches(".exe");
    let title = event
        .window_title
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    (active_app == "qq"
        && app == "qq"
        && matches!(title.as_str(), "图片查看器" | "视频播放器"))
        || ((active_app == "weixin" || active_app == "wechat")
            && matches!(app, "wechatappex" | "wechatocr" | "wechatbrowser"))
        || ((active_app == "weixin" || active_app == "wechat")
            && (app == "weixin" || app == "wechat")
            && matches!(
                title.as_str(),
                "图片查看" | "图片查看器" | "视频播放器" | "文件预览"
            ))
}

#[cfg(windows)]
fn active_page_context(
    window: windows::Win32::Foundation::HWND,
    app: &str,
    native_title: &str,
) -> Option<ActivePageContext> {
    if is_qq_app(app) {
        if let Some(context) = qq_active_conversation_context(window).ok().flatten() {
            return Some(context);
        }
    }
    if is_chat_workspace_app(app) {
        if let Some(context) = chatgpt_selected_context(window).ok().flatten() {
            return Some(context);
        }
    }
    if is_browser_app_name(app) && looks_like_chatgpt_browser_window(native_title) {
        if let Some(context) = chatgpt_selected_context(window).ok().flatten() {
            return Some(context);
        }
    }
    if is_chat_client_app(app) {
        if let Some(context) = chat_header_context(window, app).ok().flatten() {
            return Some(context);
        }
        if let Some(context) = selected_page_context(window, app).ok().flatten() {
            return Some(context);
        }
    }
    if is_file_manager_app(app) {
        if let Some(context) = explorer_active_context(window, app, native_title).ok().flatten() {
            return Some(context);
        }
    }
    if is_document_app(app) {
        let native_document = clean_document_title(native_title, app);
        if let Some(document) = native_document.as_deref() {
            let (title, workspace) = document_title_and_workspace(document, app);
            return Some(ActivePageContext {
                title,
                workspace,
                source: "document-window-title",
                kind: if is_mail_app(app) {
                    "conversation"
                } else {
                    "document"
                },
            });
        }
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
        if let Some((title, source)) = selected_tab
            .map(|title| (title, "selected-document-tab"))
            .or_else(|| visible_wps_title.map(|title| (title, "wps-visible-window")))
        {
            return Some(ActivePageContext {
                title,
                workspace: None,
                source,
                kind: if is_mail_app(app) {
                    "conversation"
                } else {
                    "document"
                },
            });
        }
    }
    if let Some(title) = clean_native_page_title(native_title, app) {
        let (title, workspace) = editor_title_and_workspace(&title, app);
        return Some(ActivePageContext {
            title,
            workspace,
            source: "window-page-title",
            kind: native_context_kind_for_window(app, native_title),
        });
    }
    supports_selected_page_fallback(app)
        .then(|| selected_page_context(window, app).ok().flatten())
        .flatten()
}

#[cfg(windows)]
fn supports_selected_page_fallback(app: &str) -> bool {
    is_browser_app_name(app) || is_editor_app_name(app) || is_terminal_app(app)
}

#[cfg(windows)]
fn is_qq_app(app: &str) -> bool {
    app.eq_ignore_ascii_case("qq.exe") || app.eq_ignore_ascii_case("qq")
}

#[cfg(windows)]
fn normalized_app_name(app: &str) -> String {
    let name = std::path::Path::new(app.trim())
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(app)
        .trim()
        .to_lowercase();
    name.strip_suffix(".exe").unwrap_or(&name).to_string()
}

#[cfg(windows)]
fn is_chat_client_app(app: &str) -> bool {
    matches!(
        normalized_app_name(app).as_str(),
        "qq"
            | "weixin"
            | "wechat"
            | "wechatappex"
            | "wechatbrowser"
            | "dingtalk"
            | "dingtalklauncher"
            | "dingtalkapp"
            | "feishu"
            | "feishuapp"
            | "lark"
            | "larkapp"
            | "wxwork"
            | "wxworkweb"
            | "wecom"
            | "wecomweb"
            | "wework"
            | "teams"
            | "ms-teams"
            | "msteams"
            | "slack"
            | "discord"
            | "telegram"
            | "telegramdesktop"
            | "signal"
            | "whatsapp"
            | "line"
            | "skype"
            | "tim"
            | "messenger"
            | "mattermost"
            | "element"
            | "viber"
    )
}

#[cfg(windows)]
fn is_openai_conversation_source(source: &str) -> bool {
    matches!(
        source,
        "chatgpt-conversation" | "codex-project-task" | "codex-task"
    )
}

#[cfg(windows)]
fn raw_openai_conversation_title(
    source: &str,
    display_title: &str,
    project: Option<&str>,
) -> String {
    match source {
        "codex-project-task" => project
            .and_then(|project| display_title.strip_prefix(&format!("项目-{project}-")))
            .unwrap_or(display_title)
            .to_string(),
        "codex-task" => display_title
            .strip_prefix("任务-")
            .unwrap_or(display_title)
            .to_string(),
        _ => display_title.to_string(),
    }
}

#[cfg(windows)]
fn is_mail_app(app: &str) -> bool {
    matches!(
        normalized_app_name(app).as_str(),
        "outlook" | "olk" | "hxoutlook" | "thunderbird" | "foxmail"
    )
}

#[cfg(windows)]
fn is_browser_app_name(app: &str) -> bool {
    matches!(
        normalized_app_name(app).as_str(),
        "chrome"
            | "msedge"
            | "firefox"
            | "brave"
            | "vivaldi"
            | "opera"
            | "opera_gx"
            | "chromium"
            | "arc"
            | "tabbit browser"
            | "thorium"
            | "floorp"
            | "waterfox"
            | "librewolf"
            | "duckduckgo"
    )
}

#[cfg(windows)]
fn looks_like_chatgpt_browser_window(title: &str) -> bool {
    let title = title.to_lowercase();
    title.contains("chatgpt") || title.contains("chat.openai.com")
}

#[cfg(windows)]
fn is_file_manager_app(app: &str) -> bool {
    matches!(
        normalized_app_name(app).as_str(),
        "explorer"
            | "totalcmd64"
            | "totalcmd"
            | "files"
            | "directory opus"
            | "dopus"
            | "freecommander"
            | "doublecmd"
            | "everything"
    )
}

#[cfg(windows)]
fn is_editor_app_name(app: &str) -> bool {
    let app = normalized_app_name(app);
    matches!(
        app.as_str(),
        "code"
            | "code - insiders"
            | "code-insiders"
            | "cursor"
            | "windsurf"
            | "codium"
            | "devenv"
            | "idea64"
            | "idea"
            | "pycharm64"
            | "pycharm"
            | "webstorm64"
            | "webstorm"
            | "rustrover64"
            | "rustrover"
            | "clion64"
            | "clion"
            | "studio64"
            | "ida64"
            | "ida"
            | "ghidra"
            | "sublime_text"
            | "zed"
            | "rstudio"
            | "matlab"
            | "unity"
            | "unityhub"
            | "unrealeditor"
            | "godot"
            | "photoshop"
            | "illustrator"
            | "figma"
            | "blender"
            | "premiere pro"
            | "afterfx"
            | "acad"
            | "rider64"
            | "rider"
            | "eclipse"
            | "netbeans64"
            | "netbeans"
            | "qtcreator"
            | "codeblocks"
            | "devcpp"
            | "arduino ide"
            | "arduino"
            | "dbeaver"
            | "datagrip64"
            | "datagrip"
            | "navicat"
            | "postman"
            | "insomnia"
            | "fiddler"
            | "wireshark"
            | "burpsuite"
            | "docker desktop"
            | "githubdesktop"
            | "gitkraken"
            | "krita"
            | "inkscape"
            | "resolve"
            | "affinity photo 2"
            | "affinity designer 2"
            | "sketchup"
    )
}

#[cfg(windows)]
fn is_terminal_app(app: &str) -> bool {
    matches!(
        normalized_app_name(app).as_str(),
        "windowsterminal"
            | "pwsh"
            | "powershell"
            | "cmd"
            | "wezterm-gui"
            | "alacritty"
            | "kitty"
            | "mintty"
            | "conemu64"
            | "conemu"
            | "mobaxterm"
            | "xshell"
            | "putty"
            | "securecrt"
            | "git-bash"
            | "bash"
            | "wsl"
    )
}

#[cfg(windows)]
fn is_meeting_app(app: &str) -> bool {
    let app = normalized_app_name(app);
    [
        "wemeetapp",
        "voovmeeting",
        "tencentmeeting",
        "zoom",
        "ciscocollabhost",
        "webexhost",
        "webexmta",
    ]
    .iter()
    .any(|needle| app.contains(needle))
}

#[cfg(windows)]
fn is_media_app(app: &str) -> bool {
    let app = normalized_app_name(app);
    [
        "vlc",
        "mpv",
        "potplayer",
        "wmplayer",
        "microsoft.media.player",
        "mpc-hc",
        "mpc-be",
        "smplayer",
        "bilibili",
        "哔哩哔哩",
        "iqiyi",
        "爱奇艺",
        "youku",
        "优酷",
        "qqmusic",
        "cloudmusic",
        "spotify",
    ]
    .iter()
    .any(|needle| app.contains(needle))
}

#[cfg(windows)]
fn looks_like_media_window(title: &str) -> bool {
    let title = title.to_lowercase();
    [
        "视频播放器",
        "视频播放",
        "video player",
        "playing video",
        "媒体播放",
    ]
    .iter()
    .any(|needle| title.contains(needle))
}

#[cfg(windows)]
fn native_context_kind(app: &str) -> &'static str {
    if is_browser_app_name(app) {
        "browser-page"
    } else if is_chat_client_app(app) || is_mail_app(app) {
        "conversation"
    } else if is_document_app(app) {
        "document"
    } else if is_editor_app_name(app) {
        "editor"
    } else if is_file_manager_app(app) {
        "folder"
    } else if is_terminal_app(app) {
        "terminal"
    } else if is_meeting_app(app) {
        "meeting"
    } else if is_media_app(app) {
        "media"
    } else {
        "page"
    }
}

#[cfg(windows)]
fn native_context_kind_for_window(app: &str, title: &str) -> &'static str {
    if is_browser_app_name(app) {
        "browser-page"
    } else if is_editor_product_title(title) {
        "editor"
    } else {
        native_context_kind(app)
    }
}

#[cfg(windows)]
fn is_editor_product_title(title: &str) -> bool {
    let title = title.to_lowercase();
    [
        "ghidra",
        "eclipse ide",
        "apache netbeans",
        "burp suite",
        "android studio",
        "intellij idea",
    ]
    .iter()
    .any(|product| title == *product || title.contains(&format!(" - {product}")))
}

#[cfg(windows)]
fn is_chat_workspace_app(app: &str) -> bool {
    ["chatgpt.exe", "codex.exe"]
        .iter()
        .any(|name| app.eq_ignore_ascii_case(name))
}

#[cfg(windows)]
fn qq_active_conversation_context(
    window: windows::Win32::Foundation::HWND,
) -> Result<Option<ActivePageContext>> {
    use windows::core::VARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_NamePropertyId,
    };

    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> windows::core::Result<Option<ActivePageContext>> {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = automation.ElementFromHandle(window)?;
            let walker = automation.ControlViewWalker()?;

            // QQ NT does not expose the selected chat as a SelectionItem. Its
            // current conversation header is the sibling immediately before the
            // call/action toolbar, which remains stable across people and groups.
            for action_name in [
                "语音通话",
                "视频通话",
                "屏幕共享",
                "Voice call",
                "Video call",
                "Share screen",
            ] {
                let condition = automation.CreatePropertyCondition(
                    UIA_NamePropertyId,
                    &VARIANT::from(action_name),
                )?;
                let Ok(action) = root.FindFirst(TreeScope_Descendants, &condition) else {
                    continue;
                };
                let Ok(toolbar) = walker.GetParentElement(&action) else {
                    continue;
                };
                let Ok(header) = walker.GetPreviousSiblingElement(&toolbar) else {
                    continue;
                };
                let name = header
                    .CurrentName()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                if let Some(title) = clean_qq_conversation_title(&name) {
                    return Ok(Some(ActivePageContext {
                        title,
                        workspace: None,
                        source: "qq-conversation-header",
                        kind: "conversation",
                    }));
                }
            }

            // Compact QQ layouts may collapse the call buttons. The message
            // list remains exposed, and its preceding siblings are the toolbar
            // and the same conversation header.
            for message_list_name in ["消息列表", "Message list"] {
                let condition = automation.CreatePropertyCondition(
                    UIA_NamePropertyId,
                    &VARIANT::from(message_list_name),
                )?;
                let Ok(message_list) = root.FindFirst(TreeScope_Descendants, &condition) else {
                    continue;
                };
                let Ok(main) = walker.GetParentElement(&message_list) else {
                    continue;
                };
                let Ok(toolbar) = walker.GetPreviousSiblingElement(&main) else {
                    continue;
                };
                let Ok(header) = walker.GetPreviousSiblingElement(&toolbar) else {
                    continue;
                };
                let name = header
                    .CurrentName()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                if let Some(title) = clean_qq_conversation_title(&name) {
                    return Ok(Some(ActivePageContext {
                        title,
                        workspace: None,
                        source: "qq-conversation-header",
                        kind: "conversation",
                    }));
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
fn clean_qq_conversation_title(value: &str) -> Option<String> {
    let mut title = value.trim();
    for prefix in [
        "在线状态 ",
        "离线状态 ",
        "手机在线 ",
        "忙碌状态 ",
        "Online status ",
        "Offline status ",
        "Mobile online ",
    ] {
        if let Some(stripped) = title.strip_prefix(prefix) {
            title = stripped.trim();
            break;
        }
    }
    let title = clean_chat_header_title(title, "QQ.exe")?;
    let normalized = title.to_lowercase();
    if [
        "语音通话",
        "视频通话",
        "屏幕共享",
        "邀请加群",
        "群应用",
        "发送",
        "voice call",
        "video call",
        "share screen",
        "send",
    ]
    .contains(&normalized.as_str())
    {
        None
    } else {
        Some(title)
    }
}

#[cfg(windows)]
fn chat_header_context(
    window: windows::Win32::Foundation::HWND,
    app: &str,
) -> Result<Option<ActivePageContext>> {
    use windows::core::VARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_ControlTypePropertyId,
        UIA_EditControlTypeId, UIA_TextControlTypeId,
    };

    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> windows::core::Result<Option<ActivePageContext>> {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = automation.ElementFromHandle(window)?;
            let mut best: Option<(i32, String)> = None;
            let text_condition = automation.CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_TextControlTypeId.0),
            )?;
            let edit_condition = automation.CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_EditControlTypeId.0),
            )?;
            let condition = automation.CreateOrCondition(&text_condition, &edit_condition)?;
            let elements = root.FindAll(TreeScope_Descendants, &condition)?;
            for index in 0..elements.Length()? {
                let Ok(element) = elements.GetElement(index) else {
                    continue;
                };
                if !matches!(element.CurrentIsOffscreen(), Ok(value) if !value.as_bool()) {
                    continue;
                }
                let is_edit = element.CurrentControlType().ok() == Some(UIA_EditControlTypeId);
                let automation_id = element
                    .CurrentAutomationId()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let class_name = element
                    .CurrentClassName()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let Some(mut score) =
                    chat_header_identity_score(&automation_id, &class_name, is_edit)
                else {
                    continue;
                };
                let name = element
                    .CurrentName()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let Some(title) = clean_chat_header_title(&name, app) else {
                    continue;
                };
                score += title.chars().count().min(30) as i32;
                if best
                    .as_ref()
                    .map_or(true, |(best_score, _)| score > *best_score)
                {
                    best = Some((score, title));
                }
            }
            Ok(best.map(|(_, title)| ActivePageContext {
                title,
                workspace: None,
                source: if is_qq_app(app) {
                    "qq-conversation-header"
                } else {
                    "chat-conversation-header"
                },
                kind: "conversation",
            }))
        })();
        if initialized {
            CoUninitialize();
        }
        result.map_err(Into::into)
    }
}

#[cfg(windows)]
fn chat_header_identity_score(automation_id: &str, class_name: &str, is_edit: bool) -> Option<i32> {
    let identity = format!("{automation_id} {class_name}").to_lowercase();
    if [
        "current_chat_name",
        "currentchatname",
        "conversation_title",
        "conversationtitle",
        "chat_title",
        "chattitle",
        "session_title",
        "sessiontitle",
        "chat_name_label",
        "chatnamelabel",
        "channel_title",
        "channeltitle",
    ]
    .iter()
    .any(|marker| identity.contains(marker))
    {
        return Some(if is_edit { 260 } else { 320 });
    }
    if [
        "chatheader",
        "chat_header",
        "conversationheader",
        "conversation_header",
        "sessionheader",
        "session_header",
        "channelheader",
        "channel_header",
    ]
    .iter()
    .any(|marker| identity.contains(marker))
    {
        return Some(if is_edit { 220 } else { 280 });
    }
    if is_edit && identity.contains("chat_input_field") {
        return Some(180);
    }
    None
}

#[cfg(windows)]
fn clean_chat_header_title(value: &str, app: &str) -> Option<String> {
    let line = value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let mut title = clean_page_label(line, app)?;
    for (open, close) in [('(', ')'), ('（', '）')] {
        if title.ends_with(close) {
            if let Some(index) = title.rfind(open) {
                let count = title[index + open.len_utf8()..title.len() - close.len_utf8()].trim();
                if !count.is_empty()
                    && count.chars().count() <= 6
                    && count.chars().all(|character| character.is_ascii_digit())
                {
                    title.truncate(index);
                    title = title.trim().to_string();
                }
            }
        }
    }
    clean_page_label(&title, app)
}

#[cfg(windows)]
fn explorer_active_context(
    window: windows::Win32::Foundation::HWND,
    app: &str,
    native_title: &str,
) -> Result<Option<ActivePageContext>> {
    use windows::core::VARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationSelectionItemPattern,
        IUIAutomationValuePattern, TreeScope_Descendants, UIA_AutomationIdPropertyId,
        UIA_ControlTypePropertyId, UIA_SelectionItemPatternId, UIA_TabItemControlTypeId,
        UIA_ValuePatternId,
    };

    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> windows::core::Result<Option<ActivePageContext>> {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = automation.ElementFromHandle(window)?;

            let address_condition = automation.CreatePropertyCondition(
                UIA_AutomationIdPropertyId,
                &VARIANT::from("TextBox"),
            )?;
            let address_boxes = root.FindAll(TreeScope_Descendants, &address_condition)?;
            for index in 0..address_boxes.Length()? {
                let element = address_boxes.GetElement(index)?;
                if element.CurrentIsOffscreen()?.as_bool() {
                    continue;
                }
                let name = element
                    .CurrentName()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                if !matches!(name.as_str(), "地址栏" | "Address bar") {
                    continue;
                }
                let value_pattern: IUIAutomationValuePattern = match element
                    .GetCurrentPatternAs(UIA_ValuePatternId)
                {
                    Ok(pattern) => pattern,
                    Err(_) => continue,
                };
                let value = value_pattern.CurrentValue()?.to_string();
                if let Some((title, workspace)) = clean_explorer_location(&value) {
                    return Ok(Some(ActivePageContext {
                        title,
                        workspace: Some(workspace),
                        source: "explorer-address-bar",
                        kind: "folder",
                    }));
                }
            }

            let tab_condition = automation.CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_TabItemControlTypeId.0),
            )?;
            let tabs = root.FindAll(TreeScope_Descendants, &tab_condition)?;
            for index in 0..tabs.Length()? {
                let element = tabs.GetElement(index)?;
                let selected: IUIAutomationSelectionItemPattern = match element
                    .GetCurrentPatternAs(UIA_SelectionItemPatternId)
                {
                    Ok(pattern) => pattern,
                    Err(_) => continue,
                };
                if selected.CurrentIsSelected()?.as_bool() {
                    let name = element.CurrentName()?.to_string();
                    if let Some(title) = clean_explorer_folder_name(&name) {
                        return Ok(Some(ActivePageContext {
                            title,
                            workspace: None,
                            source: "explorer-selected-tab",
                            kind: "folder",
                        }));
                    }
                }
            }

            Ok(clean_file_manager_window_title(native_title, app).map(|title| ActivePageContext {
                title,
                workspace: None,
                source: "explorer-window-title",
                kind: "folder",
            }))
        })();
        if initialized {
            CoUninitialize();
        }
        result.map_err(Into::into)
    }
}

#[cfg(windows)]
fn clean_explorer_location(value: &str) -> Option<(String, String)> {
    let workspace = value
        .trim()
        .strip_prefix("地址: ")
        .or_else(|| value.trim().strip_prefix("Address: "))
        .unwrap_or(value.trim())
        .trim();
    if workspace.is_empty() || matches!(workspace, "地址栏" | "Address bar") {
        return None;
    }
    let title = workspace
        .rsplit(['\\', '/', '>'])
        .map(str::trim)
        .find(|part| !part.is_empty())?;
    clean_explorer_folder_name(title).map(|title| (title, workspace.to_string()))
}

#[cfg(windows)]
fn clean_explorer_window_title(value: &str) -> Option<String> {
    let mut title = value.replace(['\r', '\n', '\t'], " ").trim().to_string();
    for suffix in [" - 文件资源管理器", " - File Explorer", " - Windows Explorer"] {
        if let Some(stripped) = title.strip_suffix(suffix) {
            title = stripped.trim().to_string();
            break;
        }
    }
    if let Some((first, rest)) = title.split_once(" 和 ") {
        if rest.contains("个其他选项卡") {
            title = first.trim().to_string();
        }
    }
    if let Some((first, rest)) = title.split_once(" and ") {
        if rest.contains("more tab") {
            title = first.trim().to_string();
        }
    }
    clean_explorer_folder_name(&title)
}

#[cfg(windows)]
fn clean_file_manager_window_title(value: &str, app: &str) -> Option<String> {
    if normalized_app_name(app) == "explorer" {
        return clean_explorer_window_title(value);
    }
    let title = strip_product_suffix(
        value,
        &[
            " - Total Commander",
            " - Files",
            " - Directory Opus",
            " - FreeCommander",
            " - Double Commander",
            " - Everything",
        ],
    );
    clean_explorer_folder_name(&title)
}

#[cfg(windows)]
fn clean_explorer_folder_name(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > 180
        || matches!(
            value.to_lowercase().as_str(),
            "文件资源管理器" | "file explorer" | "windows explorer" | "此电脑" | "this pc"
        )
    {
        None
    } else {
        Some(value.to_string())
    }
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
            "sumatrapdf.exe",
            "foxitpdfreader.exe",
            "foxitphantompdf.exe",
            "soffice.bin",
            "swriter.exe",
            "scalc.exe",
            "simpress.exe",
            "notion.exe",
            "logseq.exe",
            "joplin.exe",
            "zettlr.exe",
            "calibre.exe",
            "ebook-viewer.exe",
            "xournalpp.exe",
            "drawboardpdf.exe",
            "mupdf.exe",
        ]
        .iter()
        .any(|name| app.eq_ignore_ascii_case(name))
}

#[cfg(windows)]
fn selected_page_context(
    window: windows::Win32::Foundation::HWND,
    app: &str,
) -> Result<Option<ActivePageContext>> {
    use windows::core::VARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationSelectionItemPattern, TreeScope_Descendants,
        UIA_IsSelectionItemPatternAvailablePropertyId, UIA_ListItemControlTypeId,
        UIA_SelectionItemPatternId, UIA_TabItemControlTypeId,
    };

    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> windows::core::Result<Option<ActivePageContext>> {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = automation.ElementFromHandle(window)?;
            let condition = automation.CreatePropertyCondition(
                UIA_IsSelectionItemPatternAvailablePropertyId,
                &VARIANT::from(true),
            )?;
            let elements = root.FindAll(TreeScope_Descendants, &condition)?;
            let mut best: Option<(i32, String)> = None;
            for index in 0..elements.Length()? {
                let element = elements.GetElement(index)?;
                let selected: IUIAutomationSelectionItemPattern = match element
                    .GetCurrentPatternAs(UIA_SelectionItemPatternId)
                {
                    Ok(pattern) => pattern,
                    Err(_) => continue,
                };
                if !selected.CurrentIsSelected()?.as_bool() {
                    continue;
                }
                let name = element
                    .CurrentName()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let automation_id = element
                    .CurrentAutomationId()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let class_name = element
                    .CurrentClassName()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let Some(title) = selected_item_page_title(&name, &automation_id, app) else {
                    continue;
                };
                let control_type = element.CurrentControlType().ok();
                let mut score = 0;
                let identity = format!("{automation_id} {class_name}").to_lowercase();
                if identity.contains("session") || identity.contains("conversation") {
                    score += 100;
                } else if identity.contains("chat") {
                    score += 80;
                }
                if control_type == Some(UIA_TabItemControlTypeId) {
                    score += 40;
                } else if control_type == Some(UIA_ListItemControlTypeId) {
                    score += 30;
                }
                score += title.chars().count().min(30) as i32;
                if best
                    .as_ref()
                    .map_or(true, |(best_score, _)| score > *best_score)
                {
                    best = Some((score, title));
                }
            }
            Ok(best.map(|(_, title)| ActivePageContext {
                title,
                workspace: None,
                source: if is_chat_client_app(app) {
                    "chat-conversation-selection"
                } else {
                    "selected-page-item"
                },
                kind: if is_chat_client_app(app) {
                    "conversation"
                } else {
                    native_context_kind(app)
                },
            }))
        })();
        if initialized {
            CoUninitialize();
        }
        result.map_err(Into::into)
    }
}

#[cfg(windows)]
fn selected_item_page_title(name: &str, automation_id: &str, app: &str) -> Option<String> {
    let normalized_id = automation_id.to_lowercase();
    for marker in ["session_item_", "conversation_item_", "chat_item_"] {
        if let Some(index) = normalized_id.find(marker) {
            let value = &automation_id[index + marker.len()..];
            if let Some(title) = clean_selected_chat_label(value, app) {
                return Some(title);
            }
        }
    }
    name.lines()
        .find_map(|line| clean_selected_chat_label(line, app))
}

#[cfg(windows)]
fn clean_selected_chat_label(value: &str, app: &str) -> Option<String> {
    if is_qq_app(app) {
        clean_qq_conversation_title(value)
    } else if is_chat_client_app(app) {
        clean_chat_header_title(value, app)
    } else {
        clean_page_label(value, app)
    }
}

#[cfg(windows)]
fn clean_native_page_title(value: &str, app: &str) -> Option<String> {
    let cleaned = if is_file_manager_app(app) {
        clean_file_manager_window_title(value, app)?
    } else {
        let markers: &[&str] = if is_browser_app_name(app) {
            &[
                " - Google Chrome",
                " — Google Chrome",
                " - Microsoft Edge",
                " — Microsoft Edge",
                " — Mozilla Firefox",
                " - Mozilla Firefox",
                " - Brave",
                " - Vivaldi",
                " - Opera",
                " - Chromium",
                " - Tabbit",
                " - Thorium",
                " - Floorp",
                " - Waterfox",
                " - LibreWolf",
                " - DuckDuckGo",
            ]
        } else if is_editor_app_name(app) || is_editor_product_title(value) {
            &[
                " - Visual Studio Code",
                " - Cursor",
                " - Windsurf",
                " - VSCodium",
                " - Microsoft Visual Studio",
                " - IntelliJ IDEA",
                " - PyCharm",
                " - WebStorm",
                " - RustRover",
                " - CLion",
                " - Android Studio",
                " - IDA Pro",
                " - IDA",
                " - Ghidra",
                " - Sublime Text",
                " - Zed",
                " - RStudio",
                " - MATLAB",
                " - Unity",
                " - Unreal Editor",
                " - Godot Engine",
                " - Adobe Photoshop",
                " - Adobe Illustrator",
                " - Figma",
                " - Blender",
                " - Adobe Premiere Pro",
                " - Adobe After Effects",
                " - AutoCAD",
                " - Rider",
                " - Eclipse IDE",
                " - Apache NetBeans IDE",
                " - Qt Creator",
                " - Code::Blocks",
                " - Arduino IDE",
                " - DBeaver",
                " - DataGrip",
                " - Navicat Premium",
                " - Postman",
                " - Insomnia",
                " - Fiddler",
                " - Wireshark",
                " - Burp Suite Professional",
                " - Burp Suite Community Edition",
                " - Docker Desktop",
                " - GitHub Desktop",
                " - GitKraken",
                " - Krita",
                " - Inkscape",
                " - DaVinci Resolve",
                " - Affinity Photo 2",
                " - Affinity Designer 2",
                " - SketchUp",
            ]
        } else if is_meeting_app(app) {
            &[
                " - Zoom Workplace",
                " - Zoom",
                " - Cisco Webex",
                " - Webex",
                " - 腾讯会议",
                " - VooV Meeting",
            ]
        } else if is_terminal_app(app) {
            &[
                " - Windows Terminal",
                " - PowerShell",
                " - Command Prompt",
                " - WezTerm",
                " - Alacritty",
            ]
        } else if is_media_app(app) {
            &[
                " - VLC media player",
                " - PotPlayer",
                " - Windows Media Player",
                " - Media Player",
                " - Spotify",
                " - 网易云音乐",
                " - QQ音乐",
            ]
        } else if is_chat_client_app(app) {
            &[
                " - 微信",
                " - WeChat",
                " - 钉钉",
                " - DingTalk",
                " - 飞书",
                " - Feishu",
                " - Lark",
                " - 企业微信",
                " - WeCom",
                " - Microsoft Teams",
                " - Slack",
                " - Discord",
                " - Telegram",
                " - WhatsApp",
                " - Signal",
            ]
        } else if is_mail_app(app) {
            &[
                " - Outlook",
                " - Microsoft Outlook",
                " - Mozilla Thunderbird",
                " - Thunderbird",
                " - Foxmail",
            ]
        } else {
            &[]
        };
        strip_product_suffix(value, markers)
    };
    clean_page_label(&cleaned, app)
}

#[cfg(windows)]
fn strip_product_suffix(value: &str, markers: &[&str]) -> String {
    let normalized = value.replace(['\r', '\n', '\t'], " ");
    let lower = normalized.to_lowercase();
    for marker in markers {
        let marker = marker.to_lowercase();
        if let Some(index) = lower.rfind(&marker) {
            let tail = lower[index + marker.len()..].trim();
            if tail.is_empty()
                || tail
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_digit())
            {
                return normalized[..index].trim().to_string();
            }
        }
    }
    normalized.trim().to_string()
}

#[cfg(windows)]
fn clean_page_label(value: &str, app: &str) -> Option<String> {
    let value = value
        .replace(['\r', '\n', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let value = value.trim_matches(['-', '—', '–', '·', '|', ' ']);
    if value.is_empty() || value.chars().count() > 220 || is_generic_page_label(value, app) {
        None
    } else {
        Some(value.chars().take(120).collect())
    }
}

#[cfg(windows)]
fn is_generic_page_label(value: &str, app: &str) -> bool {
    let value = value.trim().to_lowercase();
    let app_name = std::path::Path::new(app.trim())
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(app)
        .trim()
        .to_lowercase();
    if value == app_name {
        return true;
    }
    if value.strip_prefix(&app_name).is_some_and(|tail| {
        let tail = tail.trim();
        !tail.is_empty()
            && tail
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
            && tail
                .chars()
                .all(|character| character.is_ascii_digit() || ".- _()".contains(character))
    }) {
        return true;
    }
    [
        "qq", "腾讯qq", "微信", "wechat", "weixin", "钉钉", "dingtalk", "企业微信",
        "wecom", "飞书", "feishu", "lark", "chatgpt", "codex", "screenuse", "wps",
        "wps office", "word", "microsoft word", "excel", "microsoft excel", "powerpoint",
        "microsoft powerpoint", "onenote", "microsoft onenote", "outlook", "notepad",
        "notepad++", "typora", "obsidian", "adobe acrobat", "acrobat reader",
        "google chrome", "chrome", "microsoft edge", "msedge", "firefox", "mozilla firefox",
        "tabbit browser", "windows explorer", "file explorer", "文件资源管理器",
        "消息", "聊天", "会话", "通讯录", "联系人", "工作台", "文档", "会议", "邮箱",
        "日历", "首页", "好友", "群聊", "搜索", "设置", "home", "messages", "contacts",
        "workbench", "calendar", "settings", "main window", "mainwindow", "此电脑", "this pc",
        "腾讯会议", "voov meeting", "zoom", "zoom workplace", "webex", "cisco webex",
        "visual studio code", "cursor", "windsurf", "vscodium", "intellij idea", "pycharm",
        "webstorm", "rustrover", "clion", "android studio", "windows terminal", "powershell",
        "windows powershell", "command prompt", "命令提示符", "vlc media player", "potplayer",
        "media player", "spotify", "网易云音乐", "qq音乐", "图片查看器", "视频播放器",
        "photos", "microsoft photos", "照片", "calculator", "计算器", "paint",
        "microsoft paint", "画图", "camera", "相机", "clock", "时钟",
        "语音通话", "视频通话", "屏幕共享", "邀请加群", "群应用", "发送", "send",
    ]
    .contains(&value.as_str())
        || looks_like_clock(&value)
}

#[cfg(windows)]
fn looks_like_clock(value: &str) -> bool {
    let mut parts = value.split(':');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(hour), Some(minute), None)
            if !hour.is_empty()
                && hour.len() <= 2
                && minute.len() == 2
                && hour.chars().all(|character| character.is_ascii_digit())
                && minute.chars().all(|character| character.is_ascii_digit())
    )
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
        UIA_ButtonControlTypeId, UIA_ControlTypePropertyId, UIA_NamePropertyId,
        UIA_TextControlTypeId,
    };

    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> windows::core::Result<Option<ActivePageContext>> {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let content_window = chromium_accessibility_window(window).unwrap_or(window);
            let root = automation.ElementFromHandle(content_window)?;
            let walker = automation.ControlViewWalker()?;
            let codex_mode = is_codex_workspace(&automation, &root)?;
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
            let mut header = action.and_then(|element| walker.GetParentElement(&element).ok());
            if header.is_none() {
                let value = VARIANT::from(UIA_ButtonControlTypeId.0);
                let condition =
                    automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &value)?;
                let buttons = root.FindAll(TreeScope_Descendants, &condition)?;
                for index in 0..buttons.Length()? {
                    let button = buttons.GetElement(index)?;
                    let name = button
                        .CurrentName()
                        .map(|value| value.to_string())
                        .unwrap_or_default();
                    if chatgpt_project_name(&name).is_some() {
                        header = walker.GetParentElement(&button).ok();
                        break;
                    }
                }
            }
            let new_task = codex_mode && is_codex_new_task_page(&automation, &root)?;
            let Some(parent) = header else {
                return Ok(new_task.then(codex_new_task_context));
            };
            let mut child = walker.GetFirstChildElement(&parent).ok();
            let mut title = None;
            let mut project = None;
            for _ in 0..10 {
                let Some(element) = child else { break; };
                let name = element.CurrentName().map(|value| value.to_string()).unwrap_or_default();
                if let Some(value) = chatgpt_project_name(&name) {
                    project = Some(value);
                } else if element.CurrentControlType().ok() == Some(UIA_TextControlTypeId)
                    && valid_chatgpt_title(&name)
                {
                    title = Some(name.trim().to_string());
                }
                child = walker.GetNextSiblingElement(&element).ok();
            }
            let Some(raw_title) = title else {
                return Ok(new_task.then(codex_new_task_context));
            };
            if codex_mode && project.is_none() {
                project = codex_project_for_task(&automation, &root, &walker, &raw_title)?;
            }
            let (title, source) = if codex_mode {
                codex_task_context_title(&raw_title, project.as_deref())
            } else {
                (raw_title, "chatgpt-conversation")
            };
            Ok(Some(ActivePageContext {
                title,
                workspace: project,
                source,
                kind: "conversation",
            }))
        })();
        if initialized {
            CoUninitialize();
        }
        result.map_err(Into::into)
    }
}

#[cfg(windows)]
fn chromium_accessibility_window(
    window: windows::Win32::Foundation::HWND,
) -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, GetClassNameW, IsWindowVisible,
    };

    struct SearchState {
        result: Option<HWND>,
    }

    unsafe extern "system" fn visit(child: HWND, parameter: LPARAM) -> BOOL {
        let state = &mut *(parameter.0 as *mut SearchState);
        if !IsWindowVisible(child).as_bool() {
            return BOOL(1);
        }
        let mut class_name = [0u16; 96];
        let length = GetClassNameW(child, &mut class_name).max(0) as usize;
        if String::from_utf16_lossy(&class_name[..length]) == "Chrome_RenderWidgetHostHWND" {
            state.result = Some(child);
            return BOOL(0);
        }
        BOOL(1)
    }

    let mut state = SearchState { result: None };
    unsafe {
        let _ = EnumChildWindows(
            window,
            Some(visit),
            LPARAM((&mut state as *mut SearchState) as isize),
        );
    }
    state.result
}

#[cfg(windows)]
fn is_codex_workspace(
    automation: &windows::Win32::UI::Accessibility::IUIAutomation,
    root: &windows::Win32::UI::Accessibility::IUIAutomationElement,
) -> windows::core::Result<bool> {
    use windows::core::VARIANT;
    use windows::Win32::UI::Accessibility::{TreeScope_Descendants, UIA_NamePropertyId};

    unsafe {
        for label in [
            "切换模式，当前模式：Codex",
            "切换模式，当前模式: Codex",
            "Switch mode, current mode: Codex",
        ] {
            let condition =
                automation.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(label))?;
            if root.FindFirst(TreeScope_Descendants, &condition).is_ok() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn is_codex_new_task_page(
    automation: &windows::Win32::UI::Accessibility::IUIAutomation,
    root: &windows::Win32::UI::Accessibility::IUIAutomationElement,
) -> windows::core::Result<bool> {
    use windows::core::VARIANT;
    use windows::Win32::UI::Accessibility::{TreeScope_Descendants, UIA_NamePropertyId};

    unsafe {
        for label in [
            "我们该构建什么？",
            "What should we build?",
            "What do you want to build?",
        ] {
            let condition =
                automation.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(label))?;
            if root.FindFirst(TreeScope_Descendants, &condition).is_ok() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn codex_new_task_context() -> ActivePageContext {
    ActivePageContext {
        title: "任务-新建任务".into(),
        workspace: None,
        source: "codex-task",
        kind: "conversation",
    }
}

#[cfg(windows)]
fn codex_project_for_task(
    automation: &windows::Win32::UI::Accessibility::IUIAutomation,
    root: &windows::Win32::UI::Accessibility::IUIAutomationElement,
    walker: &windows::Win32::UI::Accessibility::IUIAutomationTreeWalker,
    task_title: &str,
) -> windows::core::Result<Option<String>> {
    use std::collections::BTreeSet;
    use windows::core::VARIANT;
    use windows::Win32::UI::Accessibility::{TreeScope_Descendants, UIA_NamePropertyId};

    let projects = unsafe {
        let condition =
            automation.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(task_title))?;
        let matches = root.FindAll(TreeScope_Descendants, &condition)?;
        let mut projects = BTreeSet::new();
        for index in 0..matches.Length()? {
            let mut element = Some(matches.GetElement(index)?);
            for _ in 0..8 {
                let Some(current) = element else { break; };
                let name = current
                    .CurrentName()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                if let Some(project) = codex_project_task_list_name(&name) {
                    projects.insert(project);
                    break;
                }
                element = walker.GetParentElement(&current).ok();
            }
        }
        projects
    };
    Ok((projects.len() == 1).then(|| projects.into_iter().next()).flatten())
}

#[cfg(windows)]
fn codex_project_task_list_name(value: &str) -> Option<String> {
    let value = value.trim();
    value
        .strip_suffix("中的已安排任务")
        .or_else(|| value.strip_prefix("Scheduled tasks in "))
        .or_else(|| value.strip_suffix(" scheduled tasks"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(windows)]
fn codex_task_context_title(
    conversation: &str,
    project: Option<&str>,
) -> (String, &'static str) {
    match project.map(str::trim).filter(|project| !project.is_empty()) {
        Some(project) => (
            format!("项目-{project}-{conversation}"),
            "codex-project-task",
        ),
        None => (format!("任务-{conversation}"), "codex-task"),
    }
}

#[cfg(windows)]
fn chatgpt_project_name(value: &str) -> Option<String> {
    value
        .strip_prefix("项目：")
        .or_else(|| value.strip_prefix("Project: "))
        .or_else(|| value.strip_prefix("Project："))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
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
            " - SumatraPDF",
            " - Foxit PDF Reader",
            " - Foxit PDF Editor",
            " - LibreOffice Writer",
            " - LibreOffice Calc",
            " - LibreOffice Impress",
            " - LibreOffice",
            " - Notion",
            " | Notion",
            " — Notion",
            " - Logseq",
            " - Joplin",
            " - Zettlr",
            " - calibre",
            " - E-book viewer",
            " - Xournal++",
            " - Drawboard PDF",
            " - MuPDF",
        ]
    };
    let title = strip_product_suffix(native_title, suffixes);
    valid_document_title(&title)
}

#[cfg(windows)]
fn document_title_and_workspace(title: &str, app: &str) -> (String, Option<String>) {
    let app = normalized_app_name(app);
    if matches!(app.as_str(), "obsidian" | "logseq") {
        if let Some((document, workspace)) = title.rsplit_once(" - ") {
            let document = document.trim();
            let workspace = workspace.trim();
            if !document.is_empty() && !workspace.is_empty() {
                return (document.to_string(), Some(workspace.to_string()));
            }
        }
    }
    (title.to_string(), None)
}

#[cfg(windows)]
fn editor_title_and_workspace(title: &str, app: &str) -> (String, Option<String>) {
    if matches!(
        normalized_app_name(app).as_str(),
        "code" | "code - insiders" | "code-insiders" | "cursor" | "windsurf" | "codium"
    ) {
        if let Some((document, workspace)) = title.rsplit_once(" - ") {
            let document = document.trim();
            let workspace = workspace.trim();
            if !document.is_empty()
                && !workspace.is_empty()
                && !matches!(
                    workspace.to_lowercase().as_str(),
                    "code"
                        | "visual studio code"
                        | "visual studio code - insiders"
                        | "cursor"
                        | "windsurf"
                        | "vscodium"
                )
            {
                return (document.to_string(), Some(workspace.to_string()));
            }
        }
    }
    (title.to_string(), None)
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
                | "notepad" | "notepad++" | "sumatrapdf" | "foxit pdf reader"
                | "foxit pdf editor" | "libreoffice" | "notion" | "logseq" | "joplin"
                | "zettlr"
                | "calibre" | "e-book viewer" | "xournal++" | "drawboard pdf" | "mupdf"
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
        ".csv", ".rtf", ".odt", ".ods", ".odp", ".tex", ".epub",
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
    fn screenshot_overlays_inherit_the_active_task_but_not_idle() {
        let event = RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: "2026-07-14T01:00:00Z".into(),
            app: Some("Snipaste.exe".into()),
            window_title: Some("Snipper - Snipaste".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({}),
        };
        let observed = context_signature(&event, 180);
        assert!(!should_inherit_active_context("", &observed, &event));
        assert!(should_inherit_active_context("code|ScreenUse||||", &observed, &event));
        assert!(!should_inherit_active_context("idle", &observed, &event));

        let mut qq_screenshot = event.clone();
        qq_screenshot.app = Some("QQ.exe".into());
        qq_screenshot.window_title = Some("QQ截图".into());
        let qq_screenshot_signature = context_signature(&qq_screenshot, 180);
        assert!(should_inherit_active_context(
            "chrome.exe|成果填报||||",
            &qq_screenshot_signature,
            &qq_screenshot,
        ));

        let mut normal = event;
        normal.app = Some("Code.exe".into());
        normal.window_title = Some("ScreenUse - Visual Studio Code".into());
        let observed = context_signature(&normal, 180);
        assert!(!should_inherit_active_context("chatgpt|ScreenUse||||", &observed, &normal));

        let active = RawActivityEvent {
            app: Some("QQ.exe".into()),
            window_title: Some("科研讨论群".into()),
            timestamp: "2026-07-14T00:59:55Z".into(),
            metadata: json!({"activePageTitle": "科研讨论群"}),
            ..normal.clone()
        };
        let viewer = RawActivityEvent {
            app: Some("QQ.exe".into()),
            window_title: Some("图片查看器".into()),
            timestamp: "2026-07-14T01:00:05Z".into(),
            ..normal
        };
        let viewer_signature = context_signature(&viewer, 180);
        assert!(should_inherit_active_context(
            &context_signature(&active, 180),
            &viewer_signature,
            &viewer,
        ));
        assert!(!should_inherit_active_context(
            "chrome.exe|论文检索||||",
            &viewer_signature,
            &viewer,
        ));
        let inherited = inherit_active_context_event(&active, viewer);
        assert_eq!(inherited.app.as_deref(), Some("QQ.exe"));
        assert_eq!(inherited.window_title.as_deref(), Some("科研讨论群"));
        assert_eq!(inherited.timestamp, "2026-07-14T01:00:05Z");
        assert_eq!(
            inherited.metadata["transientOverlay"]["title"].as_str(),
            Some("图片查看器")
        );
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
    fn manual_away_returns_after_five_seconds_of_sustained_input() {
        let event = |timestamp: &str, idle_milliseconds: u64| RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: timestamp.into(),
            app: Some("chrome.exe".into()),
            window_title: Some("工作页面".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats {
                idle_seconds: idle_milliseconds / 1000,
                ..Default::default()
            },
            metadata: json!({ "inputIdleMilliseconds": idle_milliseconds }),
        };
        let mut candidate = None;
        assert!(!observe_manual_away_return(
            &mut candidate,
            &event("2026-07-17T10:00:00Z", 80),
            1,
        ));
        assert!(!observe_manual_away_return(
            &mut candidate,
            &event("2026-07-17T10:00:04Z", 120),
            1,
        ));
        assert!(observe_manual_away_return(
            &mut candidate,
            &event("2026-07-17T10:00:05Z", 90),
            1,
        ));
        let candidate = candidate.expect("return candidate");
        assert_eq!(candidate.first_event.timestamp, "2026-07-17T10:00:00Z");
        assert_eq!(candidate.latest_event.timestamp, "2026-07-17T10:00:05Z");
    }

    #[test]
    fn manual_away_return_resets_when_input_stops() {
        let event = |timestamp: &str, idle_milliseconds: u64| RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: timestamp.into(),
            app: Some("code.exe".into()),
            window_title: Some("ScreenUse".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({ "inputIdleMilliseconds": idle_milliseconds }),
        };
        let mut candidate = None;
        assert!(!observe_manual_away_return(
            &mut candidate,
            &event("2026-07-17T10:00:00Z", 30),
            1,
        ));
        assert!(!observe_manual_away_return(
            &mut candidate,
            &event("2026-07-17T10:00:03Z", 2200),
            1,
        ));
        assert!(candidate.is_none());
        assert!(!observe_manual_away_return(
            &mut candidate,
            &event("2026-07-17T10:00:04Z", 40),
            1,
        ));
        assert!(observe_manual_away_return(
            &mut candidate,
            &event("2026-07-17T10:00:09Z", 70),
            1,
        ));
    }

    #[test]
    fn manual_away_starts_at_the_tray_click_and_forces_idle_classification() {
        let mut event = RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: "2026-07-17T10:00:01Z".into(),
            app: Some("QQ.exe".into()),
            window_title: Some("科研群".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({ "inputIdleMilliseconds": 50 }),
        };
        force_manual_away_event(
            &mut event,
            180,
            Some("2026-07-17T10:00:00Z"),
        );
        assert_eq!(event.timestamp, "2026-07-17T10:00:00Z");
        assert_eq!(event.input_stats.idle_seconds, 180);
        assert_eq!(event.metadata["manualAway"].as_bool(), Some(true));
        assert_eq!(event.metadata["inputIdleSeconds"].as_u64(), Some(0));
        assert_eq!(context_signature(&event, 180), "idle");
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
    fn confirmed_playback_and_foreground_meetings_count_as_passive_attention() {
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
        assert_eq!(passive_attention_reason(&event), Some("browser-video-playing"));
        assert_eq!(passive_attention_reason(&RawActivityEvent {
            app: Some("WeMeetApp.exe".into()),
            window_title: Some("申书豪预定的会议".into()),
            metadata: json!({}),
            ..event.clone()
        }), Some("meeting-app-foreground"));
        assert_eq!(passive_attention_reason(&RawActivityEvent {
            app: Some("Zoom.exe".into()),
            window_title: Some("Weekly sync".into()),
            metadata: json!({"meetingActive": true}),
            ..event.clone()
        }), Some("meeting-app-foreground"));
        assert_eq!(passive_attention_reason(&RawActivityEvent {
            app: Some("Zoom.exe".into()),
            window_title: Some("Zoom Workplace".into()),
            metadata: json!({}),
            ..event.clone()
        }), None);
        assert_eq!(passive_attention_reason(&RawActivityEvent {
            app: Some("ms-teams.exe".into()),
            window_title: Some("产品周会 - Microsoft Teams meeting".into()),
            metadata: json!({}),
            ..event.clone()
        }), Some("collaboration-meeting-foreground"));
        assert_eq!(passive_attention_reason(&RawActivityEvent {
            app: Some("ms-teams.exe".into()),
            window_title: Some("与张三的聊天".into()),
            metadata: json!({}),
            ..event.clone()
        }), None);
        assert_eq!(passive_attention_reason(&RawActivityEvent {
            app: Some("chrome.exe".into()),
            window_title: Some("项目周会 - Google Meet".into()),
            metadata: json!({}),
            ..event.clone()
        }), Some("browser-meeting-foreground"));
        assert_eq!(passive_attention_reason(&RawActivityEvent {
            app: Some("wps.exe".into()),
            window_title: Some("腾讯会议纪要.docx".into()),
            metadata: json!({}),
            ..event.clone()
        }), None);
        assert_eq!(passive_attention_reason(&RawActivityEvent {
            app: Some("vlc.exe".into()),
            window_title: Some("recording.mp4".into()),
            metadata: json!({"mediaPlaying": true}),
            ..event.clone()
        }), Some("media-player-foreground"));
        assert_eq!(passive_attention_reason(&RawActivityEvent {
            app: Some("vlc.exe".into()),
            window_title: Some("recording.mp4".into()),
            metadata: json!({"mediaPlaying": false}),
            ..event.clone()
        }), None);
        assert_eq!(passive_attention_reason(&RawActivityEvent {
            app: Some("notepad.exe".into()),
            window_title: Some("notes.txt".into()),
            metadata: json!({}),
            ..event
        }), None);
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
        assert!(is_windows_task_view(&RawActivityEvent {
            app: Some("SearchHost.exe".into()),
            window_title: Some("搜索".into()),
            ..event.clone()
        }));
        assert!(!is_windows_task_view(&RawActivityEvent {
            app: Some("chrome.exe".into()),
            ..event
        }));
    }

    #[test]
    fn untitled_chatgpt_loading_is_a_handoff_between_semantic_tasks() {
        let semantic = RawActivityEvent {
            id: String::new(),
            source: "windows-foreground".into(),
            timestamp: "2026-07-17T03:16:37Z".into(),
            app: Some("ChatGPT.exe".into()),
            window_title: Some("项目-HDU-IOT week1".into()),
            url: None,
            file_path: None,
            workspace: Some("HDU".into()),
            input_stats: InputStats::default(),
            metadata: json!({
                "activePageTitle": "项目-HDU-IOT week1",
                "activePageSource": "codex-project-task"
            }),
        };
        let active = ActiveContext {
            id: "event".into(),
            session_id: "session".into(),
            signature: context_signature(&semantic, 180),
            started_at: semantic.timestamp.clone(),
            event: semantic,
            last_observed_at: Instant::now(),
            last_emitted_at: Instant::now(),
        };
        let mut loading = RawActivityEvent {
            id: String::new(),
            source: "windows-foreground".into(),
            timestamp: "2026-07-17T03:16:59Z".into(),
            app: Some("ChatGPT.exe".into()),
            window_title: Some("ChatGPT".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({}),
        };

        assert!(is_incomplete_chat_workspace_handoff(Some(&active), &loading));
        let handoff = TransitionHandoff {
            started_at: loading.timestamp.clone(),
            source: "chat-workspace-loading",
        };
        loading.timestamp = "2026-07-17T03:17:12Z".into();
        apply_transition_handoff(&mut loading, &handoff);
        assert_eq!(loading.timestamp, "2026-07-17T03:16:59Z");
        assert_eq!(
            loading.metadata["contextHandoff"].as_str(),
            Some("chat-workspace-loading")
        );
        assert!(!is_incomplete_chat_workspace_handoff(
            Some(&active),
            &RawActivityEvent {
                app: Some("chrome.exe".into()),
                ..loading
            }
        ));
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
        assert!(is_chat_client_app("Weixin.exe"));
        assert!(is_chat_client_app("DingTalk.exe"));
        assert!(is_chat_client_app("Feishu.exe"));
        assert!(is_chat_client_app("ms-teams.exe"));
        assert!(is_chat_client_app("Slack.exe"));
        assert!(is_document_app("wps.exe"));
        assert!(is_document_app("EXCEL.EXE"));
        assert!(is_editor_app_name("Code.exe"));
        assert!(is_editor_app_name("pycharm64.exe"));
        assert!(is_terminal_app("WindowsTerminal.exe"));
        assert!(is_file_manager_app("explorer.exe"));
        assert!(!is_document_app("steam.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn documented_common_app_families_have_a_precision_profile() {
        for app in [
            "QQ.exe",
            "Weixin.exe",
            "DingTalk.exe",
            "DingTalkApp.exe",
            "Feishu.exe",
            "FeishuApp.exe",
            "wxwork.exe",
            "WXWorkWeb.exe",
            "WeCom.exe",
            "ms-teams.exe",
            "msteams.exe",
            "Slack.exe",
            "Discord.exe",
            "Telegram.exe",
            "Signal.exe",
            "WhatsApp.exe",
            "LINE.exe",
            "TIM.exe",
        ] {
            assert!(is_chat_client_app(app), "missing chat profile for {app}");
        }
        for app in [
            "chrome.exe",
            "msedge.exe",
            "firefox.exe",
            "brave.exe",
            "vivaldi.exe",
            "opera.exe",
            "arc.exe",
            "Tabbit Browser.exe",
        ] {
            assert!(is_browser_app_name(app), "missing browser profile for {app}");
        }
        for app in [
            "wps.exe",
            "WINWORD.EXE",
            "EXCEL.EXE",
            "POWERPNT.EXE",
            "Obsidian.exe",
            "Typora.exe",
            "AcroRd32.exe",
            "soffice.bin",
            "Notion.exe",
            "calibre.exe",
        ] {
            assert!(is_document_app(app), "missing document profile for {app}");
        }
        for app in [
            "Code.exe",
            "Cursor.exe",
            "pycharm64.exe",
            "rider64.exe",
            "ida64.exe",
            "ghidra.exe",
            "DBeaver.exe",
            "Postman.exe",
            "Wireshark.exe",
            "Unity.exe",
            "Blender.exe",
        ] {
            assert!(is_editor_app_name(app), "missing work profile for {app}");
        }
        for app in [
            "WindowsTerminal.exe",
            "pwsh.exe",
            "cmd.exe",
            "MobaXterm.exe",
            "Xshell.exe",
            "putty.exe",
            "wsl.exe",
        ] {
            assert!(is_terminal_app(app), "missing terminal profile for {app}");
        }
        for app in ["explorer.exe", "TOTALCMD64.EXE", "Files.exe", "dopus.exe"] {
            assert!(is_file_manager_app(app), "missing file manager profile for {app}");
        }
        for app in ["OUTLOOK.EXE", "olk.exe", "thunderbird.exe", "Foxmail.exe"] {
            assert!(is_mail_app(app), "missing mail profile for {app}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_titles_are_cleaned_for_common_app_families() {
        assert_eq!(
            clean_native_page_title("icpc-trainer - Google Chrome", "chrome.exe"),
            Some("icpc-trainer".into())
        );
        assert_eq!(
            clean_document_title(
                "CVE-2026-44277 - WorkSpace - Obsidian 1.12.7",
                "Obsidian.exe",
            ),
            Some("CVE-2026-44277 - WorkSpace".into())
        );
        assert_eq!(
            document_title_and_workspace("CVE-2026-44277 - WorkSpace", "Obsidian.exe"),
            ("CVE-2026-44277".into(), Some("WorkSpace".into()))
        );
        assert_eq!(
            editor_title_and_workspace("main.rs - ScreenUse", "Code.exe"),
            ("main.rs".into(), Some("ScreenUse".into()))
        );
        assert_eq!(
            clean_native_page_title("main.rs - ScreenUse - Visual Studio Code", "Code.exe"),
            Some("main.rs - ScreenUse".into())
        );
        assert_eq!(
            clean_native_page_title("科研周会 - Zoom Workplace", "Zoom.exe"),
            Some("科研周会".into())
        );
        assert_eq!(
            clean_native_page_title("lecture.mp4 - VLC media player", "vlc.exe"),
            Some("lecture.mp4".into())
        );
        assert_eq!(
            clean_explorer_window_title("release 和 3 个其他选项卡 - 文件资源管理器"),
            Some("release".into())
        );
        assert_eq!(
            clean_explorer_location(r"C:\College\ICPC\icpc-trainer"),
            Some(("icpc-trainer".into(), r"C:\College\ICPC\icpc-trainer".into()))
        );
    }

    #[cfg(windows)]
    #[test]
    fn native_context_types_cover_each_precision_signal() {
        assert_eq!(native_context_kind("chrome.exe"), "browser-page");
        assert_eq!(native_context_kind("Weixin.exe"), "conversation");
        assert_eq!(native_context_kind("OUTLOOK.EXE"), "conversation");
        assert_eq!(native_context_kind("wps.exe"), "document");
        assert_eq!(native_context_kind("Code.exe"), "editor");
        assert_eq!(native_context_kind("explorer.exe"), "folder");
        assert_eq!(native_context_kind("WindowsTerminal.exe"), "terminal");
        assert_eq!(native_context_kind("WeMeetApp.exe"), "meeting");
        assert_eq!(native_context_kind("vlc.exe"), "media");
        assert!(looks_like_media_window("QQ · 视频播放器"));
        assert!(!looks_like_media_window("QQ · 图片查看器"));
        assert_eq!(
            native_context_kind_for_window("javaw.exe", "firmware.bin - Ghidra"),
            "editor"
        );
        assert_eq!(
            native_context_kind_for_window("chrome.exe", "Ghidra - Google Chrome"),
            "browser-page"
        );
    }

    #[cfg(windows)]
    #[test]
    fn hosted_uwp_resolution_rejects_windows_shell_helpers() {
        assert!(is_hosted_app_candidate(
            r"C:\Program Files\WindowsApps\Microsoft.Windows.Photos\Microsoft.Photos.exe"
        ));
        assert!(!is_hosted_app_candidate(
            r"C:\Windows\System32\ApplicationFrameHost.exe"
        ));
        assert!(!is_hosted_app_candidate(
            r"C:\Windows\SystemApps\TextInputHost.exe"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn reads_codex_project_from_the_active_header() {
        assert_eq!(chatgpt_project_name("项目：HDU"), Some("HDU".into()));
        assert_eq!(chatgpt_project_name("Project: ScreenUse"), Some("ScreenUse".into()));
        assert_eq!(chatgpt_project_name("HDU"), None);
        assert_eq!(
            codex_project_task_list_name("HDU中的已安排任务"),
            Some("HDU".into())
        );
    }

    #[cfg(windows)]
    #[test]
    fn codex_tasks_keep_the_project_and_conversation_hierarchy() {
        assert_eq!(
            codex_task_context_title("codex_work_bridge", Some("HDU")),
            ("项目-HDU-codex_work_bridge".into(), "codex-project-task")
        );
        assert_eq!(
            codex_task_context_title("整理成绩", None),
            ("任务-整理成绩".into(), "codex-task")
        );
        assert_eq!(
            raw_openai_conversation_title(
                "codex-project-task",
                "项目-HDU-codex_work_bridge",
                Some("HDU"),
            ),
            "codex_work_bridge"
        );
        assert_eq!(
            raw_openai_conversation_title("codex-task", "任务-整理成绩", None),
            "整理成绩"
        );
        let new_task = codex_new_task_context();
        assert_eq!(new_task.title, "任务-新建任务");
        assert_eq!(new_task.source, "codex-task");
    }

    #[cfg(windows)]
    #[test]
    fn selected_chat_item_keeps_only_the_current_conversation_name() {
        assert_eq!(
            selected_item_page_title(
                "微信ClawBot\n最新消息预览会不断变化…\n13:31",
                "session_item_微信ClawBot",
                "Weixin.exe",
            ),
            Some("微信ClawBot".into())
        );
        assert_eq!(
            selected_item_page_title(
                "AI众测-cncert团队白帽子1群(498)\n最新消息",
                "session_item_AI众测-cncert团队白帽子1群(498)",
                "Weixin.exe",
            ),
            Some("AI众测-cncert团队白帽子1群".into())
        );
        assert_eq!(
            selected_item_page_title("ICPC 讨论群\n今晚训练", "", "QQ.exe"),
            Some("ICPC 讨论群".into())
        );
    }

    #[cfg(windows)]
    #[test]
    fn qq_header_keeps_people_and_groups_but_rejects_toolbar_actions() {
        assert_eq!(
            clean_qq_conversation_title("科研讨论群"),
            Some("科研讨论群".into())
        );
        assert_eq!(
            clean_qq_conversation_title("张三"),
            Some("张三".into())
        );
        assert_eq!(
            clean_qq_conversation_title("在线状态 我crush了"),
            Some("我crush了".into())
        );
        assert_eq!(
            clean_qq_conversation_title("Mobile online Alice"),
            Some("Alice".into())
        );
        assert_eq!(clean_qq_conversation_title("QQ"), None);
        assert_eq!(clean_qq_conversation_title("语音通话"), None);
        assert_eq!(clean_qq_conversation_title("11:13"), None);
    }

    #[cfg(windows)]
    #[test]
    fn chat_header_detection_accepts_only_semantic_current_conversation_controls() {
        assert!(chat_header_identity_score(
            "content_view.current_chat_name_label",
            "mmui::XTextView",
            false,
        )
        .is_some());
        assert!(chat_header_identity_score("chat_input_field", "mmui::ChatInputField", true)
            .is_some());
        assert!(chat_header_identity_score("conversation_title", "TextBlock", false).is_some());
        assert!(chat_header_identity_score("search_box", "Edit", true).is_none());
        assert!(chat_header_identity_score("message_list", "List", false).is_none());
        assert_eq!(
            clean_chat_header_title("科研讨论群(123)", "Weixin.exe"),
            Some("科研讨论群".into())
        );
        assert_eq!(clean_chat_header_title("微信", "Weixin.exe"), None);
    }

    #[cfg(windows)]
    #[test]
    fn generic_app_titles_are_not_mistaken_for_page_names() {
        assert_eq!(clean_native_page_title("微信", "Weixin.exe"), None);
        assert_eq!(clean_native_page_title("QQ", "QQ.exe"), None);
        assert_eq!(clean_native_page_title("钉钉", "DingTalk.exe"), None);
        assert_eq!(clean_native_page_title("WPS Office", "wps.exe"), None);
        assert_eq!(clean_native_page_title("Google Chrome", "chrome.exe"), None);
        assert_eq!(
            clean_native_page_title("ICPC 训练群", "DingTalk.exe"),
            Some("ICPC 训练群".into())
        );
        assert!(!supports_selected_page_fallback("screenuse.exe"));
        assert!(supports_selected_page_fallback("WindowsTerminal.exe"));
    }

}
