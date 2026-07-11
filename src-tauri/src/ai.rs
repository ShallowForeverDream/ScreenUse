#![allow(dead_code)]

use crate::models::{AppSettings, EvidenceItem, RawActivityEvent, DEFAULT_CATEGORIES};
use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

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
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            base_url: settings.ai_base_url.trim_end_matches('/').to_string(),
            model: settings.ai_model.trim().to_string(),
            api_key,
        }
    }

    pub async fn analyze_metadata_block(
        &self,
        events: &[RawActivityEvent],
    ) -> Result<AiAttributionResult> {
        if events.is_empty() {
            return Err(anyhow!("no events to analyze"));
        }
        if self.model.is_empty() {
            return Err(anyhow!("AI model is empty"));
        }

        // A context normally has only a handful of stable-ID heartbeats. Keep a
        // hard cap so an imported or legacy session cannot create an expensive prompt.
        let compact_events: Vec<_> = events
            .iter()
            .rev()
            .take(80)
            .rev()
            .map(|event| {
                json!({
                    "time": event.timestamp,
                    "source": event.source,
                    "app": event.app,
                    "title": event.window_title,
                    "url": event.url.as_deref().map(strip_url_noise),
                    "file": event.file_path.as_deref().map(last_path_parts),
                    "workspace": event.workspace.as_deref().map(last_path_parts),
                    "idleSeconds": event.input_stats.idle_seconds,
                })
            })
            .collect();
        let body = json!({
            "model": self.model,
            "messages": [
                {
                    "role": "system",
                    "content": "你是个人电脑时间账本的归类器。只输出 JSON 对象，字段为 projectName、taskTitle、category、summary、confidence、evidence。category 只能是 学习、写作、开发、沟通、娱乐、杂务、离开；confidence 为 0-1；evidence 最多 3 项，每项含 kind、label、value、weight。只依据元数据，摘要不超过 30 个汉字。"
                },
                {
                    "role": "user",
                    "content": format!("将这一段连续活动归到一个项目/任务：{}", serde_json::to_string(&compact_events)?)
                }
            ],
            "temperature": 0.1,
            "max_tokens": 500,
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

        let value: serde_json::Value = response.json().await?;
        let content = value["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow!("missing AI response content"))?;
        let mut parsed: AiAttributionResult = serde_json::from_str(content).or_else(|_| {
            extract_json_object(content)
                .and_then(|json| serde_json::from_str(&json).map_err(anyhow::Error::from))
        })?;
        validate_result(&mut parsed)?;
        Ok(parsed)
    }
}

fn validate_result(result: &mut AiAttributionResult) -> Result<()> {
    if !DEFAULT_CATEGORIES.contains(&result.category.as_str()) {
        return Err(anyhow!("AI returned unsupported category: {}", result.category));
    }
    result.project_name = clean(&result.project_name, "自动发现项目", 80);
    result.task_title = clean(&result.task_title, "待确认活动", 80);
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

    #[test]
    fn removes_query_and_fragment_from_ai_payload() {
        assert_eq!(
            strip_url_noise("https://example.com/a?token=secret#part"),
            "https://example.com/a"
        );
    }

    #[test]
    fn keeps_only_last_path_components() {
        assert_eq!(last_path_parts(r"C:\\Users\\me\\Code\\ScreenUse\\src\\main.rs"), "ScreenUse/src/main.rs");
    }
}
