#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(all(not(debug_assertions), not(feature = "custom-protocol")))]
compile_error!(
    "ScreenUse release builds must use `pnpm tauri:build` (or enable `--features custom-protocol`)"
);

mod ai;
mod analysis_worker;
mod autostart;
mod classification;
mod collectors;
mod context_store;
mod db;
mod export;
mod integration_server;
mod integrations;
mod maintenance;
mod models;
mod secrets;

use collectors::{CollectorAdapter, DesktopCollector};
use db::AppDb;
use integrations::{DdlManagerAdapter, IcsAdapter, IntegrationAdapter};
use models::{AppSettings, DashboardData, SessionPatch};
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, State};

struct AppState {
    db: Arc<AppDb>,
    collector: Arc<DesktopCollector>,
}

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
fn get_dashboard_data(state: State<AppState>) -> Result<DashboardData, String> {
    state
        .db
        .dashboard(state.collector.health().running)
        .map_err(map_err)
}

#[tauri::command]
fn start_collector(state: State<AppState>) -> Result<(), String> {
    state.collector.start(state.db.clone()).map_err(map_err)
}

#[tauri::command]
fn stop_collector(state: State<AppState>) -> Result<(), String> {
    state.collector.stop().map_err(map_err)
}

#[tauri::command]
fn collector_health(state: State<AppState>) -> collectors::CollectorHealth {
    state.collector.health()
}

#[tauri::command]
fn update_session(
    state: State<AppState>,
    id: String,
    patch: SessionPatch,
) -> Result<models::WorkSession, String> {
    state.db.update_session(&id, patch).map_err(map_err)
}

#[tauri::command]
fn update_sessions(
    state: State<AppState>,
    ids: Vec<String>,
    patch: SessionPatch,
) -> Result<Vec<models::WorkSession>, String> {
    state.db.update_sessions(&ids, patch).map_err(map_err)
}

#[tauri::command]
fn create_project(
    state: State<AppState>,
    name: String,
    category: String,
) -> Result<models::Project, String> {
    state.db.create_project(&name, &category).map_err(map_err)
}

#[tauri::command]
fn delete_project(state: State<AppState>, id: String) -> Result<(), String> {
    state.db.delete_project(&id).map_err(map_err)
}

#[tauri::command]
fn create_category(state: State<AppState>, name: String) -> Result<models::CategoryOption, String> {
    state.db.create_category(&name).map_err(map_err)
}

#[tauri::command]
fn delete_category(state: State<AppState>, name: String) -> Result<(), String> {
    state.db.delete_category(&name).map_err(map_err)
}

#[tauri::command]
fn create_task(state: State<AppState>, project_id: String, title: String) -> Result<models::Task, String> {
    state.db.create_task(&project_id, &title).map_err(map_err)
}

#[tauri::command]
fn delete_task(state: State<AppState>, id: String) -> Result<(), String> {
    state.db.delete_task(&id).map_err(map_err)
}

#[tauri::command]
fn pin_context(
    state: State<AppState>,
    project_id: String,
    task_id: Option<String>,
    minutes: u32,
) -> Result<models::ContextPin, String> {
    let pin = state.db.pin_context(&project_id, task_id.as_deref(), minutes).map_err(map_err)?;
    if state.collector.health().running {
        state.collector.stop().map_err(map_err)?;
        state.collector.start(state.db.clone()).map_err(map_err)?;
    }
    let db = state.db.clone();
    let collector = state.collector.clone();
    let wait_minutes = minutes.clamp(5, 240);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(u64::from(wait_minutes) * 60));
        if matches!(db.active_context(), Ok(None)) && collector.health().running {
            let _ = collector.stop();
            let _ = collector.start(db);
        }
    });
    Ok(pin)
}

#[tauri::command]
fn clear_context_pin(state: State<AppState>) -> Result<(), String> {
    state.db.clear_context_pin().map_err(map_err)?;
    if state.collector.health().running {
        state.collector.stop().map_err(map_err)?;
        state.collector.start(state.db.clone()).map_err(map_err)?;
    }
    Ok(())
}

#[tauri::command]
fn merge_sessions(
    state: State<AppState>,
    ids: Vec<String>,
    summary: Option<String>,
) -> Result<models::WorkSession, String> {
    state.db.merge_sessions(&ids, summary).map_err(map_err)
}

#[tauri::command]
fn split_session(
    state: State<AppState>,
    id: String,
    split_at: String,
) -> Result<Vec<models::WorkSession>, String> {
    state.db.split_session(&id, &split_at).map_err(map_err)
}

#[tauri::command]
fn retry_failed_jobs(state: State<AppState>) -> Result<u32, String> {
    state.db.retry_failed_jobs().map_err(map_err)
}

#[tauri::command]
async fn run_analysis_once(state: State<'_, AppState>) -> Result<bool, String> {
    let has_pending = state.db.queue_health().map_err(map_err)?.pending > 0;
    if !has_pending {
        let queued = analysis_worker::enqueue_recent_uncertain(&state.db).map_err(map_err)?;
        if !queued {
            return Ok(false);
        }
    }
    analysis_worker::run_once(state.db.clone())
        .await
        .map_err(map_err)
}

#[tauri::command]
fn compact_sessions(state: State<AppState>) -> Result<u32, String> {
    state.db.compact_sessions().map_err(map_err)
}

#[tauri::command]
fn learn_rule_from_session(
    state: State<AppState>,
    id: String,
    keyword: Option<String>,
) -> Result<models::AttributionRule, String> {
    state.db.learn_rule_from_session(&id, keyword.as_deref()).map_err(map_err)
}

#[tauri::command]
fn cleanup_media_cache(state: State<AppState>) -> Result<u32, String> {
    maintenance::optimize_storage(&state.db, true).map_err(map_err)
}

#[tauri::command]
fn save_settings(state: State<AppState>, settings: AppSettings) -> Result<(), String> {
    let settings = settings.normalized();
    let previous = state.db.get_settings().map_err(map_err)?.normalized();
    if previous.launch_at_login != settings.launch_at_login {
        autostart::set_launch_at_login(settings.launch_at_login).map_err(map_err)?;
    }
    state.db.save_settings(&settings).map_err(map_err)
}

#[tauri::command]
fn export_data(state: State<AppState>, format: String) -> Result<String, String> {
    export::export_sessions(&state.db, &format)
        .map(|path| path.display().to_string())
        .map_err(map_err)
}

#[tauri::command]
fn backup_now(state: State<AppState>, target_dir: Option<String>) -> Result<String, String> {
    maintenance::checkpoint(&state.db).map_err(map_err)?;
    state
        .db
        .backup_now(target_dir)
        .map(|path| path.display().to_string())
        .map_err(map_err)
}

#[tauri::command]
fn reveal_data_dir(state: State<AppState>) -> String {
    state.db.data_dir().display().to_string()
}

#[tauri::command]
fn import_ddl_manager(state: State<AppState>, db_path: Option<String>) -> Result<usize, String> {
    let path = db_path.unwrap_or_else(|| {
        state
            .db
            .get_settings()
            .unwrap_or_default()
            .ddl_manager_db_path
    });
    let adapter = DdlManagerAdapter { db_path: path };
    let items = adapter.pull_plan_items().map_err(map_err)?;
    state.db.upsert_plan_items(&items).map_err(map_err)
}

#[tauri::command]
fn import_ics(state: State<AppState>, path: String) -> Result<usize, String> {
    let adapter = IcsAdapter { path };
    let items = adapter.pull_plan_items().map_err(map_err)?;
    state.db.upsert_plan_items(&items).map_err(map_err)
}

#[tauri::command]
fn import_google_calendar_placeholder() -> usize {
    integrations::google_calendar_placeholder().len()
}

#[tauri::command]
fn import_microsoft_todo_placeholder() -> usize {
    integrations::microsoft_todo_placeholder().len()
}

#[tauri::command]
fn save_secret(name: String, value: String) -> Result<String, String> {
    secrets::save_secret(&name, &value).map_err(map_err)
}

#[tauri::command]
fn read_secret_probe(name: String) -> Result<bool, String> {
    secrets::read_secret(&name)
        .map(|secret| !secret.is_empty())
        .map_err(map_err)
}

#[tauri::command]
fn delete_secret(name: String) -> Result<(), String> {
    secrets::delete_secret(&name).map_err(map_err)
}

#[tauri::command]
async fn test_ai_config(settings: AppSettings, secret_name: String) -> Result<String, String> {
    let settings = settings.normalized();
    if settings.ai_mode == "off" {
        return Err("AI 模式当前为关闭；请先选择手动复核或自动复核".into());
    }
    if settings.ai_model.trim().is_empty() {
        return Err("模型名不能为空".into());
    }
    let secret = secrets::read_secret(&secret_name).map_err(map_err)?;
    if secret.trim().is_empty() {
        return Err("凭据为空".into());
    }
    let _client = ai::OpenAiCompatibleClient::new(&settings, secret);
    Ok("配置可读取。AI 只会接收低置信会话的窗口、网址、文件和工作区元数据。".into())
}

fn run_optional_ai(db: Arc<AppDb>) {
    tauri::async_runtime::spawn(async move {
        let has_pending = db.queue_health().map(|health| health.pending > 0).unwrap_or(false);
        if !has_pending && !analysis_worker::enqueue_recent_uncertain(&db).unwrap_or(false) {
            return;
        }
        let _ = analysis_worker::run_once(db).await;
    });
}

fn setup_tray(
    app: &tauri::App,
    db: Arc<AppDb>,
    collector: Arc<DesktopCollector>,
) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "tray-open", "打开 ScreenUse", true, None::<&str>)?;
    let start = MenuItem::with_id(app, "tray-start", "开始自动记录", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "tray-pause", "暂停自动记录", true, None::<&str>)?;
    let analyze = MenuItem::with_id(app, "tray-analyze", "AI 复核一条", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "tray-quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &start, &pause, &analyze, &separator, &quit])?;

    let db_for_tray = db.clone();
    let collector_for_tray = collector.clone();
    let mut builder = TrayIconBuilder::with_id("screenuse-tray")
        .tooltip("ScreenUse · 本地元数据时间账本")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "tray-open" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "tray-start" => {
                let _ = collector_for_tray.start(db_for_tray.clone());
            }
            "tray-pause" => {
                let _ = collector_for_tray.stop();
            }
            "tray-analyze" => run_optional_ai(db_for_tray.clone()),
            "tray-quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let tray = builder.build(app)?;
    app.manage(tray);
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .setup(|app| {
            let db = Arc::new(AppDb::open().map_err(tauri::Error::Anyhow)?);
            maintenance::initialize(&db).map_err(tauri::Error::Anyhow)?;
            let settings = db.get_settings().unwrap_or_default().normalized();
            if settings.launch_at_login {
                if let Err(error) = autostart::set_launch_at_login(true) {
                    eprintln!("ScreenUse login startup refresh error: {error}");
                }
            }

            let collector = Arc::new(DesktopCollector::new());
            integration_server::start_local_ingest_server().map_err(tauri::Error::Anyhow)?;
            analysis_worker::start_analysis_worker(db.clone());
            maintenance::start_worker(db.clone());
            setup_tray(app, db.clone(), collector.clone())?;

            if settings.auto_start {
                collector
                    .start(db.clone())
                    .map_err(tauri::Error::Anyhow)?;
            }
            if autostart::background_launch_requested() {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            app.manage(AppState { db, collector });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_dashboard_data,
            start_collector,
            stop_collector,
            collector_health,
            update_session,
            update_sessions,
            create_project,
            delete_project,
            create_category,
            delete_category,
            create_task,
            delete_task,
            pin_context,
            clear_context_pin,
            merge_sessions,
            split_session,
            retry_failed_jobs,
            run_analysis_once,
            compact_sessions,
            learn_rule_from_session,
            cleanup_media_cache,
            save_settings,
            export_data,
            backup_now,
            reveal_data_dir,
            import_ddl_manager,
            import_ics,
            import_google_calendar_placeholder,
            import_microsoft_todo_placeholder,
            save_secret,
            read_secret_probe,
            delete_secret,
            test_ai_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running ScreenUse");
}
