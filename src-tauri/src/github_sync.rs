use crate::db::AppDb;
use crate::models::{
    AttributionRule, GithubSyncConfig, GithubSyncResult, GithubSyncStatus, Project, SyncCounts,
    SyncDeviceInfo, Task, WorkSession,
};
use crate::secrets;
use crate::sleep_debt;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{DateTime, Duration, NaiveDate, SecondsFormat, Utc};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rand::rngs::OsRng;
use rand::RngCore;
use reqwest::{Client, StatusCode, Url};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex as AsyncMutex;

const CONFIG_KEY: &str = "github_sync_config_v1";
const DEVICES_KEY: &str = "github_sync_devices_v1";
const SNAPSHOT_SCHEMA: u32 = 1;
const API_ROOT: &str = "https://api.github.com";
const USER_AGENT: &str = "ScreenUse/0.2";

static SYNC_LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncCategory {
    name: String,
    color: String,
    is_builtin: bool,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncSession {
    #[serde(flatten)]
    session: WorkSession,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncRule {
    #[serde(flatten)]
    rule: AttributionRule,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncTombstone {
    entity_kind: String,
    entity_id: String,
    deleted_at: String,
    device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncSnapshot {
    schema_version: u32,
    generated_at: String,
    generated_by: String,
    categories: Vec<SyncCategory>,
    projects: Vec<Project>,
    tasks: Vec<Task>,
    sessions: Vec<SyncSession>,
    rules: Vec<SyncRule>,
    tombstones: Vec<SyncTombstone>,
    devices: Vec<SyncDeviceInfo>,
    #[serde(default)]
    sleep_debt_started_on: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EncryptedEnvelope {
    version: u32,
    algorithm: String,
    compression: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, Deserialize)]
struct GithubContent {
    sha: String,
    content: String,
    encoding: String,
}

#[derive(Debug, Deserialize)]
struct GithubWriteContent {
    content: GithubWriteItem,
}

#[derive(Debug, Deserialize)]
struct GithubWriteItem {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GithubRepo {
    private: bool,
    default_branch: String,
}

#[derive(Debug, Deserialize)]
struct GithubUser {
    login: String,
}

struct RemoteSnapshot {
    snapshot: SyncSnapshot,
    sha: String,
    encrypted_bytes: u64,
}

impl AppDb {
    pub fn get_github_sync_config(&self) -> Result<GithubSyncConfig> {
        let conn = self.conn.lock();
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                params![CONFIG_KEY],
                |row| row.get(0),
            )
            .optional()?;
        Ok(raw
            .as_deref()
            .and_then(|value| serde_json::from_str::<GithubSyncConfig>(value).ok())
            .unwrap_or_default()
            .normalized())
    }

    pub fn save_github_sync_config(&self, config: &GithubSyncConfig) -> Result<()> {
        let config = config.clone().normalized();
        self.conn.lock().execute(
            "INSERT INTO settings(key,value,updated_at) VALUES(?1,?2,?3)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
            params![CONFIG_KEY, serde_json::to_string(&config)?, now()],
        )?;
        Ok(())
    }

    fn sync_counts(&self) -> Result<SyncCounts> {
        let conn = self.conn.lock();
        let count = |table: &str| -> Result<u32> {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            Ok(conn.query_row(&sql, [], |row| row.get::<_, i64>(0))? as u32)
        };
        Ok(SyncCounts {
            categories: count("activity_categories")?,
            projects: count("projects")?,
            tasks: count("tasks")?,
            sessions: count("work_sessions")?,
            rules: count("attribution_rules")?,
        })
    }

    fn sync_devices(&self) -> Result<Vec<SyncDeviceInfo>> {
        let conn = self.conn.lock();
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                params![DEVICES_KEY],
                |row| row.get(0),
            )
            .optional()?;
        Ok(raw
            .as_deref()
            .and_then(|value| serde_json::from_str::<Vec<SyncDeviceInfo>>(value).ok())
            .unwrap_or_default())
    }

    fn save_sync_devices(&self, devices: &[SyncDeviceInfo]) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO settings(key,value,updated_at) VALUES(?1,?2,?3)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
            params![DEVICES_KEY, serde_json::to_string(devices)?, now()],
        )?;
        Ok(())
    }

    fn export_sync_snapshot(&self, config: &GithubSyncConfig) -> Result<SyncSnapshot> {
        let conn = self.conn.lock();

        let categories = {
            let mut stmt = conn.prepare(
                "SELECT name,color,is_builtin,created_at,updated_at FROM activity_categories",
            )?;
            let values = collect(stmt.query_map([], |row| {
                Ok(SyncCategory {
                    name: row.get(0)?,
                    color: row.get(1)?,
                    is_builtin: row.get::<_, i64>(2)? != 0,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })?)?;
            values
        };
        let projects = {
            let mut stmt = conn.prepare(
                "SELECT id,name,category,source,color,description,created_at,updated_at FROM projects",
            )?;
            let values = collect(stmt.query_map([], |row| {
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
            })?)?;
            values
        };
        let tasks = {
            let mut stmt = conn.prepare(
                "SELECT id,project_id,title,status,source,planned_due_at,created_at,updated_at FROM tasks",
            )?;
            let values = collect(stmt.query_map([], |row| {
                Ok(Task {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    title: row.get(2)?,
                    status: row.get(3)?,
                    source: row.get(4)?,
                    planned_due_at: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?)?;
            values
        };
        let sessions = {
            let mut stmt = conn.prepare(
                r#"SELECT ws.id,ws.started_at,ws.ended_at,ws.project_id,p.name,ws.task_id,t.title,
                          ws.category,ws.summary,ws.confidence,ws.evidence_json,ws.user_confirmed,
                          ws.source,ws.updated_at
                   FROM work_sessions ws
                   LEFT JOIN projects p ON p.id=ws.project_id
                   LEFT JOIN tasks t ON t.id=ws.task_id"#,
            )?;
            let values = collect(stmt.query_map([], |row| {
                let evidence: String = row.get(10)?;
                Ok(SyncSession {
                    session: WorkSession {
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
                        evidence: serde_json::from_str(&evidence).unwrap_or_default(),
                        user_confirmed: row.get::<_, i64>(11)? != 0,
                        source: row.get(12)?,
                    },
                    updated_at: row.get(13)?,
                })
            })?)?;
            values
        };
        let rules = {
            let mut stmt = conn.prepare(
                "SELECT id,name,priority,matcher_json,project_id,task_id,category,created_from_correction,enabled,updated_at FROM attribution_rules",
            )?;
            let values = collect(stmt.query_map([], |row| {
                let matcher: String = row.get(3)?;
                Ok(SyncRule {
                    rule: AttributionRule {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        priority: row.get(2)?,
                        matcher: serde_json::from_str(&matcher).unwrap_or(Value::Null),
                        project_id: row.get(4)?,
                        task_id: row.get(5)?,
                        category: row.get(6)?,
                        created_from_correction: row.get::<_, i64>(7)? != 0,
                        enabled: row.get::<_, i64>(8)? != 0,
                    },
                    updated_at: row.get(9)?,
                })
            })?)?;
            values
        };
        let tombstones = {
            let mut stmt = conn.prepare(
                "SELECT entity_kind,entity_id,deleted_at,device_id FROM sync_tombstones",
            )?;
            let values = collect(stmt.query_map([], |row| {
                Ok(SyncTombstone {
                    entity_kind: row.get(0)?,
                    entity_id: row.get(1)?,
                    deleted_at: row.get(2)?,
                    device_id: row.get::<_, String>(3)?,
                })
            })?)?;
            values
        };
        let sleep_debt_started_on = conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                params![sleep_debt::START_DATE_SETTING_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        drop(conn);

        let generated_at = now();
        let mut devices = self.sync_devices()?;
        upsert_device(
            &mut devices,
            SyncDeviceInfo {
                id: config.device_id.clone(),
                name: config.device_name.clone(),
                last_seen_at: generated_at.clone(),
            },
        );
        Ok(SyncSnapshot {
            schema_version: SNAPSHOT_SCHEMA,
            generated_at,
            generated_by: config.device_id.clone(),
            categories,
            projects,
            tasks,
            sessions,
            rules,
            tombstones,
            devices,
            sleep_debt_started_on,
        })
    }

    fn apply_sync_snapshot(&self, snapshot: &SyncSnapshot) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        for category in &snapshot.categories {
            tx.execute(
                "INSERT INTO activity_categories(name,color,is_builtin,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(name) DO UPDATE SET color=excluded.color,is_builtin=MAX(activity_categories.is_builtin,excluded.is_builtin),updated_at=excluded.updated_at
                 WHERE excluded.updated_at>=activity_categories.updated_at",
                params![category.name, category.color, category.is_builtin as i64, category.created_at, category.updated_at],
            )?;
        }
        for project in &snapshot.projects {
            tx.execute(
                "INSERT INTO projects(id,name,category,source,color,description,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name,category=excluded.category,source=excluded.source,color=excluded.color,description=excluded.description,updated_at=excluded.updated_at
                 WHERE excluded.updated_at>=projects.updated_at",
                params![project.id, project.name, project.category, project.source, project.color, project.description, project.created_at, project.updated_at],
            )?;
        }
        for task in &snapshot.tasks {
            tx.execute(
                "INSERT INTO tasks(id,project_id,title,status,source,planned_due_at,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id,title=excluded.title,status=excluded.status,source=excluded.source,planned_due_at=excluded.planned_due_at,updated_at=excluded.updated_at
                 WHERE excluded.updated_at>=tasks.updated_at",
                params![task.id, task.project_id, task.title, task.status, task.source, task.planned_due_at, task.created_at, task.updated_at],
            )?;
        }
        for item in &snapshot.sessions {
            let session = &item.session;
            tx.execute(
                "INSERT INTO work_sessions(id,started_at,ended_at,project_id,task_id,category,summary,confidence,evidence_json,user_confirmed,source,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
                 ON CONFLICT(id) DO UPDATE SET started_at=excluded.started_at,ended_at=excluded.ended_at,project_id=excluded.project_id,task_id=excluded.task_id,category=excluded.category,summary=excluded.summary,confidence=excluded.confidence,evidence_json=excluded.evidence_json,user_confirmed=excluded.user_confirmed,source=excluded.source,updated_at=excluded.updated_at
                 WHERE excluded.updated_at>=work_sessions.updated_at",
                params![session.id, session.started_at, session.ended_at, session.project_id, session.task_id, session.category, session.summary, session.confidence, serde_json::to_string(&session.evidence)?, session.user_confirmed as i64, session.source, item.updated_at],
            )?;
        }
        for item in &snapshot.rules {
            let rule = &item.rule;
            tx.execute(
                "INSERT INTO attribution_rules(id,name,priority,matcher_json,project_id,task_id,category,created_from_correction,enabled,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name,priority=excluded.priority,matcher_json=excluded.matcher_json,project_id=excluded.project_id,task_id=excluded.task_id,category=excluded.category,created_from_correction=excluded.created_from_correction,enabled=excluded.enabled,updated_at=excluded.updated_at
                 WHERE excluded.updated_at>=attribution_rules.updated_at",
                params![rule.id, rule.name, rule.priority, rule.matcher.to_string(), rule.project_id, rule.task_id, rule.category, rule.created_from_correction as i64, rule.enabled as i64, item.updated_at],
            )?;
        }

        for tombstone in &snapshot.tombstones {
            tx.execute(
                "INSERT INTO sync_tombstones(entity_kind,entity_id,deleted_at,device_id)
                 VALUES(?1,?2,?3,?4)
                 ON CONFLICT(entity_kind,entity_id) DO UPDATE SET deleted_at=excluded.deleted_at,device_id=excluded.device_id
                 WHERE excluded.deleted_at>=sync_tombstones.deleted_at",
                params![tombstone.entity_kind, tombstone.entity_id, tombstone.deleted_at, tombstone.device_id],
            )?;
            match tombstone.entity_kind.as_str() {
                "project" => {
                    tx.execute(
                        "DELETE FROM attribution_rules WHERE project_id=?1 OR task_id IN (SELECT id FROM tasks WHERE project_id=?1)",
                        params![tombstone.entity_id],
                    )?;
                    tx.execute(
                        "DELETE FROM projects WHERE id=?1",
                        params![tombstone.entity_id],
                    )?;
                }
                "task" => {
                    tx.execute(
                        "DELETE FROM attribution_rules WHERE task_id=?1",
                        params![tombstone.entity_id],
                    )?;
                    tx.execute(
                        "DELETE FROM tasks WHERE id=?1",
                        params![tombstone.entity_id],
                    )?;
                }
                "category" => {
                    let (fallback_name, fallback_color) = tx
                        .query_row(
                            "SELECT name,color FROM activity_categories
                             WHERE name<>?1
                             ORDER BY CASE WHEN name='杂务' THEN 0 ELSE 1 END,is_builtin DESC,created_at ASC
                             LIMIT 1",
                            params![tombstone.entity_id],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                        )
                        .optional()?
                        .context("同步删除分类时找不到可用的替代分类")?;
                    tx.execute(
                        "UPDATE projects SET category=?1,color=?2,updated_at=?3 WHERE category=?4",
                        params![
                            fallback_name,
                            fallback_color,
                            tombstone.deleted_at,
                            tombstone.entity_id
                        ],
                    )?;
                    tx.execute(
                        "UPDATE work_sessions SET category=?1,updated_at=?2 WHERE category=?3",
                        params![fallback_name, tombstone.deleted_at, tombstone.entity_id],
                    )?;
                    tx.execute(
                        "UPDATE attribution_rules SET category=?1,updated_at=?2 WHERE category=?3",
                        params![fallback_name, tombstone.deleted_at, tombstone.entity_id],
                    )?;
                    tx.execute(
                        "DELETE FROM activity_categories WHERE name=?1",
                        params![tombstone.entity_id],
                    )?;
                }
                "session" => {
                    tx.execute(
                        "DELETE FROM work_sessions WHERE id=?1",
                        params![tombstone.entity_id],
                    )?;
                }
                "rule" => {
                    tx.execute(
                        "DELETE FROM attribution_rules WHERE id=?1",
                        params![tombstone.entity_id],
                    )?;
                }
                _ => {}
            }
        }
        if let Some(started_on) = snapshot.sleep_debt_started_on.as_deref().filter(|value| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
        }) {
            tx.execute(
                "INSERT INTO settings(key,value,updated_at) VALUES(?1,?2,?3)
                 ON CONFLICT(key) DO UPDATE SET
                   value=MIN(settings.value,excluded.value),updated_at=excluded.updated_at",
                params![sleep_debt::START_DATE_SETTING_KEY, started_on, now()],
            )?;
        }
        tx.commit()?;
        drop(conn);
        self.save_sync_devices(&snapshot.devices)?;
        self.rebuild_personal_memory_from_confirmed()?;
        Ok(())
    }
}

pub fn status(db: &AppDb) -> Result<GithubSyncStatus> {
    let config = db.get_github_sync_config()?;
    let token_configured = secret_exists(&config.token_secret_ref);
    let key_configured = secret_exists(&config.key_secret_ref);
    let ready = config.enabled && !config.owner.is_empty() && token_configured && key_configured;
    Ok(GithubSyncStatus {
        config,
        token_configured,
        key_configured,
        ready,
        counts: db.sync_counts()?,
        devices: db.sync_devices()?,
    })
}

pub fn save_configuration(
    db: &AppDb,
    config: GithubSyncConfig,
    token: Option<String>,
    encryption_key: Option<String>,
) -> Result<GithubSyncStatus> {
    let config = config.normalized();
    validate_config(&config)?;
    if let Some(token) = token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        secrets::save_secret(&config.token_secret_ref, &token)?;
    }
    if let Some(key) = encryption_key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        decode_key(&key)?;
        secrets::save_secret(&config.key_secret_ref, &key)?;
    }
    db.save_github_sync_config(&config)?;
    status(db)
}

pub fn generate_encryption_key(db: &AppDb) -> Result<String> {
    let config = db.get_github_sync_config()?;
    let mut key = [0_u8; 32];
    OsRng.fill_bytes(&mut key);
    let encoded = BASE64.encode(key);
    secrets::save_secret(&config.key_secret_ref, &encoded)?;
    Ok(encoded)
}

pub fn read_encryption_key(db: &AppDb) -> Result<String> {
    let config = db.get_github_sync_config()?;
    secrets::read_secret(&config.key_secret_ref)
        .context("尚未配置同步密钥；请先生成或粘贴另一台设备的同步密钥")
}

pub fn disconnect(db: &AppDb, remove_credentials: bool) -> Result<GithubSyncStatus> {
    let mut config = db.get_github_sync_config()?;
    config.enabled = false;
    config.last_error = None;
    db.save_github_sync_config(&config)?;
    if remove_credentials {
        secrets::delete_secret(&config.token_secret_ref)?;
        secrets::delete_secret(&config.key_secret_ref)?;
    }
    status(db)
}

pub async fn sync_now(db: Arc<AppDb>) -> Result<GithubSyncResult> {
    let lock = SYNC_LOCK.get_or_init(|| AsyncMutex::new(()));
    let _guard = lock.lock().await;
    match sync_inner(&db).await {
        Ok(result) => Ok(result),
        Err(error) => {
            if let Ok(mut config) = db.get_github_sync_config() {
                config.last_error = Some(error.to_string());
                let _ = db.save_github_sync_config(&config);
            }
            Err(error)
        }
    }
}

pub fn start_worker(db: Arc<AppDb>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(45)).await;
        loop {
            if let Ok(config) = db.get_github_sync_config() {
                if config.enabled && config.auto_sync && sync_due(&config) {
                    let _ = sync_now(db.clone()).await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
}

async fn sync_inner(db: &Arc<AppDb>) -> Result<GithubSyncResult> {
    let mut config = db.get_github_sync_config()?.normalized();
    validate_config(&config)?;
    if !config.enabled {
        bail!("请先开启 GitHub 同步并保存配置");
    }
    let token = secrets::read_secret(&config.token_secret_ref)
        .context("未找到 GitHub Token；请在同步设置中重新填写")?;
    let key = decode_key(
        &secrets::read_secret(&config.key_secret_ref)
            .context("未找到同步密钥；请生成或从另一台设备粘贴")?,
    )?;
    let client = github_client(&token)?;
    ensure_private_repo(&client, &config).await?;

    let mut local = db.export_sync_snapshot(&config)?;
    let mut downloaded_bytes = 0;
    let mut previous_sha = None;
    if let Some(remote) = fetch_remote(&client, &config, &key).await? {
        downloaded_bytes += remote.encrypted_bytes;
        previous_sha = Some(remote.sha);
        local = merge_snapshots(local, remote.snapshot)?;
    }

    local.generated_at = now();
    local.generated_by = config.device_id.clone();
    upsert_device(
        &mut local.devices,
        SyncDeviceInfo {
            id: config.device_id.clone(),
            name: config.device_name.clone(),
            last_seen_at: local.generated_at.clone(),
        },
    );
    db.apply_sync_snapshot(&local)?;

    let mut encrypted = encrypt_snapshot(&local, &key)?;
    let mut uploaded = put_remote(&client, &config, &encrypted, previous_sha.as_deref()).await;
    if uploaded.as_ref().is_err_and(is_conflict) {
        let remote = fetch_remote(&client, &config, &key)
            .await?
            .context("远端内容刚被其他设备更新，请再次同步")?;
        downloaded_bytes += remote.encrypted_bytes;
        local = merge_snapshots(local, remote.snapshot)?;
        db.apply_sync_snapshot(&local)?;
        encrypted = encrypt_snapshot(&local, &key)?;
        uploaded = put_remote(&client, &config, &encrypted, Some(&remote.sha)).await;
    }
    let remote_sha = uploaded?;
    let synced_at = now();
    config.last_synced_at = Some(synced_at.clone());
    config.last_remote_sha = Some(remote_sha.clone());
    config.last_error = None;
    db.save_github_sync_config(&config)?;

    Ok(GithubSyncResult {
        synced_at,
        remote_sha,
        uploaded_bytes: encrypted.len() as u64,
        downloaded_bytes,
        counts: local.counts(),
        message: "已合并本机与 GitHub 的最新记录".into(),
    })
}

fn validate_config(config: &GithubSyncConfig) -> Result<()> {
    if config.owner.is_empty() {
        bail!("GitHub 用户名不能为空");
    }
    for (label, value) in [("GitHub 用户名", &config.owner), ("仓库名", &config.repo)] {
        if !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        }) {
            bail!("{label}含有不支持的字符");
        }
    }
    if config.file_path.contains("..") || config.file_path.starts_with('/') {
        bail!("同步文件路径不合法");
    }
    Ok(())
}

fn secret_exists(name: &str) -> bool {
    secrets::read_secret(name).is_ok_and(|value| !value.trim().is_empty())
}

fn decode_key(value: &str) -> Result<[u8; 32]> {
    let decoded = BASE64
        .decode(value.trim())
        .context("同步密钥不是有效的 Base64")?;
    decoded
        .try_into()
        .map_err(|_| anyhow!("同步密钥长度不正确，应为 32 字节"))
}

fn encrypt_snapshot(snapshot: &SyncSnapshot, key: &[u8; 32]) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(snapshot)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&json)?;
    let compressed = encoder.finish()?;
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte AES key");
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), compressed.as_ref())
        .map_err(|_| anyhow!("无法加密同步快照"))?;
    Ok(serde_json::to_vec(&EncryptedEnvelope {
        version: SNAPSHOT_SCHEMA,
        algorithm: "AES-256-GCM".into(),
        compression: "gzip".into(),
        nonce: BASE64.encode(nonce),
        ciphertext: BASE64.encode(ciphertext),
    })?)
}

fn decrypt_snapshot(payload: &[u8], key: &[u8; 32]) -> Result<SyncSnapshot> {
    let envelope: EncryptedEnvelope =
        serde_json::from_slice(payload).context("GitHub 同步文件格式无效")?;
    if envelope.version != SNAPSHOT_SCHEMA
        || envelope.algorithm != "AES-256-GCM"
        || envelope.compression != "gzip"
    {
        bail!("暂不支持该同步文件版本");
    }
    let nonce = BASE64.decode(envelope.nonce)?;
    if nonce.len() != 12 {
        bail!("同步文件 nonce 长度无效");
    }
    let ciphertext = BASE64.decode(envelope.ciphertext)?;
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte AES key");
    let compressed = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| anyhow!("同步密钥不匹配，无法解密 GitHub 数据"))?;
    let mut decoder = GzDecoder::new(compressed.as_slice());
    let mut json = Vec::new();
    decoder.read_to_end(&mut json)?;
    let snapshot: SyncSnapshot = serde_json::from_slice(&json)?;
    if snapshot.schema_version != SNAPSHOT_SCHEMA {
        bail!("暂不支持该同步快照版本");
    }
    Ok(snapshot)
}

fn github_client(token: &str) -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::ACCEPT,
                "application/vnd.github+json".parse().unwrap(),
            );
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", token.trim()).parse()?,
            );
            headers.insert("X-GitHub-Api-Version", "2022-11-28".parse().unwrap());
            headers
        })
        .timeout(std::time::Duration::from_secs(35))
        .build()
        .map_err(Into::into)
}

async fn ensure_private_repo(client: &Client, config: &GithubSyncConfig) -> Result<()> {
    let url = repo_url(config)?;
    let response = client.get(url).send().await?;
    if response.status().is_success() {
        let repo: GithubRepo = response.json().await?;
        if !repo.private {
            bail!("为保护时间记录，同步仓库必须是 Private");
        }
        if config.branch != repo.default_branch && config.last_remote_sha.is_none() {
            bail!("仓库默认分支是 {}，请将同步分支改为它", repo.default_branch);
        }
        return Ok(());
    }
    if response.status() != StatusCode::NOT_FOUND {
        bail_github(response, "检查 GitHub 仓库").await?;
    }

    let user_response = client.get(format!("{API_ROOT}/user")).send().await?;
    if !user_response.status().is_success() {
        return bail_github(user_response, "验证 GitHub Token").await;
    }
    let user: GithubUser = user_response.json().await?;
    if !user.login.eq_ignore_ascii_case(&config.owner) {
        bail!(
            "当前 Token 属于 {}，不能在 {} 下创建仓库",
            user.login,
            config.owner
        );
    }
    let response = client
        .post(format!("{API_ROOT}/user/repos"))
        .json(&json!({
            "name": config.repo,
            "description": "ScreenUse encrypted personal time ledger",
            "private": true,
            "auto_init": true
        }))
        .send()
        .await?;
    if !response.status().is_success() {
        bail_github(response, "创建 Private 同步仓库").await?;
    }
    Ok(())
}

async fn fetch_remote(
    client: &Client,
    config: &GithubSyncConfig,
    key: &[u8; 32],
) -> Result<Option<RemoteSnapshot>> {
    let mut url = content_url(config)?;
    url.query_pairs_mut().append_pair("ref", &config.branch);
    let response = client.get(url).send().await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return bail_github(response, "读取 GitHub 同步文件")
            .await
            .map(|_| None);
    }
    let content: GithubContent = response.json().await?;
    if content.encoding != "base64" {
        bail!("GitHub 返回了不支持的文件编码");
    }
    let bytes = BASE64.decode(content.content.replace(['\r', '\n'], ""))?;
    Ok(Some(RemoteSnapshot {
        snapshot: decrypt_snapshot(&bytes, key)?,
        sha: content.sha,
        encrypted_bytes: bytes.len() as u64,
    }))
}

async fn put_remote(
    client: &Client,
    config: &GithubSyncConfig,
    payload: &[u8],
    sha: Option<&str>,
) -> Result<String> {
    let mut body = json!({
        "message": format!("sync: ScreenUse {}", Utc::now().format("%Y-%m-%d %H:%M:%S UTC")),
        "content": BASE64.encode(payload),
        "branch": config.branch
    });
    if let Some(sha) = sha {
        body["sha"] = Value::String(sha.to_string());
    }
    let response = client.put(content_url(config)?).json(&body).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let message = response.text().await.unwrap_or_default();
        if status == StatusCode::CONFLICT || status == StatusCode::UNPROCESSABLE_ENTITY {
            return Err(anyhow!("GITHUB_CONFLICT:{message}"));
        }
        bail!(
            "写入 GitHub 同步文件失败（{}）：{}",
            status,
            concise_github_error(&message)
        );
    }
    Ok(response.json::<GithubWriteContent>().await?.content.sha)
}

fn repo_url(config: &GithubSyncConfig) -> Result<Url> {
    api_url(&["repos", &config.owner, &config.repo])
}

fn content_url(config: &GithubSyncConfig) -> Result<Url> {
    let mut segments = vec![
        "repos",
        config.owner.as_str(),
        config.repo.as_str(),
        "contents",
    ];
    segments.extend(config.file_path.split('/').filter(|part| !part.is_empty()));
    api_url(&segments)
}

fn api_url(segments: &[&str]) -> Result<Url> {
    let mut url = Url::parse(API_ROOT)?;
    url.path_segments_mut()
        .map_err(|_| anyhow!("GitHub API 地址无效"))?
        .extend(segments.iter().copied());
    Ok(url)
}

async fn bail_github(response: reqwest::Response, action: &str) -> Result<()> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("{action}失败（{status}）：{}", concise_github_error(&body))
}

fn concise_github_error(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("message")?.as_str().map(str::to_string))
        .unwrap_or_else(|| body.chars().take(180).collect())
}

fn is_conflict(error: &anyhow::Error) -> bool {
    error.to_string().starts_with("GITHUB_CONFLICT:")
}

fn merge_snapshots(left: SyncSnapshot, right: SyncSnapshot) -> Result<SyncSnapshot> {
    if left.schema_version != SNAPSHOT_SCHEMA || right.schema_version != SNAPSHOT_SCHEMA {
        bail!("同步快照版本不兼容");
    }
    let sleep_debt_started_on = match (
        left.sleep_debt_started_on.clone(),
        right.sleep_debt_started_on.clone(),
    ) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    };
    let tombstones = merge_latest(
        left.tombstones,
        right.tombstones,
        |item| format!("{}:{}", item.entity_kind, item.entity_id),
        |item| item.deleted_at.as_str(),
    );
    let tombstone_map: HashMap<String, String> = tombstones
        .iter()
        .map(|item| {
            (
                format!("{}:{}", item.entity_kind, item.entity_id),
                item.deleted_at.clone(),
            )
        })
        .collect();

    let mut categories = merge_latest(
        left.categories,
        right.categories,
        |item| item.name.clone(),
        |item| item.updated_at.as_str(),
    );
    categories.retain(|item| !is_deleted(&tombstone_map, "category", &item.name, &item.updated_at));
    let category_names: HashSet<String> = categories.iter().map(|item| item.name.clone()).collect();
    let fallback_category = if category_names.contains("杂务") {
        "杂务".to_string()
    } else {
        categories
            .iter()
            .map(|item| item.name.clone())
            .min()
            .context("同步快照至少需要保留一个工作分类")?
    };

    let mut projects = merge_latest(
        left.projects,
        right.projects,
        |item| item.id.clone(),
        |item| item.updated_at.as_str(),
    );
    projects.retain(|item| !is_deleted(&tombstone_map, "project", &item.id, &item.updated_at));
    for item in &mut projects {
        if !category_names.contains(&item.category) {
            item.category = fallback_category.clone();
        }
    }
    let project_ids: HashSet<String> = projects.iter().map(|item| item.id.clone()).collect();

    let mut tasks = merge_latest(
        left.tasks,
        right.tasks,
        |item| item.id.clone(),
        |item| item.updated_at.as_str(),
    );
    tasks.retain(|item| {
        project_ids.contains(&item.project_id)
            && !is_deleted(&tombstone_map, "task", &item.id, &item.updated_at)
    });
    let task_ids: HashSet<String> = tasks.iter().map(|item| item.id.clone()).collect();

    let mut sessions = merge_latest(
        left.sessions,
        right.sessions,
        |item| item.session.id.clone(),
        |item| item.updated_at.as_str(),
    );
    sessions.retain(|item| {
        !is_deleted(
            &tombstone_map,
            "session",
            &item.session.id,
            &item.updated_at,
        )
    });
    for item in &mut sessions {
        if item
            .session
            .project_id
            .as_ref()
            .is_some_and(|id| !project_ids.contains(id))
        {
            item.session.project_id = None;
            item.session.project_name = None;
        }
        if item
            .session
            .task_id
            .as_ref()
            .is_some_and(|id| !task_ids.contains(id))
        {
            item.session.task_id = None;
            item.session.task_title = None;
        }
        if !category_names.contains(&item.session.category) {
            item.session.category = fallback_category.clone();
        }
    }

    let mut rules = merge_latest(
        left.rules,
        right.rules,
        |item| item.rule.id.clone(),
        |item| item.updated_at.as_str(),
    );
    rules.retain(|item| !is_deleted(&tombstone_map, "rule", &item.rule.id, &item.updated_at));
    for item in &mut rules {
        if item
            .rule
            .project_id
            .as_ref()
            .is_some_and(|id| !project_ids.contains(id))
        {
            item.rule.project_id = None;
        }
        if item
            .rule
            .task_id
            .as_ref()
            .is_some_and(|id| !task_ids.contains(id))
        {
            item.rule.task_id = None;
        }
        if !category_names.contains(&item.rule.category) {
            item.rule.category = fallback_category.clone();
        }
    }

    let devices = merge_latest(
        left.devices,
        right.devices,
        |item| item.id.clone(),
        |item| item.last_seen_at.as_str(),
    );
    Ok(SyncSnapshot {
        schema_version: SNAPSHOT_SCHEMA,
        generated_at: now(),
        generated_by: left.generated_by,
        categories,
        projects,
        tasks,
        sessions,
        rules,
        tombstones,
        devices,
        sleep_debt_started_on,
    })
}

fn merge_latest<T, K, FKey, FTime>(
    left: Vec<T>,
    right: Vec<T>,
    key: FKey,
    updated_at: FTime,
) -> Vec<T>
where
    K: Eq + std::hash::Hash,
    FKey: Fn(&T) -> K,
    FTime: Fn(&T) -> &str,
{
    let mut merged: HashMap<K, T> = HashMap::new();
    for item in left.into_iter().chain(right) {
        let item_key = key(&item);
        let replace = merged
            .get(&item_key)
            .map_or(true, |current| updated_at(&item) >= updated_at(current));
        if replace {
            merged.insert(item_key, item);
        }
    }
    merged.into_values().collect()
}

fn is_deleted(
    tombstones: &HashMap<String, String>,
    kind: &str,
    id: &str,
    updated_at: &str,
) -> bool {
    tombstones
        .get(&format!("{kind}:{id}"))
        .is_some_and(|deleted_at| deleted_at.as_str() >= updated_at)
}

fn upsert_device(devices: &mut Vec<SyncDeviceInfo>, device: SyncDeviceInfo) {
    if let Some(existing) = devices.iter_mut().find(|item| item.id == device.id) {
        if device.last_seen_at >= existing.last_seen_at {
            *existing = device;
        }
    } else {
        devices.push(device);
    }
    devices.sort_by(|left, right| right.last_seen_at.cmp(&left.last_seen_at));
    devices.truncate(20);
}

impl SyncSnapshot {
    fn counts(&self) -> SyncCounts {
        SyncCounts {
            categories: self.categories.len() as u32,
            projects: self.projects.len() as u32,
            tasks: self.tasks.len() as u32,
            sessions: self.sessions.len() as u32,
            rules: self.rules.len() as u32,
        }
    }
}

fn sync_due(config: &GithubSyncConfig) -> bool {
    let Some(last) = config
        .last_synced_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
    else {
        return true;
    };
    last.with_timezone(&Utc) + Duration::minutes(i64::from(config.interval_minutes)) <= Utc::now()
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn collect<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>> {
    let mut values = Vec::new();
    for row in rows {
        values.push(row?);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_snapshot(device: &str) -> SyncSnapshot {
        SyncSnapshot {
            schema_version: SNAPSHOT_SCHEMA,
            generated_at: "2026-01-01T00:00:00.000Z".into(),
            generated_by: device.into(),
            categories: vec![SyncCategory {
                name: "杂务".into(),
                color: "#facc15".into(),
                is_builtin: true,
                created_at: "2026-01-01T00:00:00.000Z".into(),
                updated_at: "2026-01-01T00:00:00.000Z".into(),
            }],
            projects: vec![],
            tasks: vec![],
            sessions: vec![],
            rules: vec![],
            tombstones: vec![],
            devices: vec![],
            sleep_debt_started_on: None,
        }
    }

    #[test]
    fn encrypted_snapshot_round_trips_and_rejects_wrong_key() {
        let snapshot = empty_snapshot("device-a");
        let key = [7_u8; 32];
        let encrypted = encrypt_snapshot(&snapshot, &key).expect("encrypt");
        let decrypted = decrypt_snapshot(&encrypted, &key).expect("decrypt");
        assert_eq!(decrypted.generated_by, "device-a");
        assert!(decrypt_snapshot(&encrypted, &[8_u8; 32]).is_err());
        assert!(!String::from_utf8_lossy(&encrypted).contains("device-a"));
    }

    #[test]
    fn merge_prefers_newest_record_and_honors_tombstone() {
        let mut left = empty_snapshot("device-a");
        left.projects.push(Project {
            id: "p1".into(),
            name: "old".into(),
            category: "杂务".into(),
            source: "manual".into(),
            color: "#aaa".into(),
            description: None,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-02T00:00:00.000Z".into(),
        });
        let mut right = empty_snapshot("device-b");
        right.projects.push(Project {
            id: "p1".into(),
            name: "new".into(),
            category: "杂务".into(),
            source: "manual".into(),
            color: "#bbb".into(),
            description: None,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-03T00:00:00.000Z".into(),
        });
        let merged = merge_snapshots(left.clone(), right).expect("merge newest");
        assert_eq!(merged.projects[0].name, "new");

        left.tombstones.push(SyncTombstone {
            entity_kind: "project".into(),
            entity_id: "p1".into(),
            deleted_at: "2026-01-04T00:00:00.000Z".into(),
            device_id: "device-a".into(),
        });
        let merged = merge_snapshots(left, merged).expect("merge deletion");
        assert!(merged.projects.is_empty());
    }

    #[test]
    fn builtin_category_tombstone_is_honored_with_a_valid_fallback() {
        let mut left = empty_snapshot("device-a");
        left.categories.push(SyncCategory {
            name: "开发".into(),
            color: "#60a5fa".into(),
            is_builtin: true,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
        });
        left.projects.push(Project {
            id: "p-category".into(),
            name: "旧分类项目".into(),
            category: "杂务".into(),
            source: "manual".into(),
            color: "#facc15".into(),
            description: None,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-02T00:00:00.000Z".into(),
        });
        let mut right = empty_snapshot("device-b");
        right.tombstones.push(SyncTombstone {
            entity_kind: "category".into(),
            entity_id: "杂务".into(),
            deleted_at: "2026-01-03T00:00:00.000Z".into(),
            device_id: "device-b".into(),
        });

        let merged = merge_snapshots(left, right).expect("merge builtin category deletion");
        assert!(!merged.categories.iter().any(|item| item.name == "杂务"));
        assert_eq!(merged.projects[0].category, "开发");
    }

    #[test]
    fn applying_snapshot_releases_database_lock_before_saving_devices() {
        let data_dir =
            std::env::temp_dir().join(format!("screenuse-sync-apply-{}", uuid::Uuid::new_v4()));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let config = GithubSyncConfig::default();
        let snapshot = db.export_sync_snapshot(&config).expect("export snapshot");
        db.apply_sync_snapshot(&snapshot).expect("apply snapshot");
        assert_eq!(
            db.sync_devices().expect("load devices")[0].id,
            config.device_id
        );
        drop(db);
        let _ = std::fs::remove_dir_all(data_dir);
    }
}
