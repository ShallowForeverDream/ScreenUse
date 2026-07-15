use crate::db::AppDb;
use crate::models::{CodexModelRate, CodexRateCard};
use anyhow::{anyhow, bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use reqwest::Client;
use rusqlite::{params, OptionalExtension};
use std::time::Duration;

const RATE_CARD_KEY: &str = "codex_rate_card_v1";
const RATE_CARD_URL: &str = "https://help.openai.com/en/articles/20001106-codex-rate-card.json";

pub fn get_rate_card(db: &AppDb) -> Result<CodexRateCard> {
    let raw: Option<String> = db
        .conn
        .lock()
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            params![RATE_CARD_KEY],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .filter(valid_rate_card)
        .unwrap_or_else(bundled_rate_card))
}

pub async fn refresh_rate_card(db: &AppDb) -> Result<CodexRateCard> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("ScreenUse/0.2")
        .build()?;
    let response = client
        .get(RATE_CARD_URL)
        .send()
        .await
        .context("无法读取 OpenAI Codex 官方费率")?;
    if !response.status().is_success() {
        bail!("OpenAI Codex 官方费率返回 {}", response.status());
    }
    let page = response.text().await?;
    let card = parse_rate_card(&page)?;
    db.conn.lock().execute(
        "INSERT INTO settings(key,value,updated_at) VALUES(?1,?2,?3)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
        params![
            RATE_CARD_KEY,
            serde_json::to_string(&card)?,
            card.fetched_at
        ],
    )?;
    Ok(card)
}

fn parse_rate_card(page: &str) -> Result<CodexRateCard> {
    let marker = "Codex rate card - token-based pricing";
    let start = page
        .find(marker)
        .context("OpenAI 费率页中缺少 token 计费表")?;
    let table_start = page[start..]
        .find("<table")
        .map(|offset| start + offset)
        .context("OpenAI 费率页中缺少费率表格")?;
    let table_end = page[table_start..]
        .find("</table>")
        .map(|offset| table_start + offset)
        .context("OpenAI 费率表格不完整")?;

    let mut rates = Vec::new();
    for row in page[table_start..table_end].split("<tr") {
        let cells = html_cells(row);
        if cells.len() != 4 || !cells[0].to_ascii_lowercase().starts_with("gpt-") {
            continue;
        }
        let Some(input) = parse_credits(&cells[1]) else {
            continue;
        };
        let Some(cached) = parse_credits(&cells[2]) else {
            continue;
        };
        let Some(output) = parse_credits(&cells[3]) else {
            continue;
        };
        if [input, cached, output]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0 || *value > 10_000.0)
        {
            continue;
        }
        rates.push(CodexModelRate {
            model: cells[0].chars().take(80).collect(),
            input_credits_per_million: input,
            cached_input_credits_per_million: cached,
            output_credits_per_million: output,
        });
    }
    if rates.len() < 3
        || !rates
            .iter()
            .any(|rate| normalize_model(&rate.model) == "gpt56luna")
    {
        return Err(anyhow!("OpenAI 费率表解析结果不完整，已保留本地费率"));
    }

    Ok(CodexRateCard {
        source_url: RATE_CARD_URL.trim_end_matches(".json").into(),
        fetched_at: now(),
        source_updated_label: source_updated_label(page),
        rates,
    })
}

fn html_cells(row: &str) -> Vec<String> {
    row.split("<td")
        .skip(1)
        .filter_map(|cell| {
            let body = cell.split_once('>')?.1;
            let body = body.split_once("</td>")?.0;
            Some(strip_tags(body))
        })
        .collect()
}

fn strip_tags(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    output
        .replace("&amp;", "&")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_credits(value: &str) -> Option<f64> {
    value
        .split_whitespace()
        .next()?
        .replace(',', "")
        .parse()
        .ok()
}

fn source_updated_label(page: &str) -> Option<String> {
    let marker = "Updated:";
    let start = page.find(marker)? + marker.len();
    let end = page[start..].find('<').map(|offset| start + offset)?;
    let label = strip_tags(&page[start..end]);
    (!label.is_empty()).then_some(label)
}

fn normalize_model(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn valid_rate_card(card: &CodexRateCard) -> bool {
    card.rates.len() >= 3
        && card
            .rates
            .iter()
            .any(|rate| normalize_model(&rate.model) == "gpt56luna")
}

fn bundled_rate_card() -> CodexRateCard {
    let rate = |model: &str, input, cached, output| CodexModelRate {
        model: model.into(),
        input_credits_per_million: input,
        cached_input_credits_per_million: cached,
        output_credits_per_million: output,
    };
    CodexRateCard {
        source_url: RATE_CARD_URL.trim_end_matches(".json").into(),
        fetched_at: "2026-07-15T00:00:00Z".into(),
        source_updated_label: Some("ScreenUse 内置官方费率快照".into()),
        rates: vec![
            rate("GPT-5.6 Sol", 125.0, 12.5, 750.0),
            rate("GPT-5.6 Terra", 62.5, 6.25, 375.0),
            rate("GPT-5.6 Luna", 25.0, 2.5, 150.0),
            rate("GPT-5.5", 125.0, 12.5, 750.0),
            rate("GPT-5.5 Cyber", 500.0, 50.0, 3_000.0),
            rate("GPT-5.4", 62.5, 6.25, 375.0),
            rate("GPT-5.4-Mini", 18.75, 1.875, 113.0),
            rate("GPT-5.3-Codex", 43.75, 4.375, 350.0),
            rate("GPT-5.2", 43.75, 4.375, 350.0),
        ],
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_token_rate_table() {
        let page = r#"
          <div>Updated: 15 hours ago</div>
          <h1>Codex rate card - token-based pricing</h1>
          <table><tbody>
            <tr><td><span>GPT-5.6 Sol</span></td><td>125 credits</td><td>12.50 credits</td><td>750 credits</td></tr>
            <tr><td>GPT-5.6 Terra</td><td>62.50 credits</td><td>6.250 credits</td><td>375 credits</td></tr>
            <tr><td>GPT-5.6 Luna</td><td>25 credits</td><td>2.50 credits</td><td>150 credits</td></tr>
          </tbody></table>
        "#;
        let card = parse_rate_card(page).expect("parse rate card");
        let luna = card
            .rates
            .iter()
            .find(|rate| rate.model == "GPT-5.6 Luna")
            .expect("luna rate");
        assert_eq!(luna.input_credits_per_million, 25.0);
        assert_eq!(luna.cached_input_credits_per_million, 2.5);
        assert_eq!(luna.output_credits_per_million, 150.0);
        assert_eq!(card.source_updated_label.as_deref(), Some("15 hours ago"));
    }
}
