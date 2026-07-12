use crate::models::*;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use directories::ProjectDirs;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;

use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub struct AppDb {
    conn: Mutex<Connection>,
    db_path: PathBuf,
    data_dir: PathBuf,
}

impl AppDb {
    pub fn open() -> Result<Self> {
        let dirs = ProjectDirs::from("com", "ShallowDream", "ScreenUse")
            .context("cannot locate platform data dir")?;
        Self::open_in(dirs.data_dir().to_path_buf())
    }

    fn open_in(data_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&data_dir)?;
        fs::create_dir_all(data_dir.join("exports"))?;
        fs::create_dir_all(data_dir.join("backups"))?;
        let db_path = data_dir.join("screenuse.db");
        let conn = Connection::open(&db_path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self { conn: Mutex::new(conn), db_path, data_dir };
        db.migrate()?;
        db.seed_if_empty()?;
        Ok(db)
    }

    pub fn data_dir(&self) -> &Path { &self.data_dir }

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
              error TEXT
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
              enabled INTEGER NOT NULL DEFAULT 1
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
            CREATE INDEX IF NOT EXISTS idx_work_sessions_time ON work_sessions(started_at, ended_at);
            CREATE INDEX IF NOT EXISTS idx_raw_events_time ON raw_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_jobs_status ON analysis_jobs(status);
        "#)?;
        Ok(())
    }

    fn seed_if_empty(&self) -> Result<()> {
        let conn = self.conn.lock();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))?;
        if count > 0 { return Ok(()); }
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
        conn.execute("INSERT INTO tasks VALUES (?1, ?2, '资料阅读与写作', 'active', 'seed', NULL, ?3, ?3)", params![t2, p2, now])?;
        conn.execute("INSERT INTO tasks VALUES (?1, ?2, '未归类活动整理', 'active', 'seed', NULL, ?3, ?3)", params![t3, p3, now])?;

        let s1 = Uuid::new_v4().to_string();
        let s2 = Uuid::new_v4().to_string();
        let s3 = Uuid::new_v4().to_string();
        let base = Utc::now() - Duration::hours(4);
        insert_seed_session(&conn, &s1, &p1, &t1, "开发", "搭建 ScreenUse v1 项目骨架", base, base + Duration::minutes(75), 0.86)?;
        insert_seed_session(&conn, &s2, &p2, &t2, "学习", "阅读竞品与时间追踪资料", base + Duration::minutes(90), base + Duration::minutes(145), 0.79)?;
        insert_seed_session(&conn, &s3, &p1, &t1, "开发", "设计 AI 队列与失败重试策略", base + Duration::minutes(165), base + Duration::minutes(220), 0.82)?;
        Ok(())
    }

    pub fn get_settings(&self) -> Result<AppSettings> {
        let conn = self.conn.lock();
        let raw: Option<String> = conn.query_row("SELECT value FROM settings WHERE key='app_settings'", [], |r| r.get(0)).optional()?;
        Ok(raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO settings(key, value, updated_at) VALUES('app_settings', ?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            params![serde_json::to_string(settings)?, now()],
        )?;
        Ok(())
    }

    pub fn dashboard(&self, collector_running: bool) -> Result<DashboardData> {
        Ok(DashboardData {
            settings: self.get_settings()?,
            sessions: self.list_sessions(80)?,
            projects: self.list_projects()?,
            tasks: self.list_tasks()?,
            plan_items: self.list_plan_items(100)?,
            trends: self.project_task_trends()?,
            categories: self.category_trends()?,
            queue: self.queue_health()?,
            collector_running,
        })
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id,name,category,source,color,description,created_at,updated_at FROM projects ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |r| Ok(Project {
            id: r.get(0)?, name: r.get(1)?, category: r.get(2)?, source: r.get(3)?, color: r.get(4)?, description: r.get(5)?, created_at: r.get(6)?, updated_at: r.get(7)?,
        }))?;
        collect_rows(rows)
    }

    pub fn create_project(&self, name: &str, category: &str) -> Result<Project> {
        let name = name.trim().replace(['\r', '\n', '\t'], " ");
        if name.is_empty() {
            bail!("项目名称不能为空");
        }
        let name: String = name.chars().take(80).collect();
        let category = category.trim();
        if !DEFAULT_CATEGORIES.contains(&category) {
            bail!("不支持的项目分类：{category}");
        }

        let conn = self.conn.lock();
        let duplicate = conn
            .query_row(
                "SELECT 1 FROM projects WHERE name=?1 LIMIT 1",
                params![name],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if duplicate {
            bail!("同名项目已存在，请直接选择它");
        }

        let timestamp = now();
        let project = Project {
            id: Uuid::new_v4().to_string(),
            name,
            category: category.to_string(),
            source: "manual".into(),
            color: color_for_category(category).into(),
            description: Some("在修正归类时手动创建".into()),
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
        tx.execute("DELETE FROM projects WHERE id=?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id,project_id,title,status,source,planned_due_at,created_at,updated_at FROM tasks ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |r| Ok(Task {
            id: r.get(0)?, project_id: r.get(1)?, title: r.get(2)?, status: r.get(3)?, source: r.get(4)?, planned_due_at: r.get(5)?, created_at: r.get(6)?, updated_at: r.get(7)?,
        }))?;
        collect_rows(rows)
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
        let rows = stmt.query_map(params![limit], |r| {
            let evidence_json: String = r.get(10)?;
            Ok(WorkSession {
                id: r.get(0)?, started_at: r.get(1)?, ended_at: r.get(2)?, project_id: r.get(3)?, project_name: r.get(4)?, task_id: r.get(5)?, task_title: r.get(6)?,
                category: r.get(7)?, summary: r.get(8)?, confidence: r.get(9)?, evidence: parse_json(&evidence_json), user_confirmed: r.get::<_, i64>(11)? != 0, source: r.get(12)?,
            })
        })?;
        collect_rows(rows)
    }

    pub fn update_session(&self, id: &str, patch: SessionPatch) -> Result<WorkSession> {
        let current = self.get_session(id)?.context("session not found")?;
        let project_changed = patch
            .project_id
            .as_deref()
            .is_some_and(|project_id| current.project_id.as_deref() != Some(project_id));
        let project_id = patch.project_id.or(current.project_id);
        let task_id = if patch.task_id.is_some() {
            patch.task_id
        } else if project_changed {
            None
        } else {
            current.task_id
        };
        let summary = patch.summary.unwrap_or(current.summary);
        let category = patch.category.unwrap_or(current.category);
        let confidence = patch.confidence.unwrap_or(current.confidence);
        let confirmed = patch.user_confirmed.unwrap_or(true);
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE work_sessions SET project_id=?1, task_id=?2, summary=?3, category=?4, confidence=?5, user_confirmed=?6, updated_at=?7 WHERE id=?8",
            params![project_id, task_id, summary, category, confidence, if confirmed {1} else {0}, now(), id],
        )?;
        drop(conn);
        self.get_session(id)?.context("session disappeared after update")
    }

    pub fn get_session(&self, id: &str) -> Result<Option<WorkSession>> {
        let sessions = self.list_sessions(500)?;
        Ok(sessions.into_iter().find(|s| s.id == id))
    }

    pub fn merge_sessions(&self, ids: &[String], summary: Option<String>) -> Result<WorkSession> {
        anyhow::ensure!(!ids.is_empty(), "no session ids provided");
        let conn = self.conn.lock();
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT MIN(started_at), MAX(ended_at), project_id, task_id, category, GROUP_CONCAT(summary, ' / '), AVG(confidence), GROUP_CONCAT(evidence_json, '||') FROM work_sessions WHERE id IN ({})", placeholders);
        let row = {
            let mut stmt = conn.prepare(&sql)?;
            stmt.query_row(rusqlite::params_from_iter(ids), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, f32>(6)?, r.get::<_, String>(7)?))
            })?
        };
        let new_id = Uuid::new_v4().to_string();
        let evidence = merge_evidence_blobs(&row.7);
        conn.execute(
            "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,'manual-merge',?10)",
            params![new_id, row.0, row.1, row.2, row.3, row.4, summary.unwrap_or(row.5), row.6, serde_json::to_string(&evidence)?, now()],
        )?;
        for id in ids { conn.execute("DELETE FROM work_sessions WHERE id=?1", params![id])?; }
        drop(conn);
        self.get_session(&new_id)?.context("merged session missing")
    }

    pub fn split_session(&self, id: &str, split_at: &str) -> Result<Vec<WorkSession>> {
        let session = self.get_session(id)?.context("session not found")?;
        anyhow::ensure!(split_at > session.started_at.as_str() && split_at < session.ended_at.as_str(), "split_at must be inside session range");
        let first_id = Uuid::new_v4().to_string();
        let second_id = Uuid::new_v4().to_string();
        let evidence_json = serde_json::to_string(&session.evidence)?;
        let conn = self.conn.lock();
        conn.execute("DELETE FROM work_sessions WHERE id=?1", params![id])?;
        conn.execute("INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,'manual-split',?10)", params![first_id, session.started_at, split_at, session.project_id, session.task_id, session.category, format!("{}（前半段）", session.summary), session.confidence, evidence_json, now()])?;
        let evidence_json2 = serde_json::to_string(&session.evidence)?;
        conn.execute("INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,'manual-split',?10)", params![second_id, split_at, session.ended_at, session.project_id, session.task_id, session.category, format!("{}（后半段）", session.summary), session.confidence, evidence_json2, now()])?;
        drop(conn);
        Ok(vec![self.get_session(&first_id)?.unwrap(), self.get_session(&second_id)?.unwrap()])
    }

    pub fn ingest_raw_event(&self, mut event: RawActivityEvent) -> Result<()> {
        if event.id.is_empty() { event.id = Uuid::new_v4().to_string(); }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO raw_events VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![event.id, event.source, event.timestamp, event.app, event.window_title, event.url, event.file_path, event.workspace, serde_json::to_string(&event.input_stats)?, event.metadata.to_string()],
        )?;
        drop(conn);
        self.materialize_event_session(&event)?;
        Ok(())
    }

    fn materialize_event_session(&self, event: &RawActivityEvent) -> Result<()> {
        let settings = self.get_settings()?;
        let is_idle = event.input_stats.idle_seconds >= settings.idle_threshold_seconds as u64;
        let (project_id, task_id, category, summary, confidence) = self.heuristic_attribution(event, is_idle)?;
        let evidence = vec![
            EvidenceItem { kind: "window".into(), label: "窗口".into(), value: event.window_title.clone().unwrap_or_else(|| "未知窗口".into()), weight: 0.7 },
            EvidenceItem { kind: "app".into(), label: "应用".into(), value: event.app.clone().unwrap_or_else(|| "未知应用".into()), weight: 0.5 },
        ];
        let conn = self.conn.lock();
        let last: Option<(String, String, String, String, i64)> = conn.query_row(
            "SELECT id, summary, category, ended_at, user_confirmed FROM work_sessions ORDER BY ended_at DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        ).optional()?;
        if let Some((id, last_summary, last_category, _last_end, confirmed)) = last {
            if !starts_new_context(event) && confirmed == 0 && last_summary == summary && last_category == category {
                conn.execute("UPDATE work_sessions SET ended_at=?1, confidence=MAX(confidence, ?2), evidence_json=?3, updated_at=?4 WHERE id=?5", params![event.timestamp, confidence, serde_json::to_string(&evidence)?, now(), id])?;
                return Ok(());
            }
        }
        conn.execute(
            "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,0,'collector-rule',?10)",
            params![Uuid::new_v4().to_string(), event.timestamp, event.timestamp, project_id, task_id, category, summary, confidence, serde_json::to_string(&evidence)?, now()],
        )?;
        Ok(())
    }

    fn heuristic_attribution(&self, event: &RawActivityEvent, is_idle: bool) -> Result<(Option<String>, Option<String>, String, String, f32)> {
        if is_idle { return Ok((None, None, "离开".into(), "离开/空闲".into(), 0.96)); }
        let hay = format!("{} {} {} {}", event.app.clone().unwrap_or_default(), event.window_title.clone().unwrap_or_default(), event.url.clone().unwrap_or_default(), event.file_path.clone().unwrap_or_default()).to_lowercase();
        let conn = self.conn.lock();
        let rules = {
            let mut stmt = conn.prepare("SELECT matcher_json,project_id,task_id,category,name FROM attribution_rules WHERE enabled=1 ORDER BY priority DESC")?;
            let mapped = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))?;
            collect_rows(mapped)?
        };
        for (matcher_json, project_id, task_id, category, name) in rules {
            let matcher: serde_json::Value = serde_json::from_str(&matcher_json).unwrap_or_default();
            let keyword = matcher.get("keyword").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let app = matcher.get("app").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let domain = matcher.get("domain").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let hit = (!keyword.is_empty() && hay.contains(&keyword))
                || (!app.is_empty() && event.app.as_deref().unwrap_or("").to_lowercase().contains(&app))
                || (!domain.is_empty() && event.url.as_deref().unwrap_or("").to_lowercase().contains(&domain));
            if hit {
                return Ok((project_id, task_id, category, format!("规则命中：{name}"), 0.84));
            }
        }
        let (category, summary) = if hay.contains("code") || hay.contains("rust") || hay.contains("github") || hay.contains("tauri") || hay.contains("screenuse") || hay.contains("codex") {
            ("开发", "开发与调试")
        } else if hay.contains("word") || hay.contains("wps") || hay.contains("obsidian") || hay.contains("论文") || hay.contains("markdown") {
            ("写作", "写作与文档整理")
        } else if hay.contains("bilibili") || hay.contains("course") || hay.contains("pdf") || hay.contains("知网") || hay.contains("论文") {
            ("学习", "学习资料阅读")
        } else if hay.contains("wechat") || hay.contains("qq") || hay.contains("mail") || hay.contains("teams") || hay.contains("meeting") {
            ("沟通", "沟通与消息处理")
        } else if hay.contains("steam") || hay.contains("game") || hay.contains("youtube") {
            ("娱乐", "娱乐与视频")
        } else {
            ("杂务", "未归类电脑活动")
        };
        let project: Option<(String, String)> = conn.query_row("SELECT p.id, t.id FROM projects p JOIN tasks t ON t.project_id=p.id WHERE p.category=?1 ORDER BY p.updated_at DESC LIMIT 1", params![category], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        Ok((project.as_ref().map(|x| x.0.clone()), project.as_ref().map(|x| x.1.clone()), category.into(), summary.into(), 0.55))
    }

    pub fn create_analysis_job(&self, job: &AnalysisJob) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("INSERT OR REPLACE INTO analysis_jobs VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![job.id, serde_json::to_string(&job.chunk_ids)?, job.metadata_range.started_at, job.metadata_range.ended_at, job.mode, job.retry_count, job.status, job.error])?;
        Ok(())
    }

    pub fn claim_next_analysis_job(&self) -> Result<Option<AnalysisJob>> {
        let conn = self.conn.lock();
        let row: Option<(String, String, String, String, String, u32, String, Option<String>)> = conn.query_row(
            "SELECT id,chunk_ids_json,started_at,ended_at,mode,retry_count,status,error FROM analysis_jobs WHERE status='pending' ORDER BY started_at ASC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get::<_, i64>(5)? as u32, r.get(6)?, r.get(7)?)),
        ).optional()?;
        if let Some((id, chunk_ids_json, started_at, ended_at, mode, retry_count, _status, error)) = row {
            conn.execute("UPDATE analysis_jobs SET status='running', error=NULL WHERE id=?1 AND status='pending'", params![id])?;
            Ok(Some(AnalysisJob {
                id,
                chunk_ids: parse_json(&chunk_ids_json),
                metadata_range: TimeRange { started_at, ended_at },
                mode,
                retry_count,
                status: "running".into(),
                error,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn mark_analysis_job_status(&self, id: &str, status: &str, retry_count: Option<u32>, error: Option<String>) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE analysis_jobs SET status=?1, retry_count=COALESCE(?2,retry_count), error=?3 WHERE id=?4",
            params![status, retry_count.map(|v| v as i64), error, id],
        )?;
        Ok(())
    }

    pub fn list_raw_events_between(&self, started_at: &str, ended_at: &str) -> Result<Vec<RawActivityEvent>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(r#"
            SELECT id,source,timestamp,app,window_title,url,file_path,workspace,input_stats_json,metadata_json
            FROM raw_events
            WHERE timestamp >= ?1 AND timestamp <= ?2
            ORDER BY timestamp ASC
            LIMIT 500
        "#)?;
        let rows = stmt.query_map(params![started_at, ended_at], |r| {
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

    pub fn upsert_project_by_name(&self, name: &str, category: &str, source: &str) -> Result<String> {
        let name = clean_name(name, "自动发现项目");
        let category = clean_name(category, "杂务");
        let conn = self.conn.lock();
        if let Some(id) = conn.query_row("SELECT id FROM projects WHERE name=?1 LIMIT 1", params![name], |r| r.get::<_, String>(0)).optional()? {
            conn.execute("UPDATE projects SET category=?1, updated_at=?2 WHERE id=?3", params![category, now(), id])?;
            return Ok(id);
        }
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects VALUES (?1,?2,?3,?4,?5,?6,?7,?7)",
            params![id, name, category, source, color_for_category(&category), format!("由 ScreenUse 根据窗口、URL、目录和计划源自动发现：{name}"), now()],
        )?;
        Ok(id)
    }

    pub fn upsert_task_by_title(&self, project_id: &str, title: &str, source: &str) -> Result<String> {
        let title = clean_name(title, "待确认活动");
        let conn = self.conn.lock();
        if let Some(id) = conn.query_row("SELECT id FROM tasks WHERE project_id=?1 AND title=?2 LIMIT 1", params![project_id, title], |r| r.get::<_, String>(0)).optional()? {
            conn.execute("UPDATE tasks SET status='active', updated_at=?1 WHERE id=?2", params![now(), id])?;
            return Ok(id);
        }
        let id = Uuid::new_v4().to_string();
        conn.execute("INSERT INTO tasks VALUES (?1,?2,?3,'active',?4,NULL,?5,?5)", params![id, project_id, title, source, now()])?;
        Ok(id)
    }

    pub fn materialize_attribution_session(
        &self,
        range: &TimeRange,
        project_id: Option<String>,
        task_id: Option<String>,
        category: String,
        summary: String,
        confidence: f32,
        evidence: Vec<EvidenceItem>,
        source: &str,
    ) -> Result<WorkSession> {
        let id = Uuid::new_v4().to_string();
        let evidence_json = serde_json::to_string(&evidence)?;
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM work_sessions
             WHERE user_confirmed=0
               AND source IN ('collector-rule','ai-analysis','rule-downgrade')
               AND ended_at >= ?1
               AND started_at <= ?2",
            params![range.started_at, range.ended_at],
        )?;
        conn.execute(
            "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,0,?10,?11)",
            params![
                id,
                range.started_at,
                range.ended_at,
                project_id,
                task_id,
                category,
                summary,
                confidence.clamp(0.0, 1.0),
                evidence_json,
                source,
                now()
            ],
        )?;
        conn.execute(
            "INSERT INTO activities VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![Uuid::new_v4().to_string(), id, source, "自动归因活动", summary, range.started_at, range.ended_at, serde_json::to_string(&evidence)?],
        )?;
        drop(conn);
        self.match_plan_items_for_session(&id)?;
        self.get_session(&id)?.context("inserted session missing")
    }

    fn match_plan_items_for_session(&self, session_id: &str) -> Result<()> {
        let session = match self.get_session(session_id)? { Some(s) => s, None => return Ok(()) };
        let hay = format!("{} {} {} {}", session.summary, session.category, session.project_name.unwrap_or_default(), session.task_title.unwrap_or_default()).to_lowercase();
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id,title,note,matched_session_ids_json FROM plan_items")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?)))?;
        let mut updates = Vec::new();
        for row in rows {
            let (id, title, note, matched_json) = row?;
            let needle = format!("{} {}", title, note.unwrap_or_default()).to_lowercase();
            if !needle.is_empty() && (hay.contains(&needle) || needle.split_whitespace().any(|part| part.len() >= 3 && hay.contains(part))) {
                let mut matched: Vec<String> = parse_json(&matched_json);
                if !matched.iter().any(|s| s == session_id) {
                    matched.push(session_id.to_string());
                    updates.push((id, serde_json::to_string(&matched)?));
                }
            }
        }
        drop(stmt);
        for (id, matched_json) in updates {
            conn.execute("UPDATE plan_items SET matched_session_ids_json=?1, updated_at=?2 WHERE id=?3", params![matched_json, now(), id])?;
        }
        Ok(())
    }

    pub fn compact_sessions(&self) -> Result<u32> {
        #[derive(Clone)]
        struct Row {
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
        }
        let conn = self.conn.lock();
        let rows = {
            let mut stmt = conn.prepare(r#"
                SELECT id,started_at,ended_at,project_id,task_id,category,summary,confidence,evidence_json,user_confirmed
                FROM work_sessions
                ORDER BY started_at ASC
            "#)?;
            let mapped = stmt.query_map([], |r| Ok(Row {
                id: r.get(0)?,
                started_at: r.get(1)?,
                ended_at: r.get(2)?,
                project_id: r.get(3)?,
                task_id: r.get(4)?,
                category: r.get(5)?,
                summary: r.get(6)?,
                confidence: r.get(7)?,
                evidence_json: r.get(8)?,
                user_confirmed: r.get::<_, i64>(9)? != 0,
            }))?;
            collect_rows(mapped)?
        };
        let mut changed = 0;
        let mut i = 0;
        while i + 1 < rows.len() {
            let a = &rows[i];
            let b = &rows[i + 1];
            let same = !a.user_confirmed
                && !b.user_confirmed
                && a.project_id == b.project_id
                && a.task_id == b.task_id
                && a.category == b.category
                && a.summary == b.summary
                && within_gap(&a.ended_at, &b.started_at, 10);
            if same {
                let merged_evidence = merge_evidence_blobs(&format!("{}||{}", a.evidence_json, b.evidence_json));
                conn.execute("UPDATE work_sessions SET ended_at=?1, confidence=?2, evidence_json=?3, updated_at=?4 WHERE id=?5", params![b.ended_at, a.confidence.max(b.confidence), serde_json::to_string(&merged_evidence)?, now(), a.id])?;
                conn.execute("DELETE FROM work_sessions WHERE id=?1", params![b.id])?;
                changed += 1;
            }
            i += 1;
        }
        Ok(changed)
    }

    pub fn learn_rule_from_session(&self, id: &str) -> Result<AttributionRule> {
        let session = self.get_session(id)?.context("session not found")?;
        let strongest = session.evidence.iter()
            .max_by(|a, b| a.weight.partial_cmp(&b.weight).unwrap_or(std::cmp::Ordering::Equal))
            .map(|e| e.value.clone())
            .unwrap_or_else(|| session.summary.clone());
        let keyword = strongest.split(['/', '\\', '-', '|', '—']).next().unwrap_or(&strongest).trim();
        let rule = AttributionRule {
            id: Uuid::new_v4().to_string(),
            name: format!("自动学习：{}", session.summary.chars().take(24).collect::<String>()),
            priority: 90,
            matcher: serde_json::json!({ "keyword": keyword, "summary": session.summary }),
            project_id: session.project_id,
            task_id: session.task_id,
            category: session.category,
            created_from_correction: true,
            enabled: true,
        };
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO attribution_rules VALUES (?1,?2,?3,?4,?5,?6,?7,1,1)",
            params![rule.id, rule.name, rule.priority, rule.matcher.to_string(), rule.project_id, rule.task_id, rule.category],
        )?;
        Ok(rule)
    }

    pub fn retry_failed_jobs(&self) -> Result<u32> {
        let conn = self.conn.lock();
        let changed = conn.execute("UPDATE analysis_jobs SET status='pending', error=NULL WHERE status IN ('failed','downgraded')", [])?;
        Ok(changed as u32)
    }

    pub fn queue_health(&self) -> Result<QueueHealth> {
        let temp_storage_limit_gb = self.get_settings()?.temp_storage_limit_gb as f32;
        let conn = self.conn.lock();
        let count = |status: &str| -> Result<u32> {
            Ok(conn.query_row("SELECT COUNT(*) FROM analysis_jobs WHERE status=?1", params![status], |r| r.get::<_, i64>(0))? as u32)
        };
        Ok(QueueHealth {
            pending: count("pending")?, running: count("running")?, failed: count("failed")?, downgraded: count("downgraded")?,
            temp_storage_gb: dir_size(self.data_dir.join("media-cache")) as f32 / 1_073_741_824.0,
            temp_storage_limit_gb,
        })
    }

    pub fn list_plan_items(&self, limit: i64) -> Result<Vec<PlanItem>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id,source,title,note,start_at,due_at,status,tags_json,matched_session_ids_json FROM plan_items ORDER BY COALESCE(due_at,start_at,updated_at) ASC LIMIT ?1")?;
        let rows = stmt.query_map(params![limit], |r| {
            let tags: String = r.get(7)?;
            let matched: String = r.get(8)?;
            Ok(PlanItem { id: r.get(0)?, source: r.get(1)?, title: r.get(2)?, note: r.get(3)?, start_at: r.get(4)?, due_at: r.get(5)?, status: r.get(6)?, tags: parse_json(&tags), matched_session_ids: parse_json(&matched) })
        })?;
        collect_rows(rows)
    }

    pub fn upsert_plan_items(&self, items: &[PlanItem]) -> Result<usize> {
        let conn = self.conn.lock();
        let mut count = 0;
        for item in items {
            conn.execute("INSERT OR REPLACE INTO plan_items VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![item.id, item.source, item.title, item.note, item.start_at, item.due_at, item.status, serde_json::to_string(&item.tags)?, serde_json::to_string(&item.matched_session_ids)?, now()])?;
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
            WHERE ws.category != '离开'
            GROUP BY COALESCE(p.name, '未归类'), ws.category
            ORDER BY minutes DESC LIMIT 12
        "#)?;
        let rows = stmt.query_map([], |r| Ok(TrendPoint { label: r.get(0)?, value: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0), group: r.get(2)? }))?;
        collect_rows(rows)
    }

    pub fn category_trends(&self) -> Result<Vec<TrendPoint>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(r#"
            SELECT category, ROUND(SUM((julianday(ended_at)-julianday(started_at))*24*60), 1) AS minutes, category
            FROM work_sessions GROUP BY category ORDER BY minutes DESC
        "#)?;
        let rows = stmt.query_map([], |r| Ok(TrendPoint { label: r.get(0)?, value: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0), group: r.get(2)? }))?;
        collect_rows(rows)
    }

    pub fn export_path(&self, extension: &str) -> PathBuf {
        self.data_dir.join("exports").join(format!("screenuse-{}.{}", Utc::now().format("%Y%m%d-%H%M%S"), extension))
    }

    pub fn record_export(&self, format: &str, path: &Path) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("INSERT INTO export_records VALUES (?1,?2,?3,?4)", params![Uuid::new_v4().to_string(), format, path.display().to_string(), now()])?;
        Ok(())
    }

    pub fn backup_now(&self, target_dir: Option<String>) -> Result<PathBuf> {
        let dir = target_dir.map(PathBuf::from).unwrap_or_else(|| self.data_dir.join("backups"));
        fs::create_dir_all(&dir)?;
        let target = dir.join(format!("screenuse-backup-{}.db", Utc::now().format("%Y%m%d-%H%M%S")));
        fs::copy(&self.db_path, &target)?;
        Ok(target)
    }

}

fn starts_new_context(event: &RawActivityEvent) -> bool {
    event.metadata.get("contextStart").and_then(serde_json::Value::as_bool).unwrap_or(false)
}

fn insert_seed_session(conn: &Connection, id: &str, project_id: &str, task_id: &str, category: &str, summary: &str, start: chrono::DateTime<Utc>, end: chrono::DateTime<Utc>, confidence: f32) -> Result<()> {
    let evidence = vec![
        EvidenceItem { kind: "window".into(), label: "窗口".into(), value: "Codex / VS Code / Chrome".into(), weight: 0.7 },
        EvidenceItem { kind: "ai".into(), label: "AI摘要".into(), value: summary.into(), weight: 0.9 },
    ];
    conn.execute("INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,0,'seed',?10)", params![id, fmt(start), fmt(end), project_id, task_id, category, summary, confidence, serde_json::to_string(&evidence)?, now()])?;
    Ok(())
}

pub fn now() -> String { fmt(Utc::now()) }
fn fmt(t: chrono::DateTime<Utc>) -> String { t.to_rfc3339_opts(SecondsFormat::Secs, true) }

fn parse_json<T: DeserializeOwned + Default>(s: &str) -> T { serde_json::from_str(s).unwrap_or_default() }

fn clean_name(value: &str, fallback: &str) -> String {
    let cleaned = value.trim().replace(['\r', '\n', '\t'], " ");
    if cleaned.is_empty() { fallback.to_string() } else { cleaned.chars().take(80).collect() }
}

fn color_for_category(category: &str) -> &'static str {
    match category {
        "开发" => "#38bdf8",
        "学习" => "#a78bfa",
        "写作" => "#f0abfc",
        "沟通" => "#34d399",
        "娱乐" => "#fb7185",
        "离开" => "#94a3b8",
        _ => "#facc15",
    }
}

fn within_gap(left_end: &str, right_start: &str, max_minutes: i64) -> bool {
    let left = DateTime::parse_from_rfc3339(left_end).map(|t| t.with_timezone(&Utc));
    let right = DateTime::parse_from_rfc3339(right_start).map(|t| t.with_timezone(&Utc));
    match (left, right) {
        (Ok(left), Ok(right)) => right >= left && right - left <= Duration::minutes(max_minutes),
        _ => false,
    }
}

fn collect_rows<T>(rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for row in rows { out.push(row?); }
    Ok(out)
}

fn merge_evidence_blobs(blob: &str) -> Vec<EvidenceItem> {
    blob.split("||").flat_map(|part| serde_json::from_str::<Vec<EvidenceItem>>(part).unwrap_or_default()).take(20).collect()
}

fn dir_size(path: PathBuf) -> u64 {
    if !path.exists() { return 0; }
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() { total += meta.len(); }
                else if meta.is_dir() { total += dir_size(p); }
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_start_forces_a_new_session_boundary() {
        let event = RawActivityEvent {
            id: "context-1".into(), source: "test".into(), timestamp: now(), app: None,
            window_title: None, url: None, file_path: None, workspace: None,
            input_stats: InputStats::default(), metadata: serde_json::json!({ "contextStart": true }),
        };
        assert!(starts_new_context(&event));
    }

    #[test]
    fn dashboard_load_does_not_reenter_the_database_lock() {
        let data_dir = std::env::temp_dir().join(format!("screenuse-dashboard-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let dashboard = db.dashboard(false).expect("load dashboard");
        assert!(!dashboard.projects.is_empty());
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn deleting_a_project_unassigns_its_sessions_and_tasks() {
        let data_dir = std::env::temp_dir().join(format!("screenuse-project-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let project = db.create_project("误归类项目", "开发").expect("create project");
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
                category: None,
                confidence: None,
                user_confirmed: None,
            },
        )
        .expect("assign session");

        db.delete_project(&project.id).expect("delete project");
        let session = db.get_session(&session_id).expect("load session").expect("session remains");
        assert!(session.project_id.is_none());
        assert!(session.task_id.is_none());
        assert!(!db.list_projects().expect("list projects").iter().any(|item| item.id == project.id));

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }
}
