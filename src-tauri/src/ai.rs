#![allow(dead_code)]

use crate::memory::RetrievedMemoryExample;
use crate::models::{
    AiUsage, AppSettings, CategoryOption, EvidenceItem, Project, RawActivityEvent, Task,
    WorkSession,
};
use anyhow::{anyhow, Context, Result};
use chrono::Duration as ChronoDuration;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use uuid::Uuid;

const CODEX_TIMEOUT_SECONDS: u64 = 90;
const MAX_AI_EVENTS_PER_SESSION: usize = 12;
const MAX_AI_CONTEXT_SESSIONS: usize = 36;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAttributionResult {
    pub session_id: String,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub category: String,
    pub summary: String,
    pub confidence: f32,
    pub evidence: Vec<EvidenceItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAttributionBatch {
    pub results: Vec<AiAttributionResult>,
}

pub struct AiReviewInput<'a> {
    pub targets: &'a [WorkSession],
    pub context_sessions: &'a [WorkSession],
    pub events: &'a [RawActivityEvent],
    pub categories: &'a [CategoryOption],
    pub projects: &'a [Project],
    pub tasks: &'a [Task],
    pub memories: &'a [RetrievedMemoryExample],
}

#[derive(Debug, Clone)]
pub struct AiResponse {
    pub content: String,
    pub usage: AiUsage,
}

pub struct OpenAiCompatibleClient {
    client: Client,
    base_url: String,
    model: String,
    api_key: String,
}

impl OpenAiCompatibleClient {
    pub fn new(settings: &AppSettings, api_key: String) -> Self {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(CODEX_TIMEOUT_SECONDS))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            base_url: settings.ai_base_url.trim_end_matches('/').to_string(),
            model: settings.ai_model.trim().to_string(),
            api_key,
        }
    }

    pub async fn analyze_review(&self, input: &AiReviewInput<'_>) -> Result<AiAttributionBatch> {
        if input.targets.is_empty() {
            return Err(anyhow!("no sessions to analyze"));
        }
        let system_prompt = review_instructions();
        let user_prompt = review_prompt(input)?;
        let response = self.request_review(system_prompt, &user_prompt).await?;
        parse_and_validate(&response.content, input)
    }

    pub async fn request_review(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<AiResponse> {
        if self.model.is_empty() {
            return Err(anyhow!("AI model is empty"));
        }
        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt}
            ],
            "temperature": 0.1,
            "max_tokens": 1800,
            "response_format": {"type": "json_object"}
        });
        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "AI request failed: {status} - {}",
                detail.chars().take(300).collect::<String>()
            ));
        }

        let value: Value = response.json().await?;
        let content = value["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow!("missing AI response content"))?
            .to_string();
        Ok(AiResponse {
            content,
            usage: usage_from_api_response(&value),
        })
    }
}

pub async fn analyze_with_codex_account(
    settings: &AppSettings,
    input: &AiReviewInput<'_>,
) -> Result<AiAttributionBatch> {
    if input.targets.is_empty() {
        return Err(anyhow!("no sessions to analyze"));
    }
    let system_prompt = review_instructions();
    let user_prompt = review_prompt(input)?;
    let session_ids = input
        .targets
        .iter()
        .map(|session| session.id.clone())
        .collect::<Vec<_>>();
    let response =
        request_with_codex_account(settings, system_prompt, &user_prompt, &session_ids).await?;
    parse_and_validate(&response.content, input)
}

pub async fn request_with_codex_account(
    settings: &AppSettings,
    system_prompt: &str,
    user_prompt: &str,
    session_ids: &[String],
) -> Result<AiResponse> {
    let model = settings.ai_model.trim();
    if model.is_empty() {
        return Err(anyhow!("Codex model is empty"));
    }

    let work_dir = std::env::temp_dir().join(format!("screenuse-ai-{}", Uuid::new_v4()));
    fs::create_dir_all(&work_dir)?;
    let schema_path = work_dir.join("result.schema.json");
    let output_path = work_dir.join("result.json");
    fs::write(
        &schema_path,
        serde_json::to_vec(&review_schema(session_ids))?,
    )?;

    let mut command = codex_command();
    command
        .arg("exec")
        .arg("--json")
        .arg("--ephemeral")
        .arg("--skip-git-repo-check")
        .arg("--ignore-user-config")
        .arg("--ignore-rules")
        .arg("--sandbox")
        .arg("read-only")
        .arg("--model")
        .arg(model)
        .arg("-c")
        .arg("model_reasoning_effort=\"low\"")
        .arg("--output-schema")
        .arg(&schema_path)
        .arg("--output-last-message")
        .arg(&output_path)
        .arg("--cd")
        .arg(&work_dir)
        .arg("-")
        .env_remove("CODEX_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env("RUST_LOG", "error")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_console(&mut command);

    let result = async {
        let mut child = command
            .spawn()
            .context("无法启动 Codex CLI；请先安装 Codex 并运行 codex login")?;
        let mut stdin = child.stdin.take().context("cannot open Codex stdin")?;
        stdin.write_all(system_prompt.as_bytes()).await?;
        stdin.write_all(b"\n\n").await?;
        stdin.write_all(user_prompt.as_bytes()).await?;
        stdin.shutdown().await?;
        drop(stdin);
        let output = timeout(
            Duration::from_secs(CODEX_TIMEOUT_SECONDS),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| anyhow!("Codex AI 复核超过 {CODEX_TIMEOUT_SECONDS} 秒"))??;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "Codex AI 复核失败：{}",
                detail
                    .chars()
                    .rev()
                    .take(500)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            ));
        }
        let content = fs::read_to_string(&output_path).context("Codex 未生成结构化复核结果")?;
        Ok(AiResponse {
            content,
            usage: usage_from_codex_jsonl(&output.stdout),
        })
    }
    .await;
    let _ = fs::remove_dir_all(&work_dir);
    result
}

fn usage_from_codex_jsonl(stdout: &[u8]) -> AiUsage {
    let mut usage = AiUsage::default();
    for line in String::from_utf8_lossy(stdout).lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value["type"].as_str() != Some("turn.completed") {
            continue;
        }
        let source = &value["usage"];
        usage.input_tokens = json_u64(source, &["input_tokens"]);
        usage.cached_input_tokens = json_u64(source, &["cached_input_tokens"]);
        usage.output_tokens = json_u64(source, &["output_tokens"]);
        usage.reasoning_output_tokens = json_u64(source, &["reasoning_output_tokens"]);
        usage.total_tokens = json_u64(source, &["total_tokens"])
            .max(usage.input_tokens.saturating_add(usage.output_tokens));
        usage.cost_usd = json_f64(source, &["cost_usd"])
            .or_else(|| json_f64(source, &["cost"]))
            .or_else(|| json_f64(source, &["total_cost"]));
    }
    if usage.total_tokens > 0 && usage.cost_usd.is_none() {
        usage.cost_note = Some("当前 Codex 账号未返回单次金额".into());
    }
    usage
}

fn usage_from_api_response(value: &Value) -> AiUsage {
    let source = &value["usage"];
    let input_tokens =
        json_u64(source, &["prompt_tokens"]).max(json_u64(source, &["input_tokens"]));
    let output_tokens =
        json_u64(source, &["completion_tokens"]).max(json_u64(source, &["output_tokens"]));
    let cached_input_tokens = json_u64(source, &["prompt_tokens_details", "cached_tokens"])
        .max(json_u64(source, &["input_tokens_details", "cached_tokens"]));
    let reasoning_output_tokens =
        json_u64(source, &["completion_tokens_details", "reasoning_tokens"]).max(json_u64(
            source,
            &["output_tokens_details", "reasoning_tokens"],
        ));
    let total_tokens =
        json_u64(source, &["total_tokens"]).max(input_tokens.saturating_add(output_tokens));
    let cost_usd = json_f64(source, &["cost"])
        .or_else(|| json_f64(source, &["total_cost"]))
        .or_else(|| json_f64(source, &["cost_usd"]));
    AiUsage {
        input_tokens,
        cached_input_tokens,
        output_tokens,
        reasoning_output_tokens,
        total_tokens,
        cost_usd,
        cost_note: (total_tokens > 0 && cost_usd.is_none()).then(|| "接口未返回单次金额".into()),
    }
}

fn json_u64(value: &Value, path: &[&str]) -> u64 {
    json_path(value, path).and_then(Value::as_u64).unwrap_or(0)
}

fn json_f64(value: &Value, path: &[&str]) -> Option<f64> {
    json_path(value, path).and_then(Value::as_f64)
}

fn json_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |current, key| current.get(key))
}

pub async fn probe_codex_account() -> Result<String> {
    let mut command = codex_command();
    command
        .arg("login")
        .arg("status")
        .env_remove("CODEX_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_console(&mut command);
    let output = timeout(Duration::from_secs(12), command.output())
        .await
        .map_err(|_| anyhow!("检查 Codex 登录状态超时"))?
        .context("找不到 Codex CLI；请先安装并运行 codex login")?;
    let status = format!(
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(anyhow!("Codex 尚未登录；请先运行 codex login"));
    }
    if !status.to_lowercase().contains("chatgpt") {
        return Err(anyhow!(
            "Codex 当前不是 ChatGPT 账号登录，请先使用 codex login 切换"
        ));
    }
    Ok("已连接当前 Codex ChatGPT 账号".into())
}

fn codex_command() -> Command {
    if let Some(binary) = std::env::var_os("SCREENUSE_CODEX_BIN") {
        return Command::new(binary);
    }
    #[cfg(windows)]
    {
        if let Some(path) = find_on_path("codex.cmd") {
            let mut command = Command::new("cmd.exe");
            command.arg("/D").arg("/S").arg("/C").arg(path);
            return command;
        }
        if let Some(path) = find_on_path("codex.exe") {
            return Command::new(path);
        }
    }
    Command::new("codex")
}

#[cfg(windows)]
fn find_on_path(file_name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .map(|directory| directory.join(file_name))
        .find(|path| path.is_file())
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

pub(crate) fn review_instructions() -> &'static str {
    "你是单人电脑时间账本的层级归类器。不要调用工具、读取文件或联网，只分析输入 JSON。\
按 reviewItems 原顺序，为每个 targetSession.sessionId 返回且只返回一个结果；sessionId 是不透明字符串，必须逐字复制。\
当前 category/project/task/confidence 只是旧系统建议，不是事实。只能选择 catalog 中已有值；taskId 必须属于 projectId，projectId 必须属于 category。\
决策优先级：① personalMemory 中与当前页面、对话标题、工作区、文件或域名高度相似且由用户确认的例子；② 当前页面/文档/工作区等直接线索；\
③ timelineContext 中紧邻且已确认的同一事务连续性；④ 应用名只作弱证据。Explorer、浏览器、QQ、微信、WPS、截图工具和终端可能只是同一任务的工具切换。\
若 catalog 有明确匹配任务，必须落到最具体 taskId；只有所有任务都缺乏依据时才返回 null，禁止创造名称。冲突记忆不强行套用。\
confidence 校准：精确个人记忆且无冲突 0.92-0.98；两个独立强线索 0.86-0.93；主要靠连续上下文 0.72-0.85；猜测不高于 0.65。\
summary 用不超过 30 个汉字描述实际事务；evidence 最多 3 项，只保留决定性证据。只输出符合 JSON Schema 的 JSON。"
}

pub(crate) fn review_prompt(input: &AiReviewInput<'_>) -> Result<String> {
    Ok(format!(
        "复核以下目标会话。timelineContext 是去重后的前后 30 分钟时间线；personalMemory 是按相似度检索的用户确认例子；catalog 是完整层级。不要修改上下文会话。输入：{}",
        serde_json::to_string(&review_payload(input))?
    ))
}

fn review_payload(input: &AiReviewInput<'_>) -> Value {
    let mut timeline_context = input.context_sessions.iter().collect::<Vec<_>>();
    timeline_context.sort_by_key(|session| {
        input
            .targets
            .iter()
            .map(|target| context_distance_seconds(target, session))
            .min()
            .unwrap_or(i64::MAX)
    });
    timeline_context.truncate(MAX_AI_CONTEXT_SESSIONS);
    timeline_context.sort_by(|left, right| left.started_at.cmp(&right.started_at));
    let review_items = input
        .targets
        .iter()
        .map(|session| {
            let matching_events = input
                .events
                .iter()
                .filter(|event| {
                    event.timestamp.as_str() >= session.started_at.as_str()
                        && event.timestamp.as_str() <= session.ended_at.as_str()
                })
                .collect::<Vec<_>>();
            let events = evenly_sample(&matching_events, MAX_AI_EVENTS_PER_SESSION)
                .into_iter()
                .map(compact_event)
                .collect::<Vec<_>>();
            json!({
                "targetSession": compact_session(session),
                "contextSessionIds": context_session_ids_for_target(&timeline_context, session),
                "events": events,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "reviewItems": review_items,
        "timelineContext": timeline_context.iter()
            .copied()
            .map(compact_session)
            .collect::<Vec<_>>(),
        "personalMemory": input.memories,
        "catalog": {
            "categories": input.categories.iter().map(|item| json!({
                "name": item.name,
            })).collect::<Vec<_>>(),
            "projects": input.projects.iter().map(|item| json!({
                "id": item.id,
                "name": item.name,
                "category": item.category,
            })).collect::<Vec<_>>(),
            "tasks": input.tasks.iter().map(|item| json!({
                "id": item.id,
                "projectId": item.project_id,
                "title": item.title,
                "status": item.status,
            })).collect::<Vec<_>>(),
        }
    })
}

fn context_session_ids_for_target(contexts: &[&WorkSession], target: &WorkSession) -> Vec<String> {
    let bounds = DateTimeBounds::from_session(target);
    let mut sessions = contexts
        .iter()
        .copied()
        .filter(|session| session.id != target.id)
        .filter(|session| {
            bounds
                .as_ref()
                .map_or(true, |bounds| bounds.overlaps(session))
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|session| context_distance_seconds(target, session));
    sessions
        .into_iter()
        .take(10)
        .map(|session| session.id.clone())
        .collect()
}

fn context_distance_seconds(target: &WorkSession, context: &WorkSession) -> i64 {
    let Ok(target_start) = chrono::DateTime::parse_from_rfc3339(&target.started_at) else {
        return i64::MAX;
    };
    let Ok(target_end) = chrono::DateTime::parse_from_rfc3339(&target.ended_at) else {
        return i64::MAX;
    };
    let Ok(context_start) = chrono::DateTime::parse_from_rfc3339(&context.started_at) else {
        return i64::MAX;
    };
    let Ok(context_end) = chrono::DateTime::parse_from_rfc3339(&context.ended_at) else {
        return i64::MAX;
    };
    if context_end < target_start {
        (target_start - context_end).num_seconds()
    } else if context_start > target_end {
        (context_start - target_end).num_seconds()
    } else {
        0
    }
}

fn evenly_sample<'a, T>(items: &[&'a T], limit: usize) -> Vec<&'a T> {
    if items.len() <= limit {
        return items.to_vec();
    }
    if limit <= 1 {
        return items.first().copied().into_iter().collect();
    }
    (0..limit)
        .map(|index| index * (items.len() - 1) / (limit - 1))
        .map(|index| items[index])
        .collect()
}

struct DateTimeBounds {
    start: chrono::DateTime<chrono::FixedOffset>,
    end: chrono::DateTime<chrono::FixedOffset>,
}

impl DateTimeBounds {
    fn from_session(session: &WorkSession) -> Option<Self> {
        Some(Self {
            start: chrono::DateTime::parse_from_rfc3339(&session.started_at).ok()?
                - ChronoDuration::minutes(30),
            end: chrono::DateTime::parse_from_rfc3339(&session.ended_at).ok()?
                + ChronoDuration::minutes(30),
        })
    }

    fn overlaps(&self, session: &WorkSession) -> bool {
        match (
            chrono::DateTime::parse_from_rfc3339(&session.started_at),
            chrono::DateTime::parse_from_rfc3339(&session.ended_at),
        ) {
            (Ok(start), Ok(end)) => end >= self.start && start <= self.end,
            _ => false,
        }
    }
}

fn compact_session(session: &WorkSession) -> Value {
    json!({
        "sessionId": session.id,
        "startedAt": session.started_at,
        "endedAt": session.ended_at,
        "category": session.category,
        "projectId": session.project_id,
        "projectName": session.project_name,
        "taskId": session.task_id,
        "taskTitle": session.task_title,
        "summary": session.summary,
        "confidence": session.confidence,
        "userConfirmed": session.user_confirmed,
        "source": session.source,
        "evidence": session.evidence.iter().take(4).map(|item| json!({
            "kind": item.kind,
            "label": item.label,
            "value": clean_metadata_value(&item.value),
        })).collect::<Vec<_>>(),
    })
}

fn compact_event(event: &RawActivityEvent) -> Value {
    json!({
        "time": event.timestamp,
        "source": event.source,
        "app": event.app,
        "title": event.window_title.as_deref().map(|value| clean(value, "", 160)),
        "page": event.metadata.get("activePageTitle").and_then(Value::as_str)
            .map(|value| clean(value, "", 160)),
        "url": event.url.as_deref().map(strip_url_noise),
        "file": event.file_path.as_deref().map(last_path_parts),
        "workspace": event.workspace.as_deref().map(last_path_parts),
        "idleSeconds": event.input_stats.idle_seconds,
    })
}

fn review_schema(session_ids: &[String]) -> Value {
    json!({
        "type": "object",
        "properties": {
            "results": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "sessionId": {"type": "string", "enum": session_ids},
                        "projectId": {"type": ["string", "null"]},
                        "taskId": {"type": ["string", "null"]},
                        "category": {"type": "string"},
                        "summary": {"type": "string"},
                        "confidence": {"type": "number"},
                        "evidence": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "kind": {"type": "string"},
                                    "label": {"type": "string"},
                                    "value": {"type": "string"},
                                    "weight": {"type": "number"}
                                },
                                "required": ["kind", "label", "value", "weight"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["sessionId", "projectId", "taskId", "category", "summary", "confidence", "evidence"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["results"],
        "additionalProperties": false
    })
}

pub(crate) fn parse_and_validate(
    content: &str,
    input: &AiReviewInput<'_>,
) -> Result<AiAttributionBatch> {
    let mut parsed: AiAttributionBatch = serde_json::from_str(content).or_else(|_| {
        extract_json_object(content)
            .and_then(|json| serde_json::from_str(&json).map_err(anyhow::Error::from))
    })?;
    validate_batch(&mut parsed, input)?;
    Ok(parsed)
}

fn validate_batch(batch: &mut AiAttributionBatch, input: &AiReviewInput<'_>) -> Result<()> {
    let expected_order = input
        .targets
        .iter()
        .map(|session| session.id.clone())
        .collect::<Vec<_>>();
    let expected = expected_order
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    if batch.results.len() < expected_order.len() {
        return Err(anyhow!("AI did not return every target session"));
    }
    let mut returned = std::collections::HashSet::new();
    let mut normalized = Vec::with_capacity(expected.len());
    let mut unresolved = Vec::new();
    for (index, result) in batch.results.drain(..).enumerate() {
        if expected.contains(result.session_id.as_str())
            && returned.insert(result.session_id.clone())
        {
            normalized.push(result);
            continue;
        }
        unresolved.push((index, result));
    }
    let mut missing = expected
        .iter()
        .filter(|session_id| !returned.contains(**session_id))
        .copied()
        .collect::<Vec<_>>();
    for (index, mut result) in unresolved {
        if missing.is_empty() {
            continue;
        }
        let candidates = missing
            .iter()
            .copied()
            .filter(|session_id| session_id_typo_distance(&result.session_id, session_id) <= 1)
            .collect::<Vec<_>>();
        let repaired = if candidates.len() == 1 {
            candidates[0]
        } else if expected_order
            .get(index)
            .is_some_and(|session_id| missing.contains(&session_id.as_str()))
        {
            expected_order[index].as_str()
        } else if missing.len() == 1 {
            missing[0]
        } else {
            return Err(anyhow!("AI returned ambiguous session identifiers"));
        };
        result.session_id = repaired.to_string();
        returned.insert(result.session_id.clone());
        missing.retain(|session_id| *session_id != repaired);
        normalized.push(result);
    }
    batch.results = normalized;
    if returned.len() != expected.len() {
        return Err(anyhow!("AI did not return every target session"));
    }
    for result in &mut batch.results {
        validate_result(result, input)?;
    }
    batch.results.sort_by_key(|result| {
        input
            .targets
            .iter()
            .position(|session| session.id == result.session_id)
            .unwrap_or(usize::MAX)
    });
    Ok(())
}

fn session_id_typo_distance(left: &str, right: &str) -> usize {
    if left.len() != right.len() {
        return usize::MAX;
    }
    left.bytes()
        .zip(right.bytes())
        .filter(|(left, right)| left != right)
        .count()
}

fn validate_result(result: &mut AiAttributionResult, input: &AiReviewInput<'_>) -> Result<()> {
    if let Some(task_id) = result.task_id.as_deref() {
        let task = input
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or_else(|| anyhow!("AI returned unknown taskId: {task_id}"))?;
        let project = input
            .projects
            .iter()
            .find(|project| project.id == task.project_id)
            .ok_or_else(|| anyhow!("AI returned a task whose project is missing from catalog"))?;
        result.project_id = Some(project.id.clone());
        result.category = project.category.clone();
    } else if let Some(project_id) = result.project_id.as_deref() {
        let project = input
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| anyhow!("AI returned unknown projectId: {project_id}"))?;
        result.category = project.category.clone();
    }
    if !input
        .categories
        .iter()
        .any(|item| item.name == result.category)
    {
        return Err(anyhow!(
            "AI returned unsupported category: {}",
            result.category
        ));
    }
    result.summary = clean(&result.summary, "AI 元数据复核", 100);
    result.confidence = result.confidence.clamp(0.0, 0.98);
    result.evidence.truncate(3);
    for item in &mut result.evidence {
        item.kind = clean(&item.kind, "ai", 32);
        item.label = clean(&item.label, "AI 依据", 40);
        item.value = clean(&item.value, "元数据", 200);
        item.weight = item.weight.clamp(0.0, 1.0);
    }
    Ok(())
}

fn strip_url_noise(value: &str) -> String {
    value
        .split(['?', '#'])
        .next()
        .unwrap_or(value)
        .chars()
        .take(500)
        .collect()
}

fn last_path_parts(value: &str) -> String {
    let parts: Vec<_> = value
        .split(['/', '\\'])
        .filter(|part| !part.trim().is_empty())
        .collect();
    parts
        .into_iter()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("/")
}

fn clean_metadata_value(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        strip_url_noise(value)
    } else {
        value.chars().take(300).collect()
    }
}

fn clean(value: &str, fallback: &str, max_chars: usize) -> String {
    let cleaned = value
        .replace(['\r', '\n', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned.chars().take(max_chars).collect()
    }
}

fn extract_json_object(input: &str) -> Result<String> {
    let start = input
        .find('{')
        .ok_or_else(|| anyhow!("AI response does not contain JSON object"))?;
    let end = input
        .rfind('}')
        .ok_or_else(|| anyhow!("AI response JSON object is incomplete"))?;
    Ok(input[start..=end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ContextFeatures;
    use crate::models::InputStats;

    fn sample_input<'a>(
        targets: &'a [WorkSession],
        contexts: &'a [WorkSession],
        events: &'a [RawActivityEvent],
        categories: &'a [CategoryOption],
        projects: &'a [Project],
        tasks: &'a [Task],
    ) -> AiReviewInput<'a> {
        AiReviewInput {
            targets,
            context_sessions: contexts,
            events,
            categories,
            projects,
            tasks,
            memories: &[],
        }
    }

    #[test]
    fn removes_query_and_fragment_from_ai_payload() {
        assert_eq!(
            strip_url_noise("https://example.com/a?token=secret#part"),
            "https://example.com/a"
        );
    }

    #[test]
    fn keeps_only_last_path_components() {
        assert_eq!(
            last_path_parts(r"C:\\Users\\me\\Code\\ScreenUse\\src\\main.rs"),
            "ScreenUse/src/main.rs"
        );
    }

    #[test]
    fn reads_token_usage_from_codex_json_events() {
        let usage = usage_from_codex_jsonl(
            br#"{"type":"turn.started"}
{"type":"turn.completed","usage":{"input_tokens":14408,"cached_input_tokens":7936,"output_tokens":25,"reasoning_output_tokens":20}}
"#,
        );
        assert_eq!(usage.input_tokens, 14_408);
        assert_eq!(usage.cached_input_tokens, 7_936);
        assert_eq!(usage.output_tokens, 25);
        assert_eq!(usage.reasoning_output_tokens, 20);
        assert_eq!(usage.total_tokens, 14_433);
        assert!(usage.cost_usd.is_none());
        assert_eq!(
            usage.cost_note.as_deref(),
            Some("当前 Codex 账号未返回单次金额")
        );
    }

    #[test]
    fn reads_usage_and_provider_cost_from_compatible_api() {
        let usage = usage_from_api_response(&json!({
            "usage": {
                "prompt_tokens": 1200,
                "completion_tokens": 80,
                "total_tokens": 1280,
                "prompt_tokens_details": {"cached_tokens": 500},
                "completion_tokens_details": {"reasoning_tokens": 32},
                "cost": 0.0123
            }
        }));
        assert_eq!(usage.input_tokens, 1_200);
        assert_eq!(usage.cached_input_tokens, 500);
        assert_eq!(usage.output_tokens, 80);
        assert_eq!(usage.reasoning_output_tokens, 32);
        assert_eq!(usage.total_tokens, 1_280);
        assert_eq!(usage.cost_usd, Some(0.0123));
        assert!(usage.cost_note.is_none());
    }

    #[test]
    fn repairs_one_character_session_id_typos_and_ignores_exact_duplicates() {
        let session = |id: &str| WorkSession {
            id: id.into(),
            started_at: "2026-07-14T10:00:00Z".into(),
            ended_at: "2026-07-14T10:02:00Z".into(),
            project_id: None,
            project_name: None,
            task_id: None,
            task_title: None,
            category: "杂务".into(),
            summary: "ChatGPT".into(),
            confidence: 0.55,
            evidence: vec![],
            user_confirmed: false,
            source: "context-complete".into(),
        };
        let first_id = "6c94a876-b1b6-44e2-83ab-bbe8c252f5c6";
        let second_id = "37d6bb41-0f24-4411-89da-93a481e43510";
        let targets = vec![session(first_id), session(second_id)];
        let categories = vec![CategoryOption {
            name: "杂务".into(),
            color: "#fff".into(),
            is_builtin: true,
        }];
        let input = sample_input(&targets, &[], &[], &categories, &[], &[]);
        let result = |session_id: &str| AiAttributionResult {
            session_id: session_id.into(),
            project_id: None,
            task_id: None,
            category: "杂务".into(),
            summary: "网页对话".into(),
            confidence: 0.8,
            evidence: vec![],
        };
        let mut batch = AiAttributionBatch {
            results: vec![
                result(first_id),
                result(first_id),
                result("37d6bb41-0f24-4411-89da-93a481a43510"),
            ],
        };

        validate_batch(&mut batch, &input).expect("repair model session IDs");
        assert_eq!(batch.results.len(), 2);
        assert_eq!(batch.results[0].session_id, first_id);
        assert_eq!(batch.results[1].session_id, second_id);
    }

    #[test]
    fn payload_contains_neighbor_sessions_and_the_complete_catalog() {
        let target = WorkSession {
            id: "target".into(),
            started_at: "2026-07-14T10:00:00Z".into(),
            ended_at: "2026-07-14T10:02:00Z".into(),
            project_id: None,
            project_name: None,
            task_id: None,
            task_title: None,
            category: "杂务".into(),
            summary: "QQ".into(),
            confidence: 0.55,
            evidence: vec![],
            user_confirmed: false,
            source: "context-complete".into(),
        };
        let mut context = target.clone();
        context.id = "before".into();
        context.summary = "成果填报".into();
        let categories = vec![CategoryOption {
            name: "保研".into(),
            color: "#fff".into(),
            is_builtin: false,
        }];
        let project = Project {
            id: "project".into(),
            name: "推免".into(),
            category: "保研".into(),
            source: "manual".into(),
            color: "#fff".into(),
            description: None,
            created_at: "2026-07-14T00:00:00Z".into(),
            updated_at: "2026-07-14T00:00:00Z".into(),
        };
        let task = Task {
            id: "task".into(),
            project_id: "project".into(),
            title: "成果填报".into(),
            status: "active".into(),
            source: "manual".into(),
            planned_due_at: None,
            created_at: "2026-07-14T00:00:00Z".into(),
            updated_at: "2026-07-14T00:00:00Z".into(),
        };
        let event = RawActivityEvent {
            id: "event".into(),
            source: "windows-foreground".into(),
            timestamp: "2026-07-14T10:00:00Z".into(),
            app: Some("QQ.exe".into()),
            window_title: Some("QQ".into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({"activePageTitle": "成果填报群"}),
        };
        let targets = vec![target];
        let contexts = vec![context];
        let events = vec![event];
        let projects = vec![project];
        let tasks = vec![task];
        let memories = vec![RetrievedMemoryExample {
            target_session_id: "target".into(),
            observed: ContextFeatures {
                app: "WPS.exe".into(),
                page: "成果填报表.xlsx".into(),
                ..ContextFeatures::default()
            },
            category: "保研".into(),
            project_id: "project".into(),
            project_name: "推免".into(),
            task_id: "task".into(),
            task_title: "成果填报".into(),
            similarity: 0.94,
            confirmed_at: "2026-07-13T10:00:00Z".into(),
        }];
        let input = AiReviewInput {
            targets: &targets,
            context_sessions: &contexts,
            events: &events,
            categories: &categories,
            projects: &projects,
            tasks: &tasks,
            memories: &memories,
        };
        let payload = review_payload(&input);
        assert_eq!(payload["reviewItems"].as_array().unwrap().len(), 1);
        assert_eq!(
            payload["reviewItems"][0]["targetSession"]["sessionId"],
            "target"
        );
        assert_eq!(
            payload["reviewItems"][0]["contextSessionIds"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(payload["timelineContext"].as_array().unwrap().len(), 1);
        assert_eq!(
            payload["catalog"]["categories"].as_array().unwrap().len(),
            1
        );
        assert_eq!(payload["catalog"]["projects"].as_array().unwrap().len(), 1);
        assert_eq!(payload["catalog"]["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(payload["reviewItems"][0]["events"][0]["page"], "成果填报群");
        assert_eq!(payload["personalMemory"].as_array().unwrap().len(), 1);
        assert_eq!(payload["personalMemory"][0]["targetSessionId"], "target");
        assert_eq!(payload["personalMemory"][0]["taskId"], "task");
    }

    #[test]
    fn codex_schema_only_accepts_ids_from_the_current_batch() {
        let ids = vec!["session-a".to_string(), "session-b".to_string()];
        let schema = review_schema(&ids);
        assert_eq!(
            schema["properties"]["results"]["items"]["properties"]["sessionId"]["enum"],
            json!(["session-a", "session-b"])
        );
    }

    #[test]
    fn rejects_a_task_outside_the_selected_project() {
        let target = WorkSession {
            id: "target".into(),
            started_at: "2026-07-14T10:00:00Z".into(),
            ended_at: "2026-07-14T10:02:00Z".into(),
            project_id: None,
            project_name: None,
            task_id: None,
            task_title: None,
            category: "杂务".into(),
            summary: "QQ".into(),
            confidence: 0.55,
            evidence: vec![],
            user_confirmed: false,
            source: "context-complete".into(),
        };
        let categories = vec![CategoryOption {
            name: "保研".into(),
            color: "#fff".into(),
            is_builtin: false,
        }];
        let project = Project {
            id: "project".into(),
            name: "推免".into(),
            category: "保研".into(),
            source: "manual".into(),
            color: "#fff".into(),
            description: None,
            created_at: "".into(),
            updated_at: "".into(),
        };
        let task = Task {
            id: "task".into(),
            project_id: "another-project".into(),
            title: "成果填报".into(),
            status: "active".into(),
            source: "manual".into(),
            planned_due_at: None,
            created_at: "".into(),
            updated_at: "".into(),
        };
        let targets = vec![target];
        let projects = vec![project];
        let tasks = vec![task];
        let input = sample_input(&targets, &[], &[], &categories, &projects, &tasks);
        let mut result = AiAttributionResult {
            session_id: "target".into(),
            project_id: Some("project".into()),
            task_id: Some("task".into()),
            category: "保研".into(),
            summary: "成果填报".into(),
            confidence: 0.9,
            evidence: vec![],
        };
        assert!(validate_result(&mut result, &input).is_err());
    }

    #[test]
    fn repairs_category_and_project_from_the_selected_task() {
        let target = WorkSession {
            id: "target".into(),
            started_at: "2026-07-14T10:00:00Z".into(),
            ended_at: "2026-07-14T10:02:00Z".into(),
            project_id: None,
            project_name: None,
            task_id: None,
            task_title: None,
            category: "杂务".into(),
            summary: "会议".into(),
            confidence: 0.55,
            evidence: vec![],
            user_confirmed: false,
            source: "context-complete".into(),
        };
        let categories = vec![
            CategoryOption {
                name: "学习".into(),
                color: "#fff".into(),
                is_builtin: true,
            },
            CategoryOption {
                name: "校内实习".into(),
                color: "#aaa".into(),
                is_builtin: false,
            },
        ];
        let projects = vec![
            Project {
                id: "wrong-project".into(),
                name: "校内实习".into(),
                category: "校内实习".into(),
                source: "manual".into(),
                color: "#aaa".into(),
                description: None,
                created_at: "".into(),
                updated_at: "".into(),
            },
            Project {
                id: "iot-project".into(),
                name: "科研".into(),
                category: "学习".into(),
                source: "manual".into(),
                color: "#fff".into(),
                description: None,
                created_at: "".into(),
                updated_at: "".into(),
            },
        ];
        let tasks = vec![Task {
            id: "iot-task".into(),
            project_id: "iot-project".into(),
            title: "IOT".into(),
            status: "active".into(),
            source: "manual".into(),
            planned_due_at: None,
            created_at: "".into(),
            updated_at: "".into(),
        }];
        let targets = vec![target];
        let input = sample_input(&targets, &[], &[], &categories, &projects, &tasks);
        let mut result = AiAttributionResult {
            session_id: "target".into(),
            project_id: Some("wrong-project".into()),
            task_id: Some("iot-task".into()),
            category: "校内实习".into(),
            summary: "参加 IOT 科研会议".into(),
            confidence: 0.9,
            evidence: vec![],
        };

        validate_result(&mut result, &input).expect("repair hierarchy from task");
        assert_eq!(result.project_id.as_deref(), Some("iot-project"));
        assert_eq!(result.category, "学习");
    }

    #[tokio::test]
    #[ignore = "requires a locally logged-in Codex ChatGPT account"]
    async fn codex_luna_classifies_with_neighbor_context_and_catalog() {
        let target = WorkSession {
            id: "target".into(),
            started_at: "2026-07-14T10:02:00Z".into(),
            ended_at: "2026-07-14T10:04:00Z".into(),
            project_id: None,
            project_name: None,
            task_id: None,
            task_title: None,
            category: "杂务".into(),
            summary: "QQ".into(),
            confidence: 0.55,
            evidence: vec![EvidenceItem {
                kind: "page".into(),
                label: "当前页面".into(),
                value: "成果填报群".into(),
                weight: 0.8,
            }],
            user_confirmed: false,
            source: "context-complete".into(),
        };
        let mut context = target.clone();
        context.id = "before".into();
        context.started_at = "2026-07-14T10:00:00Z".into();
        context.ended_at = "2026-07-14T10:02:00Z".into();
        context.project_id = Some("project".into());
        context.project_name = Some("推免".into());
        context.task_id = Some("task".into());
        context.task_title = Some("成果填报".into());
        context.category = "保研".into();
        context.summary = "填写推免成果材料".into();
        context.confidence = 0.98;
        context.user_confirmed = true;
        let categories = vec![
            CategoryOption {
                name: "杂务".into(),
                color: "#aaa".into(),
                is_builtin: true,
            },
            CategoryOption {
                name: "保研".into(),
                color: "#fff".into(),
                is_builtin: false,
            },
        ];
        let projects = vec![Project {
            id: "project".into(),
            name: "推免".into(),
            category: "保研".into(),
            source: "manual".into(),
            color: "#fff".into(),
            description: None,
            created_at: "".into(),
            updated_at: "".into(),
        }];
        let tasks = vec![Task {
            id: "task".into(),
            project_id: "project".into(),
            title: "成果填报".into(),
            status: "active".into(),
            source: "manual".into(),
            planned_due_at: None,
            created_at: "".into(),
            updated_at: "".into(),
        }];
        let targets = vec![target];
        let contexts = vec![context];
        let input = sample_input(&targets, &contexts, &[], &categories, &projects, &tasks);
        let mut settings = AppSettings::default().normalized();
        settings.ai_provider = "codex-account".into();
        settings.ai_model = "gpt-5.6-luna".into();
        let result = analyze_with_codex_account(&settings, &input)
            .await
            .expect("Codex Luna classification");
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].project_id.as_deref(), Some("project"));
        assert_eq!(result.results[0].task_id.as_deref(), Some("task"));
        assert_eq!(result.results[0].category, "保研");
    }
}
