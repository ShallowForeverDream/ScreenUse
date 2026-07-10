mod analysis_worker;
mod ai;
mod collectors;
mod db;
mod export;
mod integrations;
mod integration_server;
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

fn map_err(err: impl std::fmt::Display) -> String { err.to_string() }

#[tauri::command]
fn get_dashboard_data(state: State<AppState>) -> Result<DashboardData, String> {
    state.db.dashboard(state.collector.health().running).map_err(map_err)
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
fn update_session(state: State<AppState>, id: String, patch: SessionPatch) -> Result<models::WorkSession, String> {
    state.db.update_session(&id, patch).map_err(map_err)
}

#[tauri::command]
fn merge_sessions(state: State<AppState>, ids: Vec<String>, summary: Option<String>) -> Result<models::WorkSession, String> {
    state.db.merge_sessions(&ids, summary).map_err(map_err)
}

#[tauri::command]
fn split_session(state: State<AppState>, id: String, split_at: String) -> Result<Vec<models::WorkSession>, String> {
    state.db.split_session(&id, &split_at).map_err(map_err)
}

#[tauri::command]
fn retry_failed_jobs(state: State<AppState>) -> Result<u32, String> {
    state.db.retry_failed_jobs().map_err(map_err)
}

#[tauri::command]
async fn run_analysis_once(state: State<'_, AppState>) -> Result<bool, String> {
    analysis_worker::run_once(state.db.clone()).await.map_err(map_err)
}

#[tauri::command]
fn compact_sessions(state: State<AppState>) -> Result<u32, String> {
    state.db.compact_sessions().map_err(map_err)
}

#[tauri::command]
fn learn_rule_from_session(state: State<AppState>, id: String) -> Result<models::AttributionRule, String> {
    state.db.learn_rule_from_session(&id).map_err(map_err)
}

#[tauri::command]
fn cleanup_media_cache(state: State<AppState>) -> Result<u32, String> {
    state.db.cleanup_media_cache().map_err(map_err)
}

#[tauri::command]
fn save_settings(state: State<AppState>, settings: AppSettings) -> Result<(), String> {
    state.db.save_settings(&settings).map_err(map_err)
}

#[tauri::command]
fn export_data(state: State<AppState>, format: String) -> Result<String, String> {
    export::export_sessions(&state.db, &format).map(|p| p.display().to_string()).map_err(map_err)
}

#[tauri::command]
fn backup_now(state: State<AppState>, target_dir: Option<String>) -> Result<String, String> {
    state.db.backup_now(target_dir).map(|p| p.display().to_string()).map_err(map_err)
}

#[tauri::command]
fn reveal_data_dir(state: State<AppState>) -> String {
    state.db.data_dir().display().to_string()
}

#[tauri::command]
fn import_ddl_manager(state: State<AppState>, db_path: Option<String>) -> Result<usize, String> {
    let path = db_path.unwrap_or_else(|| state.db.get_settings().unwrap_or_default().ddl_manager_db_path);
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
fn import_google_calendar_placeholder() -> usize { integrations::google_calendar_placeholder().len() }

#[tauri::command]
fn import_microsoft_todo_placeholder() -> usize { integrations::microsoft_todo_placeholder().len() }

#[tauri::command]
fn save_secret(name: String, value: String) -> Result<String, String> {
    secrets::save_secret(&name, &value).map_err(map_err)
}

#[tauri::command]
fn read_secret_probe(name: String) -> Result<bool, String> {
    secrets::read_secret(&name).map(|s| !s.is_empty()).map_err(map_err)
}

#[tauri::command]
fn delete_secret(name: String) -> Result<(), String> {
    secrets::delete_secret(&name).map_err(map_err)
}

#[tauri::command]
async fn test_ai_config(settings: AppSettings, secret_name: String) -> Result<String, String> {
    let secret = secrets::read_secret(&secret_name).map_err(map_err)?;
    let _client = ai::OpenAiCompatibleClient::new(&settings, secret);
    Ok("AI 配置已读取；正式分析会通过 OpenAI 兼容 /chat/completions 调用。".into())
}

fn setup_tray(app: &tauri::App, db: Arc<AppDb>, collector: Arc<DesktopCollector>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "tray-open", "打开 ScreenUse", true, None::<&str>)?;
    let start = MenuItem::with_id(app, "tray-start", "开始采集", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "tray-pause", "暂停采集", true, None::<&str>)?;
    let analyze = MenuItem::with_id(app, "tray-analyze", "分析一次", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "tray-quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &start, &pause, &analyze, &separator, &quit])?;

    let db_for_tray = db.clone();
    let collector_for_tray = collector.clone();
    let mut builder = TrayIconBuilder::with_id("screenuse-tray")
        .tooltip("ScreenUse 正在守护你的个人时间账本")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| {
            let id = event.id().as_ref();
            match id {
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
                "tray-analyze" => {
                    let db = db_for_tray.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = analysis_worker::run_once(db).await;
                    });
                }
                "tray-quit" => app.exit(0),
                _ => {}
            }
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
            let db = Arc::new(AppDb::open().map_err(|e| tauri::Error::Anyhow(e))?);
            let collector = Arc::new(DesktopCollector::new());
            integration_server::start_local_ingest_server(db.clone()).map_err(|e| tauri::Error::Anyhow(e))?;
            analysis_worker::start_analysis_worker(db.clone());
            setup_tray(app, db.clone(), collector.clone())?;
            if db.get_settings().unwrap_or_default().auto_start {
                collector.start(db.clone()).map_err(|e| tauri::Error::Anyhow(e))?;
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
