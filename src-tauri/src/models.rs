#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_CATEGORIES: [&str; 7] = ["学习", "写作", "开发", "沟通", "娱乐", "杂务", "离开"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputStats {
    pub idle_seconds: u64,
    pub keyboard_events: u32,
    pub mouse_clicks: u32,
    pub scroll_ticks: u32,
    pub shortcut_events: Vec<String>,
}

impl Default for InputStats {
    fn default() -> Self {
        Self {
            idle_seconds: 0,
            keyboard_events: 0,
            mouse_clicks: 0,
            scroll_ticks: 0,
            shortcut_events: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawActivityEvent {
    pub id: String,
    pub source: String,
    pub timestamp: String,
    pub app: Option<String>,
    pub window_title: Option<String>,
    pub url: Option<String>,
    pub file_path: Option<String>,
    pub workspace: Option<String>,
    pub input_stats: InputStats,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaChunk {
    pub id: String,
    pub display_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub path: String,
    pub fps: f32,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisJob {
    pub id: String,
    pub chunk_ids: Vec<String>,
    pub metadata_range: TimeRange,
    pub mode: String,
    pub retry_count: u32,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeRange {
    pub started_at: String,
    pub ended_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub category: String,
    pub source: String,
    pub color: String,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub status: String,
    pub source: String,
    pub planned_due_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceItem {
    pub kind: String,
    pub label: String,
    pub value: String,
    pub weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub id: String,
    pub session_id: String,
    pub source: String,
    pub title: String,
    pub summary: String,
    pub started_at: String,
    pub ended_at: String,
    pub evidence: Vec<EvidenceItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkSession {
    pub id: String,
    pub started_at: String,
    pub ended_at: String,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub category: String,
    pub summary: String,
    pub confidence: f32,
    pub evidence: Vec<EvidenceItem>,
    pub user_confirmed: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionRule {
    pub id: String,
    pub name: String,
    pub priority: i32,
    pub matcher: Value,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub category: String,
    pub created_from_correction: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub id: String,
    pub source: String,
    pub title: String,
    pub note: Option<String>,
    pub start_at: Option<String>,
    pub due_at: Option<String>,
    pub status: String,
    pub tags: Vec<String>,
    pub matched_session_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendPoint {
    pub label: String,
    pub value: f64,
    pub group: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueHealth {
    pub pending: u32,
    pub running: u32,
    pub failed: u32,
    pub downgraded: u32,
    pub temp_storage_gb: f32,
    pub temp_storage_limit_gb: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub language: String,
    pub capture_scope: String,
    pub fps: f32,
    pub chunk_minutes: u32,
    pub analysis_timing: String,
    pub ai_base_url: String,
    pub ai_model: String,
    pub ai_secret_ref: Option<String>,
    pub temp_storage_limit_gb: u32,
    pub idle_threshold_seconds: u32,
    pub backup_dir: Option<String>,
    pub ddl_manager_db_path: String,
    pub auto_start: bool,
    pub quick_pause_enabled: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
        Self {
            language: "zh-CN".into(),
            capture_scope: "all-displays".into(),
            fps: 1.0,
            chunk_minutes: 5,
            analysis_timing: "near-realtime".into(),
            ai_base_url: "https://api.openai.com/v1".into(),
            ai_model: "gpt-4o-mini".into(),
            ai_secret_ref: None,
            temp_storage_limit_gb: 20,
            idle_threshold_seconds: 180,
            backup_dir: None,
            ddl_manager_db_path: format!(r"{}\.ddl-manager\app.db", home),
            auto_start: true,
            quick_pause_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardData {
    pub settings: AppSettings,
    pub sessions: Vec<WorkSession>,
    pub projects: Vec<Project>,
    pub tasks: Vec<Task>,
    pub plan_items: Vec<PlanItem>,
    pub trends: Vec<TrendPoint>,
    pub categories: Vec<TrendPoint>,
    pub queue: QueueHealth,
    pub collector_running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPatch {
    pub summary: Option<String>,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub category: Option<String>,
    pub confidence: Option<f32>,
    pub user_confirmed: Option<bool>,
}
