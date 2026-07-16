use crate::classification;
use crate::models::*;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use directories::ProjectDirs;
use parking_lot::Mutex;
use rusqlite::{params, Connection, DatabaseName, OptionalExtension, Row};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const ONE_SECOND_SAMPLING_MIGRATION_KEY: &str = "migration_sampling_1s_v1";
const IDLE_BOUNDARY_MIGRATION_KEY: &str = "migration_idle_boundary_v1";
const PERSONAL_MEMORY_MIGRATION_KEY: &str = "migration_personal_memory_v1";
const PERSONAL_MEMORY_CONSENSUS_MIGRATION_KEY: &str = "migration_personal_memory_consensus_v2";
const PERSONAL_MEMORY_BATCH_MIGRATION_KEY: &str = "migration_personal_memory_batch_v3";
const PERSONAL_MEMORY_COHERENCE_MIGRATION_KEY: &str = "migration_personal_memory_quality_v5";
const PERSONAL_MEMORY_AI_CONSENSUS_MIGRATION_KEY: &str =
    "migration_personal_memory_ai_consensus_v6";
const PROCESS_FILE_PATH_MIGRATION_KEY: &str = "migration_process_file_path_v1";
const PROCESS_FILE_MEMORY_MIGRATION_KEY: &str = "migration_process_file_memory_v1";
const AI_IDLE_REVIEW_REPAIR_MIGRATION_KEY: &str = "migration_ai_idle_review_repair_v1";
const AI_CONCRETE_TASK_REPAIR_MIGRATION_KEY: &str = "migration_ai_concrete_task_repair_v7";
const RECENT_MAINTENANCE_DAYS: i64 = 14;
const MAX_RECENT_MAINTENANCE_SESSIONS: i64 = 20_000;
const MAX_PERSONAL_MEMORIES: i64 = 2_000;
const LAST_CORRECTION_UNDO_FILE: &str = "last-correction-undo.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UndoSessionRow {
    id: String,
    started_at: String,
    ended_at: String,
    project_id: Option<String>,
    task_id: Option<String>,
    category: String,
    summary: String,
    confidence: f32,
    evidence_json: String,
    user_confirmed: bool,
    source: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UndoRuleRow {
    id: String,
    name: String,
    priority: i32,
    matcher_json: String,
    project_id: Option<String>,
    task_id: Option<String>,
    category: String,
    created_from_correction: bool,
    enabled: bool,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UndoContextPinRow {
    project_id: String,
    task_id: Option<String>,
    expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UndoMemoryRow {
    session_id: String,
    features_json: String,
    category: String,
    project_id: String,
    task_id: String,
    confirmed_at: String,
    last_used_at: Option<String>,
    use_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionCorrectionUndo {
    label: String,
    created_at: String,
    sessions: Vec<UndoSessionRow>,
    correction_rules: Vec<UndoRuleRow>,
    #[serde(default)]
    memories: Vec<UndoMemoryRow>,
    context_pin: Option<UndoContextPinRow>,
}

struct AttributionDecision {
    project_id: Option<String>,
    task_id: Option<String>,
    category: String,
    summary: String,
    confidence: f32,
    evidence: Option<EvidenceItem>,
}

struct LastSessionAttribution {
    id: String,
    project_id: Option<String>,
    task_id: Option<String>,
    summary: String,
    category: String,
    user_confirmed: bool,
    source: String,
}

#[cfg(test)]
struct ConfirmedContextVote {
    count: u32,
    project_id: String,
    task_id: String,
    category: String,
    first_confirmed_at: String,
}

pub(crate) struct RecentTaskContext {
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub category: String,
    pub confidence: f32,
    pub user_confirmed: bool,
    pub source: String,
    pub ended_at: String,
}

#[cfg(test)]
#[derive(Clone)]
struct ContextPropagationRow {
    id: String,
    started_at: String,
    ended_at: String,
    project_id: Option<String>,
    task_id: Option<String>,
    summary: String,
    confidence: f32,
    user_confirmed: bool,
    source: String,
}

struct SeedSession<'a> {
    id: &'a str,
    project_id: &'a str,
    task_id: &'a str,
    category: &'a str,
    summary: &'a str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    confidence: f32,
}

pub struct AppDb {
    pub(crate) conn: Mutex<Connection>,
    data_dir: PathBuf,
}

impl AppDb {
    pub fn open() -> Result<Self> {
        let dirs = ProjectDirs::from("com", "ShallowDream", "ScreenUse")
            .context("cannot locate platform data dir")?;
        Self::open_in(dirs.data_dir().to_path_buf())
    }

    pub(crate) fn open_in(data_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&data_dir)?;
        fs::create_dir_all(data_dir.join("exports"))?;
        fs::create_dir_all(data_dir.join("backups"))?;
        let db_path = data_dir.join("screenuse.db");
        let conn = Connection::open(&db_path)?;
        configure_connection(&conn)?;
        let db = Self {
            conn: Mutex::new(conn),
            data_dir,
        };
        db.migrate()?;
        db.clear_obsolete_project_descriptions()?;
        db.seed_if_empty()?;
        db.migrate_process_file_paths()?;
        db.migrate_personal_memory()?;
        db.migrate_process_file_memories()?;
        db.migrate_personal_memory_consensus()?;
        db.migrate_personal_memory_batches()?;
        db.migrate_personal_memory_coherence()?;
        db.migrate_personal_memory_ai_consensus()?;
        db.normalize_correction_rules()?;
        db.backfill_idle_boundaries()?;
        let settings = db.get_settings()?.normalized();
        let idle_project_id = db.configure_idle_target(&settings)?;
        db.repair_ai_reviewed_idle_sessions(&settings, &idle_project_id)?;
        db.migrate_incomplete_ai_review_tasks()?;
        db.repair_session_timeline()?;
        db.compact_sessions()?;
        Ok(db)
    }

    fn clear_obsolete_project_descriptions(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE projects SET description=NULL WHERE description=?1",
            params!["在修正归类时手动创建"],
        )?;
        Ok(())
    }

    fn migrate_incomplete_ai_review_tasks(&self) -> Result<u32> {
        let mut conn = self.conn.lock();
        if conn
            .query_row(
                "SELECT 1 FROM settings WHERE key=?1 LIMIT 1",
                params![AI_CONCRETE_TASK_REPAIR_MIGRATION_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some()
        {
            return Ok(0);
        }

        let tx = conn.transaction()?;
        let candidates = {
            let mut stmt = tx.prepare(
                "SELECT ws.id,t.title
                 FROM work_sessions ws
                 JOIN tasks t ON t.id=ws.task_id
                 WHERE ws.source='ai-review' AND ws.user_confirmed=0",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            collect_rows(rows)?
        };
        let mut changed = 0_u32;
        for (session_id, task_title) in candidates {
            if !crate::ai::is_placeholder_task_title(&task_title) {
                continue;
            }
            changed += tx.execute(
                "UPDATE work_sessions
                 SET project_id=NULL,task_id=NULL,confidence=MIN(confidence,0.79),
                     source='context-complete',updated_at=?1
                 WHERE id=?2 AND source='ai-review' AND user_confirmed=0",
                params![now(), session_id],
            )? as u32;
        }
        tx.execute(
            "INSERT INTO settings(key,value,updated_at) VALUES(?1,'done',?2)",
            params![AI_CONCRETE_TASK_REPAIR_MIGRATION_KEY, now()],
        )?;
        tx.commit()?;
        Ok(changed)
    }

    fn migrate_process_file_paths(&self) -> Result<u32> {
        let changed = {
            let mut conn = self.conn.lock();
            let already_migrated = conn
                .query_row(
                    "SELECT 1 FROM settings WHERE key=?1 LIMIT 1",
                    params![PROCESS_FILE_PATH_MIGRATION_KEY],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .is_some();
            if already_migrated {
                return Ok(0);
            }
            let tx = conn.transaction()?;
            let changed = tx.execute(
                r#"UPDATE raw_events
                   SET file_path=NULL
                   WHERE file_path IS NOT NULL AND app IS NOT NULL
                     AND lower(replace(file_path,'/','\')) LIKE '%\' || lower(app)"#,
                [],
            )? as u32;
            tx.execute(
                "INSERT INTO settings(key,value,updated_at) VALUES(?1,'done',?2)",
                params![PROCESS_FILE_PATH_MIGRATION_KEY, now()],
            )?;
            tx.commit()?;
            changed
        };
        if changed > 0 {
            self.rebuild_personal_memory_from_confirmed()?;
        }
        Ok(changed)
    }

    fn migrate_process_file_memories(&self) -> Result<u32> {
        let mut conn = self.conn.lock();
        let already_migrated = conn
            .query_row(
                "SELECT 1 FROM settings WHERE key=?1 LIMIT 1",
                params![PROCESS_FILE_MEMORY_MIGRATION_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if already_migrated {
            return Ok(0);
        }
        let rows = {
            let mut stmt =
                conn.prepare("SELECT session_id,features_json FROM attribution_memories")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            collect_rows(rows)?
        };
        let tx = conn.transaction()?;
        let mut changed = 0_u32;
        for (session_id, raw) in rows {
            let Ok(mut features) = serde_json::from_str::<crate::memory::ContextFeatures>(&raw)
            else {
                continue;
            };
            if crate::memory::clear_legacy_process_file(&mut features) {
                tx.execute(
                    "UPDATE attribution_memories SET features_json=?1 WHERE session_id=?2",
                    params![serde_json::to_string(&features)?, session_id],
                )?;
                changed += 1;
            }
        }
        tx.execute(
            "INSERT INTO settings(key,value,updated_at) VALUES(?1,'done',?2)",
            params![PROCESS_FILE_MEMORY_MIGRATION_KEY, now()],
        )?;
        tx.commit()?;
        Ok(changed)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS settings (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS projects (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              category TEXT NOT NULL,
              source TEXT NOT NULL,
              color TEXT NOT NULL,
              description TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS activity_categories (
              name TEXT PRIMARY KEY,
              color TEXT NOT NULL,
              is_builtin INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tasks (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL,
              title TEXT NOT NULL,
              status TEXT NOT NULL,
              source TEXT NOT NULL,
              planned_due_at TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS work_sessions (
              id TEXT PRIMARY KEY,
              started_at TEXT NOT NULL,
              ended_at TEXT NOT NULL,
              project_id TEXT,
              task_id TEXT,
              category TEXT NOT NULL,
              summary TEXT NOT NULL,
              confidence REAL NOT NULL,
              evidence_json TEXT NOT NULL,
              user_confirmed INTEGER NOT NULL DEFAULT 0,
              source TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE SET NULL,
              FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE SET NULL
            );
            CREATE TABLE IF NOT EXISTS activities (
              id TEXT PRIMARY KEY,
              session_id TEXT NOT NULL,
              source TEXT NOT NULL,
              title TEXT NOT NULL,
              summary TEXT NOT NULL,
              started_at TEXT NOT NULL,
              ended_at TEXT NOT NULL,
              evidence_json TEXT NOT NULL,
              FOREIGN KEY(session_id) REFERENCES work_sessions(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS raw_events (
              id TEXT PRIMARY KEY,
              source TEXT NOT NULL,
              timestamp TEXT NOT NULL,
              app TEXT,
              window_title TEXT,
              url TEXT,
              file_path TEXT,
              workspace TEXT,
              input_stats_json TEXT NOT NULL,
              metadata_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS media_chunks (
              id TEXT PRIMARY KEY,
              display_id TEXT NOT NULL,
              started_at TEXT NOT NULL,
              ended_at TEXT,
              path TEXT NOT NULL,
              fps REAL NOT NULL,
              status TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS analysis_jobs (
              id TEXT PRIMARY KEY,
              chunk_ids_json TEXT NOT NULL,
              started_at TEXT NOT NULL,
              ended_at TEXT NOT NULL,
              mode TEXT NOT NULL,
              retry_count INTEGER NOT NULL,
              status TEXT NOT NULL,
              error TEXT,
              provider TEXT NOT NULL DEFAULT '',
              model TEXT NOT NULL DEFAULT '',
              system_prompt TEXT,
              user_prompt TEXT,
              response TEXT,
              queued_at TEXT NOT NULL DEFAULT '',
              processing_started_at TEXT,
              completed_at TEXT,
              duration_ms INTEGER,
              result_count INTEGER NOT NULL DEFAULT 0,
              usage_json TEXT NOT NULL DEFAULT '{}'
            );
            CREATE TABLE IF NOT EXISTS attribution_rules (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              priority INTEGER NOT NULL,
              matcher_json TEXT NOT NULL,
              project_id TEXT,
              task_id TEXT,
              category TEXT NOT NULL,
              created_from_correction INTEGER NOT NULL DEFAULT 0,
              enabled INTEGER NOT NULL DEFAULT 1,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS attribution_memories (
              session_id TEXT PRIMARY KEY,
              features_json TEXT NOT NULL,
              category TEXT NOT NULL,
              project_id TEXT NOT NULL,
              task_id TEXT NOT NULL,
              confirmed_at TEXT NOT NULL,
              last_used_at TEXT,
              use_count INTEGER NOT NULL DEFAULT 0,
              FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE,
              FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS plan_items (
              id TEXT PRIMARY KEY,
              source TEXT NOT NULL,
              title TEXT NOT NULL,
              note TEXT,
              start_at TEXT,
              due_at TEXT,
              status TEXT NOT NULL,
              tags_json TEXT NOT NULL,
              matched_session_ids_json TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS export_records (
              id TEXT PRIMARY KEY,
              format TEXT NOT NULL,
              path TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS context_pin (
              singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
              project_id TEXT NOT NULL,
              task_id TEXT,
              expires_at TEXT NOT NULL,
              FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE,
              FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE SET NULL
            );
            CREATE TABLE IF NOT EXISTS sync_tombstones (
              entity_kind TEXT NOT NULL,
              entity_id TEXT NOT NULL,
              deleted_at TEXT NOT NULL,
              device_id TEXT NOT NULL DEFAULT '',
              PRIMARY KEY(entity_kind, entity_id)
            );
            CREATE INDEX IF NOT EXISTS idx_work_sessions_time ON work_sessions(started_at, ended_at);
            CREATE INDEX IF NOT EXISTS idx_work_sessions_project_time ON work_sessions(project_id, started_at);
            CREATE INDEX IF NOT EXISTS idx_work_sessions_task_time ON work_sessions(task_id, started_at);
            CREATE INDEX IF NOT EXISTS idx_work_sessions_review ON work_sessions(user_confirmed, confidence, started_at);
            CREATE INDEX IF NOT EXISTS idx_raw_events_time ON raw_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_raw_events_source_time ON raw_events(source, timestamp);
            CREATE INDEX IF NOT EXISTS idx_jobs_status ON analysis_jobs(status);
            CREATE INDEX IF NOT EXISTS idx_attribution_memories_recent
              ON attribution_memories(confirmed_at DESC);
            CREATE INDEX IF NOT EXISTS idx_attribution_memories_assignment
              ON attribution_memories(project_id, task_id, confirmed_at DESC);
        "#)?;
        ensure_column(
            &conn,
            "activity_categories",
            "updated_at",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column(
            &conn,
            "attribution_rules",
            "updated_at",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column(
            &conn,
            "analysis_jobs",
            "provider",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column(&conn, "analysis_jobs", "model", "TEXT NOT NULL DEFAULT ''")?;
        ensure_column(&conn, "analysis_jobs", "system_prompt", "TEXT")?;
        ensure_column(&conn, "analysis_jobs", "user_prompt", "TEXT")?;
        ensure_column(&conn, "analysis_jobs", "response", "TEXT")?;
        ensure_column(
            &conn,
            "analysis_jobs",
            "queued_at",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column(&conn, "analysis_jobs", "processing_started_at", "TEXT")?;
        ensure_column(&conn, "analysis_jobs", "completed_at", "TEXT")?;
        ensure_column(&conn, "analysis_jobs", "duration_ms", "INTEGER")?;
        ensure_column(
            &conn,
            "analysis_jobs",
            "result_count",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            &conn,
            "analysis_jobs",
            "usage_json",
            "TEXT NOT NULL DEFAULT '{}'",
        )?;
        conn.execute(
            "UPDATE activity_categories SET updated_at=created_at WHERE updated_at=''",
            [],
        )?;
        conn.execute(
            "UPDATE attribution_rules SET updated_at=?1 WHERE updated_at=''",
            params![now()],
        )?;
        conn.execute(
            "UPDATE analysis_jobs SET queued_at=ended_at WHERE queued_at=''",
            [],
        )?;
        conn.execute(
            "UPDATE analysis_jobs
             SET status='pending', processing_started_at=NULL,
                 completed_at=NULL, duration_ms=NULL,
                 error=COALESCE(error, '应用重启后重新排队')
             WHERE status='running'",
            [],
        )?;
        conn.execute(
            "UPDATE analysis_jobs
             SET status='skipped',
                 error=COALESCE(error, '目标时间段已被人工修正，未调用 AI'),
                 completed_at=COALESCE(completed_at,queued_at)
             WHERE status='completed'
               AND result_count=0
               AND processing_started_at IS NULL
               AND system_prompt IS NULL
               AND user_prompt IS NULL
               AND response IS NULL",
            [],
        )?;
        for category in DEFAULT_CATEGORIES {
            conn.execute(
                "INSERT OR IGNORE INTO activity_categories(name,color,is_builtin,created_at,updated_at)
                 SELECT ?1,?2,1,?3,?3
                 WHERE NOT EXISTS (
                   SELECT 1 FROM sync_tombstones WHERE entity_kind='category' AND entity_id=?1
                 )",
                params![category, color_for_category(category), now()],
            )?;
        }
        conn.execute(
            "DELETE FROM plan_items WHERE source='DDL-Manager' OR id LIKE 'ddl-task:%' OR id LIKE 'ddl-day:%'",
            [],
        )?;
        Ok(())
    }

    fn seed_if_empty(&self) -> Result<()> {
        let conn = self.conn.lock();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))?;
        if count > 0 {
            return Ok(());
        }
        let now = now();
        let p1 = Uuid::new_v4().to_string();
        let p2 = Uuid::new_v4().to_string();
        let p3 = Uuid::new_v4().to_string();
        conn.execute("INSERT INTO projects VALUES (?1, 'ScreenUse 开发', '开发', 'seed', '#7dd3fc', '当前智能时间追踪工具项目', ?2, ?2)", params![p1, now])?;
        conn.execute("INSERT INTO projects VALUES (?1, '课程与论文', '学习', 'seed', '#c4b5fd', '课程学习、论文写作与资料阅读', ?2, ?2)", params![p2, now])?;
        conn.execute("INSERT INTO projects VALUES (?1, '日常杂务', '杂务', 'seed', '#facc15', '未归入具体项目的电脑活动', ?2, ?2)", params![p3, now])?;
        let t1 = Uuid::new_v4().to_string();
        let t2 = Uuid::new_v4().to_string();
        let t3 = Uuid::new_v4().to_string();
        conn.execute("INSERT INTO tasks VALUES (?1, ?2, '实现采集与归因闭环', 'active', 'seed', NULL, ?3, ?3)", params![t1, p1, now])?;
        conn.execute(
            "INSERT INTO tasks VALUES (?1, ?2, '资料阅读与写作', 'active', 'seed', NULL, ?3, ?3)",
            params![t2, p2, now],
        )?;
        conn.execute(
            "INSERT INTO tasks VALUES (?1, ?2, '未归类活动整理', 'active', 'seed', NULL, ?3, ?3)",
            params![t3, p3, now],
        )?;

        let s1 = Uuid::new_v4().to_string();
        let s2 = Uuid::new_v4().to_string();
        let s3 = Uuid::new_v4().to_string();
        let base = Utc::now() - Duration::hours(4);
        insert_seed_session(
            &conn,
            SeedSession {
                id: &s1,
                project_id: &p1,
                task_id: &t1,
                category: "开发",
                summary: "搭建 ScreenUse v1 项目骨架",
                start: base,
                end: base + Duration::minutes(75),
                confidence: 0.86,
            },
        )?;
        insert_seed_session(
            &conn,
            SeedSession {
                id: &s2,
                project_id: &p2,
                task_id: &t2,
                category: "学习",
                summary: "阅读竞品与时间追踪资料",
                start: base + Duration::minutes(90),
                end: base + Duration::minutes(145),
                confidence: 0.79,
            },
        )?;
        insert_seed_session(
            &conn,
            SeedSession {
                id: &s3,
                project_id: &p1,
                task_id: &t1,
                category: "开发",
                summary: "设计 AI 队列与失败重试策略",
                start: base + Duration::minutes(165),
                end: base + Duration::minutes(220),
                confidence: 0.82,
            },
        )?;
        Ok(())
    }

    pub fn get_settings(&self) -> Result<AppSettings> {
        let conn = self.conn.lock();
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key='app_settings'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let removed_ddl_manager_setting = raw
            .as_deref()
            .is_some_and(|value| value.contains("ddlManagerDbPath"));
        let mut settings: AppSettings = raw
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        let migration_done = conn
            .query_row(
                "SELECT 1 FROM settings WHERE key=?1 LIMIT 1",
                params![ONE_SECOND_SAMPLING_MIGRATION_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if !migration_done {
            settings.poll_interval_seconds = 1;
            settings.heartbeat_seconds = 5;
        }
        if !migration_done || removed_ddl_manager_setting {
            let timestamp = now();
            conn.execute(
                "INSERT INTO settings(key,value,updated_at) VALUES('app_settings',?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
                params![serde_json::to_string(&settings)?, timestamp],
            )?;
            if !migration_done {
                conn.execute(
                    "INSERT INTO settings(key,value,updated_at) VALUES(?1,'done',?2)",
                    params![ONE_SECOND_SAMPLING_MIGRATION_KEY, timestamp],
                )?;
            }
        }
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO settings(key, value, updated_at) VALUES('app_settings', ?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            params![serde_json::to_string(settings)?, now()],
        )?;
        Ok(())
    }

    pub fn configure_idle_target(&self, settings: &AppSettings) -> Result<String> {
        let settings = settings.clone().normalized();
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let timestamp = now();
        tx.execute(
            "INSERT OR IGNORE INTO activity_categories(name,color,is_builtin,created_at,updated_at) VALUES(?1,'#94a3b8',0,?2,?2)",
            params![settings.idle_category, timestamp],
        )?;
        let color: String = tx.query_row(
            "SELECT color FROM activity_categories WHERE name=?1",
            params![settings.idle_category],
            |row| row.get(0),
        )?;
        let project_id = tx
            .query_row(
                "SELECT id FROM projects WHERE name=?1 AND category=?2 LIMIT 1",
                params![settings.idle_project_name, settings.idle_category],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        tx.execute(
            "INSERT OR IGNORE INTO projects(id,name,category,source,color,description,created_at,updated_at) VALUES(?1,?2,?3,'system-idle',?4,'自动记录离开与空闲时间',?5,?5)",
            params![project_id, settings.idle_project_name, settings.idle_category, color, timestamp],
        )?;
        tx.execute(
            "UPDATE work_sessions
             SET project_id=?1,task_id=NULL,category=?2,source='collector-idle',updated_at=?3
             WHERE user_confirmed=0
               AND (source='collector-idle' OR summary='离开/空闲')
               AND (project_id IS NOT ?1 OR task_id IS NOT NULL OR category<>?2 OR source<>'collector-idle')",
            params![project_id, settings.idle_category, timestamp],
        )?;
        tx.commit()?;
        Ok(project_id)
    }

    fn repair_ai_reviewed_idle_sessions(
        &self,
        settings: &AppSettings,
        idle_project_id: &str,
    ) -> Result<u32> {
        let mut conn = self.conn.lock();
        let already_migrated = conn
            .query_row(
                "SELECT 1 FROM settings WHERE key=?1 LIMIT 1",
                params![AI_IDLE_REVIEW_REPAIR_MIGRATION_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if already_migrated {
            return Ok(0);
        }

        let candidate_prompts = {
            let mut stmt = conn.prepare(
                "SELECT user_prompt FROM analysis_jobs
                 WHERE status='completed' AND user_prompt IS NOT NULL
                   AND instr(user_prompt,'离开/空闲')>0",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            collect_rows(rows)?
        };
        let mut idle_session_ids = HashSet::new();
        for prompt in candidate_prompts {
            idle_session_ids.extend(ai_prompt_idle_session_ids(&prompt, settings));
        }

        let tx = conn.transaction()?;
        let mut changed = 0;
        for session_id in idle_session_ids {
            changed += tx.execute(
                "UPDATE work_sessions
                 SET project_id=?1,task_id=NULL,category=?2,summary='离开/空闲',
                     confidence=MAX(confidence,0.96),source='collector-idle',updated_at=?3
                 WHERE id=?4 AND user_confirmed=0 AND source='ai-review'",
                params![idle_project_id, settings.idle_category, now(), session_id],
            )?;
        }
        tx.execute(
            "INSERT INTO settings(key,value,updated_at) VALUES(?1,'done',?2)",
            params![AI_IDLE_REVIEW_REPAIR_MIGRATION_KEY, now()],
        )?;
        tx.commit()?;
        Ok(changed as u32)
    }

    pub fn dashboard(&self, collector_running: bool) -> Result<DashboardData> {
        Ok(DashboardData {
            settings: self.get_settings()?.normalized(),
            sessions: self.list_sessions(5000)?,
            projects: self.list_projects()?,
            tasks: self.list_tasks()?,
            category_options: self.list_categories()?,
            active_context: self.active_context()?,
            plan_items: self.list_plan_items(100)?,
            trends: self.project_task_trends()?,
            categories: self.category_trends()?,
            queue: self.queue_health()?,
            collector_running,
        })
    }

    pub fn list_categories(&self) -> Result<Vec<CategoryOption>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT name,color,is_builtin FROM activity_categories ORDER BY is_builtin DESC, created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CategoryOption {
                name: row.get(0)?,
                color: row.get(1)?,
                is_builtin: row.get::<_, i64>(2)? != 0,
            })
        })?;
        collect_rows(rows)
    }

    pub fn create_category(&self, name: &str) -> Result<CategoryOption> {
        let name = clean_name(name, "");
        if name.is_empty() {
            bail!("分类名称不能为空");
        }
        let name: String = name.chars().take(24).collect();
        let color = custom_category_color(&name).to_string();
        let conn = self.conn.lock();
        let changed = conn.execute(
            "INSERT OR IGNORE INTO activity_categories(name,color,is_builtin,created_at,updated_at) VALUES(?1,?2,0,?3,?3)",
            params![name, color, now()],
        )?;
        if changed == 0 {
            bail!("同名分类已存在");
        }
        Ok(CategoryOption {
            name,
            color,
            is_builtin: false,
        })
    }

    pub fn rename_category(&self, old_name: &str, new_name: &str) -> Result<CategoryOption> {
        let old_name = clean_name(old_name, "");
        let new_name = clean_name(new_name, "");
        if old_name.is_empty() || new_name.is_empty() {
            bail!("分类名称不能为空");
        }
        let mut settings = self.get_settings()?.normalized();
        let renames_idle_category = settings.idle_category == old_name;
        let new_name: String = new_name.chars().take(24).collect();
        if old_name == new_name {
            return self
                .list_categories()?
                .into_iter()
                .find(|item| item.name == old_name)
                .context("分类不存在或已经删除");
        }

        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let (color, is_builtin, created_at) = tx
            .query_row(
                "SELECT color,is_builtin,created_at FROM activity_categories WHERE name=?1",
                params![old_name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)? != 0,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .context("分类不存在或已经删除")?;
        let duplicate = tx
            .query_row(
                "SELECT 1 FROM activity_categories WHERE name=?1",
                params![new_name],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if duplicate {
            bail!("同名分类已存在");
        }
        let changed_at = now();
        tx.execute(
            "INSERT INTO activity_categories(name,color,is_builtin,created_at,updated_at) VALUES(?1,?2,?3,?4,?5)",
            params![new_name, color, is_builtin as i64, created_at, changed_at],
        )?;
        tx.execute(
            "UPDATE projects SET category=?1,updated_at=?2 WHERE category=?3",
            params![new_name, changed_at, old_name],
        )?;
        tx.execute(
            "UPDATE work_sessions SET category=?1,updated_at=?2 WHERE category=?3",
            params![new_name, changed_at, old_name],
        )?;
        tx.execute(
            "UPDATE attribution_rules SET category=?1,updated_at=?2 WHERE category=?3",
            params![new_name, changed_at, old_name],
        )?;
        if renames_idle_category {
            settings.idle_category = new_name.clone();
            tx.execute(
                "INSERT INTO settings(key,value,updated_at) VALUES('app_settings',?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
                params![serde_json::to_string(&settings)?, changed_at],
            )?;
        }
        record_tombstone(&tx, "category", &old_name)?;
        tx.execute(
            "DELETE FROM activity_categories WHERE name=?1",
            params![old_name],
        )?;
        tx.commit()?;
        Ok(CategoryOption {
            name: new_name,
            color,
            is_builtin,
        })
    }

    pub fn delete_category(&self, name: &str) -> Result<String> {
        let name = clean_name(name, "");
        if self.get_settings()?.normalized().idle_category == name {
            bail!("该分类正在接收离开时间，请先在设置中更换离开归属");
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.query_row(
            "SELECT 1 FROM activity_categories WHERE name=?1",
            params![name],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .context("分类不存在或已经删除")?;
        let (fallback_name, fallback_color) = tx
            .query_row(
                "SELECT name,color FROM activity_categories
                 WHERE name<>?1
                 ORDER BY CASE WHEN name='杂务' THEN 0 ELSE 1 END,is_builtin DESC,created_at ASC
                 LIMIT 1",
                params![name],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .context("至少需要保留一个工作分类")?;
        let changed_at = now();
        tx.execute(
            "UPDATE projects SET category=?1,color=?2,updated_at=?3 WHERE category=?4",
            params![fallback_name, fallback_color, changed_at, name],
        )?;
        tx.execute(
            "UPDATE work_sessions SET category=?1,updated_at=?2 WHERE category=?3",
            params![fallback_name, changed_at, name],
        )?;
        tx.execute(
            "UPDATE attribution_rules SET category=?1,updated_at=?2 WHERE category=?3",
            params![fallback_name, changed_at, name],
        )?;
        record_tombstone(&tx, "category", &name)?;
        tx.execute(
            "DELETE FROM activity_categories WHERE name=?1",
            params![name],
        )?;
        tx.commit()?;
        Ok(fallback_name)
    }

    fn backfill_idle_boundaries(&self) -> Result<u32> {
        let mut conn = self.conn.lock();
        let idle_threshold = conn
            .query_row(
                "SELECT value FROM settings WHERE key='app_settings'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|raw| serde_json::from_str::<AppSettings>(&raw).ok())
            .unwrap_or_default()
            .normalized()
            .idle_threshold_seconds as i64;
        let already_done = conn
            .query_row(
                "SELECT 1 FROM settings WHERE key=?1 LIMIT 1",
                params![IDLE_BOUNDARY_MIGRATION_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if already_done {
            return Ok(0);
        }

        let tx = conn.transaction()?;
        let idle_sessions = {
            let mut stmt = tx.prepare(
                "SELECT id,started_at FROM work_sessions
                 WHERE category='离开' AND user_confirmed=0
                   AND source IN ('context-complete','collector-rule')
                 ORDER BY started_at ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            collect_rows(rows)?
        };

        let mut changed = 0;
        for (idle_id, idle_started_at) in idle_sessions {
            let previous = tx
                .query_row(
                    "SELECT id,started_at,ended_at FROM work_sessions
                     WHERE category<>'离开' AND user_confirmed=0
                       AND source IN ('context-complete','collector-rule')
                       AND started_at < ?1 AND ended_at <= ?1
                     ORDER BY ended_at DESC LIMIT 1",
                    params![idle_started_at],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            let Some((previous_id, previous_started_at, previous_ended_at)) = previous else {
                continue;
            };
            if !within_gap_seconds(&previous_ended_at, &idle_started_at, 3) {
                continue;
            }
            let Ok(idle_started) = DateTime::parse_from_rfc3339(&idle_started_at)
                .map(|value| value.with_timezone(&Utc))
            else {
                continue;
            };
            let Ok(previous_started) = DateTime::parse_from_rfc3339(&previous_started_at)
                .map(|value| value.with_timezone(&Utc))
            else {
                continue;
            };
            let boundary = (idle_started - Duration::seconds(idle_threshold)).max(previous_started);
            let boundary = fmt(boundary);

            if boundary == previous_started_at {
                tx.execute(
                    "DELETE FROM work_sessions WHERE id=?1",
                    params![previous_id],
                )?;
            } else {
                tx.execute(
                    "UPDATE work_sessions SET ended_at=?1,updated_at=?2 WHERE id=?3",
                    params![boundary, now(), previous_id],
                )?;
                tx.execute(
                    "UPDATE activities SET ended_at=?1 WHERE session_id=?2",
                    params![boundary, previous_id],
                )?;
            }
            tx.execute(
                "UPDATE work_sessions SET started_at=?1,updated_at=?2 WHERE id=?3",
                params![boundary, now(), idle_id],
            )?;
            tx.execute(
                "UPDATE activities SET started_at=?1 WHERE session_id=?2",
                params![boundary, idle_id],
            )?;
            changed += 1;
        }

        tx.execute(
            "INSERT INTO settings(key,value,updated_at) VALUES(?1,'done',?2)",
            params![IDLE_BOUNDARY_MIGRATION_KEY, now()],
        )?;
        tx.commit()?;
        Ok(changed)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id,name,category,source,color,description,created_at,updated_at FROM projects ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok(Project {
                id: r.get(0)?,
                name: r.get(1)?,
                category: r.get(2)?,
                source: r.get(3)?,
                color: r.get(4)?,
                description: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
            })
        })?;
        collect_rows(rows)
    }

    pub fn create_project(&self, name: &str, category: &str) -> Result<Project> {
        let name = name.trim().replace(['\r', '\n', '\t'], " ");
        if name.is_empty() {
            bail!("项目名称不能为空");
        }
        let name: String = name.chars().take(80).collect();
        let category = category.trim();
        let conn = self.conn.lock();
        let category_color = conn
            .query_row(
                "SELECT color FROM activity_categories WHERE name=?1 LIMIT 1",
                params![category],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .with_context(|| format!("不支持的项目分类：{category}"))?;
        let duplicate = conn
            .query_row(
                "SELECT 1 FROM projects WHERE name=?1 AND category=?2 LIMIT 1",
                params![name, category],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if duplicate {
            bail!("该分类下已有同名项目，请直接选择它");
        }

        let timestamp = now();
        let project = Project {
            id: Uuid::new_v4().to_string(),
            name,
            category: category.to_string(),
            source: "manual".into(),
            color: category_color,
            description: None,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        conn.execute(
            "INSERT INTO projects VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                project.id,
                project.name,
                project.category,
                project.source,
                project.color,
                project.description,
                project.created_at,
                project.updated_at,
            ],
        )?;
        Ok(project)
    }

    pub fn update_project(&self, id: &str, name: &str, category: &str) -> Result<Project> {
        let name = name.trim().replace(['\r', '\n', '\t'], " ");
        if name.is_empty() {
            bail!("项目名称不能为空");
        }
        let name: String = name.chars().take(80).collect();
        let category = category.trim();
        let mut settings = self.get_settings()?.normalized();
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut project = tx
            .query_row(
                "SELECT id,name,category,source,color,description,created_at,updated_at FROM projects WHERE id=?1",
                params![id],
                |row| {
                    Ok(Project {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        category: row.get(2)?,
                        source: row.get(3)?,
                        color: row.get(4)?,
                        description: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                },
            )
            .optional()?
            .context("项目不存在或已经删除")?;
        let category_color = tx
            .query_row(
                "SELECT color FROM activity_categories WHERE name=?1 LIMIT 1",
                params![category],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .with_context(|| format!("不支持的项目分类：{category}"))?;
        let duplicate = tx
            .query_row(
                "SELECT 1 FROM projects WHERE name=?1 AND category=?2 AND id<>?3 LIMIT 1",
                params![name, category, id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if duplicate {
            bail!("该分类下已有同名项目");
        }

        let updates_idle_target = settings.idle_project_name == project.name
            && settings.idle_category == project.category;
        let changed_at = now();
        tx.execute(
            "UPDATE projects SET name=?1,category=?2,color=?3,updated_at=?4 WHERE id=?5",
            params![name, category, category_color, changed_at, id],
        )?;
        tx.execute(
            "UPDATE work_sessions
             SET category=?1,updated_at=?2
             WHERE project_id=?3
                OR task_id IN (SELECT id FROM tasks WHERE project_id=?3)",
            params![category, changed_at, id],
        )?;
        tx.execute(
            "UPDATE attribution_rules
             SET category=?1,updated_at=?2
             WHERE project_id=?3
                OR task_id IN (SELECT id FROM tasks WHERE project_id=?3)",
            params![category, changed_at, id],
        )?;
        if updates_idle_target {
            settings.idle_project_name = name.clone();
            settings.idle_category = category.to_string();
            tx.execute(
                "INSERT INTO settings(key,value,updated_at) VALUES('app_settings',?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
                params![serde_json::to_string(&settings)?, changed_at],
            )?;
        }
        tx.commit()?;

        project.name = name;
        project.category = category.to_string();
        project.color = category_color;
        project.updated_at = changed_at;
        Ok(project)
    }

    pub fn delete_project(&self, id: &str) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let exists = tx
            .query_row(
                "SELECT 1 FROM projects WHERE id=?1 LIMIT 1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if !exists {
            bail!("项目不存在或已经删除");
        }

        tx.execute(
            "DELETE FROM attribution_rules
             WHERE project_id=?1
                OR task_id IN (SELECT id FROM tasks WHERE project_id=?1)",
            params![id],
        )?;
        let task_ids = {
            let mut stmt = tx.prepare("SELECT id FROM tasks WHERE project_id=?1")?;
            let rows = stmt.query_map(params![id], |row| row.get::<_, String>(0))?;
            collect_rows(rows)?
        };
        for task_id in task_ids {
            record_tombstone(&tx, "task", &task_id)?;
        }
        record_tombstone(&tx, "project", id)?;
        tx.execute("DELETE FROM projects WHERE id=?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id,project_id,title,status,source,planned_due_at,created_at,updated_at FROM tasks ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok(Task {
                id: r.get(0)?,
                project_id: r.get(1)?,
                title: r.get(2)?,
                status: r.get(3)?,
                source: r.get(4)?,
                planned_due_at: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
            })
        })?;
        collect_rows(rows)
    }

    pub fn create_task(&self, project_id: &str, title: &str) -> Result<Task> {
        let title = clean_name(title, "");
        if title.is_empty() {
            bail!("任务名称不能为空");
        }
        let conn = self.conn.lock();
        let project_exists = conn
            .query_row(
                "SELECT 1 FROM projects WHERE id=?1",
                params![project_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if !project_exists {
            bail!("请先选择项目");
        }
        let duplicate = conn
            .query_row(
                "SELECT 1 FROM tasks WHERE project_id=?1 AND title=?2 LIMIT 1",
                params![project_id, title],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if duplicate {
            bail!("该项目下已有同名任务");
        }
        let timestamp = now();
        let task = Task {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            title,
            status: "active".into(),
            source: "manual".into(),
            planned_due_at: None,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        conn.execute(
            "INSERT INTO tasks VALUES(?1,?2,?3,?4,?5,NULL,?6,?7)",
            params![
                task.id,
                task.project_id,
                task.title,
                task.status,
                task.source,
                task.created_at,
                task.updated_at
            ],
        )?;
        Ok(task)
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM attribution_rules WHERE task_id=?1",
            params![id],
        )?;
        let changed = tx.execute("DELETE FROM tasks WHERE id=?1", params![id])?;
        if changed == 0 {
            bail!("任务不存在或已经删除");
        }
        record_tombstone(&tx, "task", id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn pin_context(
        &self,
        project_id: &str,
        task_id: Option<&str>,
        minutes: u32,
    ) -> Result<ContextPin> {
        let conn = self.conn.lock();
        let project: (String, String) = conn.query_row(
            "SELECT name,category FROM projects WHERE id=?1",
            params![project_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if let Some(task_id) = task_id {
            let belongs = conn
                .query_row(
                    "SELECT 1 FROM tasks WHERE id=?1 AND project_id=?2",
                    params![task_id, project_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .is_some();
            if !belongs {
                bail!("所选任务不属于该项目");
            }
        }
        let expires_at = fmt(Utc::now() + Duration::minutes(i64::from(minutes.clamp(5, 240))));
        conn.execute(
            "INSERT INTO context_pin(singleton,project_id,task_id,expires_at) VALUES(1,?1,?2,?3) ON CONFLICT(singleton) DO UPDATE SET project_id=excluded.project_id,task_id=excluded.task_id,expires_at=excluded.expires_at",
            params![project_id, task_id, expires_at],
        )?;
        let task_title = task_id
            .map(|id| {
                conn.query_row("SELECT title FROM tasks WHERE id=?1", params![id], |row| {
                    row.get(0)
                })
            })
            .transpose()?;
        Ok(ContextPin {
            project_id: project_id.to_string(),
            project_name: project.0,
            task_id: task_id.map(ToOwned::to_owned),
            task_title,
            category: project.1,
            expires_at,
        })
    }

    pub fn clear_context_pin(&self) -> Result<()> {
        self.conn.lock().execute("DELETE FROM context_pin", [])?;
        Ok(())
    }

    pub fn active_context(&self) -> Result<Option<ContextPin>> {
        let conn = self.conn.lock();
        let pin = conn
            .query_row(
                "SELECT cp.project_id,p.name,cp.task_id,t.title,p.category,cp.expires_at FROM context_pin cp JOIN projects p ON p.id=cp.project_id LEFT JOIN tasks t ON t.id=cp.task_id WHERE cp.singleton=1",
                [],
                |row| Ok(ContextPin {
                    project_id: row.get(0)?,
                    project_name: row.get(1)?,
                    task_id: row.get(2)?,
                    task_title: row.get(3)?,
                    category: row.get(4)?,
                    expires_at: row.get(5)?,
                }),
            )
            .optional()?;
        if pin.as_ref().is_some_and(|pin| {
            DateTime::parse_from_rfc3339(&pin.expires_at)
                .map(|time| time.with_timezone(&Utc) <= Utc::now())
                .unwrap_or(true)
        }) {
            conn.execute("DELETE FROM context_pin", [])?;
            return Ok(None);
        }
        Ok(pin)
    }

    pub fn list_sessions(&self, limit: i64) -> Result<Vec<WorkSession>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(r#"
            SELECT ws.id, ws.started_at, ws.ended_at, ws.project_id, p.name, ws.task_id, t.title,
                   ws.category, ws.summary, ws.confidence, ws.evidence_json, ws.user_confirmed, ws.source
            FROM work_sessions ws
            LEFT JOIN projects p ON p.id = ws.project_id
            LEFT JOIN tasks t ON t.id = ws.task_id
            ORDER BY ws.started_at DESC
            LIMIT ?1
        "#)?;
        let rows = stmt.query_map(params![limit], map_work_session)?;
        collect_rows(rows)
    }

    pub fn list_sessions_in_range(
        &self,
        started_at: &str,
        ended_at: &str,
        limit: i64,
    ) -> Result<Vec<WorkSession>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(r#"
            SELECT ws.id, ws.started_at, ws.ended_at, ws.project_id, p.name, ws.task_id, t.title,
                   ws.category, ws.summary, ws.confidence, ws.evidence_json, ws.user_confirmed, ws.source
            FROM work_sessions ws
            LEFT JOIN projects p ON p.id = ws.project_id
            LEFT JOIN tasks t ON t.id = ws.task_id
            WHERE ws.ended_at >= ?1 AND ws.started_at <= ?2
            ORDER BY ws.started_at ASC
            LIMIT ?3
        "#)?;
        let rows = stmt.query_map(params![started_at, ended_at, limit], map_work_session)?;
        collect_rows(rows)
    }

    pub fn create_manual_session(
        &self,
        started_at: &str,
        ended_at: &str,
        patch: SessionPatch,
    ) -> Result<WorkSession> {
        let started_at = DateTime::parse_from_rfc3339(started_at)
            .context("开始时间格式无效")?
            .with_timezone(&Utc);
        let ended_at = DateTime::parse_from_rfc3339(ended_at)
            .context("结束时间格式无效")?
            .with_timezone(&Utc);
        if ended_at - started_at < Duration::seconds(5) {
            bail!("手动补录的时间段至少需要 5 秒");
        }

        let started_at = fmt(started_at);
        let ended_at = fmt(ended_at);
        let task_id = patch
            .task_id
            .as_deref()
            .map(|value| clean_name(value, ""))
            .filter(|value| !value.is_empty())
            .context("请选择具体任务后再添加时间段")?;
        let summary = patch
            .summary
            .as_deref()
            .map(|value| clean_name(value, ""))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "手动补录".into())
            .chars()
            .take(160)
            .collect::<String>();
        let confidence = patch.confidence.unwrap_or(1.0);
        if !confidence.is_finite() {
            bail!("置信度必须是有效数字");
        }

        let id = Uuid::new_v4().to_string();
        let evidence = serde_json::to_string(&vec![EvidenceItem {
            kind: "manual".into(),
            label: "来源".into(),
            value: "手动补录未记录时间".into(),
            weight: 1.0,
        }])?;
        let conn = self.conn.lock();
        let (project_id, category) = conn
            .query_row(
                "SELECT t.project_id,p.category FROM tasks t JOIN projects p ON p.id=t.project_id WHERE t.id=?1",
                params![task_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .context("任务不存在或已经删除")?;
        let overlaps = conn
            .query_row(
                "SELECT 1 FROM work_sessions WHERE julianday(started_at)<julianday(?1) AND julianday(ended_at)>julianday(?2) LIMIT 1",
                params![ended_at, started_at],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if overlaps {
            bail!("该空档已经出现其他时间段，请刷新后重试");
        }
        conn.execute(
            "INSERT INTO work_sessions(
                id,started_at,ended_at,project_id,task_id,category,summary,confidence,
                evidence_json,user_confirmed,source,updated_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,1,'manual-entry',?10)",
            params![
                id,
                started_at,
                ended_at,
                project_id,
                task_id,
                category,
                summary,
                confidence.clamp(0.0, 1.0),
                evidence,
                now(),
            ],
        )?;
        drop(conn);
        self.get_session(&id)?.context("手动时间段创建后未找到")
    }

    pub fn update_session(&self, id: &str, patch: SessionPatch) -> Result<WorkSession> {
        let current = self.get_session(id)?.context("session not found")?;
        let project_was_explicit = patch.project_id.is_some();
        let task_was_explicit = patch.task_id.is_some();
        let category_was_explicit = patch.category.is_some();
        let classification_edited = patch.summary.is_some()
            || project_was_explicit
            || task_was_explicit
            || patch.clear_project.unwrap_or(false)
            || patch.clear_task.unwrap_or(false)
            || category_was_explicit;
        let clear_project = patch.clear_project.unwrap_or(false);
        let clear_task = patch.clear_task.unwrap_or(false) || clear_project;
        let mut project_id = if clear_project {
            None
        } else {
            patch.project_id.clone().or(current.project_id.clone())
        };
        let project_changed = project_id != current.project_id;
        let mut task_id = if clear_task {
            None
        } else if task_was_explicit {
            patch.task_id.clone()
        } else if project_changed {
            None
        } else {
            current.task_id.clone()
        };
        let summary = match patch.summary {
            Some(value) => {
                let value = clean_name(&value, "");
                if value.is_empty() {
                    bail!("会话摘要不能为空");
                }
                value.chars().take(160).collect()
            }
            None => current.summary.clone(),
        };
        let mut category = patch
            .category
            .as_deref()
            .map(|value| clean_name(value, ""))
            .unwrap_or_else(|| current.category.clone());
        if category.is_empty() {
            bail!("分类不能为空");
        }
        let confidence = patch.confidence.unwrap_or(current.confidence);
        if !confidence.is_finite() {
            bail!("置信度必须是有效数字");
        }
        let confidence = confidence.clamp(0.0, 1.0);
        let confirmed = patch.user_confirmed.unwrap_or(true);
        let conn = self.conn.lock();

        // A direct category change means the old project/task no longer applies
        // unless the caller explicitly selected a replacement in the same patch.
        if category_was_explicit
            && !project_was_explicit
            && !task_was_explicit
            && category != current.category
        {
            project_id = None;
            task_id = None;
        }

        // Keep category -> project -> task as one canonical hierarchy. Selecting
        // a task is the strongest signal, followed by an explicitly selected project.
        if let Some(selected_task_id) = task_id.as_deref() {
            let assignment = conn
                .query_row(
                    "SELECT t.project_id,p.category FROM tasks t JOIN projects p ON p.id=t.project_id WHERE t.id=?1",
                    params![selected_task_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
                .context("任务不存在或已经删除")?;
            project_id = Some(assignment.0);
            category = assignment.1;
        } else if let Some(selected_project_id) = project_id.as_deref() {
            let project_category = conn
                .query_row(
                    "SELECT category FROM projects WHERE id=?1",
                    params![selected_project_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .context("项目不存在或已经删除")?;
            if category_was_explicit && !project_was_explicit && category != project_category {
                project_id = None;
            } else {
                category = project_category;
            }
        }

        let category_exists = conn
            .query_row(
                "SELECT 1 FROM activity_categories WHERE name=?1 LIMIT 1",
                params![category],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if !category_exists {
            bail!("分类不存在或已经删除：{category}");
        }
        conn.execute(
            "UPDATE work_sessions SET project_id=?1, task_id=?2, summary=?3, category=?4, confidence=?5, user_confirmed=?6, source=CASE WHEN ?7 THEN 'manual-correction' ELSE source END, updated_at=?8 WHERE id=?9",
            params![project_id, task_id, summary, category, confidence, if confirmed {1} else {0}, classification_edited, now(), id],
        )?;
        drop(conn);
        self.get_session(id)?
            .context("session disappeared after update")
    }

    pub fn update_sessions(&self, ids: &[String], patch: SessionPatch) -> Result<Vec<WorkSession>> {
        if ids.is_empty() {
            bail!("请至少选择一条会话");
        }
        if ids.len() > 500 {
            bail!("单次最多修正 500 条会话");
        }
        let mut unique_ids = Vec::with_capacity(ids.len());
        for id in ids {
            if !unique_ids.contains(id) {
                unique_ids.push(id.clone());
            }
        }
        for id in &unique_ids {
            if self.get_session(id)?.is_none() {
                bail!("会话不存在或已经删除：{id}");
            }
        }
        let mut updated = Vec::with_capacity(unique_ids.len());
        for id in &unique_ids {
            updated.push(self.update_session(id, patch.clone())?);
        }
        Ok(updated)
    }

    pub fn apply_session_correction(
        &self,
        ids: &[String],
        patch: SessionPatch,
        remember: bool,
        keyword: Option<&str>,
        pin_minutes: Option<u32>,
    ) -> Result<Vec<WorkSession>> {
        if ids.is_empty() {
            bail!("请至少选择一条会话");
        }
        let label = if ids.len() > 1 {
            format!("统一修正 {} 个时间段", ids.len())
        } else {
            let summary = self
                .get_session(&ids[0])?
                .map(|session| session.summary)
                .unwrap_or_else(|| "时间段".into());
            format!("修正“{}”", summary.chars().take(32).collect::<String>())
        };
        let snapshot = self.capture_session_correction_undo(ids, label)?;
        let result = (|| {
            let updated = self.update_sessions(ids, patch)?;
            self.record_correction_memories(&updated, true)?;
            if remember {
                for session in &updated {
                    self.learn_rule_from_session(&session.id, keyword)?;
                }
            }
            if let Some(minutes) = pin_minutes {
                let session = updated.first().context("修正后没有可固定的时间段")?;
                let project_id = session
                    .project_id
                    .as_deref()
                    .context("只有已归属项目的时间段才能固定当前事务")?;
                self.pin_context(project_id, session.task_id.as_deref(), minutes)?;
            }
            Ok(updated)
        })();

        match result {
            Ok(updated) => {
                if let Err(error) = self.write_session_correction_undo(&snapshot) {
                    self.restore_session_correction_undo(&snapshot)
                        .with_context(|| format!("无法保存撤销记录，且回滚修正失败：{error}"))?;
                    return Err(error.context("无法保存撤销记录，本次修正已回滚"));
                }
                Ok(updated)
            }
            Err(error) => {
                self.restore_session_correction_undo(&snapshot)
                    .with_context(|| format!("修正失败，且自动回滚失败：{error}"))?;
                Err(error)
            }
        }
    }

    pub fn undo_status(&self) -> UndoStatus {
        match self.read_session_correction_undo() {
            Ok(snapshot) => UndoStatus {
                available: true,
                label: Some(snapshot.label),
                created_at: Some(snapshot.created_at),
            },
            Err(_) => UndoStatus {
                available: false,
                label: None,
                created_at: None,
            },
        }
    }

    pub fn undo_last_session_correction(&self) -> Result<String> {
        let snapshot = self
            .read_session_correction_undo()
            .context("没有可以撤销的修正")?;
        self.restore_session_correction_undo(&snapshot)?;
        let path = self.data_dir.join(LAST_CORRECTION_UNDO_FILE);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(snapshot.label)
    }

    fn capture_session_correction_undo(
        &self,
        ids: &[String],
        label: String,
    ) -> Result<SessionCorrectionUndo> {
        let conn = self.conn.lock();
        let mut sessions = Vec::new();
        for id in ids {
            let row = conn
                .query_row(
                    "SELECT id,started_at,ended_at,project_id,task_id,category,summary,confidence,
                            evidence_json,user_confirmed,source,updated_at
                     FROM work_sessions WHERE id=?1",
                    params![id],
                    |row| {
                        Ok(UndoSessionRow {
                            id: row.get(0)?,
                            started_at: row.get(1)?,
                            ended_at: row.get(2)?,
                            project_id: row.get(3)?,
                            task_id: row.get(4)?,
                            category: row.get(5)?,
                            summary: row.get(6)?,
                            confidence: row.get(7)?,
                            evidence_json: row.get(8)?,
                            user_confirmed: row.get::<_, i64>(9)? != 0,
                            source: row.get(10)?,
                            updated_at: row.get(11)?,
                        })
                    },
                )
                .optional()?
                .with_context(|| format!("会话不存在或已经删除：{id}"))?;
            if !sessions
                .iter()
                .any(|item: &UndoSessionRow| item.id == row.id)
            {
                sessions.push(row);
            }
        }
        let correction_rules = {
            let mut stmt = conn.prepare(
                "SELECT id,name,priority,matcher_json,project_id,task_id,category,
                        created_from_correction,enabled,updated_at
                 FROM attribution_rules WHERE created_from_correction=1 ORDER BY id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(UndoRuleRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    priority: row.get(2)?,
                    matcher_json: row.get(3)?,
                    project_id: row.get(4)?,
                    task_id: row.get(5)?,
                    category: row.get(6)?,
                    created_from_correction: row.get::<_, i64>(7)? != 0,
                    enabled: row.get::<_, i64>(8)? != 0,
                    updated_at: row.get(9)?,
                })
            })?;
            collect_rows(rows)?
        };
        let mut memories = Vec::new();
        for id in ids {
            if let Some(memory) = conn
                .query_row(
                    "SELECT session_id,features_json,category,project_id,task_id,
                            confirmed_at,last_used_at,use_count
                     FROM attribution_memories WHERE session_id=?1",
                    params![id],
                    |row| {
                        Ok(UndoMemoryRow {
                            session_id: row.get(0)?,
                            features_json: row.get(1)?,
                            category: row.get(2)?,
                            project_id: row.get(3)?,
                            task_id: row.get(4)?,
                            confirmed_at: row.get(5)?,
                            last_used_at: row.get(6)?,
                            use_count: row.get(7)?,
                        })
                    },
                )
                .optional()?
            {
                memories.push(memory);
            }
        }
        let context_pin = conn
            .query_row(
                "SELECT project_id,task_id,expires_at FROM context_pin WHERE singleton=1",
                [],
                |row| {
                    Ok(UndoContextPinRow {
                        project_id: row.get(0)?,
                        task_id: row.get(1)?,
                        expires_at: row.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(SessionCorrectionUndo {
            label,
            created_at: now(),
            sessions,
            correction_rules,
            memories,
            context_pin,
        })
    }

    fn write_session_correction_undo(&self, snapshot: &SessionCorrectionUndo) -> Result<()> {
        let path = self.data_dir.join(LAST_CORRECTION_UNDO_FILE);
        let temp = self
            .data_dir
            .join(format!("{LAST_CORRECTION_UNDO_FILE}.tmp"));
        let previous = self
            .data_dir
            .join(format!("{LAST_CORRECTION_UNDO_FILE}.previous"));
        fs::write(&temp, serde_json::to_vec(snapshot)?)?;
        if previous.exists() {
            fs::remove_file(&previous)?;
        }
        if path.exists() {
            fs::rename(&path, &previous)?;
        }
        if let Err(error) = fs::rename(&temp, &path) {
            if previous.exists() {
                let _ = fs::rename(&previous, &path);
            }
            return Err(error.into());
        }
        if previous.exists() {
            let _ = fs::remove_file(previous);
        }
        Ok(())
    }

    fn read_session_correction_undo(&self) -> Result<SessionCorrectionUndo> {
        let path = self.data_dir.join(LAST_CORRECTION_UNDO_FILE);
        let bytes = fs::read(path)?;
        serde_json::from_slice(&bytes).context("撤销记录已损坏")
    }

    fn restore_session_correction_undo(&self, snapshot: &SessionCorrectionUndo) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for row in &snapshot.sessions {
            tx.execute(
                "INSERT INTO work_sessions(
                    id,started_at,ended_at,project_id,task_id,category,summary,confidence,
                    evidence_json,user_confirmed,source,updated_at
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
                 ON CONFLICT(id) DO UPDATE SET
                    started_at=excluded.started_at,ended_at=excluded.ended_at,
                    project_id=excluded.project_id,task_id=excluded.task_id,
                    category=excluded.category,summary=excluded.summary,
                    confidence=excluded.confidence,evidence_json=excluded.evidence_json,
                    user_confirmed=excluded.user_confirmed,source=excluded.source,
                    updated_at=excluded.updated_at",
                params![
                    row.id,
                    row.started_at,
                    row.ended_at,
                    row.project_id,
                    row.task_id,
                    row.category,
                    row.summary,
                    row.confidence,
                    row.evidence_json,
                    row.user_confirmed as i64,
                    row.source,
                    row.updated_at,
                ],
            )?;
        }
        tx.execute(
            "DELETE FROM attribution_rules WHERE created_from_correction=1",
            [],
        )?;
        for row in &snapshot.correction_rules {
            tx.execute(
                "INSERT INTO attribution_rules(
                    id,name,priority,matcher_json,project_id,task_id,category,
                    created_from_correction,enabled,updated_at
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    row.id,
                    row.name,
                    row.priority,
                    row.matcher_json,
                    row.project_id,
                    row.task_id,
                    row.category,
                    row.created_from_correction as i64,
                    row.enabled as i64,
                    row.updated_at,
                ],
            )?;
        }
        for row in &snapshot.sessions {
            tx.execute(
                "DELETE FROM attribution_memories WHERE session_id=?1",
                params![row.id],
            )?;
        }
        for row in &snapshot.memories {
            tx.execute(
                "INSERT INTO attribution_memories(
                    session_id,features_json,category,project_id,task_id,
                    confirmed_at,last_used_at,use_count
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    row.session_id,
                    row.features_json,
                    row.category,
                    row.project_id,
                    row.task_id,
                    row.confirmed_at,
                    row.last_used_at,
                    row.use_count,
                ],
            )?;
        }
        tx.execute("DELETE FROM context_pin", [])?;
        if let Some(pin) = &snapshot.context_pin {
            tx.execute(
                "INSERT INTO context_pin(singleton,project_id,task_id,expires_at)
                 VALUES(1,?1,?2,?3)",
                params![pin.project_id, pin.task_id, pin.expires_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn apply_ai_review(
        &self,
        id: &str,
        patch: SessionPatch,
        evidence: Vec<EvidenceItem>,
    ) -> Result<WorkSession> {
        let current = self.get_session(id)?.context("session not found")?;
        let settings = self.get_settings()?.normalized();
        if is_idle_session(&current, &settings) {
            return Ok(current);
        }
        let updated = self.update_session(id, patch)?;
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE work_sessions
             SET evidence_json=?1,source='ai-review',user_confirmed=0,updated_at=?2
             WHERE id=?3",
            params![serde_json::to_string(&evidence)?, now(), updated.id],
        )?;
        drop(conn);
        self.match_plan_items_for_session(&updated.id)?;
        let reviewed = self
            .get_session(&updated.id)?
            .context("AI-reviewed session disappeared")?;
        self.record_personal_memory(&reviewed)?;
        Ok(reviewed)
    }

    #[cfg(test)]
    fn propagate_context_from_sessions(&self, anchor_ids: &[String]) -> Result<u32> {
        let mut changed_ids = HashSet::new();
        for anchor_id in anchor_ids {
            let Some(anchor) = self.get_session(anchor_id)? else {
                continue;
            };
            let (Some(project_id), Some(task_id)) =
                (anchor.project_id.as_deref(), anchor.task_id.as_deref())
            else {
                continue;
            };
            if !anchor.user_confirmed {
                continue;
            }
            let anchor_start = DateTime::parse_from_rfc3339(&anchor.started_at)
                .context("invalid anchor start")?
                .with_timezone(&Utc);
            let anchor_end = DateTime::parse_from_rfc3339(&anchor.ended_at)
                .context("invalid anchor end")?
                .with_timezone(&Utc);
            let range_start = fmt(anchor_start - Duration::hours(4));
            let range_end = fmt(anchor_end + Duration::hours(4));
            let rows = {
                let conn = self.conn.lock();
                let mut stmt = conn.prepare(
                    "SELECT id,started_at,ended_at,project_id,task_id,summary,confidence,user_confirmed,source
                     FROM work_sessions
                     WHERE started_at<=?2 AND ended_at>=?1
                     ORDER BY started_at ASC,ended_at ASC",
                )?;
                let mapped = stmt.query_map(params![range_start, range_end], |row| {
                    Ok(ContextPropagationRow {
                        id: row.get(0)?,
                        started_at: row.get(1)?,
                        ended_at: row.get(2)?,
                        project_id: row.get(3)?,
                        task_id: row.get(4)?,
                        summary: row.get(5)?,
                        confidence: row.get(6)?,
                        user_confirmed: row.get::<_, i64>(7)? != 0,
                        source: row.get(8)?,
                    })
                })?;
                collect_rows(mapped)?
            };
            let Some(anchor_index) = rows.iter().position(|row| row.id == anchor.id) else {
                continue;
            };

            let same_assignment = |row: &ContextPropagationRow| {
                row.project_id.as_deref() == Some(project_id)
                    && row.task_id.as_deref() == Some(task_id)
            };
            let should_stop = |row: &ContextPropagationRow| {
                row.source == "collector-idle"
                    || (row.user_confirmed && !same_assignment(row))
                    || (!row.user_confirmed
                        && row.task_id.is_some()
                        && !same_assignment(row)
                        && row.confidence >= 0.90)
                    || (!row.user_confirmed
                        && !is_auto_session_source(&row.source)
                        && !same_assignment(row))
            };
            let mut update_row = |row: &ContextPropagationRow| -> Result<()> {
                let stale_idle_summary =
                    row.summary == "离开/空闲" && anchor.summary != "离开/空闲";
                if (same_assignment(row) && !stale_idle_summary)
                    || row.user_confirmed
                    || changed_ids.contains(&row.id)
                {
                    return Ok(());
                }
                self.conn.lock().execute(
                    "UPDATE work_sessions
                     SET project_id=?1,task_id=?2,category=?3,
                         summary=CASE WHEN summary='离开/空闲' THEN ?4 ELSE summary END,
                         confidence=MAX(confidence,0.90),updated_at=?5
                     WHERE id=?6 AND user_confirmed=0",
                    params![
                        project_id,
                        task_id,
                        anchor.category,
                        anchor.summary,
                        now(),
                        row.id
                    ],
                )?;
                changed_ids.insert(row.id.clone());
                Ok(())
            };

            let mut cursor_start = anchor.started_at.clone();
            for row in rows[..anchor_index].iter().rev() {
                if context_is_disconnected(&row.ended_at, &cursor_start, 30) {
                    break;
                }
                if should_stop(row) {
                    break;
                }
                update_row(row)?;
                cursor_start = row.started_at.clone();
            }

            let mut cursor_end = anchor.ended_at.clone();
            for row in rows.iter().skip(anchor_index + 1) {
                if context_is_disconnected(&cursor_end, &row.started_at, 30) {
                    break;
                }
                if should_stop(row) {
                    break;
                }
                update_row(row)?;
                cursor_end = row.ended_at.clone();
            }
        }
        Ok(changed_ids.len() as u32)
    }

    pub(crate) fn recent_task_context(
        &self,
        current_session_id: &str,
        started_at: &str,
    ) -> Result<Option<RecentTaskContext>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT project_id,task_id,category,confidence,user_confirmed,source,ended_at
             FROM work_sessions
             WHERE id<>?1 AND started_at<=?2
             ORDER BY ended_at DESC,updated_at DESC
             LIMIT 1",
            params![current_session_id, started_at],
            |row| {
                Ok(RecentTaskContext {
                    project_id: row.get(0)?,
                    task_id: row.get(1)?,
                    category: row.get(2)?,
                    confidence: row.get(3)?,
                    user_confirmed: row.get::<_, i64>(4)? != 0,
                    source: row.get(5)?,
                    ended_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_session(&self, id: &str) -> Result<Option<WorkSession>> {
        let conn = self.conn.lock();
        conn.query_row(
            r#"
                SELECT ws.id, ws.started_at, ws.ended_at, ws.project_id, p.name, ws.task_id, t.title,
                       ws.category, ws.summary, ws.confidence, ws.evidence_json, ws.user_confirmed, ws.source
                FROM work_sessions ws
                LEFT JOIN projects p ON p.id = ws.project_id
                LEFT JOIN tasks t ON t.id = ws.task_id
                WHERE ws.id = ?1
            "#,
            params![id],
            map_work_session,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn mark_session_awaiting_confirmation(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE work_sessions
             SET source=CASE
                 WHEN source='collector-idle' OR summary='离开/空闲' THEN 'collector-idle'
                 ELSE 'context-complete'
             END, updated_at=?1
             WHERE id=?2 AND user_confirmed=0",
            params![now(), id],
        )?;
        Ok(())
    }

    pub fn coalesce_session_neighbors(&self, id: &str) -> Result<WorkSession> {
        let current = self.get_session(id)?.context("session not found")?;
        let previous_id = {
            let conn = self.conn.lock();
            conn.query_row(
                "SELECT id FROM work_sessions WHERE started_at < ?1 ORDER BY started_at DESC LIMIT 1",
                params![current.started_at],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        };
        let Some(previous_id) = previous_id else {
            return Ok(current);
        };
        let previous = self
            .get_session(&previous_id)?
            .context("previous session not found")?;
        if can_auto_coalesce(&previous, &current) {
            return self.merge_sessions_into(&previous, &[current]);
        }

        let Some((anchor, mut middle)) = self.find_short_detour_anchor(&current)? else {
            return Ok(current);
        };
        middle.push(current);
        self.merge_sessions_into(&anchor, &middle)
    }

    pub fn absorb_short_auto_session(&self, id: &str) -> Result<WorkSession> {
        let current = self.get_session(id)?.context("session not found")?;
        if current.user_confirmed
            || session_duration_seconds(&current).map_or(true, |seconds| seconds >= 5)
            || !is_auto_session_source(&current.source)
        {
            return Ok(current);
        }
        let previous_id = {
            let conn = self.conn.lock();
            conn.query_row("SELECT id FROM work_sessions WHERE started_at < ?1 ORDER BY started_at DESC LIMIT 1", params![current.started_at], |row| row.get::<_, String>(0)).optional()?
        };
        let Some(previous_id) = previous_id else {
            return Ok(current);
        };
        let previous = self
            .get_session(&previous_id)?
            .context("previous session not found")?;
        if previous.user_confirmed
            || !is_auto_session_source(&previous.source)
            || !within_gap_seconds(&previous.ended_at, &current.started_at, 5)
        {
            return Ok(current);
        }
        self.merge_sessions_into(&previous, &[current])
    }

    fn find_short_detour_anchor(
        &self,
        current: &WorkSession,
    ) -> Result<Option<(WorkSession, Vec<WorkSession>)>> {
        let candidate_ids = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id FROM work_sessions
                 WHERE started_at < ?1
                 ORDER BY started_at DESC
                 LIMIT 8",
            )?;
            let rows =
                stmt.query_map(params![current.started_at], |row| row.get::<_, String>(0))?;
            collect_rows(rows)?
        };

        for candidate_id in candidate_ids {
            let Some(anchor) = self.get_session(&candidate_id)? else {
                continue;
            };
            if !can_bridge_short_detour(&anchor, current) {
                continue;
            }
            let middle_ids = {
                let conn = self.conn.lock();
                let mut stmt = conn.prepare(
                    "SELECT id FROM work_sessions
                     WHERE started_at >= ?1 AND ended_at <= ?2 AND id <> ?3
                     ORDER BY started_at ASC",
                )?;
                let rows = stmt.query_map(
                    params![anchor.ended_at, current.started_at, anchor.id],
                    |row| row.get::<_, String>(0),
                )?;
                collect_rows(rows)?
            };
            if middle_ids.is_empty() || middle_ids.len() > 6 {
                continue;
            }
            let mut middle = Vec::with_capacity(middle_ids.len());
            for middle_id in middle_ids {
                if let Some(session) = self.get_session(&middle_id)? {
                    middle.push(session);
                }
            }
            if short_detour_is_compatible(&anchor, &middle, current) {
                return Ok(Some((anchor, middle)));
            }
        }
        Ok(None)
    }

    fn merge_sessions_into(
        &self,
        anchor: &WorkSession,
        following: &[WorkSession],
    ) -> Result<WorkSession> {
        if following.is_empty() {
            return Ok(anchor.clone());
        }
        let mut summary = anchor.summary.clone();
        let mut evidence = anchor.evidence.clone();
        let mut confidence = anchor.confidence;
        let mut merged_end = anchor.ended_at.clone();
        for session in following {
            summary = preferred_coalesced_summary(&summary, &session.summary);
            evidence = merge_evidence(&evidence, &session.evidence);
            confidence = confidence.max(session.confidence);
            if timestamp_is_after(&session.ended_at, &merged_end) {
                merged_end = session.ended_at.clone();
            }
        }
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE work_sessions SET ended_at=?1,summary=?2,confidence=?3,evidence_json=?4,updated_at=?5 WHERE id=?6",
            params![merged_end, summary, confidence, serde_json::to_string(&evidence)?, now(), anchor.id],
        )?;
        conn.execute(
            "UPDATE activities SET ended_at=?1,summary=?2,evidence_json=?3 WHERE session_id=?4",
            params![
                merged_end,
                summary,
                serde_json::to_string(&evidence)?,
                anchor.id
            ],
        )?;

        for absorbed in following {
            let plan_updates = {
                let mut stmt = conn.prepare(
                    "SELECT id,matched_session_ids_json FROM plan_items WHERE matched_session_ids_json LIKE ?1",
                )?;
                let rows = stmt.query_map(params![format!("%{}%", absorbed.id)], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                let mut updates = Vec::new();
                for row in rows {
                    let (plan_id, matched_json) = row?;
                    let mut matched: Vec<String> = parse_json(&matched_json);
                    for session_id in &mut matched {
                        if session_id == &absorbed.id {
                            *session_id = anchor.id.clone();
                        }
                    }
                    matched.sort();
                    matched.dedup();
                    updates.push((plan_id, serde_json::to_string(&matched)?));
                }
                updates
            };
            for (plan_id, matched_json) in plan_updates {
                conn.execute(
                    "UPDATE plan_items SET matched_session_ids_json=?1,updated_at=?2 WHERE id=?3",
                    params![matched_json, now(), plan_id],
                )?;
            }
            record_tombstone(&conn, "session", &absorbed.id)?;
            conn.execute(
                "DELETE FROM work_sessions WHERE id=?1",
                params![absorbed.id],
            )?;
        }
        drop(conn);
        self.get_session(&anchor.id)?
            .context("coalesced session missing")
    }

    pub fn merge_sessions(&self, ids: &[String], summary: Option<String>) -> Result<WorkSession> {
        let mut unique_ids = Vec::with_capacity(ids.len());
        for id in ids {
            if !unique_ids.contains(id) {
                unique_ids.push(id.clone());
            }
        }
        anyhow::ensure!(unique_ids.len() >= 2, "请至少选择两条不同的会话");
        anyhow::ensure!(unique_ids.len() <= 500, "单次最多合并 500 条会话");

        let mut sessions = Vec::with_capacity(unique_ids.len());
        for id in &unique_ids {
            sessions.push(
                self.get_session(id)?
                    .with_context(|| format!("会话不存在或已经删除：{id}"))?,
            );
        }
        sessions.sort_by(|left, right| left.started_at.cmp(&right.started_at));
        let anchor = sessions.first().context("no sessions to merge")?;
        for session in &sessions[1..] {
            anyhow::ensure!(
                session.project_id == anchor.project_id
                    && session.task_id == anchor.task_id
                    && session.category == anchor.category,
                "只能合并分类、项目和任务一致的会话；请先批量修正归类"
            );
        }
        for pair in sessions.windows(2) {
            anyhow::ensure!(
                touch_or_overlap_within(&pair[0].ended_at, &pair[1].started_at, 5),
                "只能合并连续会话，相邻记录之间最多允许 5 秒采样间隔"
            );
        }

        let started_at = anchor.started_at.clone();
        let mut ended_at = anchor.ended_at.clone();
        let mut evidence = Vec::<EvidenceItem>::new();
        let mut confidence = 0.0_f32;
        let mut summaries = Vec::<String>::new();
        for session in &sessions {
            if timestamp_is_after(&session.ended_at, &ended_at) {
                ended_at = session.ended_at.clone();
            }
            evidence = merge_evidence(&evidence, &session.evidence);
            confidence = confidence.max(session.confidence);
            if !summaries.contains(&session.summary) {
                summaries.push(session.summary.clone());
            }
        }
        let merged_summary = clean_name(
            summary.as_deref().unwrap_or(&summaries.join(" / ")),
            "连续工作会话",
        );

        let mut conn = self.conn.lock();
        let overlapping_ids = {
            let mut stmt = conn.prepare(
                "SELECT id FROM work_sessions
                 WHERE julianday(ended_at)>julianday(started_at)
                   AND julianday(started_at)<julianday(?1)
                   AND julianday(ended_at)>julianday(?2)",
            )?;
            let rows =
                stmt.query_map(params![ended_at, started_at], |row| row.get::<_, String>(0))?;
            collect_rows(rows)?
        };
        anyhow::ensure!(
            overlapping_ids.iter().all(|id| unique_ids.contains(id)),
            "所选会话之间还有未选择的时间段，请一并选择或先修正时间轴"
        );

        let tx = conn.transaction()?;
        let new_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,'manual-merge',?10)",
            params![
                new_id,
                started_at,
                ended_at,
                anchor.project_id,
                anchor.task_id,
                anchor.category,
                merged_summary,
                confidence,
                serde_json::to_string(&evidence)?,
                now()
            ],
        )?;

        let plan_updates = {
            let mut stmt = tx.prepare("SELECT id,matched_session_ids_json FROM plan_items")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut updates = Vec::new();
            for row in rows {
                let (plan_id, matched_json) = row?;
                let mut matched: Vec<String> = parse_json(&matched_json);
                if matched.iter().any(|id| unique_ids.contains(id)) {
                    matched.retain(|id| !unique_ids.contains(id));
                    matched.push(new_id.clone());
                    matched.sort();
                    matched.dedup();
                    updates.push((plan_id, serde_json::to_string(&matched)?));
                }
            }
            updates
        };
        for (plan_id, matched_json) in plan_updates {
            tx.execute(
                "UPDATE plan_items SET matched_session_ids_json=?1,updated_at=?2 WHERE id=?3",
                params![matched_json, now(), plan_id],
            )?;
        }
        for id in &unique_ids {
            record_tombstone(&tx, "session", id)?;
            tx.execute("DELETE FROM work_sessions WHERE id=?1", params![id])?;
        }
        tx.commit()?;
        drop(conn);
        self.get_session(&new_id)?.context("merged session missing")
    }

    pub fn split_session(&self, id: &str, split_at: &str) -> Result<Vec<WorkSession>> {
        let session = self.get_session(id)?.context("session not found")?;
        let started_at = DateTime::parse_from_rfc3339(&session.started_at)
            .context("invalid session start time")?
            .with_timezone(&Utc);
        let ended_at = DateTime::parse_from_rfc3339(&session.ended_at)
            .context("invalid session end time")?
            .with_timezone(&Utc);
        let split_at = DateTime::parse_from_rfc3339(split_at)
            .context("invalid split time")?
            .with_timezone(&Utc);
        anyhow::ensure!(
            split_at - started_at >= Duration::seconds(5)
                && ended_at - split_at >= Duration::seconds(5),
            "拆分后的两段都必须至少保留 5 秒"
        );
        let split_at = fmt(split_at);
        let first_id = Uuid::new_v4().to_string();
        let second_id = Uuid::new_v4().to_string();
        let evidence_json = serde_json::to_string(&session.evidence)?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        record_tombstone(&tx, "session", id)?;
        tx.execute("DELETE FROM work_sessions WHERE id=?1", params![id])?;
        tx.execute(
            "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,'manual-split',?10)",
            params![
                first_id,
                session.started_at,
                &split_at,
                session.project_id,
                session.task_id,
                session.category,
                format!("{}（前半段）", session.summary),
                session.confidence,
                evidence_json,
                now()
            ],
        )?;
        let evidence_json2 = serde_json::to_string(&session.evidence)?;
        tx.execute(
            "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,'manual-split',?10)",
            params![
                second_id,
                &split_at,
                session.ended_at,
                session.project_id,
                session.task_id,
                session.category,
                format!("{}（后半段）", session.summary),
                session.confidence,
                evidence_json2,
                now()
            ],
        )?;
        tx.commit()?;
        drop(conn);
        Ok(vec![
            self.get_session(&first_id)?.unwrap(),
            self.get_session(&second_id)?.unwrap(),
        ])
    }

    pub fn ingest_raw_event(&self, mut event: RawActivityEvent) -> Result<()> {
        if event.id.is_empty() {
            event.id = Uuid::new_v4().to_string();
        }
        self.store_raw_event(&event)?;
        self.materialize_event_session(&event)?;
        Ok(())
    }

    pub fn heartbeat_raw_event(&self, event: &RawActivityEvent, session_id: &str) -> Result<()> {
        let input_stats = serde_json::to_string(&event.input_stats)?;
        let metadata = event.metadata.to_string();
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO raw_events VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                event.id,
                event.source,
                event.timestamp,
                event.app,
                event.window_title,
                event.url,
                event.file_path,
                event.workspace,
                input_stats,
                metadata
            ],
        )?;
        let changed = tx.execute(
            "UPDATE work_sessions SET ended_at=?1, updated_at=?2 WHERE id=?3",
            params![event.timestamp, now(), session_id],
        )?;
        anyhow::ensure!(changed == 1, "active session disappeared during heartbeat");
        tx.commit()?;
        Ok(())
    }

    fn store_raw_event(&self, event: &RawActivityEvent) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO raw_events VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                event.id,
                event.source,
                event.timestamp,
                event.app,
                event.window_title,
                event.url,
                event.file_path,
                event.workspace,
                serde_json::to_string(&event.input_stats)?,
                event.metadata.to_string()
            ],
        )?;
        Ok(())
    }

    fn materialize_event_session(&self, event: &RawActivityEvent) -> Result<()> {
        let settings = self.get_settings()?;
        let is_idle = event.input_stats.idle_seconds >= settings.idle_threshold_seconds as u64;
        let idle_project_id = if is_idle {
            Some(self.configure_idle_target(&settings)?)
        } else {
            None
        };
        let attribution = self.heuristic_attribution(event, is_idle, &settings, idle_project_id)?;
        let page_title = event
            .metadata
            .get("activePageTitle")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty());
        let mut evidence = vec![
            EvidenceItem {
                kind: if page_title.is_some() {
                    "page".into()
                } else {
                    "window".into()
                },
                label: if page_title.is_some() {
                    classification::context_evidence_label(&event.metadata).into()
                } else {
                    "窗口".into()
                },
                value: page_title
                    .map(str::to_string)
                    .or_else(|| event.window_title.clone())
                    .unwrap_or_else(|| "未知窗口".into()),
                weight: if page_title.is_some() { 0.82 } else { 0.7 },
            },
            EvidenceItem {
                kind: "app".into(),
                label: "应用".into(),
                value: event.app.clone().unwrap_or_else(|| "未知应用".into()),
                weight: 0.5,
            },
        ];
        if let Some(memory_evidence) = attribution.evidence.clone() {
            evidence.push(memory_evidence);
        }
        let evidence_json = serde_json::to_string(&evidence)?;
        let source = if is_idle {
            "collector-idle"
        } else {
            "collector-rule"
        };
        let conn = self.conn.lock();
        let last = conn
            .query_row(
                "SELECT id,project_id,task_id,summary,category,user_confirmed,source
             FROM work_sessions ORDER BY ended_at DESC, updated_at DESC LIMIT 1",
                [],
                |row| {
                    Ok(LastSessionAttribution {
                        id: row.get(0)?,
                        project_id: row.get(1)?,
                        task_id: row.get(2)?,
                        summary: row.get(3)?,
                        category: row.get(4)?,
                        user_confirmed: row.get::<_, i64>(5)? != 0,
                        source: row.get(6)?,
                    })
                },
            )
            .optional()?;
        if let Some(last) = last {
            if !starts_new_context(event)
                && !last.user_confirmed
                && last.source == source
                && last.project_id == attribution.project_id
                && last.task_id == attribution.task_id
                && last.summary == attribution.summary
                && last.category == attribution.category
            {
                conn.execute(
                    "UPDATE work_sessions
                     SET ended_at=MAX(ended_at, ?1), confidence=MAX(confidence, ?2),
                         evidence_json=?3, updated_at=?4
                     WHERE id=?5",
                    params![
                        event.timestamp,
                        attribution.confidence,
                        evidence_json,
                        now(),
                        last.id
                    ],
                )?;
                return Ok(());
            }
        }
        conn.execute(
            "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,0,?10,?11)",
            params![
                Uuid::new_v4().to_string(),
                event.timestamp,
                event.timestamp,
                attribution.project_id,
                attribution.task_id,
                attribution.category,
                attribution.summary,
                attribution.confidence,
                evidence_json,
                source,
                now()
            ],
        )?;
        Ok(())
    }

    fn heuristic_attribution(
        &self,
        event: &RawActivityEvent,
        is_idle: bool,
        settings: &AppSettings,
        idle_project_id: Option<String>,
    ) -> Result<AttributionDecision> {
        if is_idle {
            return Ok(AttributionDecision {
                project_id: idle_project_id,
                task_id: None,
                category: settings.idle_category.clone(),
                summary: "离开/空闲".into(),
                confidence: 0.96,
                evidence: None,
            });
        }
        if let Some(pin) = self.active_context()? {
            return Ok(AttributionDecision {
                project_id: Some(pin.project_id),
                task_id: pin.task_id,
                category: pin.category.clone(),
                summary: classification::summary_for_event(event, &pin.category),
                confidence: 0.98,
                evidence: Some(EvidenceItem {
                    kind: "context-pin".into(),
                    label: "固定事务".into(),
                    value: pin.project_name,
                    weight: 0.98,
                }),
            });
        }
        let hay = format!(
            "{} {} {} {}",
            event.app.clone().unwrap_or_default(),
            event.window_title.clone().unwrap_or_default(),
            event.url.clone().unwrap_or_default(),
            event.file_path.clone().unwrap_or_default()
        )
        .to_lowercase();
        let conn = self.conn.lock();
        let rules = {
            let mut stmt = conn.prepare("SELECT matcher_json,project_id,task_id,category,name,priority,updated_at FROM attribution_rules WHERE enabled=1 ORDER BY priority DESC,updated_at DESC")?;
            let mapped = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i32>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })?;
            collect_rows(mapped)?
        };
        let mut best_match = None;
        for (matcher_json, project_id, task_id, category, _name, priority, updated_at) in rules {
            let matcher: serde_json::Value =
                serde_json::from_str(&matcher_json).unwrap_or_default();
            let mut keywords = matcher
                .get("keywords")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str())
                .map(str::to_lowercase)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if let Some(keyword) = matcher.get("keyword").and_then(|v| v.as_str()) {
                if !keyword.trim().is_empty() {
                    keywords.push(keyword.to_lowercase());
                }
            }
            let app = matcher
                .get("app")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let domain = matcher
                .get("domain")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let workspace = matcher
                .get("workspace")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let has_constraint = !keywords.is_empty()
                || !app.is_empty()
                || !domain.is_empty()
                || !workspace.is_empty();
            let matched_keyword_length = keywords
                .iter()
                .filter(|keyword| hay.contains(keyword.as_str()))
                .map(|keyword| keyword.chars().count())
                .max()
                .unwrap_or(0);
            let hit = has_constraint
                && (keywords.is_empty() || matched_keyword_length > 0)
                && (app.is_empty()
                    || event
                        .app
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&app))
                && (domain.is_empty()
                    || event
                        .url
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&domain))
                && (workspace.is_empty()
                    || event
                        .workspace
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&workspace));
            if hit {
                let constraint_count = usize::from(!app.is_empty())
                    + usize::from(!domain.is_empty())
                    + usize::from(!workspace.is_empty());
                let rank = (
                    priority,
                    matched_keyword_length,
                    constraint_count,
                    updated_at,
                );
                if best_match
                    .as_ref()
                    .map_or(true, |(best_rank, _)| rank > *best_rank)
                {
                    best_match = Some((
                        rank,
                        AttributionDecision {
                            project_id,
                            task_id,
                            category: category.clone(),
                            summary: classification::summary_for_event(event, &category),
                            confidence: 0.84,
                            evidence: Some(EvidenceItem {
                                kind: "rule".into(),
                                label: "识别规则".into(),
                                value: _name,
                                weight: 0.84,
                            }),
                        },
                    ));
                }
            }
        }
        if let Some((_, decision)) = best_match {
            return Ok(decision);
        }
        drop(conn);
        if let Some(memory) = self.personal_memory_decision(event)? {
            return Ok(AttributionDecision {
                project_id: Some(memory.project_id),
                task_id: Some(memory.task_id),
                category: memory.category.clone(),
                summary: classification::summary_for_event(event, &memory.category),
                confidence: memory.confidence,
                evidence: Some(EvidenceItem {
                    kind: "personal-memory".into(),
                    label: format!("个人记忆 · {} 条支持", memory.support),
                    value: memory.matched_label,
                    weight: memory.confidence,
                }),
            });
        }
        let category = if hay.contains("code")
            || hay.contains("rust")
            || hay.contains("github")
            || hay.contains("tauri")
            || hay.contains("screenuse")
            || hay.contains("codex")
        {
            "开发"
        } else if hay.contains("scholar")
            || hay.contains("pubmed")
            || hay.contains("知网")
            || hay.contains("arxiv")
            || hay.contains("pdf")
            || hay.contains("论文")
            || hay.contains("教材")
            || hay.contains("ebook")
        {
            "学习"
        } else if hay.contains("word")
            || hay.contains("wps")
            || hay.contains("obsidian")
            || hay.contains("markdown")
        {
            "写作"
        } else if hay.contains("bilibili") || hay.contains("course") {
            "学习"
        } else if hay.contains("wechat")
            || hay.contains("qq")
            || hay.contains("mail")
            || hay.contains("teams")
            || hay.contains("meeting")
        {
            "沟通"
        } else if hay.contains("steam") || hay.contains("game") || hay.contains("youtube") {
            "娱乐"
        } else {
            "杂务"
        };
        Ok(AttributionDecision {
            project_id: None,
            task_id: None,
            category: category.into(),
            summary: classification::summary_for_event(event, category),
            confidence: 0.55,
            evidence: None,
        })
    }

    pub fn create_analysis_job(&self, job: &AnalysisJob) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO analysis_jobs(
               id,chunk_ids_json,started_at,ended_at,mode,retry_count,status,error,
               provider,model,system_prompt,user_prompt,response,queued_at,
               processing_started_at,completed_at,duration_ms,result_count,usage_json
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                job.id,
                serde_json::to_string(&job.chunk_ids)?,
                job.metadata_range.started_at,
                job.metadata_range.ended_at,
                job.mode,
                job.retry_count,
                job.status,
                job.error,
                job.provider,
                job.model,
                job.system_prompt,
                job.user_prompt,
                job.response,
                job.queued_at,
                job.processing_started_at,
                job.completed_at,
                job.duration_ms.map(|value| value as i64),
                job.result_count as i64,
                serde_json::to_string(&job.usage)?,
            ],
        )?;
        Ok(())
    }

    pub fn list_analysis_jobs(&self, limit: u32) -> Result<Vec<AnalysisJob>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id,chunk_ids_json,started_at,ended_at,mode,retry_count,status,error,
                    provider,model,NULL,NULL,NULL,queued_at,processing_started_at,
                    completed_at,duration_ms,result_count,usage_json
             FROM analysis_jobs
             ORDER BY queued_at DESC, started_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(
            params![i64::from(limit.clamp(1, 500))],
            analysis_job_from_row,
        )?;
        collect_rows(rows)
    }

    pub fn get_analysis_job(&self, id: &str) -> Result<Option<AnalysisJob>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id,chunk_ids_json,started_at,ended_at,mode,retry_count,status,error,
                    provider,model,system_prompt,user_prompt,response,queued_at,
                    processing_started_at,completed_at,duration_ms,result_count,usage_json
             FROM analysis_jobs WHERE id=?1",
            params![id],
            analysis_job_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn delete_skipped_analysis_job(&self, id: &str) -> Result<()> {
        self.delete_skipped_analysis_jobs(&[id.to_string()])?;
        Ok(())
    }

    pub fn delete_skipped_analysis_jobs(&self, ids: &[String]) -> Result<u32> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut seen = HashSet::new();
        let mut deleted = 0;
        for id in ids.iter().filter(|id| seen.insert(id.as_str())) {
            let status = tx
                .query_row(
                    "SELECT status FROM analysis_jobs WHERE id=?1",
                    params![id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            match status.as_deref() {
                Some("skipped") => {
                    deleted += tx.execute("DELETE FROM analysis_jobs WHERE id=?1", params![id])?;
                }
                Some(_) => bail!("只能删除未调用 AI 的复核记录"),
                None => bail!("复核记录不存在或已删除"),
            }
        }
        tx.commit()?;
        Ok(deleted as u32)
    }

    pub fn record_analysis_job_request(
        &self,
        id: &str,
        provider: &str,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE analysis_jobs
             SET provider=?1,model=?2,system_prompt=?3,user_prompt=?4,response=NULL,
                 result_count=0
             WHERE id=?5",
            params![provider, model, system_prompt, user_prompt, id],
        )?;
        Ok(())
    }

    pub fn record_analysis_job_response(
        &self,
        id: &str,
        response: &str,
        usage: &AiUsage,
    ) -> Result<()> {
        let conn = self.conn.lock();
        let previous = conn
            .query_row(
                "SELECT usage_json FROM analysis_jobs WHERE id=?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| serde_json::from_str::<AiUsage>(&value).ok())
            .unwrap_or_default();
        let mut cumulative = previous;
        cumulative.add_attempt(usage);
        conn.execute(
            "UPDATE analysis_jobs SET response=?1,usage_json=?2 WHERE id=?3",
            params![response, serde_json::to_string(&cumulative)?, id],
        )?;
        Ok(())
    }

    pub fn set_analysis_job_result_count(&self, id: &str, result_count: u32) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE analysis_jobs SET result_count=?1 WHERE id=?2",
            params![i64::from(result_count), id],
        )?;
        Ok(())
    }

    pub fn analysis_job_session_ids(&self) -> Result<HashSet<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT chunk_ids_json FROM analysis_jobs
             WHERE status IN ('pending','running','failed','downgraded')",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = HashSet::new();
        for row in rows {
            ids.extend(parse_json::<Vec<String>>(&row?));
        }
        Ok(ids)
    }

    pub fn claim_next_analysis_job(&self) -> Result<Option<AnalysisJob>> {
        let conn = self.conn.lock();
        let job = conn
            .query_row(
                "SELECT id,chunk_ids_json,started_at,ended_at,mode,retry_count,status,error,
                    provider,model,system_prompt,user_prompt,response,queued_at,
                    processing_started_at,completed_at,duration_ms,result_count,usage_json
             FROM analysis_jobs WHERE status='pending'
             ORDER BY queued_at ASC, started_at ASC LIMIT 1",
                [],
                analysis_job_from_row,
            )
            .optional()?;
        let Some(mut job) = job else { return Ok(None) };
        let processing_started_at = now();
        let claimed = conn.execute(
            "UPDATE analysis_jobs
             SET status='running', error=NULL, processing_started_at=?1,
                 completed_at=NULL, duration_ms=NULL
             WHERE id=?2 AND status='pending'",
            params![processing_started_at, job.id],
        )?;
        if claimed == 0 {
            return Ok(None);
        }
        job.status = "running".into();
        job.error = None;
        job.processing_started_at = Some(processing_started_at);
        job.completed_at = None;
        job.duration_ms = None;
        Ok(Some(job))
    }

    pub fn claim_analysis_job(&self, id: &str) -> Result<Option<AnalysisJob>> {
        let conn = self.conn.lock();
        let job = conn
            .query_row(
                "SELECT id,chunk_ids_json,started_at,ended_at,mode,retry_count,status,error,
                        provider,model,system_prompt,user_prompt,response,queued_at,
                        processing_started_at,completed_at,duration_ms,result_count,usage_json
                 FROM analysis_jobs WHERE id=?1 AND status='pending'",
                params![id],
                analysis_job_from_row,
            )
            .optional()?;
        let Some(mut job) = job else {
            return Ok(None);
        };
        let processing_started_at = now();
        let claimed = conn.execute(
            "UPDATE analysis_jobs
             SET status='running', error=NULL, processing_started_at=?1,
                 completed_at=NULL, duration_ms=NULL
             WHERE id=?2 AND status='pending'",
            params![processing_started_at, id],
        )?;
        if claimed == 0 {
            return Ok(None);
        }
        job.status = "running".into();
        job.error = None;
        job.processing_started_at = Some(processing_started_at);
        job.completed_at = None;
        job.duration_ms = None;
        Ok(Some(job))
    }

    pub fn mark_analysis_job_status(
        &self,
        id: &str,
        status: &str,
        retry_count: Option<u32>,
        error: Option<String>,
    ) -> Result<()> {
        let conn = self.conn.lock();
        let terminal = matches!(status, "completed" | "failed" | "downgraded" | "skipped");
        let completed_at = terminal.then(now);
        conn.execute(
            "UPDATE analysis_jobs
             SET status=?1,
                 retry_count=COALESCE(?2,retry_count),
                 error=?3,
                 processing_started_at=CASE WHEN ?1='pending' THEN NULL ELSE processing_started_at END,
                 completed_at=?5,
                 duration_ms=CASE
                   WHEN ?5 IS NULL THEN NULL
                   ELSE MAX(0, CAST((julianday(?5)-julianday(COALESCE(processing_started_at,queued_at)))*86400000 AS INTEGER))
                 END
             WHERE id=?4",
            params![
                status,
                retry_count.map(i64::from),
                error,
                id,
                completed_at
            ],
        )?;
        Ok(())
    }

    pub fn list_raw_events_between(
        &self,
        started_at: &str,
        ended_at: &str,
    ) -> Result<Vec<RawActivityEvent>> {
        self.list_raw_events_between_with_limit(started_at, ended_at, 500)
    }

    fn list_raw_events_between_with_limit(
        &self,
        started_at: &str,
        ended_at: &str,
        limit: i64,
    ) -> Result<Vec<RawActivityEvent>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(r#"
            SELECT id,source,timestamp,app,window_title,url,file_path,workspace,input_stats_json,metadata_json
            FROM raw_events
            WHERE timestamp >= ?1 AND timestamp <= ?2
            ORDER BY timestamp ASC
            LIMIT ?3
        "#)?;
        let rows = stmt.query_map(params![started_at, ended_at, limit], |r| {
            let input_stats_json: String = r.get(8)?;
            let metadata_json: String = r.get(9)?;
            Ok(RawActivityEvent {
                id: r.get(0)?,
                source: r.get(1)?,
                timestamp: r.get(2)?,
                app: r.get(3)?,
                window_title: r.get(4)?,
                url: r.get(5)?,
                file_path: r.get(6)?,
                workspace: r.get(7)?,
                input_stats: parse_json(&input_stats_json),
                metadata: serde_json::from_str(&metadata_json).unwrap_or_default(),
            })
        })?;
        collect_rows(rows)
    }

    pub fn upsert_project_by_name(
        &self,
        name: &str,
        category: &str,
        source: &str,
    ) -> Result<String> {
        let name = clean_name(name, "自动发现项目");
        let category = clean_name(category, "杂务");
        let conn = self.conn.lock();
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM projects WHERE name=?1 AND category=?2 LIMIT 1",
                params![name, category],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            conn.execute(
                "UPDATE projects SET updated_at=?1 WHERE id=?2",
                params![now(), id],
            )?;
            return Ok(id);
        }
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects VALUES (?1,?2,?3,?4,?5,?6,?7,?7)",
            params![
                id,
                name,
                category,
                source,
                color_for_category(&category),
                format!("由 ScreenUse 根据窗口、URL、目录和计划源自动发现：{name}"),
                now()
            ],
        )?;
        Ok(id)
    }

    pub fn upsert_task_by_title(
        &self,
        project_id: &str,
        title: &str,
        source: &str,
    ) -> Result<String> {
        let title = clean_name(title, "待确认活动");
        let conn = self.conn.lock();
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM tasks WHERE project_id=?1 AND title=?2 LIMIT 1",
                params![project_id, title],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            conn.execute(
                "UPDATE tasks SET status='active', updated_at=?1 WHERE id=?2",
                params![now(), id],
            )?;
            return Ok(id);
        }
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO tasks VALUES (?1,?2,?3,'active',?4,NULL,?5,?5)",
            params![id, project_id, title, source, now()],
        )?;
        Ok(id)
    }

    fn match_plan_items_for_session(&self, session_id: &str) -> Result<()> {
        let session = match self.get_session(session_id)? {
            Some(s) => s,
            None => return Ok(()),
        };
        let hay = format!(
            "{} {} {} {}",
            session.summary,
            session.category,
            session.project_name.unwrap_or_default(),
            session.task_title.unwrap_or_default()
        )
        .to_lowercase();
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT id,title,note,matched_session_ids_json FROM plan_items")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut updates = Vec::new();
        for row in rows {
            let (id, title, note, matched_json) = row?;
            let needle = format!("{} {}", title, note.unwrap_or_default()).to_lowercase();
            if !needle.is_empty()
                && (hay.contains(&needle)
                    || needle
                        .split_whitespace()
                        .any(|part| part.len() >= 3 && hay.contains(part)))
            {
                let mut matched: Vec<String> = parse_json(&matched_json);
                if !matched.iter().any(|s| s == session_id) {
                    matched.push(session_id.to_string());
                    updates.push((id, serde_json::to_string(&matched)?));
                }
            }
        }
        drop(stmt);
        for (id, matched_json) in updates {
            conn.execute(
                "UPDATE plan_items SET matched_session_ids_json=?1, updated_at=?2 WHERE id=?3",
                params![matched_json, now(), id],
            )?;
        }
        Ok(())
    }

    pub fn repair_session_timeline(&self) -> Result<u32> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut changed = 0_u32;
        let recent_cutoff = format!("-{RECENT_MAINTENANCE_DAYS} days");

        // A task is the most specific assignment. Repair legacy rows produced
        // before hierarchy validation by deriving their project and category.
        changed += tx.execute(
            "UPDATE work_sessions AS ws
             SET project_id=(SELECT t.project_id FROM tasks t WHERE t.id=ws.task_id),
                 category=(SELECT p.category FROM tasks t JOIN projects p ON p.id=t.project_id WHERE t.id=ws.task_id),
                 updated_at=?1
             WHERE ws.task_id IS NOT NULL
               AND EXISTS(SELECT 1 FROM tasks t JOIN projects p ON p.id=t.project_id WHERE t.id=ws.task_id)
               AND (ws.project_id IS NOT (SELECT t.project_id FROM tasks t WHERE t.id=ws.task_id)
                    OR ws.category<>(SELECT p.category FROM tasks t JOIN projects p ON p.id=t.project_id WHERE t.id=ws.task_id))",
            params![now()],
        )? as u32;
        changed += tx.execute(
            "UPDATE work_sessions
             SET task_id=NULL,updated_at=?1
             WHERE task_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM tasks t WHERE t.id=work_sessions.task_id)",
            params![now()],
        )? as u32;
        // If only the old project conflicts with a category correction, preserve
        // the user's category choice and drop the stale project reference.
        changed += tx.execute(
            "UPDATE work_sessions AS ws
             SET project_id=NULL,task_id=NULL,updated_at=?1
             WHERE ws.project_id IS NOT NULL AND ws.task_id IS NULL
               AND (NOT EXISTS(SELECT 1 FROM projects p WHERE p.id=ws.project_id)
                    OR EXISTS(SELECT 1 FROM projects p WHERE p.id=ws.project_id AND p.category<>ws.category))",
            params![now()],
        )? as u32;

        let nonpositive_ids = {
            let mut stmt = tx.prepare(
                "SELECT id FROM work_sessions
                 WHERE user_confirmed=0
                   AND source IN ('collector-rule','collector-idle','context-complete')
                   AND julianday(ended_at)<=julianday(started_at)",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            collect_rows(rows)?
        };
        for id in nonpositive_ids {
            record_tombstone(&tx, "session", &id)?;
            changed += tx.execute("DELETE FROM work_sessions WHERE id=?1", params![id])? as u32;
        }

        let sessions = {
            let mut stmt = tx.prepare(
                "SELECT ws.id,ws.started_at,ws.ended_at,ws.project_id,p.name,ws.task_id,t.title,
                        ws.category,ws.summary,ws.confidence,ws.evidence_json,ws.user_confirmed,ws.source
                 FROM work_sessions ws
                 LEFT JOIN projects p ON p.id=ws.project_id
                 LEFT JOIN tasks t ON t.id=ws.task_id
                 WHERE ws.user_confirmed=0
                   AND ws.source IN ('collector-rule','collector-idle','context-complete')
                   AND julianday(ws.ended_at)>julianday(ws.started_at)
                   AND julianday(ws.ended_at)>=julianday('now',?1)
                 ORDER BY ws.started_at ASC,ws.ended_at ASC,ws.updated_at ASC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(
                params![recent_cutoff, MAX_RECENT_MAINTENANCE_SESSIONS],
                map_work_session,
            )?;
            collect_rows(rows)?
        };

        let mut previous: Option<WorkSession> = None;
        for current in sessions {
            let Some(mut anchor) = previous.take() else {
                previous = Some(current);
                continue;
            };
            if !sessions_overlap(&anchor, &current) {
                previous = Some(current);
                continue;
            }

            if can_repair_overlapping_sessions(&anchor, &current) {
                let merged_end = if timestamp_is_after(&current.ended_at, &anchor.ended_at) {
                    current.ended_at.clone()
                } else {
                    anchor.ended_at.clone()
                };
                anchor.summary = preferred_coalesced_summary(&anchor.summary, &current.summary);
                anchor.confidence = anchor.confidence.max(current.confidence);
                anchor.evidence = merge_evidence(&anchor.evidence, &current.evidence);
                anchor.ended_at = merged_end.clone();
                tx.execute(
                    "UPDATE work_sessions SET ended_at=?1,summary=?2,confidence=?3,evidence_json=?4,updated_at=?5 WHERE id=?6",
                    params![merged_end, anchor.summary, anchor.confidence, serde_json::to_string(&anchor.evidence)?, now(), anchor.id],
                )?;
                tx.execute(
                    "UPDATE activities SET ended_at=?1,summary=?2,evidence_json=?3 WHERE session_id=?4",
                    params![merged_end, anchor.summary, serde_json::to_string(&anchor.evidence)?, anchor.id],
                )?;
                record_tombstone(&tx, "session", &current.id)?;
                tx.execute("DELETE FROM work_sessions WHERE id=?1", params![current.id])?;
                changed += 1;
                previous = Some(anchor);
                continue;
            }

            if timestamp_is_after(&current.started_at, &anchor.started_at) {
                anchor.ended_at = current.started_at.clone();
                tx.execute(
                    "UPDATE work_sessions SET ended_at=?1,updated_at=?2 WHERE id=?3",
                    params![anchor.ended_at, now(), anchor.id],
                )?;
                tx.execute(
                    "UPDATE activities SET ended_at=?1 WHERE session_id=?2",
                    params![anchor.ended_at, anchor.id],
                )?;
                changed += 1;
                previous = Some(current);
            } else {
                // Two collectors started at exactly the same moment. Keep the
                // longer (then more confident) automatic row instead of counting twice.
                let keep_current = timestamp_is_after(&current.ended_at, &anchor.ended_at)
                    || (current.ended_at == anchor.ended_at
                        && current.confidence > anchor.confidence);
                let removed_id = if keep_current {
                    &anchor.id
                } else {
                    &current.id
                };
                record_tombstone(&tx, "session", removed_id)?;
                tx.execute("DELETE FROM work_sessions WHERE id=?1", params![removed_id])?;
                changed += 1;
                previous = Some(if keep_current { current } else { anchor });
            }
        }

        tx.commit()?;
        Ok(changed)
    }

    pub fn compact_sessions(&self) -> Result<u32> {
        let ids = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id FROM (
                   SELECT id,started_at FROM work_sessions
                   WHERE julianday(ended_at)>=julianday('now',?1)
                   ORDER BY started_at DESC LIMIT ?2
                 ) ORDER BY started_at ASC",
            )?;
            let cutoff = format!("-{RECENT_MAINTENANCE_DAYS} days");
            let rows = stmt.query_map(params![cutoff, MAX_RECENT_MAINTENANCE_SESSIONS], |row| {
                row.get::<_, String>(0)
            })?;
            collect_rows(rows)?
        };
        let mut changed = 0;
        for id in ids {
            if self.get_session(&id)?.is_some() {
                let absorbed = self.absorb_short_auto_session(&id)?;
                if absorbed.id != id {
                    changed += 1;
                }
                let absorbed_id = absorbed.id;
                let session = self.coalesce_session_neighbors(&absorbed_id)?;
                if session.id != absorbed_id {
                    changed += 1;
                }
            }
        }
        Ok(changed)
    }

    fn migrate_personal_memory(&self) -> Result<()> {
        let already_migrated = self
            .conn
            .lock()
            .query_row(
                "SELECT 1 FROM settings WHERE key=?1 LIMIT 1",
                params![PERSONAL_MEMORY_MIGRATION_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if already_migrated {
            return Ok(());
        }

        self.rebuild_personal_memory_from_confirmed()?;

        // v0.2 的纠错规则把同一任务的大量窗口标题合成一个 OR 列表，像
        // “ChatGPT / 新标签页”这样的弱线索会覆盖后续页面。个人记忆已经从
        // 已确认会话回填，因此一次性停用这些旧生成规则；新规则带 generation=2。
        let legacy_rule_ids = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id,matcher_json FROM attribution_rules
                 WHERE created_from_correction=1 AND enabled=1",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            collect_rows(rows)?
                .into_iter()
                .filter_map(|(id, matcher_json)| {
                    let matcher = serde_json::from_str::<serde_json::Value>(&matcher_json).ok()?;
                    (matcher
                        .get("generation")
                        .and_then(serde_json::Value::as_u64)
                        != Some(2))
                    .then_some(id)
                })
                .collect::<Vec<_>>()
        };
        if !legacy_rule_ids.is_empty() {
            let mut conn = self.conn.lock();
            let tx = conn.transaction()?;
            for id in legacy_rule_ids {
                tx.execute(
                    "UPDATE attribution_rules SET enabled=0,updated_at=?1 WHERE id=?2",
                    params![now(), id],
                )?;
            }
            tx.commit()?;
        }
        self.conn.lock().execute(
            "INSERT INTO settings(key,value,updated_at) VALUES(?1,'done',?2)",
            params![PERSONAL_MEMORY_MIGRATION_KEY, now()],
        )?;
        Ok(())
    }

    fn migrate_personal_memory_consensus(&self) -> Result<()> {
        let already_migrated = self
            .conn
            .lock()
            .query_row(
                "SELECT 1 FROM settings WHERE key=?1 LIMIT 1",
                params![PERSONAL_MEMORY_CONSENSUS_MIGRATION_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if already_migrated {
            return Ok(());
        }
        self.rebuild_personal_memory_from_confirmed()?;
        self.conn.lock().execute(
            "INSERT INTO settings(key,value,updated_at) VALUES(?1,'done',?2)",
            params![PERSONAL_MEMORY_CONSENSUS_MIGRATION_KEY, now()],
        )?;
        Ok(())
    }

    fn migrate_personal_memory_batches(&self) -> Result<()> {
        let already_migrated = self
            .conn
            .lock()
            .query_row(
                "SELECT 1 FROM settings WHERE key=?1 LIMIT 1",
                params![PERSONAL_MEMORY_BATCH_MIGRATION_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if already_migrated {
            return Ok(());
        }
        self.rebuild_personal_memory_from_confirmed()?;
        self.conn.lock().execute(
            "INSERT INTO settings(key,value,updated_at) VALUES(?1,'done',?2)",
            params![PERSONAL_MEMORY_BATCH_MIGRATION_KEY, now()],
        )?;
        Ok(())
    }

    fn migrate_personal_memory_coherence(&self) -> Result<()> {
        let already_migrated = self
            .conn
            .lock()
            .query_row(
                "SELECT 1 FROM settings WHERE key=?1 LIMIT 1",
                params![PERSONAL_MEMORY_COHERENCE_MIGRATION_KEY],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if already_migrated {
            return Ok(());
        }
        self.rebuild_personal_memory_from_confirmed()?;
        self.conn.lock().execute(
            "INSERT INTO settings(key,value,updated_at) VALUES(?1,'done',?2)",
            params![PERSONAL_MEMORY_COHERENCE_MIGRATION_KEY, now()],
        )?;
        Ok(())
    }

    fn migrate_personal_memory_ai_consensus(&self) -> Result<()> {
        let already_migrated = self
            .conn
            .lock()
            .query_row(
                "SELECT value FROM settings WHERE key=?1 LIMIT 1",
                params![PERSONAL_MEMORY_AI_CONSENSUS_MIGRATION_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .is_some();
        if already_migrated {
            return Ok(());
        }

        // Start from the clean, user-confirmed memory set. AI results produced
        // after this cutoff may be retained as low-trust observations, but only
        // three agreeing observations can affect a future local decision.
        self.rebuild_personal_memory_from_confirmed()?;
        let cutoff = now();
        self.conn.lock().execute(
            "INSERT INTO settings(key,value,updated_at) VALUES(?1,?2,?2)",
            params![PERSONAL_MEMORY_AI_CONSENSUS_MIGRATION_KEY, cutoff],
        )?;
        Ok(())
    }

    fn provisional_ai_memory_cutoff(&self) -> Result<Option<String>> {
        self.conn
            .lock()
            .query_row(
                "SELECT value FROM settings WHERE key=?1 LIMIT 1",
                params![PERSONAL_MEMORY_AI_CONSENSUS_MIGRATION_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
    }

    fn is_provisional_ai_memory_session(&self, session: &WorkSession) -> Result<bool> {
        if session.user_confirmed
            || session.source != "ai-review"
            || session.confidence < 0.92
            || session.project_id.is_none()
            || session.task_id.is_none()
            || session
                .task_title
                .as_deref()
                .map_or(true, crate::ai::is_placeholder_task_title)
        {
            return Ok(false);
        }
        let Some(cutoff) = self.provisional_ai_memory_cutoff()? else {
            return Ok(false);
        };
        let updated_at = self
            .conn
            .lock()
            .query_row(
                "SELECT updated_at FROM work_sessions WHERE id=?1",
                params![session.id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(updated_at.is_some_and(|updated_at| updated_at >= cutoff))
    }

    pub(crate) fn rebuild_personal_memory_from_confirmed(&self) -> Result<u32> {
        self.conn
            .lock()
            .execute("DELETE FROM attribution_memories", [])?;
        let sessions = self.list_sessions(MAX_PERSONAL_MEMORIES)?;
        let confirmed = sessions
            .iter()
            .filter(|session| is_reliable_memory_session(session) && session.task_id.is_some())
            .cloned()
            .collect::<Vec<_>>();
        let mut stored = self.record_correction_memories(&confirmed, false)?;
        for session in &sessions {
            if self.is_provisional_ai_memory_session(session)? {
                stored += u32::from(self.record_personal_memory(session)?);
            }
        }
        Ok(stored)
    }

    fn record_correction_memories(
        &self,
        sessions: &[WorkSession],
        current_batch: bool,
    ) -> Result<u32> {
        if sessions.is_empty() {
            return Ok(0);
        }

        struct Candidate<'a> {
            session: &'a WorkSession,
            features: crate::memory::ContextFeatures,
            confirmed_at: String,
            group_key: String,
            feature_key: String,
        }

        let confirmed_at = {
            let conn = self.conn.lock();
            let mut values = HashMap::new();
            for session in sessions {
                if let Some(updated_at) = conn
                    .query_row(
                        "SELECT updated_at FROM work_sessions WHERE id=?1",
                        params![session.id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                {
                    values.insert(session.id.clone(), updated_at);
                }
            }
            values
        };

        let all_events = sessions
            .iter()
            .map(|session| (session.started_at.as_str(), session.ended_at.as_str()))
            .reduce(|range, value| (range.0.min(value.0), range.1.max(value.1)))
            .map(|(started_at, ended_at)| {
                self.list_raw_events_between_with_limit(started_at, ended_at, i64::MAX)
            })
            .transpose()?
            .unwrap_or_default();

        let mut candidates = Vec::new();
        for session in sessions {
            self.delete_personal_memory(&session.id)?;
            if !is_reliable_memory_session(session)
                || session.source == "collector-idle"
                || session.summary == "离开/空闲"
            {
                continue;
            }
            let (Some(project_id), Some(task_id)) =
                (session.project_id.as_deref(), session.task_id.as_deref())
            else {
                continue;
            };
            if session.project_name.is_none() || session.task_title.is_none() {
                continue;
            }
            if session
                .task_title
                .as_deref()
                .is_some_and(crate::ai::is_placeholder_task_title)
            {
                continue;
            }
            let first_event = all_events
                .partition_point(|event| event.timestamp.as_str() < session.started_at.as_str());
            let after_last_event = all_events
                .partition_point(|event| event.timestamp.as_str() <= session.ended_at.as_str());
            let events = &all_events[first_event..after_last_event];
            if crate::memory::has_ambiguous_session_context(session, events) {
                continue;
            }
            let features = crate::memory::features_from_session(session, events);
            if !crate::memory::is_discriminative(&features) {
                continue;
            }
            let timestamp = confirmed_at
                .get(&session.id)
                .cloned()
                .unwrap_or_else(|| session.ended_at.clone());
            let batch_label = if current_batch {
                "current-correction"
            } else {
                timestamp.as_str()
            };
            let group_key = format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}",
                batch_label, session.category, project_id, task_id
            );
            let feature_key = serde_json::to_string(&features)?;
            candidates.push(Candidate {
                session,
                features,
                confirmed_at: timestamp,
                group_key,
                feature_key,
            });
        }

        let mut group_counts = HashMap::<String, usize>::new();
        let mut feature_counts = HashMap::<(String, String), usize>::new();
        let mut latest_feature_assignments = HashMap::<String, (String, HashSet<String>)>::new();
        for candidate in &candidates {
            if candidate.session.user_confirmed {
                *group_counts.entry(candidate.group_key.clone()).or_default() += 1;
                *feature_counts
                    .entry((candidate.group_key.clone(), candidate.feature_key.clone()))
                    .or_default() += 1;
                let assignment = format!(
                    "{}\u{1f}{}\u{1f}{}",
                    candidate.session.category,
                    candidate.session.project_id.as_deref().unwrap_or_default(),
                    candidate.session.task_id.as_deref().unwrap_or_default()
                );
                match latest_feature_assignments.get_mut(&candidate.feature_key) {
                    Some((latest_at, assignments))
                        if candidate.confirmed_at.as_str() > latest_at.as_str() =>
                    {
                        *latest_at = candidate.confirmed_at.clone();
                        assignments.clear();
                        assignments.insert(assignment);
                    }
                    Some((latest_at, assignments))
                        if candidate.confirmed_at.as_str() == latest_at.as_str() =>
                    {
                        assignments.insert(assignment);
                    }
                    Some(_) => {}
                    None => {
                        latest_feature_assignments.insert(
                            candidate.feature_key.clone(),
                            (candidate.confirmed_at.clone(), HashSet::from([assignment])),
                        );
                    }
                }
            }
        }

        let mut stored = 0_u32;
        for candidate in candidates {
            let manual_group_size = group_counts
                .get(&candidate.group_key)
                .copied()
                .unwrap_or_default();
            let repeated_context = feature_counts
                .get(&(candidate.group_key.clone(), candidate.feature_key.clone()))
                .copied()
                .unwrap_or_default()
                >= 2;
            let assignment_anchored = crate::memory::relates_to_assignment(
                &candidate.features,
                candidate
                    .session
                    .project_name
                    .as_deref()
                    .unwrap_or_default(),
                candidate.session.task_title.as_deref().unwrap_or_default(),
            );
            let assignment = format!(
                "{}\u{1f}{}\u{1f}{}",
                candidate.session.category,
                candidate.session.project_id.as_deref().unwrap_or_default(),
                candidate.session.task_id.as_deref().unwrap_or_default()
            );
            let is_current_intent = !candidate.session.user_confirmed
                || latest_feature_assignments
                    .get(&candidate.feature_key)
                    .is_some_and(|(_, assignments)| {
                        assignments.len() == 1 && assignments.contains(&assignment)
                    });
            let keep = is_current_intent
                && (!candidate.session.user_confirmed
                    || manual_group_size <= 1
                    || repeated_context
                    || assignment_anchored);
            if keep {
                stored += u32::from(self.store_personal_memory(
                    candidate.session,
                    &candidate.features,
                    &candidate.confirmed_at,
                )?);
            }
        }
        Ok(stored)
    }

    fn record_personal_memory(&self, session: &WorkSession) -> Result<bool> {
        if session.project_id.is_none() {
            self.delete_personal_memory(&session.id)?;
            return Ok(false);
        }
        if session.task_id.is_none() {
            self.delete_personal_memory(&session.id)?;
            return Ok(false);
        }
        if session
            .task_title
            .as_deref()
            .map_or(true, crate::ai::is_placeholder_task_title)
        {
            self.delete_personal_memory(&session.id)?;
            return Ok(false);
        }
        if !(is_reliable_memory_session(session)
            || self.is_provisional_ai_memory_session(session)?)
            || session.source == "collector-idle"
            || session.summary == "离开/空闲"
        {
            self.delete_personal_memory(&session.id)?;
            return Ok(false);
        }
        let events = self.list_raw_events_between(&session.started_at, &session.ended_at)?;
        if crate::memory::has_ambiguous_session_context(session, &events) {
            self.delete_personal_memory(&session.id)?;
            return Ok(false);
        }
        let features = crate::memory::features_from_session(session, &events);
        if !crate::memory::is_discriminative(&features) {
            self.delete_personal_memory(&session.id)?;
            return Ok(false);
        }
        let confirmed_at = self
            .conn
            .lock()
            .query_row(
                "SELECT updated_at FROM work_sessions WHERE id=?1",
                params![session.id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .context("memory source session disappeared")?;
        self.store_personal_memory(session, &features, &confirmed_at)
    }

    fn store_personal_memory(
        &self,
        session: &WorkSession,
        features: &crate::memory::ContextFeatures,
        confirmed_at: &str,
    ) -> Result<bool> {
        let Some(project_id) = session.project_id.as_deref() else {
            return Ok(false);
        };
        let Some(task_id) = session.task_id.as_deref() else {
            return Ok(false);
        };
        let settings = self.get_settings()?.normalized();
        let conn = self.conn.lock();
        let project_name = conn
            .query_row(
                "SELECT name FROM projects WHERE id=?1",
                params![project_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .context("memory project disappeared")?;
        if session.category == settings.idle_category && project_name == settings.idle_project_name
        {
            conn.execute(
                "DELETE FROM attribution_memories WHERE session_id=?1",
                params![session.id],
            )?;
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO attribution_memories(
                session_id,features_json,category,project_id,task_id,confirmed_at,last_used_at,use_count
             ) VALUES(?1,?2,?3,?4,?5,?6,NULL,0)
             ON CONFLICT(session_id) DO UPDATE SET
                features_json=excluded.features_json,category=excluded.category,
                project_id=excluded.project_id,task_id=excluded.task_id,
                confirmed_at=excluded.confirmed_at",
            params![
                session.id,
                serde_json::to_string(features)?,
                session.category,
                project_id,
                task_id,
                confirmed_at,
            ],
        )?;
        conn.execute(
            "DELETE FROM attribution_memories
             WHERE session_id IN (
               SELECT session_id FROM attribution_memories
               ORDER BY confirmed_at DESC LIMIT -1 OFFSET ?1
             )",
            params![MAX_PERSONAL_MEMORIES],
        )?;
        Ok(true)
    }

    fn delete_personal_memory(&self, session_id: &str) -> Result<()> {
        self.conn.lock().execute(
            "DELETE FROM attribution_memories WHERE session_id=?1",
            params![session_id],
        )?;
        Ok(())
    }

    fn load_personal_memories(&self) -> Result<Vec<crate::memory::MemoryRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT m.session_id,m.features_json,m.category,m.project_id,p.name,
                    m.task_id,t.title,m.confirmed_at,ws.user_confirmed
             FROM attribution_memories m
             JOIN projects p ON p.id=m.project_id
             JOIN tasks t ON t.id=m.task_id AND t.project_id=m.project_id
             JOIN work_sessions ws ON ws.id=m.session_id
             ORDER BY m.confirmed_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![MAX_PERSONAL_MEMORIES], |row| {
            let features_json: String = row.get(1)?;
            Ok(crate::memory::MemoryRecord {
                session_id: row.get(0)?,
                features: serde_json::from_str(&features_json).unwrap_or_default(),
                category: row.get(2)?,
                project_id: row.get(3)?,
                project_name: row.get(4)?,
                task_id: row.get(5)?,
                task_title: row.get(6)?,
                confirmed_at: row.get(7)?,
                user_confirmed: row.get::<_, i64>(8)? != 0,
            })
        })?;
        collect_rows(rows)
    }

    pub(crate) fn relevant_personal_memories(
        &self,
        targets: &[WorkSession],
        per_target: usize,
    ) -> Result<Vec<crate::memory::RetrievedMemoryExample>> {
        let records = self.load_personal_memories()?;
        Ok(crate::memory::retrieve_examples(
            targets, &records, per_target,
        ))
    }

    fn personal_memory_decision(
        &self,
        event: &RawActivityEvent,
    ) -> Result<Option<crate::memory::MemoryDecision>> {
        let query = crate::memory::features_from_event(event);
        let records = self.load_personal_memories()?;
        let decision = crate::memory::choose_assignment(&query, &records);
        if let Some(decision) = &decision {
            self.conn.lock().execute(
                "UPDATE attribution_memories
                 SET last_used_at=?1,use_count=use_count+1 WHERE session_id=?2",
                params![now(), decision.memory_session_id],
            )?;
        }
        Ok(decision)
    }

    fn normalize_correction_rules(&self) -> Result<u32> {
        let rules = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id,matcher_json,project_id,task_id,category
                 FROM attribution_rules
                 WHERE created_from_correction=1 AND enabled=1
                 ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;
            collect_rows(rows)?
        };

        let mut seen = HashSet::new();
        let mut changed = 0_u32;
        let conn = self.conn.lock();
        for (id, original_matcher, project_id, task_id, category) in rules {
            let mut matcher: serde_json::Value =
                serde_json::from_str(&original_matcher).unwrap_or_else(|_| serde_json::json!({}));
            let mut keywords = matcher
                .get("keywords")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(crate::memory::canonical_context)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if let Some(keyword) = matcher
                .get("keyword")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let keyword = crate::memory::canonical_context(keyword);
                if !keyword.is_empty() {
                    keywords.push(keyword);
                }
            }
            if task_id.is_some() {
                if let Some(object) = matcher.as_object_mut() {
                    object.remove("app");
                }
            }
            let mut normalized = HashSet::new();
            keywords.retain(|keyword| normalized.insert(keyword.to_lowercase()));
            keywords.sort_by_key(|keyword| keyword.to_lowercase());
            keywords.truncate(32);
            if let Some(object) = matcher.as_object_mut() {
                object.remove("keyword");
                object.insert("keywords".into(), serde_json::json!(keywords));
            }
            let matcher_json = matcher.to_string();
            let identity = serde_json::to_string(&(
                &matcher_json,
                project_id.as_deref(),
                task_id.as_deref(),
                &category,
            ))?;
            if !seen.insert(identity) {
                conn.execute("DELETE FROM attribution_rules WHERE id=?1", params![id])?;
                changed += 1;
                continue;
            }
            if matcher_json != original_matcher {
                conn.execute(
                    "UPDATE attribution_rules SET matcher_json=?1,updated_at=?2 WHERE id=?3",
                    params![matcher_json, now(), id],
                )?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    #[cfg(test)]
    fn repair_sessions_from_confirmed_context(&self) -> Result<u32> {
        let confirmed = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT project_id,task_id,category,evidence_json,started_at
                 FROM work_sessions
                 WHERE user_confirmed=1 AND project_id IS NOT NULL AND task_id IS NOT NULL
                 ORDER BY updated_at DESC LIMIT 5000",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;
            collect_rows(rows)?
        };
        let mut votes: HashMap<String, HashMap<String, ConfirmedContextVote>> = HashMap::new();
        for (project_id, task_id, category, evidence_json, started_at) in confirmed {
            let evidence = parse_json::<Vec<EvidenceItem>>(&evidence_json);
            let Some(signature) = evidence_context_signature(&evidence) else {
                continue;
            };
            let key = context_assignment_key(Some(&project_id), Some(&task_id), &category);
            let vote = votes
                .entry(signature)
                .or_default()
                .entry(key)
                .or_insert_with(|| ConfirmedContextVote {
                    count: 0,
                    project_id,
                    task_id,
                    category,
                    first_confirmed_at: started_at.clone(),
                });
            vote.count += 1;
            if started_at.as_str() < vote.first_confirmed_at.as_str() {
                vote.first_confirmed_at = started_at;
            }
        }
        let mut memory = HashMap::new();
        for (signature, assignments) in votes {
            if assignments.len() != 1 {
                continue;
            }
            let Some(winner) = assignments.into_values().next() else {
                continue;
            };
            if winner.count >= 3 {
                memory.insert(
                    signature,
                    (
                        winner.project_id,
                        winner.task_id,
                        winner.category,
                        winner.first_confirmed_at,
                    ),
                );
            }
        }
        if memory.is_empty() {
            return Ok(0);
        }
        let candidates = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id,project_id,task_id,category,evidence_json,started_at,summary,source FROM work_sessions
                 WHERE user_confirmed=0 ORDER BY started_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![MAX_RECENT_MAINTENANCE_SESSIONS], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })?;
            collect_rows(rows)?
        };
        let mut repairs = Vec::new();
        for (id, project_id, task_id, category, evidence_json, started_at, summary, source) in
            candidates
        {
            if summary == "离开/空闲" || source == "collector-idle" {
                continue;
            }
            let evidence = parse_json::<Vec<EvidenceItem>>(&evidence_json);
            let Some(signature) = evidence_context_signature(&evidence) else {
                continue;
            };
            let Some((
                remembered_project,
                remembered_task,
                remembered_category,
                first_confirmed_at,
            )) = memory.get(&signature)
            else {
                continue;
            };
            if started_at.as_str() < first_confirmed_at.as_str() {
                continue;
            }
            if project_id.as_deref() == Some(remembered_project.as_str())
                && task_id.as_deref() == Some(remembered_task.as_str())
                && category == *remembered_category
            {
                continue;
            }
            repairs.push((
                id,
                remembered_project.clone(),
                remembered_task.clone(),
                remembered_category.clone(),
            ));
        }
        if repairs.is_empty() {
            return Ok(0);
        }
        let timestamp = now();
        let mut conn = self.conn.lock();
        let transaction = conn.transaction()?;
        for (id, project_id, task_id, category) in &repairs {
            transaction.execute(
                "UPDATE work_sessions SET project_id=?1,task_id=?2,category=?3,
                 confidence=MAX(confidence,0.92),source='context-memory',updated_at=?4
                 WHERE id=?5 AND user_confirmed=0",
                params![project_id, task_id, category, timestamp, id],
            )?;
        }
        transaction.commit()?;
        Ok(repairs.len() as u32)
    }

    pub fn learn_rule_from_session(
        &self,
        id: &str,
        keyword: Option<&str>,
    ) -> Result<AttributionRule> {
        let session = self.get_session(id)?.context("session not found")?;
        let app = session
            .evidence
            .iter()
            .find(|item| item.kind == "app")
            .map(|item| item.value.trim().trim_end_matches(".exe").to_lowercase())
            .unwrap_or_default();
        let window = session
            .evidence
            .iter()
            .find(|item| matches!(item.kind.as_str(), "page" | "window"))
            .map(|item| item.value.trim().to_string())
            .unwrap_or_default();
        let mut keywords = Vec::new();
        if let Some(keyword) = keyword.map(str::trim).filter(|value| !value.is_empty()) {
            keywords.extend(
                keyword
                    .split([',', '，', ';', '；'])
                    .map(str::trim)
                    .filter(|value| value.chars().count() >= 2)
                    .map(|value| value.chars().take(48).collect::<String>()),
            );
        }
        for candidate in [
            session.project_name.as_deref(),
            session.task_title.as_deref(),
        ] {
            if let Some(candidate) = candidate.and_then(context_memory_keyword) {
                keywords.push(candidate.chars().take(48).collect());
            }
        }
        if let Some(window) = context_memory_keyword(&window).filter(|value| {
            value.to_lowercase() != app && !is_generic_context_label(&value.to_lowercase())
        }) {
            keywords.push(window.chars().take(64).collect());
        }
        keywords = keywords
            .into_iter()
            .map(|value| crate::memory::canonical_context(&value))
            .filter(|value| !value.is_empty())
            .collect();
        keywords.sort();
        keywords.dedup();
        if keywords.is_empty() {
            bail!("当前窗口没有可区分线索，请填写识别词或固定当前事务");
        }
        let mut matcher = serde_json::json!({ "generation": 2, "keywords": keywords });
        if !app.is_empty() && session.task_id.is_none() {
            matcher["app"] = serde_json::Value::String(app);
        }
        let mut rule = AttributionRule {
            id: Uuid::new_v4().to_string(),
            name: format!(
                "自动学习：{}",
                session.summary.chars().take(24).collect::<String>()
            ),
            priority: 90,
            matcher,
            project_id: session.project_id,
            task_id: session.task_id,
            category: session.category,
            created_from_correction: true,
            enabled: true,
        };
        let conn = self.conn.lock();
        if rule.task_id.is_some() {
            let existing = {
                let mut stmt = conn.prepare(
                    "SELECT id,matcher_json FROM attribution_rules
                     WHERE created_from_correction=1 AND enabled=1
                       AND COALESCE(project_id,'')=COALESCE(?1,'')
                       AND COALESCE(task_id,'')=COALESCE(?2,'') AND category=?3
                     ORDER BY updated_at DESC",
                )?;
                let rows = stmt.query_map(
                    params![rule.project_id, rule.task_id, rule.category],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )?;
                collect_rows(rows)?
            };
            if let Some((keeper_id, _)) = existing.first() {
                let mut merged = rule
                    .matcher
                    .get("keywords")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>();
                for (_, matcher_json) in &existing {
                    let matcher =
                        serde_json::from_str::<serde_json::Value>(matcher_json).unwrap_or_default();
                    merged.extend(
                        matcher
                            .get("keywords")
                            .and_then(serde_json::Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(serde_json::Value::as_str)
                            .map(ToOwned::to_owned),
                    );
                }
                let mut seen = HashSet::new();
                merged.retain(|value| {
                    !is_context_memory_noise(value) && seen.insert(value.trim().to_lowercase())
                });
                merged.truncate(48);
                rule.matcher = serde_json::json!({ "generation": 2, "keywords": merged });
                conn.execute(
                    "UPDATE attribution_rules SET name=?1,priority=?2,matcher_json=?3,updated_at=?4 WHERE id=?5",
                    params![rule.name, rule.priority, rule.matcher.to_string(), now(), keeper_id],
                )?;
                for (duplicate_id, _) in existing.iter().skip(1) {
                    conn.execute(
                        "DELETE FROM attribution_rules WHERE id=?1",
                        params![duplicate_id],
                    )?;
                }
                rule.id = keeper_id.clone();
                return Ok(rule);
            }
        }
        let matcher_json = rule.matcher.to_string();
        if let Some(existing_id) = conn
            .query_row(
                "SELECT id FROM attribution_rules
                 WHERE created_from_correction=1 AND enabled=1
                   AND matcher_json=?1
                   AND COALESCE(project_id,'')=COALESCE(?2,'')
                   AND COALESCE(task_id,'')=COALESCE(?3,'')
                   AND category=?4
                 ORDER BY updated_at DESC
                 LIMIT 1",
                params![matcher_json, rule.project_id, rule.task_id, rule.category],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            conn.execute(
                "UPDATE attribution_rules SET name=?1,priority=?2,updated_at=?3 WHERE id=?4",
                params![rule.name, rule.priority, now(), existing_id],
            )?;
            rule.id = existing_id;
            return Ok(rule);
        }
        conn.execute(
            "INSERT INTO attribution_rules(id,name,priority,matcher_json,project_id,task_id,category,created_from_correction,enabled,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,1,1,?8)",
            params![rule.id, rule.name, rule.priority, matcher_json, rule.project_id, rule.task_id, rule.category, now()],
        )?;
        Ok(rule)
    }

    pub fn retry_failed_jobs(&self) -> Result<u32> {
        let conn = self.conn.lock();
        let changed = conn.execute(
            "UPDATE analysis_jobs
             SET status='pending', error=NULL, processing_started_at=NULL,
                 completed_at=NULL, duration_ms=NULL, response=NULL, result_count=0
             WHERE status IN ('failed','downgraded')",
            [],
        )?;
        Ok(changed as u32)
    }

    pub fn retry_analysis_jobs(&self, ids: &[String]) -> Result<u32> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut seen = HashSet::new();
        let mut changed = 0;
        for id in ids.iter().filter(|id| seen.insert(id.as_str())) {
            let status = tx
                .query_row(
                    "SELECT status FROM analysis_jobs WHERE id=?1",
                    params![id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            match status.as_deref() {
                Some("failed" | "downgraded") => {
                    changed += tx.execute(
                        "UPDATE analysis_jobs
                         SET status='pending', error=NULL, processing_started_at=NULL,
                             completed_at=NULL, duration_ms=NULL, response=NULL, result_count=0
                         WHERE id=?1",
                        params![id],
                    )?;
                }
                Some(_) => bail!("只能重试失败的 AI 复核记录"),
                None => bail!("复核记录不存在或已删除"),
            }
        }
        tx.commit()?;
        Ok(changed as u32)
    }

    pub fn queue_health(&self) -> Result<QueueHealth> {
        let temp_storage_limit_gb = self.get_settings()?.temp_storage_limit_gb as f32;
        let conn = self.conn.lock();
        let count = |status: &str| -> Result<u32> {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM analysis_jobs WHERE status=?1",
                params![status],
                |r| r.get::<_, i64>(0),
            )? as u32)
        };
        Ok(QueueHealth {
            pending: count("pending")?,
            running: count("running")?,
            failed: count("failed")?,
            downgraded: count("downgraded")?,
            temp_storage_gb: dir_size(self.data_dir.join("media-cache")) as f32 / 1_073_741_824.0,
            temp_storage_limit_gb,
            personal_memory_count: conn.query_row(
                "SELECT COUNT(*) FROM attribution_memories",
                [],
                |row| row.get::<_, i64>(0),
            )? as u32,
            personal_memory_uses: conn.query_row(
                "SELECT COALESCE(SUM(use_count),0) FROM attribution_memories",
                [],
                |row| row.get::<_, i64>(0),
            )? as u64,
        })
    }

    pub fn list_plan_items(&self, limit: i64) -> Result<Vec<PlanItem>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id,source,title,note,start_at,due_at,status,tags_json,matched_session_ids_json FROM plan_items ORDER BY COALESCE(due_at,start_at,updated_at) ASC LIMIT ?1")?;
        let rows = stmt.query_map(params![limit], |r| {
            let tags: String = r.get(7)?;
            let matched: String = r.get(8)?;
            Ok(PlanItem {
                id: r.get(0)?,
                source: r.get(1)?,
                title: r.get(2)?,
                note: r.get(3)?,
                start_at: r.get(4)?,
                due_at: r.get(5)?,
                status: r.get(6)?,
                tags: parse_json(&tags),
                matched_session_ids: parse_json(&matched),
            })
        })?;
        collect_rows(rows)
    }

    pub fn upsert_plan_items(&self, items: &[PlanItem]) -> Result<usize> {
        let conn = self.conn.lock();
        let mut count = 0;
        for item in items {
            conn.execute(
                "INSERT OR REPLACE INTO plan_items VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    item.id,
                    item.source,
                    item.title,
                    item.note,
                    item.start_at,
                    item.due_at,
                    item.status,
                    serde_json::to_string(&item.tags)?,
                    serde_json::to_string(&item.matched_session_ids)?,
                    now()
                ],
            )?;
            count += 1;
        }
        Ok(count)
    }

    pub fn project_task_trends(&self) -> Result<Vec<TrendPoint>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(r#"
            SELECT COALESCE(p.name, '未归类') AS label,
                   ROUND(SUM((julianday(ws.ended_at)-julianday(ws.started_at))*24*60), 1) AS minutes,
                   ws.category
            FROM work_sessions ws LEFT JOIN projects p ON p.id=ws.project_id
            WHERE ws.category != '离开' AND NOT (ws.source='collector-idle' AND ws.user_confirmed=0)
            GROUP BY COALESCE(p.name, '未归类'), ws.category
            ORDER BY minutes DESC LIMIT 12
        "#)?;
        let rows = stmt.query_map([], |r| {
            Ok(TrendPoint {
                label: r.get(0)?,
                value: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                group: r.get(2)?,
            })
        })?;
        collect_rows(rows)
    }

    pub fn category_trends(&self) -> Result<Vec<TrendPoint>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(r#"
            SELECT category, ROUND(SUM((julianday(ended_at)-julianday(started_at))*24*60), 1) AS minutes, category
            FROM work_sessions GROUP BY category ORDER BY minutes DESC
        "#)?;
        let rows = stmt.query_map([], |r| {
            Ok(TrendPoint {
                label: r.get(0)?,
                value: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                group: r.get(2)?,
            })
        })?;
        collect_rows(rows)
    }

    pub fn export_path(&self, extension: &str) -> PathBuf {
        self.data_dir.join("exports").join(format!(
            "screenuse-{}.{}",
            Utc::now().format("%Y%m%d-%H%M%S"),
            extension
        ))
    }

    pub fn record_export(&self, format: &str, path: &Path) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO export_records VALUES (?1,?2,?3,?4)",
            params![
                Uuid::new_v4().to_string(),
                format,
                path.display().to_string(),
                now()
            ],
        )?;
        Ok(())
    }

    pub fn backup_now(&self, target_dir: Option<String>) -> Result<PathBuf> {
        let dir = target_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| self.data_dir.join("backups"));
        fs::create_dir_all(&dir)?;
        let timestamp = Utc::now();
        let target = dir.join(format!(
            "screenuse-backup-{}-{:03}.db",
            timestamp.format("%Y%m%d-%H%M%S"),
            timestamp.timestamp_subsec_millis(),
        ));
        let conn = self.conn.lock();
        conn.backup(DatabaseName::Main, &target, None)?;
        Ok(target)
    }
}

fn starts_new_context(event: &RawActivityEvent) -> bool {
    event
        .metadata
        .get("contextStart")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn insert_seed_session(conn: &Connection, session: SeedSession<'_>) -> Result<()> {
    let evidence = vec![
        EvidenceItem {
            kind: "window".into(),
            label: "窗口".into(),
            value: "Codex / VS Code / Chrome".into(),
            weight: 0.7,
        },
        EvidenceItem {
            kind: "ai".into(),
            label: "AI摘要".into(),
            value: session.summary.into(),
            weight: 0.9,
        },
    ];
    conn.execute(
        "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,0,'seed',?10)",
        params![
            session.id,
            fmt(session.start),
            fmt(session.end),
            session.project_id,
            session.task_id,
            session.category,
            session.summary,
            session.confidence,
            serde_json::to_string(&evidence)?,
            now()
        ],
    )?;
    Ok(())
}

pub fn now() -> String {
    fmt(Utc::now())
}
fn fmt(t: chrono::DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse_json<T: DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_str(s).unwrap_or_default()
}

fn analysis_job_from_row(row: &Row<'_>) -> rusqlite::Result<AnalysisJob> {
    let chunk_ids_json: String = row.get(1)?;
    Ok(AnalysisJob {
        id: row.get(0)?,
        chunk_ids: parse_json(&chunk_ids_json),
        metadata_range: TimeRange {
            started_at: row.get(2)?,
            ended_at: row.get(3)?,
        },
        mode: row.get(4)?,
        retry_count: row.get::<_, i64>(5)?.max(0) as u32,
        status: row.get(6)?,
        error: row.get(7)?,
        provider: row.get(8)?,
        model: row.get(9)?,
        system_prompt: row.get(10)?,
        user_prompt: row.get(11)?,
        response: row.get(12)?,
        queued_at: row.get(13)?,
        processing_started_at: row.get(14)?,
        completed_at: row.get(15)?,
        duration_ms: row
            .get::<_, Option<i64>>(16)?
            .map(|value| value.max(0) as u64),
        result_count: row.get::<_, i64>(17)?.max(0) as u32,
        usage: row
            .get::<_, Option<String>>(18)?
            .map(|value| parse_json(&value))
            .unwrap_or_default(),
    })
}

fn configure_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         PRAGMA temp_store=MEMORY;
         PRAGMA cache_size=-2000;
         PRAGMA wal_autocheckpoint=256;
         PRAGMA journal_size_limit=1048576;",
    )?;
    Ok(())
}

fn clean_name(value: &str, fallback: &str) -> String {
    let cleaned = value.trim().replace(['\r', '\n', '\t'], " ");
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned.chars().take(80).collect()
    }
}

fn ensure_column(conn: &Connection, table: &str, column: &str, declaration: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for existing in columns {
        if existing? == column {
            return Ok(());
        }
    }
    conn.execute_batch(&format!(
        "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
    ))?;
    Ok(())
}

fn record_tombstone(conn: &Connection, entity_kind: &str, entity_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_tombstones(entity_kind,entity_id,deleted_at,device_id)
         VALUES(?1,?2,?3,'')
         ON CONFLICT(entity_kind,entity_id) DO UPDATE SET deleted_at=excluded.deleted_at",
        params![entity_kind, entity_id, now()],
    )?;
    Ok(())
}

fn color_for_category(category: &str) -> &'static str {
    match category {
        "开发" => "#38bdf8",
        "学习" => "#a78bfa",
        "写作" => "#f0abfc",
        "沟通" => "#34d399",
        "娱乐" => "#fb7185",
        "无效" => "#94a3b8",
        "离开" => "#94a3b8",
        _ => "#facc15",
    }
}

fn within_gap_seconds(left_end: &str, right_start: &str, max_seconds: i64) -> bool {
    let left = DateTime::parse_from_rfc3339(left_end).map(|t| t.with_timezone(&Utc));
    let right = DateTime::parse_from_rfc3339(right_start).map(|t| t.with_timezone(&Utc));
    match (left, right) {
        (Ok(left), Ok(right)) => right >= left && right - left <= Duration::seconds(max_seconds),
        _ => false,
    }
}

#[cfg(test)]
fn context_gap_seconds(left_end: &str, right_start: &str) -> Option<i64> {
    let left = DateTime::parse_from_rfc3339(left_end)
        .ok()?
        .with_timezone(&Utc);
    let right = DateTime::parse_from_rfc3339(right_start)
        .ok()?
        .with_timezone(&Utc);
    Some((right - left).num_seconds().max(0))
}

#[cfg(test)]
fn context_is_disconnected(left_end: &str, right_start: &str, max_seconds: i64) -> bool {
    match context_gap_seconds(left_end, right_start) {
        Some(gap) => gap > max_seconds,
        None => true,
    }
}

fn can_auto_coalesce(left: &WorkSession, right: &WorkSession) -> bool {
    if right.user_confirmed || !within_gap_seconds(&left.ended_at, &right.started_at, 5) {
        return false;
    }
    // Screenshot utilities are part of the task that was active immediately before them.
    // The previous task may have been corrected by the user, so it remains a valid anchor;
    // only an automatically collected overlay is allowed to be absorbed.
    if is_task_overlay_session(right) {
        return is_auto_session_source(&right.source)
            && left.source != "collector-idle"
            && left.category != "离开";
    }
    if left.user_confirmed {
        return false;
    }
    if left.source == "collector-idle" && right.source == "collector-idle" {
        return left.project_id == right.project_id && left.category == right.category;
    }
    if left.source != "context-complete" || right.source != "context-complete" {
        return false;
    }
    if left.project_id != right.project_id
        || left.task_id != right.task_id
        || left.category != right.category
    {
        return false;
    }
    let left_app = primary_session_app(left);
    let right_app = primary_session_app(right);
    left_app.is_some()
        && left_app == right_app
        && (left.project_id.is_some() || left.task_id.is_some() || left.summary == right.summary)
}

fn can_bridge_short_detour(left: &WorkSession, right: &WorkSession) -> bool {
    !left.user_confirmed
        && !right.user_confirmed
        && left.source == "context-complete"
        && right.source == "context-complete"
        && left.category != "离开"
        && left.project_id == right.project_id
        && left.task_id == right.task_id
        && left.category == right.category
        && (left.project_id.is_some() || left.task_id.is_some())
        && primary_session_app(left).is_some()
        && primary_session_app(left) == primary_session_app(right)
        && within_gap_seconds(&left.ended_at, &right.started_at, 90)
}

fn short_detour_is_compatible(
    anchor: &WorkSession,
    middle: &[WorkSession],
    current: &WorkSession,
) -> bool {
    if middle.is_empty() {
        return false;
    }
    let mut previous_end = anchor.ended_at.as_str();
    for session in middle {
        let compatible_assignment = session.project_id == anchor.project_id
            || (session.project_id.is_none() && session.category == "杂务");
        if session.user_confirmed
            || session.source != "context-complete"
            || session.category == "离开"
            || !compatible_assignment
            || !within_gap_seconds(previous_end, &session.started_at, 3)
        {
            return false;
        }
        previous_end = &session.ended_at;
    }
    within_gap_seconds(previous_end, &current.started_at, 3)
}

fn session_duration_seconds(session: &WorkSession) -> Option<i64> {
    let start = DateTime::parse_from_rfc3339(&session.started_at).ok()?;
    let end = DateTime::parse_from_rfc3339(&session.ended_at).ok()?;
    Some((end - start).num_seconds().max(0))
}

fn touch_or_overlap_within(left_end: &str, right_start: &str, max_seconds: i64) -> bool {
    let left = DateTime::parse_from_rfc3339(left_end).map(|time| time.with_timezone(&Utc));
    let right = DateTime::parse_from_rfc3339(right_start).map(|time| time.with_timezone(&Utc));
    match (left, right) {
        (Ok(left), Ok(right)) => right <= left + Duration::seconds(max_seconds),
        _ => false,
    }
}

fn timestamp_is_after(candidate: &str, current: &str) -> bool {
    match (
        DateTime::parse_from_rfc3339(candidate),
        DateTime::parse_from_rfc3339(current),
    ) {
        (Ok(candidate), Ok(current)) => candidate > current,
        _ => candidate > current,
    }
}

fn sessions_overlap(left: &WorkSession, right: &WorkSession) -> bool {
    let left_start = DateTime::parse_from_rfc3339(&left.started_at);
    let left_end = DateTime::parse_from_rfc3339(&left.ended_at);
    let right_start = DateTime::parse_from_rfc3339(&right.started_at);
    let right_end = DateTime::parse_from_rfc3339(&right.ended_at);
    match (left_start, left_end, right_start, right_end) {
        (Ok(left_start), Ok(left_end), Ok(right_start), Ok(right_end)) => {
            left_start < right_end && right_start < left_end
        }
        _ => right.started_at < left.ended_at && left.started_at < right.ended_at,
    }
}

fn can_repair_overlapping_sessions(left: &WorkSession, right: &WorkSession) -> bool {
    if left.user_confirmed
        || right.user_confirmed
        || !is_auto_session_source(&left.source)
        || !is_auto_session_source(&right.source)
        || left.project_id != right.project_id
        || left.task_id != right.task_id
        || left.category != right.category
    {
        return false;
    }
    if left.source == "collector-idle" && right.source == "collector-idle" {
        return true;
    }
    left.summary == right.summary
        || ((left.project_id.is_some() || left.task_id.is_some())
            && primary_session_app(left).is_some()
            && primary_session_app(left) == primary_session_app(right))
}

fn is_auto_session_source(source: &str) -> bool {
    matches!(
        source,
        "collector-rule" | "collector-idle" | "context-complete"
    )
}

fn is_idle_session(session: &WorkSession, settings: &AppSettings) -> bool {
    session.source == "collector-idle"
        || session.summary.trim() == "离开/空闲"
        || (session.category == settings.idle_category
            && session.project_name.as_deref() == Some(settings.idle_project_name.as_str()))
}

fn ai_prompt_idle_session_ids(prompt: &str, settings: &AppSettings) -> HashSet<String> {
    let Some(input_start) = prompt.find("输入：") else {
        return HashSet::new();
    };
    let payload = prompt[input_start + "输入：".len()..]
        .trim()
        .trim_end_matches('。');
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(payload) else {
        return HashSet::new();
    };
    payload
        .get("reviewItems")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("targetSession"))
        .filter(|target| {
            let summary_is_idle = target
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value.trim() == "离开/空闲");
            let source_is_idle =
                target.get("source").and_then(serde_json::Value::as_str) == Some("collector-idle");
            let configured_idle_target = target.get("category").and_then(serde_json::Value::as_str)
                == Some(settings.idle_category.as_str())
                && target
                    .get("projectName")
                    .and_then(serde_json::Value::as_str)
                    == Some(settings.idle_project_name.as_str());
            summary_is_idle || source_is_idle || configured_idle_target
        })
        .filter_map(|target| {
            target
                .get("sessionId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

fn is_reliable_memory_session(session: &WorkSession) -> bool {
    !matches!(session.source.as_str(), "manual-entry" | "manual-merge") && session.user_confirmed
}

fn is_task_overlay_session(session: &WorkSession) -> bool {
    primary_session_app(session)
        .as_deref()
        .is_some_and(is_task_overlay_app_name)
        || is_screenshot_overlay_title(&session.summary)
        || session.evidence.iter().any(|item| {
            matches!(item.kind.as_str(), "window" | "page")
                && is_screenshot_overlay_title(&item.value)
        })
}

fn is_task_overlay_app_name(app: &str) -> bool {
    let app = app.trim().trim_end_matches(".exe");
    matches!(
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
    )
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

fn primary_session_app(session: &WorkSession) -> Option<String> {
    session
        .evidence
        .iter()
        .find(|item| item.kind == "app")
        .map(|item| item.value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
}

fn preferred_coalesced_summary(left: &str, right: &str) -> String {
    if is_transient_summary(left) && !is_transient_summary(right) {
        right.to_string()
    } else {
        left.to_string()
    }
}

fn is_transient_summary(value: &str) -> bool {
    let value = value.to_lowercase();
    [
        "图片查看器",
        "snipaste",
        "snipping tool",
        "截图工具",
        "无标题",
        "新标签页",
        "loading",
        "加载中",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

fn merge_evidence(left: &[EvidenceItem], right: &[EvidenceItem]) -> Vec<EvidenceItem> {
    let mut merged = Vec::new();
    for item in left.iter().chain(right.iter()) {
        if !merged
            .iter()
            .any(|known: &EvidenceItem| known.kind == item.kind && known.value == item.value)
        {
            merged.push(item.clone());
        }
        if merged.len() >= 20 {
            break;
        }
    }
    merged
}

fn custom_category_color(name: &str) -> &'static str {
    const COLORS: [&str; 8] = [
        "#8b5cf6", "#ec4899", "#06b6d4", "#14b8a6", "#f97316", "#6366f1", "#84cc16", "#d946ef",
    ];
    let hash = name.bytes().fold(0usize, |value, byte| {
        value.wrapping_mul(31).wrapping_add(byte as usize)
    });
    COLORS[hash % COLORS.len()]
}

#[cfg(test)]
fn context_assignment_key(
    project_id: Option<&str>,
    task_id: Option<&str>,
    category: &str,
) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        project_id.unwrap_or_default(),
        task_id.unwrap_or_default(),
        category
    )
}

fn context_memory_keyword(value: &str) -> Option<String> {
    let value = value.replace(['\r', '\n', '\t'], " ").trim().to_string();
    let normalized = value.to_lowercase();
    if value.chars().count() < 3
        || value.chars().count() > 160
        || is_generic_context_label(&normalized)
        || is_transient_summary(&normalized)
        || is_context_memory_noise(&normalized)
    {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
fn evidence_context_signature(evidence: &[EvidenceItem]) -> Option<String> {
    let app = evidence
        .iter()
        .find(|item| item.kind == "app")
        .map(|item| item.value.trim().to_lowercase())
        .filter(|value| !value.is_empty())?;
    let context = evidence
        .iter()
        .find(|item| matches!(item.kind.as_str(), "page" | "window"))
        .and_then(|item| context_memory_keyword(&item.value))?
        .to_lowercase();
    Some(format!("{app}\u{1f}{context}"))
}

fn is_context_memory_noise(value: &str) -> bool {
    let normalized = value.trim().to_lowercase();
    matches!(
        normalized.as_str(),
        "program manager"
            | "task switching"
            | "desktop"
            | "会议"
            | "腾讯会议"
            | "加入会议"
            | "学习"
            | "开发"
            | "科研"
            | "工作"
            | "任务"
            | "沟通"
            | "杂务"
            | "日常杂务"
            | "无效"
            | "离开"
            | "校内实习"
    ) || [
        "系统托盘溢出窗口",
        "任务视图",
        "任务切换",
        "程序管理器",
        "桌面",
    ]
    .iter()
    .any(|label| normalized.contains(label))
}

fn is_generic_context_label(value: &str) -> bool {
    matches!(
        value.trim().trim_end_matches(".exe"),
        "chatgpt"
            | "codex"
            | "chrome"
            | "msedge"
            | "firefox"
            | "brave"
            | "qq"
            | "wechat"
            | "weixin"
            | "wps"
            | "explorer"
            | "screenuse"
            | "new tab"
            | "新标签页"
            | "电脑活动"
    )
}

fn map_work_session(row: &Row<'_>) -> rusqlite::Result<WorkSession> {
    let evidence_json: String = row.get(10)?;
    Ok(WorkSession {
        id: row.get(0)?,
        started_at: row.get(1)?,
        ended_at: row.get(2)?,
        project_id: row.get(3)?,
        project_name: row.get(4)?,
        task_id: row.get(5)?,
        task_title: row.get(6)?,
        category: row.get(7)?,
        summary: row.get(8)?,
        confidence: row.get(9)?,
        evidence: parse_json(&evidence_json),
        user_confirmed: row.get::<_, i64>(11)? != 0,
        source: row.get(12)?,
    })
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn dir_size(path: PathBuf) -> u64 {
    if !path.exists() {
        return 0;
    }
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total += meta.len();
                } else if meta.is_dir() {
                    total += dir_size(p);
                }
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat_event(id: &str, title: &str) -> RawActivityEvent {
        RawActivityEvent {
            id: id.into(),
            source: "windows-foreground".into(),
            timestamp: now(),
            app: Some("ChatGPT.exe".into()),
            window_title: Some(title.into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: serde_json::json!({ "contextStart": true }),
        }
    }

    fn context_event(
        id: &str,
        app: &str,
        title: &str,
        timestamp: DateTime<Utc>,
    ) -> RawActivityEvent {
        RawActivityEvent {
            id: id.into(),
            source: "windows-foreground".into(),
            timestamp: fmt(timestamp),
            app: Some(app.into()),
            window_title: Some(title.into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: serde_json::json!({ "contextStart": true }),
        }
    }

    #[test]
    fn context_start_forces_a_new_session_boundary() {
        let event = RawActivityEvent {
            id: "context-1".into(),
            source: "test".into(),
            timestamp: now(),
            app: None,
            window_title: None,
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: serde_json::json!({ "contextStart": true }),
        };
        assert!(starts_new_context(&event));
    }

    #[test]
    fn active_page_title_is_saved_as_the_primary_evidence() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-page-evidence-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        db.ingest_raw_event(RawActivityEvent {
            id: "page-evidence".into(),
            source: "windows-foreground".into(),
            timestamp: now(),
            app: Some("wps.exe".into()),
            window_title: Some("ICPC 训练计划.docx".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: serde_json::json!({
                "contextStart": true,
                "activePageTitle": "ICPC 训练计划.docx",
                "activePageSource": "document-window-title"
            }),
        })
        .expect("ingest document event");
        let session = db.list_sessions(1).expect("list sessions")[0].clone();
        assert_eq!(session.evidence[0].kind, "page");
        assert_eq!(session.evidence[0].label, "当前文档");
        assert_eq!(session.evidence[0].value, "ICPC 训练计划.docx");
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn qq_conversation_is_labeled_as_the_current_chat() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-qq-evidence-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        db.ingest_raw_event(RawActivityEvent {
            id: "qq-conversation-evidence".into(),
            source: "windows-foreground".into(),
            timestamp: now(),
            app: Some("QQ.exe".into()),
            window_title: Some("科研讨论群".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: serde_json::json!({
                "contextStart": true,
                "activePageTitle": "科研讨论群",
                "activePageSource": "qq-conversation-header",
                "conversationTitle": "科研讨论群"
            }),
        })
        .expect("ingest QQ conversation event");
        let session = db.list_sessions(1).expect("list sessions")[0].clone();
        assert_eq!(session.evidence[0].kind, "page");
        assert_eq!(session.evidence[0].label, "当前会话");
        assert_eq!(session.evidence[0].value, "科研讨论群");
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn legacy_ai_placeholder_tasks_return_to_the_review_queue() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-ai-concrete-task-migration-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let category = db
            .list_categories()
            .expect("list categories")
            .into_iter()
            .find(|category| category.name == "杂务")
            .expect("built-in category");
        let project = db
            .create_project("历史 AI 杂务", &category.name)
            .expect("create project");
        let placeholder = db
            .create_task(&project.id, "others")
            .expect("create placeholder task");
        let session =
            classification::ingest_event(&db, &chat_event("legacy-ai-placeholder", "待确认聊天"))
                .expect("ingest target")
                .expect("target session");
        db.apply_ai_review(
            &session.id,
            SessionPatch {
                summary: Some("未明确归属的事务".into()),
                project_id: Some(project.id.clone()),
                task_id: Some(placeholder.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: Some(category.name),
                confidence: Some(0.96),
                user_confirmed: Some(false),
            },
            Vec::new(),
        )
        .expect("apply legacy AI review");
        db.conn
            .lock()
            .execute(
                "DELETE FROM settings WHERE key=?1",
                params![AI_CONCRETE_TASK_REPAIR_MIGRATION_KEY],
            )
            .expect("reset migration marker");

        assert_eq!(
            db.migrate_incomplete_ai_review_tasks()
                .expect("repair incomplete AI result"),
            1
        );
        let repaired = db
            .get_session(&session.id)
            .expect("load repaired session")
            .expect("repaired session exists");
        assert!(repaired.project_id.is_none());
        assert!(repaired.task_id.is_none());
        assert_eq!(repaired.source, "context-complete");
        assert!(repaired.confidence < 0.8);
        assert_eq!(
            db.conn
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM attribution_memories WHERE session_id=?1",
                    params![session.id],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count placeholder memories"),
            0
        );
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn three_consistent_ai_reviews_become_a_low_trust_local_memory() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-ai-in-place-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let session = classification::ingest_event(&db, &chat_event("ai-target", "申书豪材料群"))
            .expect("ingest target")
            .expect("target session");
        let category = db.create_category("保研").expect("create category");
        let project = db
            .create_project("推免", &category.name)
            .expect("create project");
        let task = db
            .create_task(&project.id, "成果填报")
            .expect("create task");
        let count_before = db.list_sessions(500).expect("sessions before").len();
        let reviewed = db
            .apply_ai_review(
                &session.id,
                SessionPatch {
                    summary: Some("沟通推免成果填报".into()),
                    project_id: Some(project.id.clone()),
                    task_id: Some(task.id.clone()),
                    clear_project: Some(false),
                    clear_task: Some(false),
                    category: Some(category.name.clone()),
                    confidence: Some(0.93),
                    user_confirmed: Some(false),
                },
                vec![EvidenceItem {
                    kind: "ai".into(),
                    label: "上下文".into(),
                    value: "前一时段为成果填报".into(),
                    weight: 0.9,
                }],
            )
            .expect("apply AI review");
        assert_eq!(reviewed.id, session.id);
        assert_eq!(reviewed.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(reviewed.task_id.as_deref(), Some(task.id.as_str()));
        assert_eq!(reviewed.source, "ai-review");
        assert!(!reviewed.user_confirmed);
        assert_eq!(
            db.list_sessions(500).expect("sessions after").len(),
            count_before
        );
        assert_eq!(
            db.conn
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM attribution_memories WHERE session_id=?1",
                    params![session.id],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count AI memories"),
            1
        );
        let mut follow_up = context_event(
            "ai-memory-target",
            "wps.exe",
            "申书豪材料群",
            Utc::now() + Duration::minutes(2),
        );
        follow_up.metadata["activePageTitle"] = serde_json::Value::String("申书豪材料群".into());
        let first_repeat = classification::ingest_event(&db, &follow_up)
            .expect("ingest first repeated context")
            .expect("first repeated session");
        assert!(first_repeat.task_id.is_none());
        db.apply_ai_review(
            &first_repeat.id,
            SessionPatch {
                summary: Some("沟通推免成果填报".into()),
                project_id: Some(project.id.clone()),
                task_id: Some(task.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: Some(category.name.clone()),
                confidence: Some(0.93),
                user_confirmed: Some(false),
            },
            Vec::new(),
        )
        .expect("apply corroborating AI review");
        let mut second_repeat = context_event(
            "ai-memory-corroborated-target",
            "QQ.exe",
            "申书豪材料群",
            Utc::now() + Duration::minutes(4),
        );
        second_repeat.metadata["activePageTitle"] =
            serde_json::Value::String("申书豪材料群".into());
        let still_unresolved = classification::ingest_event(&db, &second_repeat)
            .expect("ingest repeated AI context")
            .expect("repeated AI session");
        assert!(still_unresolved.task_id.is_none());
        assert!(!still_unresolved
            .evidence
            .iter()
            .any(|item| item.kind == "personal-memory"));
        db.apply_ai_review(
            &still_unresolved.id,
            SessionPatch {
                summary: Some("沟通推免成果填报".into()),
                project_id: Some(project.id.clone()),
                task_id: Some(task.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: Some(category.name.clone()),
                confidence: Some(0.93),
                user_confirmed: Some(false),
            },
            Vec::new(),
        )
        .expect("apply third consistent AI review");

        let mut third_repeat = context_event(
            "ai-memory-consensus-target",
            "chrome.exe",
            "申书豪材料群",
            Utc::now() + Duration::minutes(6),
        );
        third_repeat.metadata["activePageTitle"] = serde_json::Value::String("申书豪材料群".into());
        let learned = classification::ingest_event(&db, &third_repeat)
            .expect("ingest consensus context")
            .expect("consensus session");
        assert_eq!(learned.task_id.as_deref(), Some(task.id.as_str()));
        assert!(learned
            .evidence
            .iter()
            .any(|item| item.kind == "personal-memory"));
        assert_eq!(
            db.conn
                .lock()
                .query_row("SELECT COUNT(*) FROM attribution_memories", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count consensus memories"),
            3
        );
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn dashboard_load_does_not_reenter_the_database_lock() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-dashboard-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let dashboard = db.dashboard(false).expect("load dashboard");
        assert!(!dashboard.projects.is_empty());
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn existing_sampling_settings_migrate_to_one_second_only_once() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-sampling-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let mut settings = AppSettings {
            poll_interval_seconds: 10,
            heartbeat_seconds: 10,
            ..AppSettings::default()
        };
        db.save_settings(&settings).expect("save legacy settings");
        db.conn
            .lock()
            .execute(
                "DELETE FROM settings WHERE key=?1",
                params![ONE_SECOND_SAMPLING_MIGRATION_KEY],
            )
            .expect("reset migration marker");

        let migrated = db.get_settings().expect("migrate settings");
        assert_eq!(migrated.poll_interval_seconds, 1);
        assert_eq!(migrated.heartbeat_seconds, 5);

        settings.poll_interval_seconds = 7;
        settings.heartbeat_seconds = 7;
        db.save_settings(&settings).expect("save user override");
        let preserved = db.get_settings().expect("load user override");
        assert_eq!(preserved.poll_interval_seconds, 7);
        assert_eq!(preserved.heartbeat_seconds, 7);

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn legacy_process_paths_are_removed_without_deleting_real_binary_files() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-process-path-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        {
            let conn = db.conn.lock();
            conn.execute(
                "DELETE FROM settings WHERE key=?1",
                params![PROCESS_FILE_PATH_MIGRATION_KEY],
            )
            .expect("reset process path migration");
            for (id, app, file_path) in [
                (
                    "legacy-process",
                    "QQ.exe",
                    r"C:\Program Files\Tencent\QQ.exe",
                ),
                ("real-binary", "ida.exe", r"D:\CTF\challenge.exe"),
            ] {
                conn.execute(
                    "INSERT INTO raw_events(
                        id,source,timestamp,app,window_title,url,file_path,workspace,
                        input_stats_json,metadata_json
                     ) VALUES(?1,'test',?2,?3,?3,NULL,?4,NULL,'{}','{}')",
                    params![id, now(), app, file_path],
                )
                .expect("insert legacy event");
            }
        }

        assert_eq!(db.migrate_process_file_paths().expect("migrate paths"), 1);
        let conn = db.conn.lock();
        let process_path: Option<String> = conn
            .query_row(
                "SELECT file_path FROM raw_events WHERE id='legacy-process'",
                [],
                |row| row.get(0),
            )
            .expect("read process event");
        let real_file: Option<String> = conn
            .query_row(
                "SELECT file_path FROM raw_events WHERE id='real-binary'",
                [],
                |row| row.get(0),
            )
            .expect("read real binary event");
        assert!(process_path.is_none());
        assert_eq!(real_file.as_deref(), Some(r"D:\CTF\challenge.exe"));
        drop(conn);
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn historical_idle_boundary_moves_back_to_the_last_input_time_once() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-idle-boundary-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let active_id = Uuid::new_v4().to_string();
        let idle_id = Uuid::new_v4().to_string();
        let evidence = serde_json::to_string(&vec![EvidenceItem {
            kind: "app".into(),
            label: "应用".into(),
            value: "QQ.exe".into(),
            weight: 0.75,
        }])
        .expect("serialize evidence");
        {
            let conn = db.conn.lock();
            conn.execute(
                "DELETE FROM settings WHERE key=?1",
                params![IDLE_BOUNDARY_MIGRATION_KEY],
            )
            .expect("reset migration marker");
            conn.execute(
                "INSERT INTO work_sessions VALUES (?1,'2026-07-12T10:00:00Z','2026-07-12T10:05:00Z',NULL,NULL,'杂务','QQ',0.8,?2,0,'context-complete',?3)",
                params![active_id, evidence, now()],
            )
            .expect("insert active session");
            conn.execute(
                "INSERT INTO work_sessions VALUES (?1,'2026-07-12T10:05:00Z','2026-07-12T10:06:00Z',NULL,NULL,'离开','离开/空闲',0.96,?2,0,'context-complete',?3)",
                params![idle_id, evidence, now()],
            )
            .expect("insert idle session");
        }

        assert_eq!(db.backfill_idle_boundaries().expect("backfill idle"), 1);
        assert_eq!(db.backfill_idle_boundaries().expect("do not repeat"), 0);
        let active = db
            .get_session(&active_id)
            .expect("load active")
            .expect("active exists");
        let idle = db
            .get_session(&idle_id)
            .expect("load idle")
            .expect("idle exists");
        assert_eq!(active.ended_at, "2026-07-12T10:02:00Z");
        assert_eq!(idle.started_at, "2026-07-12T10:02:00Z");

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn removed_ddl_manager_items_are_purged_on_open() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-ddl-removal-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        db.upsert_plan_items(&[PlanItem {
            id: "ddl-task:legacy".into(),
            source: "DDL-Manager".into(),
            title: "旧 DDL 项目".into(),
            note: None,
            start_at: None,
            due_at: None,
            status: "todo".into(),
            tags: vec![],
            matched_session_ids: vec![],
        }])
        .expect("insert legacy item");
        drop(db);

        let reopened = AppDb::open_in(data_dir.clone()).expect("reopen test database");
        assert!(reopened
            .list_plan_items(50)
            .expect("list plan items")
            .iter()
            .all(|item| item.source != "DDL-Manager" && !item.id.starts_with("ddl-")));
        drop(reopened);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn adjacent_same_app_assignment_is_coalesced_but_real_app_switches_remain() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-coalesce-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db
            .create_project("连续会话", "杂务")
            .expect("create project");
        let task = db.create_task(&project.id, "QQ").expect("create task");
        let base = Utc::now() + Duration::hours(2);
        let insert = |id: &str,
                      start: chrono::DateTime<Utc>,
                      end: chrono::DateTime<Utc>,
                      summary: &str,
                      app: &str| {
            let evidence = vec![EvidenceItem {
                kind: "app".into(),
                label: "应用".into(),
                value: app.into(),
                weight: 0.5,
            }];
            db.conn
                .lock()
                .execute(
                    "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,'杂务',?6,0.88,?7,0,'context-complete',?8)",
                    params![id, fmt(start), fmt(end), project.id, task.id, summary, serde_json::to_string(&evidence).unwrap(), now()],
                )
                .expect("insert session");
        };

        insert(
            "qq-main",
            base,
            base + Duration::seconds(40),
            "QQ",
            "QQ.exe",
        );
        insert(
            "qq-viewer",
            base + Duration::seconds(40),
            base + Duration::seconds(55),
            "QQ · 图片查看器",
            "QQ.exe",
        );
        insert(
            "chat-switch",
            base + Duration::seconds(55),
            base + Duration::seconds(65),
            "ChatGPT",
            "ChatGPT.exe",
        );
        let other_project = db
            .create_project("另一个事务", "开发")
            .expect("create other project");
        let other_task = db
            .create_task(&other_project.id, "独立任务")
            .expect("create other task");
        db.conn
            .lock()
            .execute(
                "UPDATE work_sessions SET project_id=?1,task_id=?2,category='开发' WHERE id='chat-switch'",
                params![other_project.id, other_task.id],
            )
            .expect("move real switch to another task");
        insert(
            "qq-return",
            base + Duration::seconds(65),
            base + Duration::seconds(80),
            "QQ",
            "QQ.exe",
        );

        assert_eq!(db.compact_sessions().expect("compact sessions"), 1);
        let merged = db
            .get_session("qq-main")
            .expect("load merged")
            .expect("merged exists");
        assert_eq!(merged.ended_at, fmt(base + Duration::seconds(55)));
        assert_eq!(merged.summary, "QQ");
        assert!(db.get_session("qq-viewer").expect("load removed").is_none());
        assert!(db
            .get_session("chat-switch")
            .expect("load switch")
            .is_some());
        assert!(db.get_session("qq-return").expect("load return").is_some());

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn short_same_task_detour_is_folded_into_one_session() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-short-detour-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db
            .create_project("短暂切换测试", "开发")
            .expect("create project");
        let primary_task = db
            .create_task(&project.id, "使用测试")
            .expect("create primary task");
        let helper_task = db
            .create_task(&project.id, "开发与调试")
            .expect("create helper task");
        let base = Utc::now() + Duration::hours(3);
        let insert = |id: &str,
                      start: chrono::DateTime<Utc>,
                      end: chrono::DateTime<Utc>,
                      project_id: Option<&str>,
                      task_id: Option<&str>,
                      category: &str,
                      summary: &str,
                      app: &str| {
            let evidence = vec![EvidenceItem {
                kind: "app".into(),
                label: "应用".into(),
                value: app.into(),
                weight: 0.5,
            }];
            db.conn
                .lock()
                .execute(
                    "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,0.88,?8,0,'context-complete',?9)",
                    params![id, fmt(start), fmt(end), project_id, task_id, category, summary, serde_json::to_string(&evidence).unwrap(), now()],
                )
                .expect("insert session");
        };

        insert(
            "screenuse-before",
            base,
            base + Duration::seconds(60),
            Some(&project.id),
            Some(&primary_task.id),
            "开发",
            "screenuse",
            "screenuse.exe",
        );
        insert(
            "chat-helper",
            base + Duration::seconds(60),
            base + Duration::seconds(75),
            Some(&project.id),
            Some(&helper_task.id),
            "开发",
            "ScreenUse · ChatGPT.exe",
            "ChatGPT.exe",
        );
        insert(
            "chat-generic",
            base + Duration::seconds(75),
            base + Duration::seconds(85),
            None,
            None,
            "杂务",
            "ChatGPT",
            "ChatGPT.exe",
        );
        insert(
            "screenuse-return",
            base + Duration::seconds(85),
            base + Duration::seconds(120),
            Some(&project.id),
            Some(&primary_task.id),
            "开发",
            "screenuse",
            "screenuse.exe",
        );

        assert_eq!(db.compact_sessions().expect("compact detour"), 1);
        let merged = db
            .get_session("screenuse-before")
            .expect("load merged")
            .expect("merged exists");
        assert_eq!(merged.ended_at, fmt(base + Duration::seconds(120)));
        assert_eq!(merged.task_id.as_deref(), Some(primary_task.id.as_str()));
        assert!(merged
            .evidence
            .iter()
            .any(|item| item.value.eq_ignore_ascii_case("ChatGPT.exe")));
        for removed in ["chat-helper", "chat-generic", "screenuse-return"] {
            assert!(db.get_session(removed).expect("load absorbed").is_none());
        }

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn screenshot_utility_is_merged_into_confirmed_previous_session() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-screenshot-overlay-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db
            .create_project("截图测试", "开发")
            .expect("create project");
        let task = db
            .create_task(&project.id, "界面修正")
            .expect("create task");
        let base = Utc::now() + Duration::hours(4);
        let evidence = |app: &str| {
            serde_json::to_string(&vec![EvidenceItem {
                kind: "app".into(),
                label: "应用".into(),
                value: app.into(),
                weight: 0.5,
            }])
            .expect("serialize evidence")
        };
        {
            let conn = db.conn.lock();
            conn.execute(
                "INSERT INTO work_sessions VALUES ('work-before',?1,?2,?3,?4,'开发','ScreenUse',0.9,?5,1,'manual-correction',?6)",
                params![fmt(base), fmt(base + Duration::seconds(40)), project.id, task.id, evidence("screenuse.exe"), now()],
            )
            .expect("insert work");
            conn.execute(
                "INSERT INTO work_sessions VALUES ('snipaste-overlay',?1,?2,NULL,NULL,'杂务','Snipper - Snipaste',0.56,?3,0,'context-complete',?4)",
                params![fmt(base + Duration::seconds(40)), fmt(base + Duration::seconds(55)), evidence("Snipaste.exe"), now()],
            )
            .expect("insert screenshot overlay");
            conn.execute(
                "INSERT INTO work_sessions VALUES ('qq-screenshot-overlay',?1,?2,NULL,NULL,'沟通','QQ · QQ截图',0.56,?3,0,'context-complete',?4)",
                params![fmt(base + Duration::seconds(55)), fmt(base + Duration::seconds(70)), evidence("QQ.exe"), now()],
            )
            .expect("insert QQ screenshot overlay");
        }

        assert_eq!(db.compact_sessions().expect("compact screenshot"), 2);
        let merged = db
            .get_session("work-before")
            .expect("load merged")
            .expect("merged exists");
        assert_eq!(merged.ended_at, fmt(base + Duration::seconds(70)));
        assert_eq!(merged.project_id.as_deref(), Some(project.id.as_str()));
        assert!(db
            .get_session("snipaste-overlay")
            .expect("load screenshot")
            .is_none());
        assert!(db
            .get_session("qq-screenshot-overlay")
            .expect("load QQ screenshot")
            .is_none());

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn multiple_selected_sessions_are_corrected_together() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-bulk-correction-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db
            .create_project("批量修正项目", "学习")
            .expect("create project");
        let task = db.create_task(&project.id, "旧任务").expect("create task");
        let sessions = db.list_sessions(2).expect("list sessions");
        let ids = sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        for id in &ids {
            db.update_session(
                id,
                SessionPatch {
                    summary: None,
                    project_id: Some(project.id.clone()),
                    task_id: Some(task.id.clone()),
                    clear_project: Some(false),
                    clear_task: Some(false),
                    category: Some("学习".into()),
                    confidence: None,
                    user_confirmed: Some(false),
                },
            )
            .expect("prepare session");
        }

        let updated = db
            .update_sessions(
                &ids,
                SessionPatch {
                    summary: None,
                    project_id: None,
                    task_id: None,
                    clear_project: Some(true),
                    clear_task: Some(true),
                    category: Some("杂务".into()),
                    confidence: Some(0.98),
                    user_confirmed: Some(true),
                },
            )
            .expect("bulk correct sessions");

        assert_eq!(updated.len(), 2);
        assert!(updated.iter().all(|session| session.category == "杂务"));
        assert!(updated.iter().all(|session| session.project_id.is_none()));
        assert!(updated.iter().all(|session| session.task_id.is_none()));
        assert!(updated.iter().all(|session| session.user_confirmed));

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn one_correction_changes_only_the_selected_session_and_can_be_undone() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-single-correction-undo-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db
            .create_project("浪费时间", "无效")
            .expect("create project");
        let task = db
            .create_task(&project.id, "无目的浏览")
            .expect("create task");
        let sessions = db.list_sessions(2).expect("list sessions");
        let selected_before = sessions[0].clone();
        let untouched_before = sessions[1].clone();
        let rule_count_before: i64 = db
            .conn
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM attribution_rules WHERE created_from_correction=1",
                [],
                |row| row.get(0),
            )
            .expect("count rules");

        let updated = db
            .apply_session_correction(
                std::slice::from_ref(&selected_before.id),
                SessionPatch {
                    summary: Some("浪费".into()),
                    project_id: Some(project.id.clone()),
                    task_id: Some(task.id.clone()),
                    clear_project: Some(false),
                    clear_task: Some(false),
                    category: Some("无效".into()),
                    confidence: Some(0.98),
                    user_confirmed: Some(true),
                },
                true,
                Some("无目的浏览"),
                Some(30),
            )
            .expect("apply correction");

        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].summary, "浪费");
        assert_eq!(updated[0].task_id.as_deref(), Some(task.id.as_str()));
        let untouched_after = db
            .get_session(&untouched_before.id)
            .expect("load untouched session")
            .expect("untouched session exists");
        assert_eq!(untouched_after.summary, untouched_before.summary);
        assert_eq!(untouched_after.category, untouched_before.category);
        assert_eq!(untouched_after.project_id, untouched_before.project_id);
        assert_eq!(untouched_after.task_id, untouched_before.task_id);
        assert_eq!(
            untouched_after.user_confirmed,
            untouched_before.user_confirmed
        );
        assert!(db.undo_status().available);
        assert_eq!(
            db.active_context()
                .expect("load pin")
                .expect("pin exists")
                .project_id,
            project.id
        );

        db.undo_last_session_correction().expect("undo correction");
        let restored = db
            .get_session(&selected_before.id)
            .expect("load restored session")
            .expect("restored session exists");
        assert_eq!(restored.summary, selected_before.summary);
        assert_eq!(restored.category, selected_before.category);
        assert_eq!(restored.project_id, selected_before.project_id);
        assert_eq!(restored.task_id, selected_before.task_id);
        assert_eq!(restored.user_confirmed, selected_before.user_confirmed);
        assert_eq!(restored.source, selected_before.source);
        assert!(db.active_context().expect("load restored pin").is_none());
        let rule_count_after: i64 = db
            .conn
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM attribution_rules WHERE created_from_correction=1",
                [],
                |row| row.get(0),
            )
            .expect("count restored rules");
        assert_eq!(rule_count_after, rule_count_before);
        assert!(!db.undo_status().available);

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn deleting_a_project_unassigns_its_sessions_and_tasks() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-project-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db
            .create_project("误归类项目", "开发")
            .expect("create project");
        let task_id = db
            .upsert_task_by_title(&project.id, "临时任务", "manual")
            .expect("create task");
        let session_id = db.list_sessions(1).expect("list sessions")[0].id.clone();
        db.update_session(
            &session_id,
            SessionPatch {
                summary: None,
                project_id: Some(project.id.clone()),
                task_id: Some(task_id),
                clear_project: Some(false),
                clear_task: Some(false),
                category: None,
                confidence: None,
                user_confirmed: None,
            },
        )
        .expect("assign session");

        db.delete_project(&project.id).expect("delete project");
        let session = db
            .get_session(&session_id)
            .expect("load session")
            .expect("session remains");
        assert!(session.project_id.is_none());
        assert!(session.task_id.is_none());
        assert!(!db
            .list_projects()
            .expect("list projects")
            .iter()
            .any(|item| item.id == project.id));

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn updating_a_project_renames_it_and_moves_related_assignments() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-update-project-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let category = db.create_category("保研").expect("create category");
        let project = db.create_project("旧项目", "杂务").expect("create project");
        let task = db
            .create_task(&project.id, "成果填报")
            .expect("create task");
        let session_id = db.list_sessions(1).expect("list sessions")[0].id.clone();
        db.update_session(
            &session_id,
            SessionPatch {
                summary: None,
                project_id: Some(project.id.clone()),
                task_id: Some(task.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: None,
                confidence: None,
                user_confirmed: None,
            },
        )
        .expect("assign session");
        db.conn
            .lock()
            .execute(
                "INSERT INTO attribution_rules(id,name,priority,matcher_json,project_id,task_id,category,created_from_correction,enabled,updated_at)
                 VALUES('move-project-rule','成果填报',100,'{}',?1,?2,'杂务',1,1,?3)",
                params![project.id, task.id, now()],
            )
            .expect("insert rule");

        let updated = db
            .update_project(&project.id, "预推免", &category.name)
            .expect("update project");
        assert_eq!(updated.name, "预推免");
        assert_eq!(updated.category, "保研");
        assert_eq!(updated.color, category.color);
        let session = db
            .get_session(&session_id)
            .expect("load session")
            .expect("session remains");
        assert_eq!(session.project_name.as_deref(), Some("预推免"));
        assert_eq!(session.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(session.task_id.as_deref(), Some(task.id.as_str()));
        assert_eq!(session.category, "保研");
        let rule_category: String = db
            .conn
            .lock()
            .query_row(
                "SELECT category FROM attribution_rules WHERE id='move-project-rule'",
                [],
                |row| row.get(0),
            )
            .expect("load rule");
        assert_eq!(rule_category, "保研");

        db.create_project("重复项目", "保研")
            .expect("create duplicate target");
        assert!(db.update_project(&project.id, "重复项目", "保研").is_err());
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn custom_categories_and_tasks_can_be_created_and_removed() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-taxonomy-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let category = db.create_category("竞赛").expect("create category");
        let project = db
            .create_project("ICPC", &category.name)
            .expect("create project");
        assert!(project.description.is_none());
        let task = db
            .create_task(&project.id, "网站开发")
            .expect("create task");
        assert!(db
            .list_tasks()
            .expect("list tasks")
            .iter()
            .any(|item| item.id == task.id));
        db.delete_task(&task.id).expect("delete task");
        assert!(!db
            .list_tasks()
            .expect("list tasks")
            .iter()
            .any(|item| item.id == task.id));
        let fallback = db.delete_category(&category.name).expect("delete category");
        let updated = db
            .list_projects()
            .expect("list projects")
            .into_iter()
            .find(|item| item.id == project.id)
            .expect("project remains");
        assert_eq!(updated.category, fallback);
        db.conn
            .lock()
            .execute(
                "UPDATE projects SET description=?1 WHERE id=?2",
                params!["在修正归类时手动创建", project.id],
            )
            .expect("restore obsolete project description");
        drop(db);

        let reopened = AppDb::open_in(data_dir.clone()).expect("reopen test database");
        let reopened_project = reopened
            .list_projects()
            .expect("list reopened projects")
            .into_iter()
            .find(|item| item.id == project.id)
            .expect("reopened project remains");
        assert!(reopened_project.description.is_none());
        drop(reopened);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn builtin_categories_can_be_renamed_and_deleted_durably() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-builtin-category-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let renamed = db
            .rename_category("开发", "编程")
            .expect("rename builtin category");
        assert_eq!(renamed.name, "编程");
        assert!(renamed.is_builtin);
        assert!(db
            .list_projects()
            .expect("list projects")
            .iter()
            .any(|item| item.name == "ScreenUse 开发" && item.category == "编程"));

        let fallback = db.delete_category("学习").expect("delete builtin category");
        assert_ne!(fallback, "学习");
        assert_ne!(fallback, "离开");
        assert!(db.rename_category("离开", "休息").is_err());
        assert!(db.delete_category("离开").is_err());
        drop(db);

        let reopened = AppDb::open_in(data_dir.clone()).expect("reopen test database");
        let names = reopened
            .list_categories()
            .expect("list reopened categories")
            .into_iter()
            .map(|item| item.name)
            .collect::<Vec<_>>();
        assert!(names.iter().any(|name| name == "编程"));
        assert!(!names.iter().any(|name| name == "开发"));
        assert!(!names.iter().any(|name| name == "学习"));
        drop(reopened);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn generic_chat_app_is_not_assigned_to_the_latest_project() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-generic-chat-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let session = classification::ingest_event(&db, &chat_event("chat-generic", "ChatGPT"))
            .expect("classify")
            .expect("session");
        assert!(session.project_id.is_none());
        assert!(session.task_id.is_none());
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn same_names_are_scoped_by_category_and_project() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-same-name-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let first_category = db.create_category("分类甲").expect("create first category");
        let second_category = db
            .create_category("分类乙")
            .expect("create second category");
        let first_project = db
            .create_project("同名项目", &first_category.name)
            .expect("create first project");
        let second_project = db
            .create_project("同名项目", &second_category.name)
            .expect("create second project");

        assert_ne!(first_project.id, second_project.id);
        assert!(db.create_project("同名项目", &first_category.name).is_err());

        let first_task = db
            .create_task(&first_project.id, "同名任务")
            .expect("create first task");
        let second_task = db
            .create_task(&second_project.id, "同名任务")
            .expect("create second task");
        assert_ne!(first_task.id, second_task.id);
        assert!(db.create_task(&first_project.id, "同名任务").is_err());

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn chat_title_uses_project_and_task_context_instead_of_app_name() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-chat-context-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db.create_project("ICPC", "开发").expect("create project");
        let task = db
            .create_task(&project.id, "网站开发")
            .expect("create task");
        let session = classification::ingest_event(
            &db,
            &chat_event("chat-icpc", "ICPC · icpc-trainer 网站开发"),
        )
        .expect("classify")
        .expect("session");
        assert_eq!(session.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(session.task_id.as_deref(), Some(task.id.as_str()));
        assert_eq!(session.category, "开发");
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn context_pin_handles_apps_with_no_visible_context() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-context-pin-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db.create_project("ICPC", "开发").expect("create project");
        let task = db
            .create_task(&project.id, "网站开发")
            .expect("create task");
        db.pin_context(&project.id, Some(&task.id), 30)
            .expect("pin context");
        let session = classification::ingest_event(&db, &chat_event("chat-pinned", "ChatGPT"))
            .expect("classify")
            .expect("session");
        assert_eq!(session.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(session.task_id.as_deref(), Some(task.id.as_str()));
        assert_eq!(session.confidence, 0.98);
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn corrected_chat_context_learns_scoped_keywords() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-chat-rule-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db.create_project("ICPC", "开发").expect("create project");
        let task = db
            .create_task(&project.id, "网站开发")
            .expect("create task");
        let first = classification::ingest_event(&db, &chat_event("chat-corrected", "ChatGPT"))
            .expect("classify")
            .expect("session");
        db.update_session(
            &first.id,
            SessionPatch {
                summary: Some("ICPC 网站开发".into()),
                project_id: Some(project.id.clone()),
                task_id: Some(task.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: Some("开发".into()),
                confidence: Some(0.98),
                user_confirmed: Some(true),
            },
        )
        .expect("correct session");
        db.learn_rule_from_session(&first.id, Some("ICPC, icpc-trainer"))
            .expect("learn scoped rule");

        let second =
            classification::ingest_event(&db, &chat_event("chat-rule-hit", "ICPC trainer 对话"))
                .expect("classify learned context")
                .expect("session");
        assert_eq!(second.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(second.task_id.as_deref(), Some(task.id.as_str()));
        assert_eq!(second.category, "开发");
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn specific_correction_rule_beats_an_older_generic_rule() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-specific-rule-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let internship = db
            .create_project("校内实习", "学习")
            .expect("create internship");
        let meeting = db
            .create_task(&internship.id, "会议")
            .expect("create meeting task");
        let research = db.create_project("科研", "学习").expect("create research");
        let iot = db
            .create_task(&research.id, "IOT")
            .expect("create IOT task");
        let base = Utc::now() + Duration::minutes(5);
        {
            let conn = db.conn.lock();
            conn.execute(
                "INSERT INTO attribution_rules(id,name,priority,matcher_json,project_id,task_id,category,created_from_correction,enabled,updated_at)
                 VALUES ('generic-meeting','旧会议规则',90,?1,?2,?3,'学习',1,1,?4)",
                params![serde_json::json!({"keywords": ["会议", "腾讯会议"]}).to_string(), internship.id, meeting.id, fmt(base - Duration::days(1))],
            ).expect("insert generic rule");
            conn.execute(
                "INSERT INTO attribution_rules(id,name,priority,matcher_json,project_id,task_id,category,created_from_correction,enabled,updated_at)
                 VALUES ('specific-iot','IOT 会议规则',90,?1,?2,?3,'学习',1,1,?4)",
                params![serde_json::json!({"keywords": ["申书豪预定的会议", "IOT"]}).to_string(), research.id, iot.id, fmt(base)],
            ).expect("insert specific rule");
        }
        let session = classification::ingest_event(
            &db,
            &context_event(
                "specific-meeting",
                "WeMeetApp.exe",
                "申书豪预定的会议",
                base + Duration::seconds(1),
            ),
        )
        .expect("classify meeting")
        .expect("meeting session");
        assert_eq!(session.project_id.as_deref(), Some(research.id.as_str()));
        assert_eq!(session.task_id.as_deref(), Some(iot.id.as_str()));
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn category_only_rule_still_resolves_a_concrete_project_and_task() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-category-only-rule-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db
            .create_project("icpc-trainer", "开发")
            .expect("create project");
        let task = db
            .create_task(&project.id, "开发与测试")
            .expect("create task");
        db.conn.lock().execute(
            "INSERT INTO attribution_rules(id,name,priority,matcher_json,project_id,task_id,category,created_from_correction,enabled,updated_at)
             VALUES ('category-only-icpc','旧分类规则',90,?1,NULL,NULL,'杂务',1,1,?2)",
            params![serde_json::json!({
                "app": "chrome",
                "keywords": ["icpc-trainer — 中文竞赛训练工作台 - Google Chrome"]
            }).to_string(), now()],
        ).expect("insert category-only rule");

        let session = classification::ingest_event(
            &db,
            &context_event(
                "category-only-icpc-event",
                "chrome.exe",
                "icpc-trainer — 中文竞赛训练工作台 - Google Chrome",
                Utc::now() + Duration::minutes(5),
            ),
        )
        .expect("classify category-only match")
        .expect("work session");

        assert_eq!(session.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(session.task_id.as_deref(), Some(task.id.as_str()));
        assert_eq!(session.category, "开发");
        assert_eq!(session.source, "context-complete");
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn repeated_confirmed_context_repairs_matching_unconfirmed_sessions() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-context-memory-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let wrong_project = db
            .create_project("校内实习", "学习")
            .expect("create wrong project");
        let wrong_task = db
            .create_task(&wrong_project.id, "会议")
            .expect("create wrong task");
        let right_project = db
            .create_project("科研", "学习")
            .expect("create right project");
        let right_task = db
            .create_task(&right_project.id, "IOT")
            .expect("create right task");
        db.conn.lock().execute(
            "INSERT INTO attribution_rules(id,name,priority,matcher_json,project_id,task_id,category,created_from_correction,enabled,updated_at)
             VALUES ('meeting-default','会议默认规则',90,?1,?2,?3,'学习',1,1,?4)",
            params![serde_json::json!({"keywords": ["申书豪预定的会议"]}).to_string(), wrong_project.id, wrong_task.id, now()],
        ).expect("insert wrong rule");
        let base = Utc::now() + Duration::minutes(10);
        let mut sessions = Vec::new();
        for index in 0..4 {
            sessions.push(
                classification::ingest_event(
                    &db,
                    &context_event(
                        &format!("remembered-meeting-{index}"),
                        "WeMeetApp.exe",
                        "申书豪预定的会议",
                        base + Duration::seconds(index * 10),
                    ),
                )
                .expect("classify meeting")
                .expect("meeting session"),
            );
        }
        for session in sessions.iter().take(3) {
            db.update_session(
                &session.id,
                SessionPatch {
                    summary: None,
                    project_id: Some(right_project.id.clone()),
                    task_id: Some(right_task.id.clone()),
                    clear_project: Some(false),
                    clear_task: Some(false),
                    category: Some("学习".into()),
                    confidence: Some(0.98),
                    user_confirmed: Some(true),
                },
            )
            .expect("confirm right assignment");
        }
        assert_eq!(
            db.repair_sessions_from_confirmed_context()
                .expect("repair contexts"),
            1
        );
        let repaired = db
            .get_session(&sessions[3].id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(
            repaired.project_id.as_deref(),
            Some(right_project.id.as_str())
        );
        assert_eq!(repaired.task_id.as_deref(), Some(right_task.id.as_str()));
        assert_eq!(repaired.source, "context-memory");
        assert!(!repaired.user_confirmed);
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn confirmed_task_context_follows_across_apps_but_yields_to_strong_context() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-cross-app-context-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let category = db.create_category("保研").expect("create category");
        let project = db
            .create_project("推免", &category.name)
            .expect("create project");
        let task = db
            .create_task(&project.id, "成果填报")
            .expect("create task");
        let other_project = db
            .create_project("ScreenUse 专项", "开发")
            .expect("create other project");
        let other_task = db
            .create_task(&other_project.id, "开发与测试")
            .expect("create other task");
        let base = Utc::now() + Duration::minutes(5);

        let first = classification::ingest_event(
            &db,
            &context_event("context-anchor", "chrome.exe", "教务系统", base),
        )
        .expect("ingest anchor")
        .expect("anchor session");
        db.update_session(
            &first.id,
            SessionPatch {
                summary: Some("预推免成果填报".into()),
                project_id: Some(project.id.clone()),
                task_id: Some(task.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: Some(category.name.clone()),
                confidence: Some(0.98),
                user_confirmed: Some(true),
            },
        )
        .expect("correct anchor");

        let qq = classification::ingest_event(
            &db,
            &context_event("context-qq", "QQ.exe", "QQ", base + Duration::seconds(10)),
        )
        .expect("ingest qq")
        .expect("qq session");
        assert_eq!(qq.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(qq.task_id.as_deref(), Some(task.id.as_str()));
        assert_eq!(qq.category, category.name);

        let wps = classification::ingest_event(
            &db,
            &context_event(
                "context-wps",
                "wps.exe",
                "证明材料.pdf",
                base + Duration::seconds(20),
            ),
        )
        .expect("ingest wps")
        .expect("wps session");
        assert_eq!(wps.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(wps.task_id.as_deref(), Some(task.id.as_str()));

        let strong = classification::ingest_event(
            &db,
            &context_event(
                "context-strong",
                "ChatGPT.exe",
                "ScreenUse 专项",
                base + Duration::seconds(30),
            ),
        )
        .expect("ingest strong context")
        .expect("strong session");
        assert_eq!(
            strong.project_id.as_deref(),
            Some(other_project.id.as_str())
        );
        assert_eq!(strong.task_id.as_deref(), Some(other_task.id.as_str()));
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn explicit_project_page_overrides_an_old_wrong_personal_memory() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-explicit-over-memory-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let old_project = db.create_project("IOT", "开发").expect("old project");
        let old_task = db
            .create_task(&old_project.id, "漏洞复现")
            .expect("old task");
        let exact_project = db
            .create_project("ScreenUse 专项", "开发")
            .expect("exact project");
        let exact_task = db
            .create_task(&exact_project.id, "开发与测试")
            .expect("exact task");
        let base = Utc::now() + Duration::minutes(20);
        let source = classification::ingest_event(
            &db,
            &context_event("wrong-memory-source", "ChatGPT.exe", "ScreenUse 专项", base),
        )
        .expect("ingest memory source")
        .expect("memory source");
        db.apply_session_correction(
            std::slice::from_ref(&source.id),
            SessionPatch {
                summary: None,
                project_id: Some(old_project.id.clone()),
                task_id: Some(old_task.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: Some("开发".into()),
                confidence: Some(0.98),
                user_confirmed: Some(true),
            },
            false,
            None,
            None,
        )
        .expect("store old wrong memory");

        let target = classification::ingest_event(
            &db,
            &context_event(
                "explicit-project-target",
                "wps.exe",
                "ScreenUse 专项",
                base + Duration::minutes(2),
            ),
        )
        .expect("ingest explicit target")
        .expect("explicit target");
        assert_eq!(
            target.project_id.as_deref(),
            Some(exact_project.id.as_str())
        );
        assert_eq!(target.task_id.as_deref(), Some(exact_task.id.as_str()));

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn one_manual_correction_repairs_adjacent_weak_sessions_until_idle() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-context-propagation-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let category = db.create_category("保研").expect("create category");
        let project = db
            .create_project("推免", &category.name)
            .expect("create project");
        let task = db
            .create_task(&project.id, "成果填报")
            .expect("create task");
        let base = Utc::now() + Duration::minutes(10);
        let first = classification::ingest_event(
            &db,
            &context_event("repair-explorer", "explorer.exe", "材料", base),
        )
        .expect("ingest explorer")
        .expect("explorer session");
        db.conn
            .lock()
            .execute(
                "UPDATE work_sessions SET summary='离开/空闲' WHERE id=?1",
                params![&first.id],
            )
            .expect("mark stale idle summary");
        let anchor = classification::ingest_event(
            &db,
            &context_event(
                "repair-chrome",
                "chrome.exe",
                "教务系统",
                base + Duration::seconds(10),
            ),
        )
        .expect("ingest chrome")
        .expect("chrome session");
        let third = classification::ingest_event(
            &db,
            &context_event("repair-qq", "QQ.exe", "QQ", base + Duration::seconds(20)),
        )
        .expect("ingest qq")
        .expect("qq session");
        let wrong_project = db
            .create_project("日常沟通", "沟通")
            .expect("create wrong project");
        let wrong_task = db
            .create_task(&wrong_project.id, "QQ")
            .expect("create wrong task");
        db.conn
            .lock()
            .execute(
                "UPDATE work_sessions
                 SET project_id=?1,task_id=?2,category='沟通',confidence=0.88,source='context-complete'
                 WHERE id=?3",
                params![wrong_project.id, wrong_task.id, third.id],
            )
            .expect("assign weak wrong task");
        let mut idle_event =
            context_event("repair-idle", "QQ.exe", "QQ", base + Duration::seconds(30));
        idle_event.input_stats.idle_seconds = 180;
        classification::ingest_event(&db, &idle_event).expect("ingest idle");
        let after_idle = classification::ingest_event(
            &db,
            &context_event(
                "repair-after-idle",
                "QQ.exe",
                "QQ",
                base + Duration::seconds(40),
            ),
        )
        .expect("ingest after idle")
        .expect("after idle session");

        db.update_session(
            &anchor.id,
            SessionPatch {
                summary: Some("预推免成果填报".into()),
                project_id: Some(project.id.clone()),
                task_id: Some(task.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: Some(category.name.clone()),
                confidence: Some(0.98),
                user_confirmed: Some(true),
            },
        )
        .expect("correct anchor");
        let changed = db
            .propagate_context_from_sessions(&[anchor.id])
            .expect("propagate correction");
        assert_eq!(changed, 1);
        let idle_before = db
            .get_session(&first.id)
            .expect("load idle boundary")
            .expect("idle boundary");
        assert_eq!(idle_before.source, "collector-idle");
        assert_eq!(idle_before.summary, "离开/空闲");
        assert!(idle_before.task_id.is_none());

        let repaired = db
            .get_session(&third.id)
            .expect("load repaired")
            .expect("session");
        assert_eq!(repaired.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(repaired.task_id.as_deref(), Some(task.id.as_str()));
        assert!(!repaired.user_confirmed);
        let untouched = db
            .get_session(&after_idle.id)
            .expect("load after idle")
            .expect("after idle");
        assert_ne!(untouched.task_id.as_deref(), Some(task.id.as_str()));
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn learned_task_rule_uses_page_title_across_apps_and_is_upserted() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-cross-app-rule-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let category = db.create_category("保研").expect("create category");
        let project = db
            .create_project("推免", &category.name)
            .expect("create project");
        let task = db
            .create_task(&project.id, "成果填报")
            .expect("create task");
        let base = Utc::now() + Duration::minutes(15);
        let mut first_event = context_event(
            "rule-page",
            "chrome.exe",
            "湖北大学楚才学院 - Google Chrome",
            base,
        );
        first_event.metadata["activePageTitle"] =
            serde_json::Value::String("预推免成果填报".into());
        let first = classification::ingest_event(&db, &first_event)
            .expect("ingest page")
            .expect("page session");
        db.update_session(
            &first.id,
            SessionPatch {
                summary: Some("预推免成果填报".into()),
                project_id: Some(project.id.clone()),
                task_id: Some(task.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: Some(category.name.clone()),
                confidence: Some(0.98),
                user_confirmed: Some(true),
            },
        )
        .expect("correct page");
        let learned = db
            .learn_rule_from_session(&first.id, Some("成果填报,预推免"))
            .expect("learn rule");
        let learned_again = db
            .learn_rule_from_session(&first.id, Some("成果填报,预推免"))
            .expect("upsert rule");
        assert_eq!(learned.id, learned_again.id);
        assert!(learned.matcher.get("app").is_none());
        assert!(learned
            .matcher
            .get("keywords")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|keywords| keywords.iter().any(|value| value == "预推免成果填报")));

        let wps = classification::ingest_event(
            &db,
            &context_event(
                "rule-wps",
                "wps.exe",
                "成果填报证明材料.pdf",
                base + Duration::minutes(10),
            ),
        )
        .expect("ingest cross app rule")
        .expect("wps session");
        assert_eq!(wps.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(wps.task_id.as_deref(), Some(task.id.as_str()));
        let count = db
            .conn
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM attribution_rules WHERE project_id=?1 AND task_id=?2 AND created_from_correction=1",
                params![project.id, task.id],
                |row| row.get::<_, i64>(0),
            )
            .expect("count rules");
        assert_eq!(count, 1);
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn correction_rule_normalization_merges_app_duplicates_and_keeps_page_memory() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-rule-normalization-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let category = db.create_category("保研").expect("create category");
        let project = db
            .create_project("推免", &category.name)
            .expect("create project");
        let task = db
            .create_task(&project.id, "成果填报")
            .expect("create task");
        assert!(context_memory_keyword("Program Manager").is_none());
        assert!(context_memory_keyword("任务切换").is_none());
        let base = Utc::now() + Duration::minutes(20);
        let pages = [
            ("memory-chrome", "chrome.exe", "湖北大学推免填报系统"),
            ("memory-wps", "wps.exe", "推免成果证明材料.pdf"),
        ];
        for (offset, (id, app, page)) in pages.iter().enumerate() {
            let mut event =
                context_event(id, app, page, base + Duration::seconds(offset as i64 * 60));
            event.metadata["activePageTitle"] = serde_json::Value::String((*page).into());
            let session = classification::ingest_event(&db, &event)
                .expect("ingest memory page")
                .expect("memory session");
            db.update_session(
                &session.id,
                SessionPatch {
                    summary: Some("预推免成果填报".into()),
                    project_id: Some(project.id.clone()),
                    task_id: Some(task.id.clone()),
                    clear_project: Some(false),
                    clear_task: Some(false),
                    category: Some(category.name.clone()),
                    confidence: Some(0.98),
                    user_confirmed: Some(true),
                },
            )
            .expect("correct memory page");
            db.learn_rule_from_session(&session.id, Some("成果填报,预推免"))
                .expect("learn memory rule");
        }
        let before = db
            .conn
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM attribution_rules WHERE project_id=?1 AND task_id=?2 AND created_from_correction=1",
                params![project.id, task.id],
                |row| row.get::<_, i64>(0),
            )
            .expect("count rules before normalization");
        assert_eq!(before, 1);
        db.normalize_correction_rules().expect("normalize rules");
        let conn = db.conn.lock();
        let (after, matcher_json) = conn
            .query_row(
                "SELECT COUNT(*),MAX(matcher_json) FROM attribution_rules WHERE project_id=?1 AND task_id=?2 AND created_from_correction=1",
                params![project.id, task.id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("load normalized rule");
        assert_eq!(after, 1);
        let matcher: serde_json::Value =
            serde_json::from_str(&matcher_json).expect("parse normalized matcher");
        assert!(matcher.get("app").is_none());
        let keywords = matcher
            .get("keywords")
            .and_then(serde_json::Value::as_array)
            .expect("normalized keywords");
        assert!(pages
            .iter()
            .all(|(_, _, page)| keywords.iter().any(|value| value == page)));
        drop(conn);
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn heartbeats_extend_one_block_and_context_start_creates_only_one_more() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-heartbeat-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let initial_count = db.list_sessions(500).expect("initial sessions").len();
        let base = Utc::now() + Duration::seconds(2);
        let make_event = |id: &str,
                          title: &str,
                          timestamp: chrono::DateTime<Utc>,
                          metadata: serde_json::Value| {
            RawActivityEvent {
                id: id.into(),
                source: "test-observer".into(),
                timestamp: fmt(timestamp),
                app: Some("Acrobat.exe".into()),
                window_title: Some(title.into()),
                url: None,
                file_path: None,
                workspace: None,
                input_stats: InputStats::default(),
                metadata,
            }
        };

        db.ingest_raw_event(make_event(
            "stream-1",
            "paper.pdf",
            base,
            serde_json::json!({"contextStart": true}),
        ))
        .expect("start context");
        let first = db.list_sessions(1).expect("first block")[0].clone();
        db.ingest_raw_event(make_event(
            "stream-1",
            "paper.pdf",
            base + Duration::seconds(10),
            serde_json::json!({"heartbeat": true}),
        ))
        .expect("extend context");

        let after_heartbeat = db.list_sessions(500).expect("sessions after heartbeat");
        assert_eq!(after_heartbeat.len(), initial_count + 1);
        let extended = db
            .get_session(&first.id)
            .expect("load first")
            .expect("first exists");
        assert_eq!(extended.started_at, fmt(base));
        assert_eq!(extended.ended_at, fmt(base + Duration::seconds(10)));
        assert_ne!(extended.source, "context-complete");

        db.mark_session_awaiting_confirmation(&first.id)
            .expect("mark completed");
        assert_eq!(
            db.get_session(&first.id)
                .expect("load completed")
                .expect("completed exists")
                .source,
            "context-complete",
        );

        db.ingest_raw_event(RawActivityEvent {
            id: "stream-2".into(),
            source: "test-observer".into(),
            timestamp: fmt(base + Duration::seconds(20)),
            app: Some("chrome.exe".into()),
            window_title: Some("Google Scholar".into()),
            url: Some("https://scholar.google.com/".into()),
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: serde_json::json!({"contextStart": true}),
        })
        .expect("start second context");
        assert_eq!(
            db.list_sessions(500).expect("final sessions").len(),
            initial_count + 2
        );

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn fast_heartbeat_extends_the_known_session_without_reclassification() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-fast-heartbeat-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let base = Utc::now() + Duration::seconds(2);
        let mut event = RawActivityEvent {
            id: "fast-stream".into(),
            source: "test-observer".into(),
            timestamp: fmt(base),
            app: Some("Code.exe".into()),
            window_title: Some("ScreenUse - Visual Studio Code".into()),
            url: None,
            file_path: Some("src/App.tsx".into()),
            workspace: Some("ScreenUse".into()),
            input_stats: InputStats::default(),
            metadata: serde_json::json!({"contextStart": true}),
        };
        db.ingest_raw_event(event.clone()).expect("start context");
        let session = db.list_sessions(1).expect("load active session")[0].clone();

        event.metadata = serde_json::json!({"heartbeat": true});
        for index in 1..=1000 {
            event.timestamp = fmt(base + Duration::seconds(index * 5));
            db.heartbeat_raw_event(&event, &session.id)
                .expect("extend known session");
        }

        let extended = db
            .get_session(&session.id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(extended.started_at, fmt(base));
        assert_eq!(extended.ended_at, fmt(base + Duration::seconds(5000)));
        assert_eq!(extended.summary, session.summary);
        let conn = db.conn.lock();
        let raw_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM raw_events WHERE id='fast-stream'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw_rows, 1);
        assert_eq!(
            conn.pragma_query_value(None, "wal_autocheckpoint", |row| row.get::<_, i64>(0))
                .unwrap(),
            256
        );
        drop(conn);
        let database_bytes = fs::metadata(data_dir.join("screenuse.db"))
            .map(|item| item.len())
            .unwrap_or_default();
        let wal_bytes = fs::metadata(data_dir.join("screenuse.db-wal"))
            .map(|item| item.len())
            .unwrap_or_default();
        assert!(database_bytes < 2 * 1024 * 1024);
        assert!(wal_bytes <= 2 * 1024 * 1024);

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn collector_does_not_merge_identical_titles_across_projects() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-project-boundary-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let first_project = db.create_project("项目甲", "开发").expect("create project");
        let second_project = db.create_project("项目乙", "开发").expect("create project");
        let first_task = db
            .create_task(&first_project.id, "同名任务")
            .expect("create task");
        let second_task = db
            .create_task(&second_project.id, "同名任务")
            .expect("create task");
        let start = Utc::now();

        db.pin_context(&first_project.id, Some(&first_task.id), 30)
            .expect("pin first context");
        let mut first = chat_event("project-boundary-1", "ChatGPT");
        first.timestamp = fmt(start);
        first.metadata = serde_json::json!({});
        db.ingest_raw_event(first).expect("ingest first event");

        db.pin_context(&second_project.id, Some(&second_task.id), 30)
            .expect("pin second context");
        let mut second = chat_event("project-boundary-2", "ChatGPT");
        second.timestamp = fmt(start + Duration::seconds(1));
        second.metadata = serde_json::json!({});
        db.ingest_raw_event(second).expect("ingest second event");

        let collected = db
            .list_sessions(20)
            .expect("list sessions")
            .into_iter()
            .filter(|session| session.source == "collector-rule")
            .collect::<Vec<_>>();
        assert_eq!(collected.len(), 2);
        assert!(collected
            .iter()
            .any(|session| session.project_id.as_deref() == Some(first_project.id.as_str())));
        assert!(collected
            .iter()
            .any(|session| session.project_id.as_deref() == Some(second_project.id.as_str())));

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn idle_sessions_use_the_configured_category_and_project() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-idle-target-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let mut event = RawActivityEvent {
            id: "idle-target".into(),
            source: "windows-foreground".into(),
            timestamp: now(),
            app: Some("QQ.exe".into()),
            window_title: Some("QQ".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats {
                idle_seconds: 180,
                ..Default::default()
            },
            metadata: serde_json::json!({ "contextStart": true }),
        };
        db.ingest_raw_event(event.clone())
            .expect("record idle session");
        let first = db.list_sessions(1).expect("load idle session")[0].clone();
        assert_eq!(first.category, "无效");
        assert_eq!(first.project_name.as_deref(), Some("离开"));
        assert_eq!(first.source, "collector-idle");

        let mut settings = db.get_settings().expect("load settings");
        settings.idle_category = "休息".into();
        settings.idle_project_name = "暂离".into();
        db.configure_idle_target(&settings)
            .expect("configure custom idle target");
        db.save_settings(&settings)
            .expect("save custom idle target");
        event.id = "idle-target-2".into();
        event.timestamp = fmt(Utc::now() + Duration::seconds(10));
        db.ingest_raw_event(event)
            .expect("record custom idle session");
        let latest = db.list_sessions(1).expect("load custom idle session")[0].clone();
        assert_eq!(latest.category, "休息");
        assert_eq!(latest.project_name.as_deref(), Some("暂离"));
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn idle_context_with_a_project_like_page_cannot_be_reclassified_as_work() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-idle-project-title-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db
            .create_project("ScreenUse 专项", "开发")
            .expect("create project");
        db.create_task(&project.id, "开发与测试")
            .expect("create task");
        let mut event = context_event(
            "idle-project-title",
            "ChatGPT.exe",
            "ScreenUse 专项",
            Utc::now() + Duration::hours(1),
        );
        event.input_stats.idle_seconds = 300;
        let idle = classification::ingest_event(&db, &event)
            .expect("ingest idle context")
            .expect("idle session");
        assert_eq!(idle.source, "collector-idle");
        assert_eq!(idle.summary, "离开/空闲");
        assert_eq!(idle.category, "无效");
        assert!(idle.task_id.is_none());

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn finalized_idle_context_keeps_its_idle_source_and_rejects_ai_overwrite() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-finalized-idle-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db
            .create_project("工作项目", "开发")
            .expect("create project");
        let task = db.create_task(&project.id, "开发").expect("create task");
        let mut event = context_event(
            "finalized-idle",
            "ChatGPT.exe",
            "工作项目",
            Utc::now() + Duration::hours(1),
        );
        let session = classification::ingest_event(&db, &event)
            .expect("ingest active context")
            .expect("active session");
        event.input_stats.idle_seconds = 300;
        let idle = classification::finalize_context(&db, &event, &session.id)
            .expect("finalize idle context")
            .expect("idle session");
        assert_eq!(idle.source, "collector-idle");
        assert_eq!(idle.summary, "离开/空闲");
        assert!(idle.task_id.is_none());

        let after_ai = db
            .apply_ai_review(
                &idle.id,
                SessionPatch {
                    summary: Some("错误工作归类".into()),
                    project_id: Some(project.id),
                    task_id: Some(task.id),
                    clear_project: Some(false),
                    clear_task: Some(false),
                    category: Some("开发".into()),
                    confidence: Some(0.99),
                    user_confirmed: Some(false),
                },
                vec![],
            )
            .expect("ignore AI overwrite");
        assert_eq!(after_ai.source, "collector-idle");
        assert_eq!(after_ai.summary, "离开/空闲");
        assert!(after_ai.task_id.is_none());

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn ai_idle_repair_reads_only_idle_review_targets() {
        let settings = AppSettings::default();
        let prompt = r#"复核输入：{"reviewItems":[{"targetSession":{"sessionId":"work","summary":"工作","source":"context-complete","category":"开发","projectName":"项目","taskId":"task"}},{"targetSession":{"sessionId":"idle","summary":"离开/空闲","source":"context-complete","category":"无效","projectName":"离开","taskId":null}}],"timelineContext":[{"sessionId":"context-idle","summary":"离开/空闲"}]}"#;
        let ids = ai_prompt_idle_session_ids(prompt, &settings);
        assert_eq!(ids, HashSet::from(["idle".to_string()]));
    }

    #[test]
    fn short_automatic_session_is_absorbed_without_a_gap() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-short-block-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let base = Utc::now() + Duration::hours(1);
        let evidence = serde_json::to_string(&Vec::<EvidenceItem>::new()).unwrap();
        {
            let conn = db.conn.lock();
            conn.execute("INSERT INTO work_sessions VALUES ('before',?1,?2,NULL,NULL,'杂务','前一个事务',0.8,?3,0,'context-complete',?4)", params![fmt(base), fmt(base + Duration::seconds(10)), evidence, now()]).expect("insert previous session");
            conn.execute("INSERT INTO work_sessions VALUES ('short',?1,?2,NULL,NULL,'沟通','短暂切换',0.8,?3,0,'context-complete',?4)", params![fmt(base + Duration::seconds(10)), fmt(base + Duration::seconds(13)), evidence, now()]).expect("insert short session");
        }
        let absorbed = db
            .absorb_short_auto_session("short")
            .expect("absorb short session");
        assert_eq!(absorbed.id, "before");
        assert_eq!(absorbed.ended_at, fmt(base + Duration::seconds(13)));
        assert!(db
            .get_session("short")
            .expect("load short session")
            .is_none());
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn nearby_idle_sessions_are_joined_across_a_five_second_sampling_gap() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-idle-gap-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let settings = db.get_settings().expect("load settings");
        let project_id = db
            .configure_idle_target(&settings)
            .expect("load idle project");
        let base = Utc::now() + Duration::hours(1);
        let evidence = serde_json::to_string(&Vec::<EvidenceItem>::new()).unwrap();
        {
            let conn = db.conn.lock();
            conn.execute("INSERT INTO work_sessions VALUES ('idle-a',?1,?2,?3,NULL,'无效','离开/空闲',0.99,?4,0,'collector-idle',?5)", params![fmt(base), fmt(base + Duration::seconds(30)), project_id, evidence, now()]).expect("insert first idle session");
            conn.execute("INSERT INTO work_sessions VALUES ('idle-b',?1,?2,?3,NULL,'无效','离开/空闲',0.99,?4,0,'collector-idle',?5)", params![fmt(base + Duration::seconds(34)), fmt(base + Duration::seconds(60)), project_id, evidence, now()]).expect("insert second idle session");
        }
        db.compact_sessions().expect("compact idle sessions");
        let merged = db
            .get_session("idle-a")
            .expect("load idle session")
            .expect("idle session exists");
        assert_eq!(merged.ended_at, fmt(base + Duration::seconds(60)));
        assert!(db
            .get_session("idle-b")
            .expect("load second idle session")
            .is_none());
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn dashboard_keeps_more_than_eighty_daily_segments() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-dashboard-session-limit-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let base = Utc::now() + Duration::hours(1);
        let evidence = serde_json::to_string(&Vec::<EvidenceItem>::new()).unwrap();
        let conn = db.conn.lock();
        for index in 0..120 {
            let start = base + Duration::seconds(index * 10);
            conn.execute("INSERT INTO work_sessions VALUES (?1,?2,?3,NULL,NULL,'杂务',?4,0.8,?5,0,'context-complete',?6)", params![format!("many-{index}"), fmt(start), fmt(start + Duration::seconds(10)), format!("事务 {index}"), evidence, now()]).expect("insert dashboard session");
        }
        drop(conn);
        assert!(db.dashboard(false).expect("load dashboard").sessions.len() >= 120);
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn session_corrections_keep_category_project_and_task_consistent() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-assignment-invariant-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let development = db
            .create_project("开发项目", "开发")
            .expect("create development project");
        let study = db
            .create_project("学习项目", "学习")
            .expect("create study project");
        let study_task = db
            .create_task(&study.id, "阅读资料")
            .expect("create study task");
        let session_id = db.list_sessions(1).expect("list sessions")[0].id.clone();

        let corrected = db
            .update_session(
                &session_id,
                SessionPatch {
                    summary: Some("跨项目修正".into()),
                    project_id: Some(development.id),
                    task_id: Some(study_task.id.clone()),
                    clear_project: Some(false),
                    clear_task: Some(false),
                    category: Some("开发".into()),
                    confidence: Some(1.2),
                    user_confirmed: Some(true),
                },
            )
            .expect("correct with task");
        assert_eq!(corrected.project_id.as_deref(), Some(study.id.as_str()));
        assert_eq!(corrected.task_id.as_deref(), Some(study_task.id.as_str()));
        assert_eq!(corrected.category, "学习");
        assert_eq!(corrected.confidence, 1.0);
        assert_eq!(corrected.source, "manual-correction");

        let category_only = db
            .update_session(
                &session_id,
                SessionPatch {
                    summary: None,
                    project_id: None,
                    task_id: None,
                    clear_project: Some(false),
                    clear_task: Some(false),
                    category: Some("开发".into()),
                    confidence: None,
                    user_confirmed: Some(true),
                },
            )
            .expect("change category only");
        assert_eq!(category_only.category, "开发");
        assert!(category_only.project_id.is_none());
        assert!(category_only.task_id.is_none());

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn timeline_repair_removes_stale_rows_and_resolves_automatic_overlaps() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-timeline-repair-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let base = Utc::now() + Duration::hours(2);
        let evidence = serde_json::to_string(&vec![EvidenceItem {
            kind: "app".into(),
            label: "应用".into(),
            value: "QQ.exe".into(),
            weight: 0.5,
        }])
        .expect("serialize evidence");
        {
            let conn = db.conn.lock();
            conn.execute(
                "INSERT INTO work_sessions VALUES ('stale',?1,?1,NULL,NULL,'沟通','QQ',0.8,?2,0,'collector-rule',?3)",
                params![fmt(base), evidence, now()],
            )
            .expect("insert stale row");
            conn.execute(
                "INSERT INTO work_sessions VALUES ('confirmed-zero',?1,?1,NULL,NULL,'沟通','手工保留',1.0,?2,1,'manual-correction',?3)",
                params![fmt(base - Duration::seconds(1)), evidence, now()],
            )
            .expect("insert confirmed zero row");
            conn.execute(
                "INSERT INTO work_sessions VALUES ('overlap-a',?1,?2,NULL,NULL,'沟通','QQ',0.8,?3,0,'context-complete',?4)",
                params![fmt(base), fmt(base + Duration::seconds(20)), evidence, now()],
            )
            .expect("insert first overlap");
            conn.execute(
                "INSERT INTO work_sessions VALUES ('overlap-b',?1,?2,NULL,NULL,'沟通','QQ',0.9,?3,0,'context-complete',?4)",
                params![fmt(base + Duration::seconds(15)), fmt(base + Duration::seconds(30)), evidence, now()],
            )
            .expect("insert compatible overlap");
            conn.execute(
                "INSERT INTO work_sessions VALUES ('next-context',?1,?2,NULL,NULL,'开发','ScreenUse',0.9,?3,0,'context-complete',?4)",
                params![fmt(base + Duration::seconds(28)), fmt(base + Duration::seconds(40)), evidence, now()],
            )
            .expect("insert incompatible overlap");
        }

        assert!(db.repair_session_timeline().expect("repair timeline") >= 3);
        assert!(db.get_session("stale").expect("load stale").is_none());
        assert!(db
            .get_session("confirmed-zero")
            .expect("load confirmed zero")
            .is_some());
        let first = db
            .get_session("overlap-a")
            .expect("load first overlap")
            .expect("first overlap remains");
        assert_eq!(first.ended_at, fmt(base + Duration::seconds(28)));
        assert!(db
            .get_session("overlap-b")
            .expect("load merged overlap")
            .is_none());
        assert_eq!(
            db.get_session("next-context")
                .expect("load next context")
                .expect("next context remains")
                .started_at,
            fmt(base + Duration::seconds(28))
        );

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn manual_merge_is_atomic_contiguous_and_updates_plan_links() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-manual-merge-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let base = Utc::now() + Duration::hours(3);
        let evidence = serde_json::to_string(&Vec::<EvidenceItem>::new()).unwrap();
        {
            let conn = db.conn.lock();
            conn.execute(
                "INSERT INTO work_sessions VALUES ('merge-a',?1,?2,NULL,NULL,'沟通','QQ',0.8,?3,0,'context-complete',?4)",
                params![fmt(base), fmt(base + Duration::seconds(20)), evidence, now()],
            )
            .expect("insert first merge row");
            conn.execute(
                "INSERT INTO work_sessions VALUES ('merge-b',?1,?2,NULL,NULL,'沟通','QQ',0.9,?3,0,'context-complete',?4)",
                params![fmt(base + Duration::seconds(23)), fmt(base + Duration::seconds(40)), evidence, now()],
            )
            .expect("insert second merge row");
            conn.execute(
                "INSERT INTO plan_items(id,source,title,note,start_at,due_at,status,tags_json,matched_session_ids_json,updated_at)
                 VALUES('merge-plan','manual','沟通复盘',NULL,NULL,NULL,'active','[]','[\"merge-a\",\"merge-b\"]',?1)",
                params![now()],
            )
            .expect("insert plan link");
        }

        let merged = db
            .merge_sessions(
                &["merge-b".to_string(), "merge-a".to_string()],
                Some("QQ 沟通".into()),
            )
            .expect("merge contiguous rows");
        assert_eq!(merged.started_at, fmt(base));
        assert_eq!(merged.ended_at, fmt(base + Duration::seconds(40)));
        assert_eq!(merged.source, "manual-merge");
        assert!(merged.user_confirmed);
        assert!(db.get_session("merge-a").expect("load first").is_none());
        assert!(db.get_session("merge-b").expect("load second").is_none());
        let plan_links: String = db
            .conn
            .lock()
            .query_row(
                "SELECT matched_session_ids_json FROM plan_items WHERE id='merge-plan'",
                [],
                |row| row.get(0),
            )
            .expect("load plan links");
        let plan_links: Vec<String> = parse_json(&plan_links);
        assert_eq!(plan_links, vec![merged.id]);

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn manual_session_fills_a_gap_with_a_concrete_task() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-manual-entry-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db
            .create_project("手动补录项目", "开发")
            .expect("create project");
        let task = db
            .create_task(&project.id, "补录任务")
            .expect("create task");
        let base = Utc::now() + Duration::days(30);
        let patch = SessionPatch {
            summary: Some("补录空档".into()),
            project_id: None,
            task_id: Some(task.id.clone()),
            clear_project: None,
            clear_task: None,
            category: Some("杂务".into()),
            confidence: Some(1.0),
            user_confirmed: Some(true),
        };

        let created = db
            .create_manual_session(
                &fmt(base),
                &fmt(base + Duration::seconds(20)),
                patch.clone(),
            )
            .expect("create manual session");
        assert_eq!(created.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(created.task_id.as_deref(), Some(task.id.as_str()));
        assert_eq!(created.category, "开发");
        assert_eq!(created.summary, "补录空档");
        assert_eq!(created.source, "manual-entry");
        assert!(created.user_confirmed);
        assert_eq!(created.evidence[0].value, "手动补录未记录时间");

        assert!(db
            .create_manual_session(
                &fmt(base + Duration::seconds(5)),
                &fmt(base + Duration::seconds(25)),
                patch.clone(),
            )
            .is_err());
        assert!(db
            .create_manual_session(
                &fmt(base + Duration::seconds(30)),
                &fmt(base + Duration::seconds(33)),
                patch,
            )
            .is_err());

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn manual_split_requires_two_five_second_segments() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-manual-split-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let base = Utc::now() + Duration::hours(3);
        let evidence = serde_json::to_string(&Vec::<EvidenceItem>::new()).unwrap();
        db.conn
            .lock()
            .execute(
                "INSERT INTO work_sessions VALUES ('split-source',?1,?2,NULL,NULL,'开发','ScreenUse',0.9,?3,1,'manual-correction',?4)",
                params![fmt(base), fmt(base + Duration::seconds(20)), evidence, now()],
            )
            .expect("insert split source");

        assert!(db
            .split_session("split-source", &fmt(base + Duration::seconds(3)))
            .is_err());
        assert!(db
            .get_session("split-source")
            .expect("load source after rejected split")
            .is_some());
        let split = db
            .split_session("split-source", &fmt(base + Duration::seconds(10)))
            .expect("split valid source");
        assert_eq!(split.len(), 2);
        assert!(split.iter().all(|item| item.source == "manual-split"));
        assert!(split
            .iter()
            .all(|item| session_duration_seconds(item).is_some_and(|seconds| seconds >= 5)));
        assert!(db
            .get_session("split-source")
            .expect("load removed source")
            .is_none());

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn analysis_job_keeps_a_complete_ai_audit_trail() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-analysis-job-audit-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let queued_at = now();
        db.create_analysis_job(&AnalysisJob {
            id: "audit-job".into(),
            chunk_ids: vec!["session-a".into(), "session-b".into()],
            metadata_range: TimeRange {
                started_at: "2026-07-14T10:00:00Z".into(),
                ended_at: "2026-07-14T10:02:00Z".into(),
            },
            mode: "metadata-context-review".into(),
            provider: "codex-account".into(),
            model: "gpt-5.4".into(),
            retry_count: 0,
            status: "pending".into(),
            error: None,
            system_prompt: None,
            user_prompt: None,
            response: None,
            queued_at,
            processing_started_at: None,
            completed_at: None,
            duration_ms: None,
            result_count: 0,
            usage: AiUsage::default(),
        })
        .expect("create job");

        let summaries = db.list_analysis_jobs(20).expect("list jobs");
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].system_prompt.is_none());
        assert!(summaries[0].response.is_none());

        let claimed = db
            .claim_next_analysis_job()
            .expect("claim job")
            .expect("pending job");
        assert_eq!(claimed.status, "running");
        assert!(claimed.processing_started_at.is_some());
        db.record_analysis_job_request(
            &claimed.id,
            "codex-account",
            "gpt-5.4",
            "system prompt",
            "user prompt",
        )
        .expect("record request");
        let usage = AiUsage {
            input_tokens: 1_200,
            cached_input_tokens: 500,
            output_tokens: 80,
            reasoning_output_tokens: 32,
            total_tokens: 1_280,
            cost_usd: None,
            cost_note: Some("当前 Codex 账号未返回单次金额".into()),
        };
        db.record_analysis_job_response(&claimed.id, "{\"results\":[]}", &usage)
            .expect("record response");
        db.record_analysis_job_request(
            &claimed.id,
            "codex-account",
            "gpt-5.4",
            "retry system prompt",
            "retry user prompt",
        )
        .expect("record retry request");
        let retry_usage = AiUsage {
            input_tokens: 300,
            cached_input_tokens: 100,
            output_tokens: 20,
            reasoning_output_tokens: 8,
            total_tokens: 320,
            cost_usd: None,
            cost_note: Some("当前 Codex 账号未返回单次金额".into()),
        };
        db.record_analysis_job_response(&claimed.id, "{\"results\":[{}]}", &retry_usage)
            .expect("record retry response");
        db.set_analysis_job_result_count(&claimed.id, 2)
            .expect("record result count");
        db.mark_analysis_job_status(&claimed.id, "completed", None, None)
            .expect("complete job");

        let detail = db
            .get_analysis_job(&claimed.id)
            .expect("load job")
            .expect("saved job");
        assert_eq!(detail.status, "completed");
        assert_eq!(detail.provider, "codex-account");
        assert_eq!(detail.model, "gpt-5.4");
        assert_eq!(detail.system_prompt.as_deref(), Some("retry system prompt"));
        assert_eq!(detail.user_prompt.as_deref(), Some("retry user prompt"));
        assert_eq!(detail.response.as_deref(), Some("{\"results\":[{}]}"));
        assert_eq!(detail.result_count, 2);
        assert_eq!(detail.usage.total_tokens, 1_600);
        assert_eq!(detail.usage.input_tokens, 1_500);
        assert_eq!(detail.usage.cached_input_tokens, 600);
        assert_eq!(detail.usage.output_tokens, 100);
        assert_eq!(
            detail.usage.cost_note.as_deref(),
            Some("当前 Codex 账号未返回单次金额")
        );
        assert!(detail.completed_at.is_some());
        assert!(detail.duration_ms.is_some());

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn only_skipped_analysis_jobs_can_be_deleted() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-analysis-job-delete-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let queued_at = now();
        for (id, status) in [("skipped-job", "skipped"), ("completed-job", "completed")] {
            db.create_analysis_job(&AnalysisJob {
                id: id.into(),
                chunk_ids: vec![format!("{id}-session")],
                metadata_range: TimeRange {
                    started_at: queued_at.clone(),
                    ended_at: queued_at.clone(),
                },
                mode: "metadata-context-review".into(),
                provider: if status == "completed" {
                    "codex-account".into()
                } else {
                    String::new()
                },
                model: if status == "completed" {
                    "gpt-5.4".into()
                } else {
                    String::new()
                },
                retry_count: 0,
                status: status.into(),
                error: None,
                system_prompt: None,
                user_prompt: None,
                response: None,
                queued_at: queued_at.clone(),
                processing_started_at: None,
                completed_at: Some(queued_at.clone()),
                duration_ms: None,
                result_count: 0,
                usage: AiUsage::default(),
            })
            .expect("create analysis job");
        }

        db.delete_skipped_analysis_job("skipped-job")
            .expect("delete skipped job");
        assert!(db
            .get_analysis_job("skipped-job")
            .expect("load deleted job")
            .is_none());

        let error = db
            .delete_skipped_analysis_job("completed-job")
            .expect_err("completed job must remain");
        assert!(error.to_string().contains("只能删除未调用 AI"));
        assert!(db
            .get_analysis_job("completed-job")
            .expect("load completed job")
            .is_some());

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn analysis_jobs_support_atomic_batch_delete_and_retry() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-analysis-job-batch-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let queued_at = now();
        for (id, status) in [
            ("skipped-a", "skipped"),
            ("skipped-b", "skipped"),
            ("failed-a", "failed"),
            ("downgraded-a", "downgraded"),
            ("completed-a", "completed"),
        ] {
            db.create_analysis_job(&AnalysisJob {
                id: id.into(),
                chunk_ids: vec![format!("{id}-session")],
                metadata_range: TimeRange {
                    started_at: queued_at.clone(),
                    ended_at: queued_at.clone(),
                },
                mode: "metadata-context-review".into(),
                provider: String::new(),
                model: String::new(),
                retry_count: 0,
                status: status.into(),
                error: (status == "failed").then(|| "request failed".into()),
                system_prompt: None,
                user_prompt: None,
                response: None,
                queued_at: queued_at.clone(),
                processing_started_at: None,
                completed_at: None,
                duration_ms: None,
                result_count: 0,
                usage: AiUsage::default(),
            })
            .expect("create analysis job");
        }

        let mixed_delete = vec!["skipped-a".to_string(), "completed-a".to_string()];
        assert!(db.delete_skipped_analysis_jobs(&mixed_delete).is_err());
        assert!(db
            .get_analysis_job("skipped-a")
            .expect("load rolled back skipped job")
            .is_some());

        let deleted = db
            .delete_skipped_analysis_jobs(&["skipped-a".to_string(), "skipped-b".to_string()])
            .expect("delete skipped jobs");
        assert_eq!(deleted, 2);

        let retried = db
            .retry_analysis_jobs(&["failed-a".to_string(), "downgraded-a".to_string()])
            .expect("retry failed jobs");
        assert_eq!(retried, 2);
        for id in ["failed-a", "downgraded-a"] {
            let job = db
                .get_analysis_job(id)
                .expect("load retried job")
                .expect("retried job remains");
            assert_eq!(job.status, "pending");
            assert!(job.error.is_none());
        }
        assert!(db
            .retry_analysis_jobs(&["completed-a".to_string()])
            .is_err());

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn completed_job_without_an_ai_request_is_migrated_to_skipped() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-analysis-job-skipped-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let queued_at = now();
        db.create_analysis_job(&AnalysisJob {
            id: "legacy-empty-completed-job".into(),
            chunk_ids: vec!["already-corrected".into()],
            metadata_range: TimeRange {
                started_at: queued_at.clone(),
                ended_at: queued_at.clone(),
            },
            mode: "metadata-context-review".into(),
            provider: String::new(),
            model: String::new(),
            retry_count: 0,
            status: "completed".into(),
            error: None,
            system_prompt: None,
            user_prompt: None,
            response: None,
            queued_at,
            processing_started_at: None,
            completed_at: None,
            duration_ms: None,
            result_count: 0,
            usage: AiUsage::default(),
        })
        .expect("insert legacy empty job");
        drop(db);

        let reopened = AppDb::open_in(data_dir.clone()).expect("reopen test database");
        let migrated = reopened
            .get_analysis_job("legacy-empty-completed-job")
            .expect("load migrated job")
            .expect("migrated job remains");
        assert_eq!(migrated.status, "skipped");
        assert!(migrated
            .error
            .as_deref()
            .is_some_and(|value| value.contains("未调用 AI")));
        assert!(migrated.completed_at.is_some());

        drop(reopened);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn every_confirmed_correction_builds_an_isolated_future_memory() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-personal-memory-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let category = db.create_category("保研").expect("create category");
        let project = db
            .create_project("推免", &category.name)
            .expect("create project");
        let task = db
            .create_task(&project.id, "成果填报")
            .expect("create task");
        let base = Utc::now() + Duration::minutes(1);
        let mut first_event = context_event(
            "memory-source",
            "chrome.exe",
            "预推免成果填报 - Google Chrome",
            base,
        );
        first_event.metadata["activePageTitle"] =
            serde_json::Value::String("预推免成果填报".into());
        let first = classification::ingest_event(&db, &first_event)
            .expect("ingest source")
            .expect("source session");
        db.apply_session_correction(
            std::slice::from_ref(&first.id),
            SessionPatch {
                summary: Some("预推免成果填报".into()),
                project_id: Some(project.id.clone()),
                task_id: Some(task.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: Some(category.name.clone()),
                confidence: Some(0.98),
                user_confirmed: Some(true),
            },
            false,
            None,
            None,
        )
        .expect("correct without explicit rule");
        assert_eq!(
            db.conn
                .lock()
                .query_row("SELECT COUNT(*) FROM attribution_memories", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count memories"),
            1
        );
        assert_eq!(
            db.conn
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM attribution_rules WHERE enabled=1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count explicit rules"),
            0
        );

        let mut next_event = context_event(
            "memory-target",
            "wps.exe",
            "预推免成果填报",
            base + Duration::minutes(10),
        );
        next_event.metadata["activePageTitle"] = serde_json::Value::String("预推免成果填报".into());
        let remembered = classification::ingest_event(&db, &next_event)
            .expect("ingest remembered page")
            .expect("remembered session");
        assert_eq!(remembered.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(remembered.task_id.as_deref(), Some(task.id.as_str()));
        assert!(remembered.confidence >= 0.88);
        assert!(remembered
            .evidence
            .iter()
            .any(|item| item.kind == "personal-memory"));

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn batch_correction_does_not_turn_incidental_apps_into_permanent_rules() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-batch-memory-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        db.create_category("科研").expect("create category");
        let project = db.create_project("IOT", "科研").expect("create project");
        let task = db.create_task(&project.id, "会议").expect("create task");
        let base = Utc::now() + Duration::minutes(2);
        let insert = |id: &str, start: DateTime<Utc>, page: &str, app: &str| {
            let evidence = serde_json::to_string(&vec![
                EvidenceItem {
                    kind: "page".into(),
                    label: "当前页面".into(),
                    value: page.into(),
                    weight: 0.95,
                },
                EvidenceItem {
                    kind: "app".into(),
                    label: "应用".into(),
                    value: app.into(),
                    weight: 0.8,
                },
            ])
            .expect("serialize evidence");
            db.conn
                .lock()
                .execute(
                    "INSERT INTO work_sessions VALUES (?1,?2,?3,NULL,NULL,'杂务',?4,0.55,?5,0,'context-complete',?6)",
                    params![id, fmt(start), fmt(start + Duration::seconds(10)), page, evidence, now()],
                )
                .expect("insert session");
        };
        insert("batch-related", base, "IOT week1", "ChatGPT.exe");
        insert(
            "batch-incidental",
            base + Duration::seconds(10),
            "ScreenUse开发",
            "ScreenUse.exe",
        );

        db.apply_session_correction(
            &["batch-related".into(), "batch-incidental".into()],
            SessionPatch {
                summary: None,
                project_id: Some(project.id.clone()),
                task_id: Some(task.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: Some("科研".into()),
                confidence: Some(0.98),
                user_confirmed: Some(true),
            },
            false,
            None,
            None,
        )
        .expect("batch correction");

        let stored = db
            .conn
            .lock()
            .prepare(
                "SELECT session_id FROM attribution_memories
                 WHERE session_id IN ('batch-related','batch-incidental') ORDER BY session_id",
            )
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .expect("load memories");
        assert_eq!(stored, vec!["batch-related".to_string()]);

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn undo_restores_the_previous_personal_memory_state() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-personal-memory-undo-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let category = db.create_category("保研").expect("create category");
        let project = db
            .create_project("推免", &category.name)
            .expect("create project");
        let task = db
            .create_task(&project.id, "成果填报")
            .expect("create task");
        let mut source = chat_event("memory-undo", "预推免成果填报");
        source.metadata["activePageTitle"] = serde_json::Value::String("预推免成果填报".into());
        let session = classification::ingest_event(&db, &source)
            .expect("ingest source")
            .expect("source session");
        db.apply_session_correction(
            std::slice::from_ref(&session.id),
            SessionPatch {
                summary: Some("成果填报".into()),
                project_id: Some(project.id.clone()),
                task_id: Some(task.id.clone()),
                clear_project: Some(false),
                clear_task: Some(false),
                category: Some(category.name.clone()),
                confidence: Some(0.98),
                user_confirmed: Some(true),
            },
            false,
            None,
            None,
        )
        .expect("apply correction");
        assert_eq!(
            db.conn
                .lock()
                .query_row("SELECT COUNT(*) FROM attribution_memories", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count memory"),
            1
        );
        db.undo_last_session_correction().expect("undo correction");
        assert_eq!(
            db.conn
                .lock()
                .query_row("SELECT COUNT(*) FROM attribution_memories", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count memory after undo"),
            0
        );
        assert!(
            !db.get_session(&session.id)
                .expect("load restored")
                .expect("restored session")
                .user_confirmed
        );

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn synthetic_manual_ranges_never_become_context_memories() {
        let mut session = WorkSession {
            id: "synthetic".into(),
            started_at: "2026-07-16T10:00:00Z".into(),
            ended_at: "2026-07-16T10:10:00Z".into(),
            project_id: Some("project".into()),
            project_name: Some("项目".into()),
            task_id: Some("task".into()),
            task_title: Some("任务".into()),
            category: "学习".into(),
            summary: "手动补录".into(),
            confidence: 1.0,
            evidence: Vec::new(),
            user_confirmed: true,
            source: "manual-entry".into(),
        };
        assert!(!is_reliable_memory_session(&session));
        session.source = "manual-merge".into();
        assert!(!is_reliable_memory_session(&session));
        session.source = "manual-correction".into();
        assert!(is_reliable_memory_session(&session));
    }

    #[test]
    fn configured_idle_target_is_idle_even_with_a_concrete_task() {
        let settings = AppSettings::default().normalized();
        let session = WorkSession {
            id: "idle".into(),
            started_at: "2026-07-16T10:00:00Z".into(),
            ended_at: "2026-07-16T10:03:00Z".into(),
            project_id: Some("idle-project".into()),
            project_name: Some(settings.idle_project_name.clone()),
            task_id: Some("nothing".into()),
            task_title: Some("nothing".into()),
            category: settings.idle_category.clone(),
            summary: "会议中未操作".into(),
            confidence: 0.95,
            evidence: Vec::new(),
            user_confirmed: false,
            source: "ai-review".into(),
        };
        assert!(is_idle_session(&session, &settings));
    }

    #[test]
    #[ignore = "requires SCREENUSE_REPLAY_DATA_DIR pointing to a copied real ledger"]
    fn replay_personal_memory_against_real_corrections() {
        let data_dir = std::env::var("SCREENUSE_REPLAY_DATA_DIR")
            .map(PathBuf::from)
            .expect("set SCREENUSE_REPLAY_DATA_DIR to a copied ledger directory");
        let db = AppDb::open_in(data_dir).expect("open replay database");
        db.rebuild_personal_memory_from_confirmed()
            .expect("rebuild coherent memories");
        let records = db.load_personal_memories().expect("load memories");
        let targets = records
            .iter()
            .filter(|record| record.user_confirmed)
            .collect::<Vec<_>>();
        assert!(
            !targets.is_empty(),
            "replay ledger has no manual corrections"
        );

        let evaluate = |chronological: bool| {
            let mut predicted = 0_usize;
            let mut correct = 0_usize;
            let mut errors = Vec::new();
            let mut abstentions = Vec::new();
            for target in &targets {
                let pool = records
                    .iter()
                    .filter(|candidate| candidate.session_id != target.session_id)
                    .filter(|candidate| {
                        !chronological || candidate.confirmed_at < target.confirmed_at
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let Some(decision) = crate::memory::choose_assignment(&target.features, &pool)
                else {
                    if abstentions.len() < 20 {
                        abstentions.push(format!(
                            "{} | {:?} (expected {}/{}/{})",
                            target.session_id,
                            target.features,
                            target.category,
                            target.project_id,
                            target.task_id
                        ));
                    }
                    continue;
                };
                predicted += 1;
                if decision.category == target.category
                    && decision.project_id == target.project_id
                    && decision.task_id == target.task_id
                {
                    correct += 1;
                } else if errors.len() < 20 {
                    errors.push(format!(
                        "{} | {:?} -> {}/{}/{} (expected {}/{}/{})",
                        target.session_id,
                        target.features,
                        decision.category,
                        decision.project_id,
                        decision.task_id,
                        target.category,
                        target.project_id,
                        target.task_id
                    ));
                }
            }
            let coverage = predicted as f64 / targets.len() as f64;
            let precision = if predicted == 0 {
                0.0
            } else {
                correct as f64 / predicted as f64
            };
            let overall = correct as f64 / targets.len() as f64;
            (
                predicted,
                correct,
                coverage,
                precision,
                overall,
                errors,
                abstentions,
            )
        };

        for (label, chronological) in [("leave-one-out", false), ("chronological", true)] {
            let (predicted, correct, coverage, precision, overall, errors, abstentions) =
                evaluate(chronological);
            eprintln!(
                "{label}: targets={} predicted={predicted} correct={correct} coverage={:.2}% precision={:.2}% overall={:.2}%",
                targets.len(),
                coverage * 100.0,
                precision * 100.0,
                overall * 100.0
            );
            for error in errors {
                eprintln!("  {error}");
            }
            for abstention in abstentions {
                eprintln!("  abstain: {abstention}");
            }
        }
    }

    #[test]
    #[ignore = "requires SCREENUSE_REPLAY_DATA_DIR pointing to a copied real ledger"]
    fn replay_complete_local_attribution_chronologically() {
        let data_dir = std::env::var("SCREENUSE_REPLAY_DATA_DIR")
            .map(PathBuf::from)
            .expect("set SCREENUSE_REPLAY_DATA_DIR to a copied ledger directory");
        let db = AppDb::open_in(data_dir).expect("open replay database");
        db.rebuild_personal_memory_from_confirmed()
            .expect("rebuild coherent memories");
        let mut targets = db
            .load_personal_memories()
            .expect("load confirmed examples")
            .into_iter()
            .filter(|record| record.user_confirmed)
            .collect::<Vec<_>>();
        targets.sort_by(|left, right| left.confirmed_at.cmp(&right.confirmed_at));
        assert!(
            !targets.is_empty(),
            "replay ledger has no manual corrections"
        );

        {
            let conn = db.conn.lock();
            conn.execute("DELETE FROM attribution_memories", [])
                .expect("clear future memories");
            conn.execute(
                "DELETE FROM attribution_rules WHERE created_from_correction=1",
                [],
            )
            .expect("clear future learned rules");
            conn.execute("DELETE FROM context_pin", [])
                .expect("clear pinned context");
        }

        let settings = db.get_settings().expect("load settings").normalized();
        let mut automatic = 0_usize;
        let mut correct = 0_usize;
        let mut errors = Vec::new();
        let mut fallbacks = Vec::new();
        let mut outcomes = Vec::new();
        for target in &targets {
            let session = db
                .get_session(&target.session_id)
                .expect("load target session")
                .expect("target session exists");
            let event = RawActivityEvent {
                id: format!("replay:{}", target.session_id),
                source: "replay-local-attribution".into(),
                timestamp: session.started_at.clone(),
                app: (!target.features.app.is_empty()).then(|| target.features.app.clone()),
                window_title: (!target.features.window.is_empty())
                    .then(|| target.features.window.clone())
                    .or_else(|| {
                        (!target.features.page.is_empty()).then(|| target.features.page.clone())
                    }),
                url: (!target.features.domain.is_empty())
                    .then(|| format!("https://{}/", target.features.domain)),
                file_path: (!target.features.file.is_empty()).then(|| target.features.file.clone()),
                workspace: (!target.features.workspace.is_empty())
                    .then(|| target.features.workspace.clone()),
                input_stats: InputStats::default(),
                metadata: if target.features.page.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::json!({"activePageTitle": target.features.page})
                },
            };

            let mut decision = db
                .heuristic_attribution(&event, false, &settings, None)
                .expect("classify replay event");
            let (local_category, local_confidence) =
                classification::classify_category(&event, settings.idle_threshold_seconds);
            if decision.confidence < 0.84 {
                decision.category = local_category.into();
                decision.confidence = decision.confidence.max(local_confidence);
            }
            if let Some(contextual) =
                classification::resolve_project_task(&db, &event, &decision.category)
                    .expect("resolve project and task")
            {
                let current_is_complete = decision.project_id.is_some()
                    && decision.task_id.is_some()
                    && decision.confidence >= 0.84;
                if !current_is_complete
                    || (contextual.specificity >= 100 && contextual.task_id.is_some())
                    || contextual.confidence > decision.confidence + 0.01
                {
                    decision.project_id = Some(contextual.project_id);
                    decision.task_id = contextual.task_id;
                    decision.category = contextual.category;
                    decision.confidence = decision.confidence.max(contextual.confidence);
                }
            }

            let concrete = decision.project_id.is_some()
                && decision.task_id.is_some()
                && decision.confidence >= 0.8;
            let mut is_correct = false;
            if concrete {
                automatic += 1;
                is_correct = decision.category == target.category
                    && decision.project_id.as_deref() == Some(target.project_id.as_str())
                    && decision.task_id.as_deref() == Some(target.task_id.as_str());
                correct += usize::from(is_correct);
                if !is_correct && errors.len() < 30 {
                    errors.push(format!(
                        "{} | {:?} -> {}/{:?}/{:?} {:.2} (expected {}/{}/{})",
                        target.session_id,
                        target.features,
                        decision.category,
                        decision.project_id,
                        decision.task_id,
                        decision.confidence,
                        target.category,
                        target.project_id,
                        target.task_id
                    ));
                }
            } else if fallbacks.len() < 30 {
                fallbacks.push(format!(
                    "{} | {:?} -> {}/{:?}/{:?} {:.2} (expected {}/{}/{})",
                    target.session_id,
                    target.features,
                    decision.category,
                    decision.project_id,
                    decision.task_id,
                    decision.confidence,
                    target.category,
                    target.project_id,
                    target.task_id
                ));
            }
            outcomes.push((concrete, is_correct));

            db.conn
                .lock()
                .execute(
                    "INSERT OR REPLACE INTO attribution_memories(
                       session_id,features_json,category,project_id,task_id,
                       confirmed_at,last_used_at,use_count
                     ) VALUES(?1,?2,?3,?4,?5,?6,?6,0)",
                    params![
                        target.session_id,
                        serde_json::to_string(&target.features).expect("serialize features"),
                        target.category,
                        target.project_id,
                        target.task_id,
                        target.confirmed_at
                    ],
                )
                .expect("learn target memory after prediction");
            let _ = db.learn_rule_from_session(&target.session_id, None);
        }

        let precision = if automatic == 0 {
            0.0
        } else {
            correct as f64 / automatic as f64
        };
        let coverage = automatic as f64 / targets.len() as f64;
        let overall = correct as f64 / targets.len() as f64;
        eprintln!(
            "complete-local chronological: targets={} automatic={automatic} correct={correct} coverage={:.2}% precision={:.2}% overall={:.2}% ai_fallbacks={}",
            targets.len(),
            coverage * 100.0,
            precision * 100.0,
            overall * 100.0,
            targets.len() - automatic
        );
        for error in errors {
            eprintln!("  error: {error}");
        }
        for fallback in fallbacks {
            eprintln!("  fallback: {fallback}");
        }
        for window in [50_usize, 100, 200] {
            let suffix = &outcomes[outcomes.len().saturating_sub(window)..];
            let suffix_automatic = suffix.iter().filter(|(auto, _)| *auto).count();
            let suffix_correct = suffix.iter().filter(|(_, correct)| *correct).count();
            let suffix_precision = if suffix_automatic == 0 {
                0.0
            } else {
                suffix_correct as f64 / suffix_automatic as f64
            };
            eprintln!(
                "  last {}: automatic={suffix_automatic} correct={suffix_correct} coverage={:.2}% precision={:.2}% overall={:.2}%",
                suffix.len(),
                suffix_automatic as f64 / suffix.len() as f64 * 100.0,
                suffix_precision * 100.0,
                suffix_correct as f64 / suffix.len() as f64 * 100.0
            );
        }
    }
}
