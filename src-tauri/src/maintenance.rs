use crate::db::AppDb;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use tokio::time::{sleep, Duration};

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
    let mut removed = remove_directory_children(&db.data_dir().join("media-cache"))?;

    let conn = Connection::open(&db_path).with_context(|| format!("cannot open {}", db_path.display()))?;
    conn.busy_timeout(StdDuration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA temp_store=MEMORY;
         PRAGMA foreign_keys=ON;",
    )?;

    let retention = format!("-{} days", settings.raw_event_retention_days.clamp(7, 3650));
    removed += conn.execute(
        "DELETE FROM raw_events WHERE julianday(timestamp) < julianday('now', ?1)",
        params![retention],
    )? as u32;
    removed += conn.execute(
        "DELETE FROM analysis_jobs
         WHERE status IN ('completed','failed','downgraded')
           AND julianday(ended_at) < julianday('now', ?1)",
        params![retention],
    )? as u32;

    // v0.1 created screenshot-backed jobs and sample sessions. The metadata-first
    // runtime never needs those rows or files.
    removed += conn.execute("DELETE FROM analysis_jobs WHERE chunk_ids_json <> '[]'", [])? as u32;
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
