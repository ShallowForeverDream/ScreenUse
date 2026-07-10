#![allow(dead_code)]

use crate::models::{AppSettings, EvidenceItem, RawActivityEvent};
use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAttributionResult {
    pub project_name: String,
    pub task_title: String,
    pub category: String,
    pub summary: String,
    pub confidence: f32,
    pub evidence: Vec<EvidenceItem>,
}

pub struct OpenAiCompatibleClient {
    client: Client,
    base_url: String,
    model: String,
    api_key: String,
}

impl OpenAiCompatibleClient {
    pub fn new(settings: &AppSettings, api_key: String) -> Self {
        Self {
            client: Client::new(),
            base_url: settings.ai_base_url.trim_end_matches('/').to_string(),
            model: settings.ai_model.clone(),
            api_key,
        }
    }

    pub async fn analyze_metadata_block(&self, events: &[RawActivityEvent]) -> Result<AiAttributionResult> {
        if events.is_empty() { return Err(anyhow!("no events to analyze")); }
        let compact_events: Vec<_> = events.iter().take(240).map(|e| json!({
            "time": e.timestamp,
            "source": e.source,
            "app": e.app,
            "title": e.window_title,
            "url": e.url,
            "filePath": e.file_path,
            "workspace": e.workspace,
            "idleSeconds": e.input_stats.idle_seconds,
            "shortcuts": e.input_stats.shortcut_events,
            "metadata": e.metadata,
        })).collect();
        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": "你是 ScreenUse 的个人时间归因引擎。只输出合法 JSON：projectName、taskTitle、category、summary、confidence、evidence。category 必须从 学习、写作、开发、沟通、娱乐、杂务、离开 中选择；confidence 为 0-1；evidence 是数组，每项包含 kind、label、value、weight。不要编造网页正文或文件正文，只依据元数据。"},
                {"role": "user", "content": format!("请把下面按时间排列的窗口、URL、文件、工作区、输入空闲等元数据归因为一个自然工作会话，并给出中文摘要：{}", serde_json::to_string(&compact_events)?) }
            ],
            "temperature": 0.2,
            "response_format": {"type": "json_object"}
        });
        let resp = self.client.post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(anyhow!("AI request failed: {} - {}", resp.status(), resp.text().await.unwrap_or_default()));
        }
        let value: serde_json::Value = resp.json().await?;
        let content = value["choices"][0]["message"]["content"].as_str().ok_or_else(|| anyhow!("missing content"))?;
        let parsed: AiAttributionResult = serde_json::from_str(content)
            .or_else(|_| extract_json_object(content).and_then(|s| serde_json::from_str(&s).map_err(anyhow::Error::from)))?;
        Ok(parsed)
    }
}

pub fn fallback_rule_summary(events: &[RawActivityEvent]) -> AiAttributionResult {
    let joined = events.iter().rev().take(8).filter_map(|e| e.window_title.clone()).collect::<Vec<_>>().join(" / ");
    AiAttributionResult {
        project_name: "自动发现项目".into(),
        task_title: "待确认活动".into(),
        category: "杂务".into(),
        summary: if joined.is_empty() { "规则降级：未识别活动".into() } else { format!("规则降级：{}", joined) },
        confidence: 0.35,
        evidence: vec![EvidenceItem { kind: "rule".into(), label: "降级规则".into(), value: "AI 不可用或重试失败".into(), weight: 0.35 }],
    }
}

fn extract_json_object(input: &str) -> Result<String> {
    let start = input.find('{').ok_or_else(|| anyhow!("AI response does not contain JSON object"))?;
    let end = input.rfind('}').ok_or_else(|| anyhow!("AI response JSON object is incomplete"))?;
    Ok(input[start..=end].to_string())
}
