use crate::db::AppDb;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use tokio::time::{sleep, Duration};

const AI_TRACE_RETENTION_DAYS: u32 = 14;
const TOMBSTONE_RETENTION_DAYS: u32 = 180;
const MAX_AI_JOB_HISTORY: i64 = 1000;
const MAX_CORRECTION_RULES: i64 = 2000;
const MAX_PLAN_SESSION_LINKS: usize = 256;

pub fn initialize(db: &AppDb) -> Result<u32> {
    let settings = db.get_settings()?.normalized();
    db.save_settings(&settings)?;
    optimize_storage(db, false)
}

pub fn start_worker(db: Arc<AppDb>) {
    tauri::async_runtime::spawn(async move {
        loop {
            sleep(Duration::from_secs(6 * 60 * 60)).await;
            let settings = match db.get_settings() {
                Ok(settings) => settings.normalized(),
                Err(error) => {
                    eprintln!("ScreenUse maintenance settings error: {error}");
                    continue;
                }
            };
            if settings.auto_maintenance {
                if let Err(error) = optimize_storage(&db, false) {
                    eprintln!("ScreenUse maintenance error: {error}");
                }
            }
        }
    });
}

pub fn optimize_storage(db: &AppDb, aggressive: bool) -> Result<u32> {
    let settings = db.get_settings()?.normalized();
    let db_path = db.data_dir().join("screenuse.db");
    let mut removed = db.repair_session_timeline()?;
    removed += db.compact_sessions()?;
    removed += remove_directory_children(&db.data_dir().join("media-cache"))?;

    let conn = Connection::open(&db_path).with_context(|| format!("cannot open {}", db_path.display()))?;
    conn.busy_timeout(StdDuration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA temp_store=MEMORY;
         PRAGMA foreign_keys=ON;
         PRAGMA cache_size=-2000;
         PRAGMA wal_autocheckpoint=256;
         PRAGMA journal_size_limit=1048576;",
    )?;

    let retention = format!("-{} days", settings.raw_event_retention_days.clamp(7, 3650));
    removed += conn.execute(
        "DELETE FROM raw_events WHERE julianday(timestamp) < julianday('now', ?1)",
        params![retention],
    )? as u32;
    removed += conn.execute(
        "DELETE FROM analysis_jobs
         WHERE status IN ('completed','failed','downgraded','skipped')
           AND julianday(ended_at) < julianday('now', ?1)",
        params![retention],
    )? as u32;
    let ai_trace_retention = format!("-{AI_TRACE_RETENTION_DAYS} days");
    removed += conn.execute(
        "UPDATE analysis_jobs
         SET system_prompt=NULL,user_prompt=NULL,response=NULL
         WHERE status IN ('completed','failed','downgraded','skipped')
           AND julianday(COALESCE(completed_at,ended_at)) < julianday('now', ?1)
           AND (system_prompt IS NOT NULL OR user_prompt IS NOT NULL OR response IS NOT NULL)",
        params![ai_trace_retention],
    )? as u32;
    removed += conn.execute(
        "DELETE FROM analysis_jobs
         WHERE id IN (
           SELECT id FROM analysis_jobs
           WHERE status IN ('completed','failed','downgraded','skipped')
           ORDER BY COALESCE(completed_at,queued_at,ended_at) DESC
           LIMIT -1 OFFSET ?1
         )",
        params![MAX_AI_JOB_HISTORY],
    )? as u32;

    removed += prune_correction_rules(&conn)?;
    removed += prune_plan_session_links(&conn)?;
    let tombstone_retention = format!("-{TOMBSTONE_RETENTION_DAYS} days");
    removed += conn.execute(
        "DELETE FROM sync_tombstones
         WHERE julianday(deleted_at) < julianday('now', ?1)",
        params![tombstone_retention],
    )? as u32;

    // v0.1 media chunks are obsolete. Current metadata AI jobs remain as an audit trail.
    removed += conn.execute("DELETE FROM media_chunks", [])? as u32;
    removed += conn.execute("DELETE FROM work_sessions WHERE source='seed'", [])? as u32;

    conn.execute_batch("PRAGMA optimize; PRAGMA wal_checkpoint(PASSIVE);")?;
    if aggressive {
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
    }
    Ok(removed)
}

pub fn checkpoint(db: &AppDb) -> Result<()> {
    let db_path = db.data_dir().join("screenuse.db");
    let conn = Connection::open(&db_path).with_context(|| format!("cannot open {}", db_path.display()))?;
    conn.busy_timeout(StdDuration::from_secs(5))?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

fn remove_directory_children(path: &Path) -> Result<u32> {
    if !path.exists() { return Ok(0); }
    let mut removed = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        let result = if child.is_dir() {
            fs::remove_dir_all(&child)
        } else {
            fs::remove_file(&child)
        };
        if result.is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

fn prune_correction_rules(conn: &Connection) -> Result<u32> {
    let overflow = format!(
        "SELECT id FROM attribution_rules
         WHERE created_from_correction=1
         ORDER BY updated_at DESC
         LIMIT -1 OFFSET {MAX_CORRECTION_RULES}"
    );
    conn.execute_batch(&format!(
        "INSERT OR REPLACE INTO sync_tombstones(entity_kind,entity_id,deleted_at,device_id)
         SELECT 'rule',id,strftime('%Y-%m-%dT%H:%M:%SZ','now'),''
         FROM attribution_rules WHERE id IN ({overflow});"
    ))?;
    Ok(conn.execute(
        &format!("DELETE FROM attribution_rules WHERE id IN ({overflow})"),
        [],
    )? as u32)
}

fn prune_plan_session_links(conn: &Connection) -> Result<u32> {
    let mut stmt = conn.prepare(
        "SELECT id,matched_session_ids_json FROM plan_items
         WHERE length(matched_session_ids_json)>2",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut updates = Vec::new();
    for row in rows {
        let (id, json) = row?;
        let mut ids: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
        if ids.len() <= MAX_PLAN_SESSION_LINKS {
            continue;
        }
        ids.drain(..ids.len() - MAX_PLAN_SESSION_LINKS);
        updates.push((id, serde_json::to_string(&ids)?));
    }
    drop(stmt);
    for (id, json) in &updates {
        conn.execute(
            "UPDATE plan_items SET matched_session_ids_json=?1 WHERE id=?2",
            params![json, id],
        )?;
    }
    Ok(updates.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::now;
    use crate::models::{AiUsage, AnalysisJob, TimeRange};
    use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
    use rusqlite::params;
    use uuid::Uuid;

    #[test]
    fn maintenance_bounds_derived_history_and_ai_traces() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-storage-maintenance-test-{}",
            Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let old = (Utc::now() - ChronoDuration::days(20))
            .to_rfc3339_opts(SecondsFormat::Secs, true);
        db.create_analysis_job(&AnalysisJob {
            id: "old-ai-trace".into(),
            chunk_ids: vec!["session-a".into()],
            metadata_range: TimeRange {
                started_at: old.clone(),
                ended_at: old.clone(),
            },
            mode: "metadata-context-review".into(),
            provider: "codex-account".into(),
            model: "test-model".into(),
            retry_count: 0,
            status: "completed".into(),
            error: None,
            system_prompt: Some("system".repeat(100)),
            user_prompt: Some("user".repeat(100)),
            response: Some("response".repeat(100)),
            queued_at: old.clone(),
            processing_started_at: Some(old.clone()),
            completed_at: Some(old),
            duration_ms: Some(1000),
            result_count: 1,
            usage: AiUsage::default(),
        })
        .expect("insert AI trace");

        {
            let mut conn = db.conn.lock();
            let tx = conn.transaction().expect("start fixture transaction");
            let stale_tombstone = (Utc::now() - ChronoDuration::days(181))
                .to_rfc3339_opts(SecondsFormat::Secs, true);
            tx.execute(
                "INSERT INTO sync_tombstones(entity_kind,entity_id,deleted_at,device_id)
                 VALUES('session','stale',?1,'')",
                params![stale_tombstone],
            )
            .expect("insert old tombstone");
            for index in 0..(MAX_CORRECTION_RULES + 5) {
                tx.execute(
                    "INSERT INTO attribution_rules(
                       id,name,priority,matcher_json,project_id,task_id,category,
                       created_from_correction,enabled,updated_at
                     ) VALUES(?1,?2,100,'{}',NULL,NULL,'杂务',1,1,?3)",
                    params![format!("rule-{index}"), format!("Rule {index}"), now()],
                )
                .expect("insert correction rule");
            }
            let links = (0..300)
                .map(|index| format!("session-{index}"))
                .collect::<Vec<_>>();
            tx.execute(
                "INSERT INTO plan_items(
                   id,source,title,note,start_at,due_at,status,tags_json,
                   matched_session_ids_json,updated_at
                 ) VALUES('plan','test','Plan',NULL,NULL,NULL,'active','[]',?1,?2)",
                params![serde_json::to_string(&links).unwrap(), now()],
            )
            .expect("insert plan links");
            tx.commit().expect("commit fixtures");
        }

        optimize_storage(&db, false).expect("run maintenance");

        let detail = db
            .get_analysis_job("old-ai-trace")
            .expect("load AI job")
            .expect("AI history remains");
        assert!(detail.system_prompt.is_none());
        assert!(detail.user_prompt.is_none());
        assert!(detail.response.is_none());
        let conn = db.conn.lock();
        let rules: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attribution_rules WHERE created_from_correction=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rules, MAX_CORRECTION_RULES);
        let tombstones: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_tombstones WHERE entity_id='stale'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tombstones, 0);
        let links: String = conn
            .query_row(
                "SELECT matched_session_ids_json FROM plan_items WHERE id='plan'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Vec<String>>(&links).unwrap().len(), 256);
        assert_eq!(conn.pragma_query_value(None, "wal_autocheckpoint", |row| row.get::<_, i64>(0)).unwrap(), 256);
        drop(conn);
        drop(db);
        let _ = fs::remove_dir_all(data_dir);
    }
}
