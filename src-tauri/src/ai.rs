#![allow(dead_code)]

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
const MAX_AI_EVENTS_PER_SESSION: usize = 30;
const MAX_AI_CONTEXT_SESSIONS_PER_TARGET: usize = 24;

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

    pub async fn analyze_review(
        &self,
        input: &AiReviewInput<'_>,
    ) -> Result<AiAttributionBatch> {
        if input.targets.is_empty() {
            return Err(anyhow!("no sessions to analyze"));
        }
        let system_prompt = review_instructions();
        let user_prompt = review_prompt(input)?;
        let response = self.request_review(system_prompt, &user_prompt).await?;
        parse_and_validate(&response.content, input)
    }

    pub async fn request_review(&self, system_prompt: &str, user_prompt: &str) -> Result<AiResponse> {
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
    let response = request_with_codex_account(settings, system_prompt, &user_prompt).await?;
    parse_and_validate(&response.content, input)
}

pub async fn request_with_codex_account(
    settings: &AppSettings,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<AiResponse> {
    let model = settings.ai_model.trim();
    if model.is_empty() {
        return Err(anyhow!("Codex model is empty"));
    }

    let work_dir = std::env::temp_dir().join(format!("screenuse-ai-{}", Uuid::new_v4()));
    fs::create_dir_all(&work_dir)?;
    let schema_path = work_dir.join("result.schema.json");
    let output_path = work_dir.join("result.json");
    fs::write(&schema_path, serde_json::to_vec(&review_schema())?)?;

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
                detail.chars().rev().take(500).collect::<String>().chars().rev().collect::<String>()
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
    let input_tokens = json_u64(source, &["prompt_tokens"])
        .max(json_u64(source, &["input_tokens"]));
    let output_tokens = json_u64(source, &["completion_tokens"])
        .max(json_u64(source, &["output_tokens"]));
    let cached_input_tokens = json_u64(source, &["prompt_tokens_details", "cached_tokens"])
        .max(json_u64(source, &["input_tokens_details", "cached_tokens"]));
    let reasoning_output_tokens = json_u64(
        source,
        &["completion_tokens_details", "reasoning_tokens"],
    )
    .max(json_u64(
        source,
        &["output_tokens_details", "reasoning_tokens"],
    ));
    let total_tokens = json_u64(source, &["total_tokens"])
        .max(input_tokens.saturating_add(output_tokens));
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
        cost_note: (total_tokens > 0 && cost_usd.is_none())
            .then(|| "接口未返回单次金额".into()),
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
        return Err(anyhow!("Codex 当前不是 ChatGPT 账号登录，请先使用 codex login 切换"));
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
    "你是个人电脑时间账本的复核器。不要调用工具、不要读取文件、不要联网，只分析输入 JSON。\
必须为 reviewItems 中每个 targetSession.sessionId 返回且只返回一个结果。只能选择 catalog 中现有的 category、projectId、taskId；\
taskId 必须属于 projectId，projectId 必须属于 category。目标尚未归到具体任务时，必须优先从 catalog 选择最具体且合理的任务；只有 catalog 确实没有匹配项时才返回 null，禁止创造新名称。\
每个 reviewItem 的 contextSessions 只是该目标前后时间线线索，不可作为待修改目标。优先根据当前页面、文档、窗口标题、工作区和前后事务连续性判断，\
不要仅按应用名归类；跨 Explorer、浏览器、QQ、微信、WPS 等软件仍可能是同一任务。summary 简洁描述当时实际事务。\
confidence 为 0 到 0.98；evidence 最多 3 项。只输出符合给定 JSON Schema 的 JSON。"
}

pub(crate) fn review_prompt(input: &AiReviewInput<'_>) -> Result<String> {
    Ok(format!(
        "复核以下目标会话。前后上下文窗口为目标前后 30 分钟；catalog 包含当前全部分类、项目和任务。输入：{}",
        serde_json::to_string(&review_payload(input))?
    ))
}

fn review_payload(input: &AiReviewInput<'_>) -> Value {
    let review_items = input
        .targets
        .iter()
        .map(|session| {
            let events = input
                .events
                .iter()
                .filter(|event| {
                    event.timestamp.as_str() >= session.started_at.as_str()
                        && event.timestamp.as_str() <= session.ended_at.as_str()
                })
                .rev()
                .take(MAX_AI_EVENTS_PER_SESSION)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(compact_event)
                .collect::<Vec<_>>();
            json!({
                "targetSession": compact_session(session),
                "contextSessions": context_sessions_for_target(input, session),
                "events": events,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "reviewItems": review_items,
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

fn context_sessions_for_target(
    input: &AiReviewInput<'_>,
    target: &WorkSession,
) -> Vec<Value> {
    let bounds = DateTimeBounds::from_session(target);
    input
        .context_sessions
        .iter()
        .filter(|session| session.id != target.id)
        .filter(|session| bounds.as_ref().map_or(true, |bounds| bounds.overlaps(session)))
        .take(MAX_AI_CONTEXT_SESSIONS_PER_TARGET)
        .map(compact_session)
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
        "evidence": session.evidence.iter().take(12).map(|item| json!({
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
        "title": event.window_title,
        "page": event.metadata.get("activePageTitle").and_then(Value::as_str),
        "url": event.url.as_deref().map(strip_url_noise),
        "file": event.file_path.as_deref().map(last_path_parts),
        "workspace": event.workspace.as_deref().map(last_path_parts),
        "idleSeconds": event.input_stats.idle_seconds,
    })
}

fn review_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "results": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "sessionId": {"type": "string"},
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
    let expected = input
        .targets
        .iter()
        .map(|session| session.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut returned = std::collections::HashSet::new();
    for result in &mut batch.results {
        if !expected.contains(result.session_id.as_str()) || !returned.insert(result.session_id.clone()) {
            return Err(anyhow!("AI returned an unexpected or duplicate sessionId"));
        }
        validate_result(result, input)?;
    }
    if returned.len() != expected.len() {
        return Err(anyhow!("AI did not return every target session"));
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
    if !input.categories.iter().any(|item| item.name == result.category) {
        return Err(anyhow!("AI returned unsupported category: {}", result.category));
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
        assert_eq!(usage.cost_note.as_deref(), Some("当前 Codex 账号未返回单次金额"));
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
        let input = sample_input(
            &targets,
            &contexts,
            &events,
            &categories,
            &projects,
            &tasks,
        );
        let payload = review_payload(&input);
        assert_eq!(payload["reviewItems"].as_array().unwrap().len(), 1);
        assert_eq!(payload["reviewItems"][0]["targetSession"]["sessionId"], "target");
        assert_eq!(payload["reviewItems"][0]["contextSessions"].as_array().unwrap().len(), 1);
        assert_eq!(payload["catalog"]["categories"].as_array().unwrap().len(), 1);
        assert_eq!(payload["catalog"]["projects"].as_array().unwrap().len(), 1);
        assert_eq!(payload["catalog"]["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(payload["reviewItems"][0]["events"][0]["page"], "成果填报群");
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
            CategoryOption { name: "学习".into(), color: "#fff".into(), is_builtin: true },
            CategoryOption { name: "校内实习".into(), color: "#aaa".into(), is_builtin: false },
        ];
        let projects = vec![
            Project {
                id: "wrong-project".into(), name: "校内实习".into(), category: "校内实习".into(),
                source: "manual".into(), color: "#aaa".into(), description: None,
                created_at: "".into(), updated_at: "".into(),
            },
            Project {
                id: "iot-project".into(), name: "科研".into(), category: "学习".into(),
                source: "manual".into(), color: "#fff".into(), description: None,
                created_at: "".into(), updated_at: "".into(),
            },
        ];
        let tasks = vec![Task {
            id: "iot-task".into(), project_id: "iot-project".into(), title: "IOT".into(),
            status: "active".into(), source: "manual".into(), planned_due_at: None,
            created_at: "".into(), updated_at: "".into(),
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
