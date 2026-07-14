use crate::ai::{AiAttributionResult, OpenAiCompatibleClient};
use crate::db::AppDb;
use crate::models::{
    AnalysisJob, AttributionSessionInput, EvidenceItem, RawActivityEvent, TimeRange,
};
use crate::secrets;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use uuid::Uuid;

const DEFAULT_REVIEW_CONFIDENCE_THRESHOLD: f32 = 0.8;

pub fn start_analysis_worker(db: Arc<AppDb>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let settings = db.get_settings().unwrap_or_default().normalized();
            if settings.ai_mode == "auto" {
                if let Err(error) = run_once(db.clone()).await {
                    eprintln!("ScreenUse optional AI worker error: {error}");
                }
                sleep(Duration::from_secs(30)).await;
            } else {
                sleep(Duration::from_secs(120)).await;
            }
        }
    });
}

pub fn enqueue_recent_uncertain(db: &AppDb) -> Result<bool> {
    let settings = db.get_settings()?.normalized();
    if settings.ai_mode == "off" {
        return Err(anyhow!("AI 已关闭；本地规则会继续自动归类"));
    }
    if settings.ai_model.trim().is_empty() {
        return Err(anyhow!("请先填写可用的 OpenAI-compatible 模型名"));
    }

    let minimum_minutes = settings.min_ai_session_minutes as i64;
    let candidate = db.list_sessions(200)?.into_iter().find(|session| {
        !session.user_confirmed
            && session.category != "离开"
            && session.confidence < DEFAULT_REVIEW_CONFIDENCE_THRESHOLD
            && duration_minutes(&session.started_at, &session.ended_at) >= minimum_minutes
    });
    let Some(session) = candidate else {
        return Ok(false);
    };

    db.create_analysis_job(&AnalysisJob {
        id: Uuid::new_v4().to_string(),
        chunk_ids: vec![],
        metadata_range: TimeRange {
            started_at: session.started_at,
            ended_at: session.ended_at,
        },
        mode: "metadata-review".into(),
        retry_count: 0,
        status: "pending".into(),
        error: None,
    })?;
    Ok(true)
}

pub async fn run_once(db: Arc<AppDb>) -> Result<bool> {
    let Some(job) = db.claim_next_analysis_job()? else {
        return Ok(false);
    };
    let events = db.list_raw_events_between(&job.metadata_range.started_at, &job.metadata_range.ended_at)?;
    if events.is_empty() {
        let message = "该会话的原始元数据已按保留策略清理，无法再次分析";
        db.mark_analysis_job_status(&job.id, "failed", None, Some(message.into()))?;
        return Err(anyhow!(message));
    }

    let settings = db.get_settings()?.normalized();
    let categories = db.list_categories()?.into_iter().map(|item| item.name).collect::<Vec<_>>();
    match maybe_ai(&settings, &events, &categories).await {
        Ok(result) => {
            persist_result(&db, &job, &events, result)?;
            Ok(true)
        }
        Err(error) => {
            let retry_count = job.retry_count + 1;
            if settings.ai_mode == "auto" && retry_count < 2 {
                db.mark_analysis_job_status(&job.id, "pending", Some(retry_count), Some(error.to_string()))?;
            } else {
                db.mark_analysis_job_status(&job.id, "failed", Some(retry_count), Some(error.to_string()))?;
            }
            Err(error)
        }
    }
}

async fn maybe_ai(
    settings: &crate::models::AppSettings,
    events: &[RawActivityEvent],
    categories: &[String],
) -> Result<AiAttributionResult> {
    let secret_name = settings.ai_secret_ref.as_deref().unwrap_or_default().trim();
    if secret_name.is_empty() {
        return Err(anyhow!("未配置 AI 凭据；本地分类不受影响"));
    }
    let api_key = secrets::read_secret(secret_name)?;
    if api_key.trim().is_empty() {
        return Err(anyhow!("AI 凭据为空；本地分类不受影响"));
    }
    OpenAiCompatibleClient::new(settings, api_key)
        .analyze_metadata_block(events, categories)
        .await
}

fn persist_result(
    db: &AppDb,
    job: &AnalysisJob,
    events: &[RawActivityEvent],
    result: AiAttributionResult,
) -> Result<()> {
    let project_id = if result.category == "离开" {
        None
    } else {
        Some(db.upsert_project_by_name(&result.project_name, &result.category, "ai-review")?)
    };
    let task_id = match project_id.as_deref() {
        Some(project_id) if result.category != "离开" => {
            Some(db.upsert_task_by_title(project_id, &result.task_title, "ai-review")?)
        }
        _ => None,
    };
    let mut evidence = result.evidence;
    evidence.extend(metadata_evidence(events));
    db.materialize_attribution_session(AttributionSessionInput {
        range: job.metadata_range.clone(),
        project_id,
        task_id,
        category: result.category,
        summary: result.summary,
        confidence: result.confidence,
        evidence,
        source: "ai-review".into(),
    })?;
    db.mark_analysis_job_status(&job.id, "completed", None, None)?;
    Ok(())
}

fn metadata_evidence(events: &[RawActivityEvent]) -> Vec<EvidenceItem> {
    let mut evidence = Vec::new();
    if let Some(page_title) = events.iter().rev().find_map(|event| {
        event
            .metadata
            .get("activePageTitle")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
    }) {
        evidence.push(EvidenceItem {
            kind: "page".into(),
            label: "当前页面".into(),
            value: page_title.to_string(),
            weight: 0.82,
        });
    } else if let Some(event) = events.iter().rev().find(|event| event.window_title.as_deref().is_some_and(|value| !value.is_empty())) {
        evidence.push(EvidenceItem {
            kind: "window".into(),
            label: "窗口".into(),
            value: event.window_title.clone().unwrap_or_default(),
            weight: 0.70,
        });
    }
    if let Some(event) = events.iter().rev().find(|event| event.url.as_deref().is_some_and(|value| !value.is_empty())) {
        evidence.push(EvidenceItem {
            kind: "url".into(),
            label: "网页".into(),
            value: event.url.clone().unwrap_or_default(),
            weight: 0.66,
        });
    }
    if let Some(event) = events.iter().rev().find(|event| event.workspace.as_deref().is_some_and(|value| !value.is_empty())) {
        evidence.push(EvidenceItem {
            kind: "workspace".into(),
            label: "工作区".into(),
            value: event.workspace.clone().unwrap_or_default(),
            weight: 0.72,
        });
    }
    evidence
}

fn duration_minutes(start: &str, end: &str) -> i64 {
    let start = DateTime::parse_from_rfc3339(start).map(|value| value.with_timezone(&Utc));
    let end = DateTime::parse_from_rfc3339(end).map(|value| value.with_timezone(&Utc));
    match (start, end) {
        (Ok(start), Ok(end)) => (end - start).num_minutes().max(0),
        _ => 0,
    }
}
