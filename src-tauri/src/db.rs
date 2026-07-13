use crate::classification;
use crate::models::*;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use directories::ProjectDirs;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::de::DeserializeOwned;

use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const ONE_SECOND_SAMPLING_MIGRATION_KEY: &str = "migration_sampling_1s_v1";
const IDLE_BOUNDARY_MIGRATION_KEY: &str = "migration_idle_boundary_v1";

pub struct AppDb {
    pub(crate) conn: Mutex<Connection>,
    db_path: PathBuf,
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
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self {
            conn: Mutex::new(conn),
            db_path,
            data_dir,
        };
        db.migrate()?;
        db.seed_if_empty()?;
        db.backfill_idle_boundaries()?;
        db.compact_sessions()?;
        Ok(db)
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
              enabled INTEGER NOT NULL DEFAULT 1,
              updated_at TEXT NOT NULL
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
            CREATE INDEX IF NOT EXISTS idx_raw_events_time ON raw_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_jobs_status ON analysis_jobs(status);
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
        conn.execute(
            "UPDATE activity_categories SET updated_at=created_at WHERE updated_at=''",
            [],
        )?;
        conn.execute(
            "UPDATE attribution_rules SET updated_at=?1 WHERE updated_at=''",
            params![now()],
        )?;
        for category in DEFAULT_CATEGORIES {
            conn.execute(
                "INSERT OR IGNORE INTO activity_categories(name,color,is_builtin,created_at,updated_at) VALUES(?1,?2,1,?3,?3)",
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
            &s1,
            &p1,
            &t1,
            "开发",
            "搭建 ScreenUse v1 项目骨架",
            base,
            base + Duration::minutes(75),
            0.86,
        )?;
        insert_seed_session(
            &conn,
            &s2,
            &p2,
            &t2,
            "学习",
            "阅读竞品与时间追踪资料",
            base + Duration::minutes(90),
            base + Duration::minutes(145),
            0.79,
        )?;
        insert_seed_session(
            &conn,
            &s3,
            &p1,
            &t1,
            "开发",
            "设计 AI 队列与失败重试策略",
            base + Duration::minutes(165),
            base + Duration::minutes(220),
            0.82,
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
            settings.heartbeat_seconds = 1;
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

    pub fn dashboard(&self, collector_running: bool) -> Result<DashboardData> {
        Ok(DashboardData {
            settings: self.get_settings()?.normalized(),
            sessions: self.list_sessions(80)?,
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

    pub fn delete_category(&self, name: &str) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let builtin = tx
            .query_row(
                "SELECT is_builtin FROM activity_categories WHERE name=?1",
                params![name],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .context("分类不存在或已经删除")?;
        if builtin != 0 {
            bail!("默认分类不能删除");
        }
        tx.execute(
            "UPDATE projects SET category='杂务', color=?1, updated_at=?2 WHERE category=?3",
            params![color_for_category("杂务"), now(), name],
        )?;
        tx.execute(
            "UPDATE work_sessions SET category='杂务', updated_at=?1 WHERE category=?2",
            params![now(), name],
        )?;
        tx.execute(
            "UPDATE attribution_rules SET category='杂务',updated_at=?1 WHERE category=?2",
            params![now(), name],
        )?;
        record_tombstone(&tx, "category", name)?;
        tx.execute(
            "DELETE FROM activity_categories WHERE name=?1",
            params![name],
        )?;
        tx.commit()?;
        Ok(())
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
        let category_exists = conn
            .query_row(
                "SELECT 1 FROM activity_categories WHERE name=?1 LIMIT 1",
                params![category],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if !category_exists {
            bail!("不支持的项目分类：{category}");
        }
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

    pub fn update_session(&self, id: &str, patch: SessionPatch) -> Result<WorkSession> {
        let current = self.get_session(id)?.context("session not found")?;
        let clear_project = patch.clear_project.unwrap_or(false);
        let clear_task = patch.clear_task.unwrap_or(false) || clear_project;
        let project_id = if clear_project {
            None
        } else {
            patch.project_id.or(current.project_id.clone())
        };
        let project_changed = project_id != current.project_id;
        let task_id = if clear_task {
            None
        } else if patch.task_id.is_some() {
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
        let mut updated = Vec::with_capacity(ids.len());
        for id in ids {
            updated.push(self.update_session(id, patch.clone())?);
        }
        Ok(updated)
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
             SET source='context-complete', updated_at=?1
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
        if !can_auto_coalesce(&previous, &current) {
            return Ok(current);
        }

        let summary = preferred_coalesced_summary(&previous.summary, &current.summary);
        let evidence = merge_evidence(&previous.evidence, &current.evidence);
        let confidence = previous.confidence.max(current.confidence);
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE work_sessions SET ended_at=?1,summary=?2,confidence=?3,evidence_json=?4,updated_at=?5 WHERE id=?6",
            params![current.ended_at, summary, confidence, serde_json::to_string(&evidence)?, now(), previous.id],
        )?;
        conn.execute(
            "UPDATE activities SET ended_at=?1,summary=?2,evidence_json=?3 WHERE session_id=?4",
            params![
                current.ended_at,
                summary,
                serde_json::to_string(&evidence)?,
                previous.id
            ],
        )?;

        let plan_updates = {
            let mut stmt = conn.prepare(
                "SELECT id,matched_session_ids_json FROM plan_items WHERE matched_session_ids_json LIKE ?1",
            )?;
            let rows = stmt.query_map(params![format!("%{}%", current.id)], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut updates = Vec::new();
            for row in rows {
                let (plan_id, matched_json) = row?;
                let mut matched: Vec<String> = parse_json(&matched_json);
                for session_id in &mut matched {
                    if session_id == &current.id {
                        *session_id = previous.id.clone();
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
        record_tombstone(&conn, "session", &current.id)?;
        conn.execute("DELETE FROM work_sessions WHERE id=?1", params![current.id])?;
        drop(conn);
        self.get_session(&previous.id)?
            .context("coalesced session missing")
    }

    pub fn merge_sessions(&self, ids: &[String], summary: Option<String>) -> Result<WorkSession> {
        anyhow::ensure!(!ids.is_empty(), "no session ids provided");
        let conn = self.conn.lock();
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT MIN(started_at), MAX(ended_at), project_id, task_id, category, GROUP_CONCAT(summary, ' / '), AVG(confidence), GROUP_CONCAT(evidence_json, '||') FROM work_sessions WHERE id IN ({})", placeholders);
        let row = {
            let mut stmt = conn.prepare(&sql)?;
            stmt.query_row(rusqlite::params_from_iter(ids), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, f32>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })?
        };
        let new_id = Uuid::new_v4().to_string();
        let evidence = merge_evidence_blobs(&row.7);
        conn.execute(
            "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,'manual-merge',?10)",
            params![
                new_id,
                row.0,
                row.1,
                row.2,
                row.3,
                row.4,
                summary.unwrap_or(row.5),
                row.6,
                serde_json::to_string(&evidence)?,
                now()
            ],
        )?;
        for id in ids {
            record_tombstone(&conn, "session", id)?;
            conn.execute("DELETE FROM work_sessions WHERE id=?1", params![id])?;
        }
        drop(conn);
        self.get_session(&new_id)?.context("merged session missing")
    }

    pub fn split_session(&self, id: &str, split_at: &str) -> Result<Vec<WorkSession>> {
        let session = self.get_session(id)?.context("session not found")?;
        anyhow::ensure!(
            split_at > session.started_at.as_str() && split_at < session.ended_at.as_str(),
            "split_at must be inside session range"
        );
        let first_id = Uuid::new_v4().to_string();
        let second_id = Uuid::new_v4().to_string();
        let evidence_json = serde_json::to_string(&session.evidence)?;
        let conn = self.conn.lock();
        record_tombstone(&conn, "session", id)?;
        conn.execute("DELETE FROM work_sessions WHERE id=?1", params![id])?;
        conn.execute(
            "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,'manual-split',?10)",
            params![
                first_id,
                session.started_at,
                split_at,
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
        conn.execute(
            "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,'manual-split',?10)",
            params![
                second_id,
                split_at,
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
                input_stats,
                metadata
            ],
        )?;
        let changed = conn.execute(
            "UPDATE work_sessions SET ended_at=?1, updated_at=?2 WHERE id=?3",
            params![event.timestamp, now(), session_id],
        )?;
        anyhow::ensure!(changed == 1, "active session disappeared during heartbeat");
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
        let (project_id, task_id, category, summary, confidence) =
            self.heuristic_attribution(event, is_idle)?;
        let page_title = event
            .metadata
            .get("activePageTitle")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty());
        let evidence = vec![
            EvidenceItem {
                kind: if page_title.is_some() {
                    "page".into()
                } else {
                    "window".into()
                },
                label: if page_title.is_some() {
                    "当前页面".into()
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
        let conn = self.conn.lock();
        let last: Option<(String, String, String, String, i64)> = conn.query_row(
            "SELECT id, summary, category, ended_at, user_confirmed FROM work_sessions ORDER BY ended_at DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        ).optional()?;
        if let Some((id, last_summary, last_category, _last_end, confirmed)) = last {
            if !starts_new_context(event)
                && confirmed == 0
                && last_summary == summary
                && last_category == category
            {
                conn.execute("UPDATE work_sessions SET ended_at=?1, confidence=MAX(confidence, ?2), evidence_json=?3, updated_at=?4 WHERE id=?5", params![event.timestamp, confidence, serde_json::to_string(&evidence)?, now(), id])?;
                return Ok(());
            }
        }
        conn.execute(
            "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,0,'collector-rule',?10)",
            params![
                Uuid::new_v4().to_string(),
                event.timestamp,
                event.timestamp,
                project_id,
                task_id,
                category,
                summary,
                confidence,
                serde_json::to_string(&evidence)?,
                now()
            ],
        )?;
        Ok(())
    }

    fn heuristic_attribution(
        &self,
        event: &RawActivityEvent,
        is_idle: bool,
    ) -> Result<(Option<String>, Option<String>, String, String, f32)> {
        if is_idle {
            return Ok((None, None, "离开".into(), "离开/空闲".into(), 0.96));
        }
        if let Some(pin) = self.active_context()? {
            return Ok((
                Some(pin.project_id),
                pin.task_id,
                pin.category.clone(),
                classification::summary_for_event(event, &pin.category),
                0.98,
            ));
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
            let mut stmt = conn.prepare("SELECT matcher_json,project_id,task_id,category,name FROM attribution_rules WHERE enabled=1 ORDER BY priority DESC")?;
            let mapped = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?;
            collect_rows(mapped)?
        };
        for (matcher_json, project_id, task_id, category, _name) in rules {
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
            let hit = has_constraint
                && (keywords.is_empty() || keywords.iter().any(|keyword| hay.contains(keyword)))
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
                return Ok((
                    project_id,
                    task_id,
                    category.clone(),
                    classification::summary_for_event(event, &category),
                    0.84,
                ));
            }
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
        Ok((
            None,
            None,
            category.into(),
            classification::summary_for_event(event, category),
            0.55,
        ))
    }

    pub fn create_analysis_job(&self, job: &AnalysisJob) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO analysis_jobs VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                job.id,
                serde_json::to_string(&job.chunk_ids)?,
                job.metadata_range.started_at,
                job.metadata_range.ended_at,
                job.mode,
                job.retry_count,
                job.status,
                job.error
            ],
        )?;
        Ok(())
    }

    pub fn claim_next_analysis_job(&self) -> Result<Option<AnalysisJob>> {
        let conn = self.conn.lock();
        let row: Option<(String, String, String, String, String, u32, String, Option<String>)> = conn.query_row(
            "SELECT id,chunk_ids_json,started_at,ended_at,mode,retry_count,status,error FROM analysis_jobs WHERE status='pending' ORDER BY started_at ASC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get::<_, i64>(5)? as u32, r.get(6)?, r.get(7)?)),
        ).optional()?;
        if let Some((id, chunk_ids_json, started_at, ended_at, mode, retry_count, _status, error)) =
            row
        {
            conn.execute("UPDATE analysis_jobs SET status='running', error=NULL WHERE id=?1 AND status='pending'", params![id])?;
            Ok(Some(AnalysisJob {
                id,
                chunk_ids: parse_json(&chunk_ids_json),
                metadata_range: TimeRange {
                    started_at,
                    ended_at,
                },
                mode,
                retry_count,
                status: "running".into(),
                error,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn mark_analysis_job_status(
        &self,
        id: &str,
        status: &str,
        retry_count: Option<u32>,
        error: Option<String>,
    ) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE analysis_jobs SET status=?1, retry_count=COALESCE(?2,retry_count), error=?3 WHERE id=?4",
            params![status, retry_count.map(|v| v as i64), error, id],
        )?;
        Ok(())
    }

    pub fn list_raw_events_between(
        &self,
        started_at: &str,
        ended_at: &str,
    ) -> Result<Vec<RawActivityEvent>> {
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
            params![
                Uuid::new_v4().to_string(),
                id,
                source,
                "自动归因活动",
                summary,
                range.started_at,
                range.ended_at,
                serde_json::to_string(&evidence)?
            ],
        )?;
        drop(conn);
        self.match_plan_items_for_session(&id)?;
        self.get_session(&id)?.context("inserted session missing")
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

    pub fn compact_sessions(&self) -> Result<u32> {
        let ids = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare("SELECT id FROM work_sessions ORDER BY started_at ASC")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            collect_rows(rows)?
        };
        let mut changed = 0;
        for id in ids {
            if self.get_session(&id)?.is_some() {
                let session = self.coalesce_session_neighbors(&id)?;
                if session.id != id {
                    changed += 1;
                }
            }
        }
        Ok(changed)
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
            .find(|item| item.kind == "window")
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
            if let Some(candidate) = candidate
                .map(str::trim)
                .filter(|value| value.chars().count() >= 2)
            {
                keywords.push(candidate.chars().take(48).collect());
            }
        }
        let normalized_window = window.to_lowercase();
        if !window.is_empty()
            && normalized_window != app
            && !is_generic_context_label(&normalized_window)
        {
            keywords.push(window.chars().take(64).collect());
        }
        keywords.sort();
        keywords.dedup();
        if keywords.is_empty() {
            bail!("当前窗口没有可区分线索，请填写识别词或固定当前事务");
        }
        let mut matcher = serde_json::json!({ "keywords": keywords });
        if !app.is_empty() {
            matcher["app"] = serde_json::Value::String(app);
        }
        let rule = AttributionRule {
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
        conn.execute(
            "INSERT INTO attribution_rules(id,name,priority,matcher_json,project_id,task_id,category,created_from_correction,enabled,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,1,1,?8)",
            params![rule.id, rule.name, rule.priority, rule.matcher.to_string(), rule.project_id, rule.task_id, rule.category, now()],
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
            WHERE ws.category != '离开'
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
        let target = dir.join(format!(
            "screenuse-backup-{}.db",
            Utc::now().format("%Y%m%d-%H%M%S")
        ));
        fs::copy(&self.db_path, &target)?;
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

fn insert_seed_session(
    conn: &Connection,
    id: &str,
    project_id: &str,
    task_id: &str,
    category: &str,
    summary: &str,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
    confidence: f32,
) -> Result<()> {
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
            value: summary.into(),
            weight: 0.9,
        },
    ];
    conn.execute(
        "INSERT INTO work_sessions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,0,'seed',?10)",
        params![
            id,
            fmt(start),
            fmt(end),
            project_id,
            task_id,
            category,
            summary,
            confidence,
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

fn can_auto_coalesce(left: &WorkSession, right: &WorkSession) -> bool {
    if left.user_confirmed
        || right.user_confirmed
        || left.source != "context-complete"
        || right.source != "context-complete"
        || left.project_id != right.project_id
        || left.task_id != right.task_id
        || left.category != right.category
        || !within_gap_seconds(&left.ended_at, &right.started_at, 3)
    {
        return false;
    }
    let left_app = primary_session_app(left);
    let right_app = primary_session_app(right);
    left_app.is_some()
        && left_app == right_app
        && (left.project_id.is_some() || left.task_id.is_some() || left.summary == right.summary)
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
    ["图片查看器", "无标题", "新标签页", "loading", "加载中"]
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

fn is_generic_context_label(value: &str) -> bool {
    matches!(
        value.trim().trim_end_matches(".exe"),
        "chatgpt"
            | "codex"
            | "chrome"
            | "msedge"
            | "firefox"
            | "brave"
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

fn merge_evidence_blobs(blob: &str) -> Vec<EvidenceItem> {
    blob.split("||")
        .flat_map(|part| serde_json::from_str::<Vec<EvidenceItem>>(part).unwrap_or_default())
        .take(20)
        .collect()
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
        assert_eq!(session.evidence[0].label, "当前页面");
        assert_eq!(session.evidence[0].value, "ICPC 训练计划.docx");
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
        let mut settings = AppSettings::default();
        settings.poll_interval_seconds = 10;
        settings.heartbeat_seconds = 10;
        db.save_settings(&settings).expect("save legacy settings");

        let migrated = db.get_settings().expect("migrate settings");
        assert_eq!(migrated.poll_interval_seconds, 1);
        assert_eq!(migrated.heartbeat_seconds, 1);

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
    fn custom_categories_and_tasks_can_be_created_and_removed() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-taxonomy-test-{}", Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let category = db.create_category("竞赛").expect("create category");
        let project = db
            .create_project("ICPC", &category.name)
            .expect("create project");
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
        db.delete_category(&category.name).expect("delete category");
        let updated = db
            .list_projects()
            .expect("list projects")
            .into_iter()
            .find(|item| item.id == project.id)
            .expect("project remains");
        assert_eq!(updated.category, "杂务");
        drop(db);
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

        event.timestamp = fmt(base + Duration::seconds(10));
        event.metadata = serde_json::json!({"heartbeat": true});
        db.heartbeat_raw_event(&event, &session.id)
            .expect("extend known session");

        let extended = db
            .get_session(&session.id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(extended.started_at, fmt(base));
        assert_eq!(extended.ended_at, fmt(base + Duration::seconds(10)));
        assert_eq!(extended.summary, session.summary);

        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }
}
