use crate::ai::{fallback_rule_summary, AiAttributionResult, OpenAiCompatibleClient};
use crate::db::AppDb;
use crate::models::{EvidenceItem, InputStats, RawActivityEvent};
use crate::secrets;
use anyhow::{anyhow, Result};
use serde_json::json;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

pub fn start_analysis_worker(db: Arc<AppDb>) {
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(err) = run_once(db.clone()).await {
                eprintln!("ScreenUse analysis worker error: {err}");
            }
            sleep(Duration::from_secs(12)).await;
        }
    });
}

pub async fn run_once(db: Arc<AppDb>) -> Result<bool> {
    let Some(job) = db.claim_next_analysis_job()? else { return Ok(false); };
    let chunks = db.media_chunks_by_ids(&job.chunk_ids)?;
    let mut events = db.list_raw_events_between(&job.metadata_range.started_at, &job.metadata_range.ended_at)?;
    if !chunks.is_empty() {
        events.push(media_summary_event(&chunks, &job.metadata_range.ended_at));
    }
    let settings = db.get_settings()?;

    let result = match maybe_ai(&settings, &events).await {
        Ok(result) => {
            persist_result(&db, &job, &events, result, "ai-analysis", "completed", true, None)?;
            Ok(())
        }
        Err(err) => {
            let retry_count = job.retry_count + 1;
            if retry_count < 3 && settings.ai_secret_ref.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false) {
                db.mark_analysis_job_status(&job.id, "pending", Some(retry_count), Some(err.to_string()))?;
                return Ok(true);
            }
            let fallback = stronger_fallback(&events, &chunks, &err.to_string());
            persist_result(&db, &job, &events, fallback, "rule-downgrade", "downgraded", false, Some(err.to_string()))?;
            Ok(())
        }
    };

    if let Err(err) = db.cleanup_media_cache() {
        eprintln!("ScreenUse media cache cleanup failed: {err}");
    }
    result.map(|_| true)
}

async fn maybe_ai(settings: &crate::models::AppSettings, events: &[RawActivityEvent]) -> Result<AiAttributionResult> {
    let secret_name = settings.ai_secret_ref.as_deref().unwrap_or("").trim();
    if secret_name.is_empty() {
        return Err(anyhow!("未配置 AI 凭据，使用本地规则降级"));
    }
    let api_key = secrets::read_secret(secret_name)?;
    if api_key.trim().is_empty() {
        return Err(anyhow!("AI 凭据为空，使用本地规则降级"));
    }
    OpenAiCompatibleClient::new(settings, api_key).analyze_metadata_block(events).await
}

fn persist_result(
    db: &AppDb,
    job: &crate::models::AnalysisJob,
    events: &[RawActivityEvent],
    result: AiAttributionResult,
    source: &str,
    job_status: &str,
    delete_media: bool,
    error: Option<String>,
) -> Result<()> {
    let project_id = if result.category == "离开" {
        None
    } else {
        Some(db.upsert_project_by_name(&result.project_name, &result.category, source)?)
    };
    let task_id = match project_id.as_deref() {
        Some(project_id) if result.category != "离开" => Some(db.upsert_task_by_title(project_id, &result.task_title, source)?),
        _ => None,
    };
    let mut evidence = result.evidence;
    evidence.extend(metadata_evidence(events));
    db.materialize_attribution_session(
        &job.metadata_range,
        project_id,
        task_id,
        result.category,
        result.summary,
        result.confidence,
        evidence,
        source,
    )?;
    db.mark_analysis_job_status(&job.id, job_status, None, error)?;
    if delete_media {
        db.set_media_chunks_status(&job.chunk_ids, "analyzed", true)?;
    }
    Ok(())
}

fn metadata_evidence(events: &[RawActivityEvent]) -> Vec<EvidenceItem> {
    let mut out = Vec::new();
    if let Some(event) = events.iter().rev().find(|e| e.window_title.as_deref().map(|s| !s.is_empty()).unwrap_or(false)) {
        out.push(EvidenceItem {
            kind: "window".into(),
            label: "窗口".into(),
            value: event.window_title.clone().unwrap_or_default(),
            weight: 0.7,
        });
    }
    if let Some(event) = events.iter().rev().find(|e| e.url.as_deref().map(|s| !s.is_empty()).unwrap_or(false)) {
        out.push(EvidenceItem {
            kind: "url".into(),
            label: "网页".into(),
            value: event.url.clone().unwrap_or_default(),
            weight: 0.65,
        });
    }
    if let Some(event) = events.iter().rev().find(|e| e.workspace.as_deref().map(|s| !s.is_empty()).unwrap_or(false)) {
        out.push(EvidenceItem {
            kind: "workspace".into(),
            label: "工作区".into(),
            value: event.workspace.clone().unwrap_or_default(),
            weight: 0.68,
        });
    }
    out
}

fn media_summary_event(chunks: &[crate::models::MediaChunk], timestamp: &str) -> RawActivityEvent {
    RawActivityEvent {
        id: format!("media-summary:{}", chunks.iter().map(|c| c.id.as_str()).collect::<Vec<_>>().join(",")),
        source: "media-cache".into(),
        timestamp: timestamp.to_string(),
        app: Some("ScreenUse Capture".into()),
        window_title: Some(format!("{} 个屏幕切片待分析", chunks.len())),
        url: None,
        file_path: chunks.first().map(|c| c.path.clone()),
        workspace: None,
        input_stats: InputStats::default(),
        metadata: json!({
            "chunks": chunks.iter().map(|c| json!({
                "id": c.id,
                "displayId": c.display_id,
                "startedAt": c.started_at,
                "endedAt": c.ended_at,
                "path": c.path,
                "fps": c.fps,
                "status": c.status,
            })).collect::<Vec<_>>()
        }),
    }
}

fn stronger_fallback(events: &[RawActivityEvent], chunks: &[crate::models::MediaChunk], reason: &str) -> AiAttributionResult {
    let mut result = fallback_rule_summary(events);
    let hay = events.iter().map(|e| format!(
        "{} {} {} {} {}",
        e.app.clone().unwrap_or_default(),
        e.window_title.clone().unwrap_or_default(),
        e.url.clone().unwrap_or_default(),
        e.file_path.clone().unwrap_or_default(),
        e.workspace.clone().unwrap_or_default(),
    )).collect::<Vec<_>>().join(" ").to_lowercase();

    let (project, task, category, summary, confidence) = if hay.contains("screenuse") || hay.contains("codex") || hay.contains("vscode") || hay.contains("visual studio code") || hay.contains(".rs") || hay.contains("github") {
        ("自动发现：开发", "开发与调试", "开发", "本地规则：开发与调试", 0.62)
    } else if hay.contains("pdf") || hay.contains("course") || hay.contains("bilibili") || hay.contains("知网") || hay.contains("论文") {
        ("自动发现：学习", "资料阅读", "学习", "本地规则：学习资料阅读", 0.58)
    } else if hay.contains("word") || hay.contains("wps") || hay.contains("obsidian") || hay.contains("markdown") || hay.contains(".md") {
        ("自动发现：写作", "文档写作", "写作", "本地规则：写作与文档整理", 0.58)
    } else if hay.contains("wechat") || hay.contains("mail") || hay.contains("teams") || hay.contains("meeting") || hay.contains("qq") {
        ("自动发现：沟通", "消息处理", "沟通", "本地规则：沟通与消息处理", 0.56)
    } else if events.iter().any(|e| e.input_stats.idle_seconds >= 180) {
        ("离开", "空闲", "离开", "本地规则：离开/空闲", 0.9)
    } else {
        ("自动发现项目", "待确认活动", "杂务", result.summary.as_str(), result.confidence)
    };

    result.project_name = project.into();
    result.task_title = task.into();
    result.category = category.into();
    result.summary = summary.into();
    result.confidence = confidence;
    result.evidence.push(EvidenceItem {
        kind: "analysis".into(),
        label: "降级原因".into(),
        value: reason.chars().take(180).collect(),
        weight: 0.3,
    });
    if let Some(chunk) = chunks.first() {
        result.evidence.push(EvidenceItem {
            kind: "media".into(),
            label: "媒体切片".into(),
            value: json!({ "path": chunk.path, "status": chunk.status }).to_string(),
            weight: 0.35,
        });
    }
    result
}
