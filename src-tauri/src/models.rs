#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

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
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub language: String,
    pub theme: String,

    // Metadata-first runtime settings.
    pub poll_interval_seconds: u32,
    pub heartbeat_seconds: u32,
    pub raw_event_retention_days: u32,
    pub idle_threshold_seconds: u32,
    pub auto_maintenance: bool,
    pub auto_start: bool,
    pub launch_at_login: bool,
    pub quick_pause_enabled: bool,

    // AI is optional and disabled by default. "manual" analyzes one uncertain
    // completed session when explicitly requested; "auto" also processes queued jobs.
    pub ai_mode: String,
    pub min_ai_session_minutes: u32,
    pub ai_base_url: String,
    pub ai_model: String,
    pub ai_secret_ref: Option<String>,

    pub backup_dir: Option<String>,
    pub ddl_manager_db_path: String,

    // v0.1 compatibility fields. They remain deserializable so existing settings
    // migrate cleanly, but the runtime normalizes them to metadata-only values.
    pub capture_scope: String,
    pub fps: f32,
    pub chunk_minutes: u32,
    pub analysis_timing: String,
    pub temp_storage_limit_gb: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".into());
        Self {
            language: "zh-CN".into(),
            theme: "light".into(),
            poll_interval_seconds: 2,
            heartbeat_seconds: 30,
            raw_event_retention_days: 30,
            idle_threshold_seconds: 180,
            auto_maintenance: true,
            auto_start: true,
            launch_at_login: false,
            quick_pause_enabled: true,
            ai_mode: "off".into(),
            min_ai_session_minutes: 10,
            ai_base_url: "https://api.openai.com/v1".into(),
            ai_model: "".into(),
            ai_secret_ref: None,
            backup_dir: None,
            ddl_manager_db_path: PathBuf::from(home)
                .join(".ddl-manager")
                .join("app.db")
                .display()
                .to_string(),
            capture_scope: "metadata-only".into(),
            fps: 0.0,
            chunk_minutes: 0,
            analysis_timing: "local-only".into(),
            temp_storage_limit_gb: 1,
        }
    }
}

impl AppSettings {
    pub fn normalized(mut self) -> Self {
        self.theme = match self.theme.as_str() {
            "light" => "light",
            "dark" => "dark",
            "system" => "system",
            _ => "light",
        }
        .into();
        self.poll_interval_seconds = self.poll_interval_seconds.clamp(1, 15);
        self.heartbeat_seconds = self
            .heartbeat_seconds
            .clamp(10, 300)
            .max(self.poll_interval_seconds.saturating_mul(2));
        self.raw_event_retention_days = self.raw_event_retention_days.clamp(7, 3650);
        self.idle_threshold_seconds = self.idle_threshold_seconds.clamp(30, 3600);
        self.min_ai_session_minutes = self.min_ai_session_minutes.clamp(1, 240);
        self.ai_mode = match self.ai_mode.as_str() {
            "manual" => "manual",
            "auto" => "auto",
            _ => "off",
        }
        .into();

        self.capture_scope = "metadata-only".into();
        self.fps = 0.0;
        self.chunk_minutes = 0;
        self.analysis_timing = if self.ai_mode == "off" {
            "local-only"
        } else {
            "local-first"
        }
        .into();
        self.temp_storage_limit_gb = 1;
        self
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

#[cfg(test)]
mod tests {
    use super::AppSettings;

    #[test]
    fn theme_defaults_and_normalizes_for_existing_settings() {
        let existing: AppSettings = serde_json::from_str("{}").expect("deserialize defaults");
        assert_eq!(existing.theme, "light");

        let mut invalid = AppSettings::default();
        invalid.theme = "unknown".into();
        assert_eq!(invalid.normalized().theme, "light");

        let mut dark = AppSettings::default();
        dark.theme = "dark".into();
        assert_eq!(dark.normalized().theme, "dark");
    }
}
