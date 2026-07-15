use crate::ai::{
    parse_and_validate, request_with_codex_account, review_instructions, review_prompt,
    AiAttributionBatch, AiResponse, AiReviewInput, OpenAiCompatibleClient,
};
use crate::db::{now, AppDb};
use crate::models::{AiUsage, AnalysisJob, EvidenceItem, RawActivityEvent, SessionPatch, TimeRange, WorkSession};
use crate::secrets;
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{sleep, Duration};
use uuid::Uuid;

const DEFAULT_REVIEW_CONFIDENCE_THRESHOLD: f32 = 0.8;
const MAX_REVIEW_BATCH: usize = 8;
const CONTEXT_WINDOW_MINUTES: i64 = 30;
const MAX_CONTEXT_SESSIONS_PER_TARGET: usize = 24;

static AI_RUN_LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();

pub fn start_analysis_worker(db: Arc<AppDb>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let settings = db.get_settings().unwrap_or_default().normalized();
            if settings.ai_mode == "auto" {
                if let Err(error) = enqueue_recent_uncertain(&db) {
                    eprintln!("ScreenUse optional AI enqueue error: {error}");
                }
                if let Err(error) = run_once(db.clone()).await {
                    eprintln!("ScreenUse optional AI worker error: {error}");
                }
                sleep(Duration::from_secs(60)).await;
            } else if settings.ai_mode == "manual" {
                if let Err(error) = enqueue_recent_uncertain(&db) {
                    eprintln!("ScreenUse optional AI enqueue error: {error}");
                }
                sleep(Duration::from_secs(60)).await;
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
        return Err(anyhow!("请先填写 AI 模型名"));
    }

    let minimum_seconds = i64::from(settings.min_ai_session_minutes) * 60;
    let queued = db.analysis_job_session_ids()?;
    let candidates = db
        .list_sessions(2000)?
        .into_iter()
        .filter(|session| is_review_candidate(session, minimum_seconds, &queued))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(false);
    }
    for sessions in candidates.chunks(MAX_REVIEW_BATCH) {
        let mut batch = sessions.to_vec();
        batch.sort_by(|left, right| left.started_at.cmp(&right.started_at));
        let started_at = batch
            .first()
            .map(|session| session.started_at.clone())
            .context("AI review batch has no start")?;
        let ended_at = batch
            .iter()
            .map(|session| session.ended_at.clone())
            .max()
            .context("AI review batch has no end")?;

        db.create_analysis_job(&AnalysisJob {
            id: Uuid::new_v4().to_string(),
            chunk_ids: batch.into_iter().map(|session| session.id).collect(),
            metadata_range: TimeRange {
                started_at,
                ended_at,
            },
            mode: "metadata-context-review".into(),
            provider: settings.ai_provider.clone(),
            model: settings.ai_model.clone(),
            retry_count: 0,
            status: "pending".into(),
            error: None,
            system_prompt: None,
            user_prompt: None,
            response: None,
            queued_at: now(),
            processing_started_at: None,
            completed_at: None,
            duration_ms: None,
            result_count: 0,
            usage: AiUsage::default(),
        })?;
    }
    Ok(true)
}

pub async fn run_once(db: Arc<AppDb>) -> Result<bool> {
    let lock = AI_RUN_LOCK.get_or_init(|| AsyncMutex::new(()));
    let Ok(_guard) = lock.try_lock() else {
        return Ok(false);
    };
    let Some(job) = db.claim_next_analysis_job()? else {
        return Ok(false);
    };
    let settings = db.get_settings()?.normalized();
    let mut targets = load_job_targets(&db, &job)?;
    targets.retain(|session| !session.user_confirmed && session.source != "ai-review");
    if targets.is_empty() {
        db.mark_analysis_job_status(
            &job.id,
            "skipped",
            None,
            Some("目标时间段已被人工修正，未调用 AI".into()),
        )?;
        return Ok(true);
    }
    targets.sort_by(|left, right| left.started_at.cmp(&right.started_at));

    let context_sessions = load_context_sessions(&db, &targets)?;
    let events = load_target_events(&db, &targets)?;
    let categories = db.list_categories()?;
    let projects = db.list_projects()?;
    let tasks = db.list_tasks()?;
    let input = AiReviewInput {
        targets: &targets,
        context_sessions: &context_sessions,
        events: &events,
        categories: &categories,
        projects: &projects,
        tasks: &tasks,
    };

    let review_result: Result<AiAttributionBatch> = async {
        let system_prompt = review_instructions().to_string();
        let user_prompt = review_prompt(&input)?;
        db.record_analysis_job_request(
            &job.id,
            &settings.ai_provider,
            &settings.ai_model,
            &system_prompt,
            &user_prompt,
        )?;
        let response = maybe_ai(&settings, &system_prompt, &user_prompt).await?;
        db.record_analysis_job_response(&job.id, &response.content, &response.usage)?;
        parse_and_validate(&response.content, &input)
    }
    .await;

    match review_result {
        Ok(result) => {
            persist_results(&db, &job, &targets, &events, result)?;
            Ok(true)
        }
        Err(error) => {
            let retry_count = job.retry_count + 1;
            if settings.ai_mode == "auto" && retry_count < 2 {
                db.mark_analysis_job_status(
                    &job.id,
                    "pending",
                    Some(retry_count),
                    Some(error.to_string()),
                )?;
            } else {
                db.mark_analysis_job_status(
                    &job.id,
                    "failed",
                    Some(retry_count),
                    Some(error.to_string()),
                )?;
            }
            Err(error)
        }
    }
}

fn load_context_sessions(db: &AppDb, targets: &[WorkSession]) -> Result<Vec<WorkSession>> {
    let mut contexts = Vec::new();
    let mut seen = HashSet::new();
    for target in targets {
        let started_at = format_time(
            parse_time(&target.started_at)? - ChronoDuration::minutes(CONTEXT_WINDOW_MINUTES),
        );
        let ended_at = format_time(
            parse_time(&target.ended_at)? + ChronoDuration::minutes(CONTEXT_WINDOW_MINUTES),
        );
        let nearby = nearest_context_sessions(
            db.list_sessions_in_range(&started_at, &ended_at, 200)?,
            &target.started_at,
            &target.ended_at,
        );
        for session in nearby {
            if seen.insert(session.id.clone()) {
                contexts.push(session);
            }
        }
    }
    contexts.sort_by(|left, right| left.started_at.cmp(&right.started_at));
    Ok(contexts)
}

fn load_target_events(db: &AppDb, targets: &[WorkSession]) -> Result<Vec<RawActivityEvent>> {
    let mut events = Vec::new();
    let mut seen = HashSet::new();
    for target in targets {
        for event in db.list_raw_events_between(&target.started_at, &target.ended_at)? {
            if seen.insert(event.id.clone()) {
                events.push(event);
            }
        }
    }
    events.sort_by(|left, right| left.timestamp.cmp(&right.timestamp));
    Ok(events)
}

fn nearest_context_sessions(
    mut sessions: Vec<WorkSession>,
    target_start: &str,
    target_end: &str,
) -> Vec<WorkSession> {
    let target_start = parse_time(target_start).ok();
    let target_end = parse_time(target_end).ok();
    sessions.sort_by_key(|session| {
        let started_at = parse_time(&session.started_at).ok();
        let ended_at = parse_time(&session.ended_at).ok();
        match (target_start, target_end, started_at, ended_at) {
            (Some(target_start), Some(_), Some(_), Some(ended_at))
                if ended_at < target_start =>
            {
                (target_start - ended_at).num_seconds()
            }
            (Some(_), Some(target_end), Some(started_at), Some(_))
                if started_at > target_end =>
            {
                (started_at - target_end).num_seconds()
            }
            (Some(_), Some(_), Some(_), Some(_)) => 0,
            _ => i64::MAX,
        }
    });
    sessions.truncate(MAX_CONTEXT_SESSIONS_PER_TARGET);
    sessions.sort_by(|left, right| left.started_at.cmp(&right.started_at));
    sessions
}

fn load_job_targets(db: &AppDb, job: &AnalysisJob) -> Result<Vec<WorkSession>> {
    let mut targets = Vec::new();
    for id in &job.chunk_ids {
        if let Some(session) = db.get_session(id)? {
            targets.push(session);
        }
    }
    if targets.is_empty() {
        targets = db
            .list_sessions_in_range(
                &job.metadata_range.started_at,
                &job.metadata_range.ended_at,
                MAX_REVIEW_BATCH as i64,
            )?
            .into_iter()
            .filter(|session| !session.user_confirmed && session.category != "离开")
            .take(MAX_REVIEW_BATCH)
            .collect();
    }
    Ok(targets)
}

async fn maybe_ai(
    settings: &crate::models::AppSettings,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<AiResponse> {
    if settings.ai_provider == "codex-account" {
        return request_with_codex_account(settings, system_prompt, user_prompt).await;
    }
    let secret_name = settings.ai_secret_ref.as_deref().unwrap_or_default().trim();
    if secret_name.is_empty() {
        return Err(anyhow!("未配置 AI 凭据；本地分类不受影响"));
    }
    let api_key = secrets::read_secret(secret_name)?;
    if api_key.trim().is_empty() {
        return Err(anyhow!("AI 凭据为空；本地分类不受影响"));
    }
    OpenAiCompatibleClient::new(settings, api_key)
        .request_review(system_prompt, user_prompt)
        .await
}

fn persist_results(
    db: &AppDb,
    job: &AnalysisJob,
    targets: &[WorkSession],
    events: &[RawActivityEvent],
    batch: AiAttributionBatch,
) -> Result<()> {
    let result_count = batch.results.len() as u32;
    for result in batch.results {
        let target = targets
            .iter()
            .find(|session| session.id == result.session_id)
            .context("AI result target disappeared")?;
        if db
            .get_session(&target.id)?
            .map_or(true, |session| session.user_confirmed)
        {
            continue;
        }
        let mut evidence = target.evidence.clone();
        evidence.extend(result.evidence);
        evidence.extend(metadata_evidence(events_for_session(events, target)));
        deduplicate_evidence(&mut evidence);
        db.apply_ai_review(
            &target.id,
            SessionPatch {
                summary: Some(result.summary),
                project_id: result.project_id.clone(),
                task_id: result.task_id.clone(),
                clear_project: Some(result.project_id.is_none()),
                clear_task: Some(result.task_id.is_none()),
                category: Some(result.category),
                confidence: Some(result.confidence),
                user_confirmed: Some(false),
            },
            evidence,
        )?;
    }
    db.set_analysis_job_result_count(&job.id, result_count)?;
    db.mark_analysis_job_status(&job.id, "completed", None, None)?;
    Ok(())
}

fn events_for_session<'a>(
    events: &'a [RawActivityEvent],
    session: &WorkSession,
) -> Vec<&'a RawActivityEvent> {
    events
        .iter()
        .filter(|event| {
            event.timestamp.as_str() >= session.started_at.as_str()
                && event.timestamp.as_str() <= session.ended_at.as_str()
        })
        .collect()
}

fn metadata_evidence(events: Vec<&RawActivityEvent>) -> Vec<EvidenceItem> {
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
    } else if let Some(event) = events.iter().rev().find(|event| {
        event
            .window_title
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    }) {
        evidence.push(EvidenceItem {
            kind: "window".into(),
            label: "窗口".into(),
            value: event.window_title.clone().unwrap_or_default(),
            weight: 0.70,
        });
    }
    if let Some(event) = events.iter().rev().find(|event| {
        event.url.as_deref().is_some_and(|value| !value.is_empty())
    }) {
        evidence.push(EvidenceItem {
            kind: "url".into(),
            label: "网页".into(),
            value: event.url.clone().unwrap_or_default(),
            weight: 0.66,
        });
    }
    if let Some(event) = events.iter().rev().find(|event| {
        event
            .workspace
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    }) {
        evidence.push(EvidenceItem {
            kind: "workspace".into(),
            label: "工作区".into(),
            value: event.workspace.clone().unwrap_or_default(),
            weight: 0.72,
        });
    }
    evidence
}

fn deduplicate_evidence(evidence: &mut Vec<EvidenceItem>) {
    let mut seen = HashSet::new();
    evidence.retain(|item| {
        seen.insert(format!(
            "{}\u{1f}{}",
            item.kind.to_lowercase(),
            item.value.to_lowercase()
        ))
    });
    evidence.truncate(20);
}

fn is_review_candidate(
    session: &WorkSession,
    minimum_seconds: i64,
    queued: &HashSet<String>,
) -> bool {
    !session.user_confirmed
        && session.category != "离开"
        && !matches!(
            session.source.as_str(),
            "collector-idle" | "collector-rule" | "ai-review"
        )
        && (session.task_id.is_none()
            || session.confidence < DEFAULT_REVIEW_CONFIDENCE_THRESHOLD)
        && duration_seconds(&session.started_at, &session.ended_at) >= minimum_seconds
        && !queued.contains(&session.id)
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

fn format_time(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn duration_seconds(start: &str, end: &str) -> i64 {
    match (parse_time(start), parse_time(end)) {
        (Ok(start), Ok(end)) => (end - start).num_seconds().max(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(duration_seconds: i64) -> WorkSession {
        let start = Utc::now();
        WorkSession {
            id: "candidate".into(),
            started_at: format_time(start),
            ended_at: format_time(start + ChronoDuration::seconds(duration_seconds)),
            project_id: None,
            project_name: None,
            task_id: None,
            task_title: None,
            category: "杂务".into(),
            summary: "待复核".into(),
            confidence: 0.55,
            evidence: vec![],
            user_confirmed: false,
            source: "context-complete".into(),
        }
    }

    #[test]
    fn one_minute_is_the_minimum_ai_review_duration() {
        let queued = HashSet::new();
        assert!(!is_review_candidate(&session(59), 60, &queued));
        assert!(is_review_candidate(&session(60), 60, &queued));
    }

    #[test]
    fn a_high_confidence_session_without_a_task_still_needs_ai_review() {
        let queued = HashSet::new();
        let mut value = session(60);
        value.confidence = 0.96;
        assert!(is_review_candidate(&value, 60, &queued));

        value.project_id = Some("project".into());
        value.task_id = Some("task".into());
        assert!(!is_review_candidate(&value, 60, &queued));
    }

    #[test]
    fn confirmed_idle_and_already_reviewed_sessions_are_not_candidates() {
        let queued = HashSet::new();
        let mut value = session(60);
        value.user_confirmed = true;
        assert!(!is_review_candidate(&value, 60, &queued));
        value.user_confirmed = false;
        value.source = "ai-review".into();
        assert!(!is_review_candidate(&value, 60, &queued));
        value.source = "collector-idle".into();
        assert!(!is_review_candidate(&value, 60, &queued));
    }
}
