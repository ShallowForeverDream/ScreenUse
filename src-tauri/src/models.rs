#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const DEFAULT_CATEGORIES: [&str; 7] = ["学习", "写作", "开发", "沟通", "娱乐", "杂务", "无效"];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputStats {
    pub idle_seconds: u64,
    pub keyboard_events: u32,
    pub mouse_clicks: u32,
    pub scroll_ticks: u32,
    pub shortcut_events: Vec<String>,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct AiUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
    pub cost_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisJob {
    pub id: String,
    pub chunk_ids: Vec<String>,
    pub metadata_range: TimeRange,
    pub mode: String,
    pub provider: String,
    pub model: String,
    pub retry_count: u32,
    pub status: String,
    pub error: Option<String>,
    pub system_prompt: Option<String>,
    pub user_prompt: Option<String>,
    pub response: Option<String>,
    pub queued_at: String,
    pub processing_started_at: Option<String>,
    pub completed_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub result_count: u32,
    pub usage: AiUsage,
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
pub struct CategoryOption {
    pub name: String,
    pub color: String,
    pub is_builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPin {
    pub project_id: String,
    pub project_name: String,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub category: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoStatus {
    pub available: bool,
    pub label: Option<String>,
    pub created_at: Option<String>,
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
    pub idle_category: String,
    pub idle_project_name: String,
    pub passive_content_counts_as_active: bool,
    pub auto_maintenance: bool,
    pub auto_start: bool,
    pub launch_at_login: bool,
    pub quick_pause_enabled: bool,

    // AI is optional and disabled by default. "manual" analyzes one uncertain
    // completed session when explicitly requested; "auto" also processes queued jobs.
    pub ai_mode: String,
    pub ai_provider: String,
    pub min_ai_session_minutes: u32,
    pub ai_base_url: String,
    pub ai_model: String,
    pub ai_secret_ref: Option<String>,

    pub backup_dir: Option<String>,

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
        Self {
            language: "zh-CN".into(),
            theme: "light".into(),
            poll_interval_seconds: 1,
            heartbeat_seconds: 5,
            raw_event_retention_days: 30,
            idle_threshold_seconds: 180,
            idle_category: "无效".into(),
            idle_project_name: "离开".into(),
            passive_content_counts_as_active: true,
            auto_maintenance: true,
            auto_start: true,
            launch_at_login: false,
            quick_pause_enabled: true,
            ai_mode: "off".into(),
            ai_provider: String::new(),
            min_ai_session_minutes: 1,
            ai_base_url: "https://api.openai.com/v1".into(),
            ai_model: "".into(),
            ai_secret_ref: None,
            backup_dir: None,
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
        self.poll_interval_seconds = self.poll_interval_seconds.clamp(1, 60);
        // Foreground changes are still observed every second, while durable
        // heartbeats are batched to reduce SQLite and SSD write amplification.
        self.heartbeat_seconds = self.heartbeat_seconds.clamp(5, 60);
        self.raw_event_retention_days = self.raw_event_retention_days.clamp(7, 3650);
        self.idle_threshold_seconds = self.idle_threshold_seconds.clamp(30, 3600);
        self.idle_category = clean_setting_label(&self.idle_category, "无效");
        self.idle_project_name = clean_setting_label(&self.idle_project_name, "离开");
        self.min_ai_session_minutes = self.min_ai_session_minutes.clamp(1, 240);
        self.ai_mode = match self.ai_mode.as_str() {
            "manual" => "manual",
            "auto" => "auto",
            _ => "off",
        }
        .into();
        self.ai_provider = match self.ai_provider.as_str() {
            "codex-account" => "codex-account",
            "openai-compatible" => "openai-compatible",
            _ if self.ai_secret_ref.as_deref().is_some_and(|value| !value.trim().is_empty())
                || (!self.ai_model.trim().is_empty()
                    && self.ai_model.trim() != "gpt-5.6-luna")
                || self.ai_base_url.trim_end_matches('/') != "https://api.openai.com/v1" =>
            {
                "openai-compatible"
            }
            _ => "codex-account",
        }
        .into();
        if self.ai_provider == "codex-account" && self.ai_model.trim().is_empty() {
            self.ai_model = "gpt-5.6-luna".into();
        }

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

fn clean_setting_label(value: &str, fallback: &str) -> String {
    let value = value.trim().replace(['\r', '\n', '\t'], " ");
    let value: String = value.chars().take(80).collect();
    if value.is_empty() { fallback.into() } else { value }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardData {
    pub settings: AppSettings,
    pub sessions: Vec<WorkSession>,
    pub projects: Vec<Project>,
    pub tasks: Vec<Task>,
    pub category_options: Vec<CategoryOption>,
    pub active_context: Option<ContextPin>,
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
    pub clear_project: Option<bool>,
    pub clear_task: Option<bool>,
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
        assert_eq!(existing.poll_interval_seconds, 1);
        assert!(existing.passive_content_counts_as_active);
        let normalized_existing = existing.normalized();
        assert_eq!(normalized_existing.ai_provider, "codex-account");
        assert_eq!(normalized_existing.ai_model, "gpt-5.6-luna");
        assert_eq!(normalized_existing.min_ai_session_minutes, 1);

        let invalid = AppSettings {
            theme: "unknown".into(),
            ..AppSettings::default()
        };
        assert_eq!(invalid.normalized().theme, "light");

        let dark = AppSettings {
            theme: "dark".into(),
            ..AppSettings::default()
        };
        assert_eq!(dark.normalized().theme, "dark");

        let legacy: AppSettings =
            serde_json::from_str(r#"{"pollIntervalSeconds":2,"heartbeatSeconds":30}"#)
                .expect("deserialize legacy sampling settings");
        let migrated = legacy.normalized();
        assert_eq!(migrated.poll_interval_seconds, 2);
        assert_eq!(migrated.heartbeat_seconds, 30);

        let write_heavy = AppSettings {
            poll_interval_seconds: 1,
            heartbeat_seconds: 1,
            ..AppSettings::default()
        };
        assert_eq!(write_heavy.normalized().heartbeat_seconds, 5);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GithubSyncConfig {
    pub enabled: bool,
    pub owner: String,
    pub repo: String,
    pub branch: String,
    pub file_path: String,
    pub auto_sync: bool,
    pub interval_minutes: u32,
    pub device_id: String,
    pub device_name: String,
    pub token_secret_ref: String,
    pub key_secret_ref: String,
    pub last_synced_at: Option<String>,
    pub last_remote_sha: Option<String>,
    pub last_error: Option<String>,
}

impl Default for GithubSyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            owner: String::new(),
            repo: "ScreenUse-Data".into(),
            branch: "main".into(),
            file_path: "screenuse/snapshot-v1.json.gz.enc".into(),
            auto_sync: true,
            interval_minutes: 15,
            device_id: Uuid::new_v4().to_string(),
            device_name: default_device_name(),
            token_secret_ref: "github-sync-token".into(),
            key_secret_ref: "github-sync-key".into(),
            last_synced_at: None,
            last_remote_sha: None,
            last_error: None,
        }
    }
}

impl GithubSyncConfig {
    pub fn normalized(mut self) -> Self {
        self.owner = self
            .owner
            .trim()
            .trim_start_matches('@')
            .chars()
            .take(80)
            .collect();
        self.repo = self
            .repo
            .trim()
            .trim_end_matches(".git")
            .chars()
            .take(100)
            .collect();
        if self.repo.is_empty() {
            self.repo = "ScreenUse-Data".into();
        }
        self.branch = self.branch.trim().chars().take(100).collect();
        if self.branch.is_empty() {
            self.branch = "main".into();
        }
        self.file_path = self
            .file_path
            .trim()
            .trim_start_matches('/')
            .chars()
            .take(180)
            .collect();
        if self.file_path.is_empty() {
            self.file_path = "screenuse/snapshot-v1.json.gz.enc".into();
        }
        self.interval_minutes = self.interval_minutes.clamp(5, 1_440);
        if self.device_id.trim().is_empty() {
            self.device_id = Uuid::new_v4().to_string();
        }
        self.device_name = self.device_name.trim().chars().take(80).collect();
        if self.device_name.is_empty() {
            self.device_name = default_device_name();
        }
        if self.token_secret_ref.trim().is_empty() {
            self.token_secret_ref = "github-sync-token".into();
        }
        if self.key_secret_ref.trim().is_empty() {
            self.key_secret_ref = "github-sync-key".into();
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncCounts {
    pub categories: u32,
    pub projects: u32,
    pub tasks: u32,
    pub sessions: u32,
    pub rules: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDeviceInfo {
    pub id: String,
    pub name: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubSyncStatus {
    pub config: GithubSyncConfig,
    pub token_configured: bool,
    pub key_configured: bool,
    pub ready: bool,
    pub counts: SyncCounts,
    pub devices: Vec<SyncDeviceInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubSyncResult {
    pub synced_at: String,
    pub remote_sha: String,
    pub uploaded_bytes: u64,
    pub downloaded_bytes: u64,
    pub counts: SyncCounts,
    pub message: String,
}

fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "这台电脑".into())
}
