use crate::db::AppDb;
use crate::models::{AiUsage, CodexModelRate, CodexRateCard};
use anyhow::{anyhow, bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use reqwest::Client;
use rusqlite::{params, OptionalExtension};
use std::time::Duration;

const RATE_CARD_KEY: &str = "codex_rate_card_v1";
const RATE_CARD_URL: &str = "https://help.openai.com/en/articles/20001106-codex-rate-card.json";
const CREDIT_VALUE_URL: &str =
    "https://help.openai.com/en/articles/20001147-codex-credits-for-students-terms-of-service.json";
const DEFAULT_USD_PER_CREDIT: f64 = 0.04;

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
    let page = fetch_page(&client, RATE_CARD_URL, "OpenAI Codex 官方费率").await?;
    let mut card = parse_rate_card(&page)?;
    if let Ok(credit_page) = fetch_page(&client, CREDIT_VALUE_URL, "OpenAI Credits 换算依据").await {
        if let Some(value) = parse_usd_per_credit(&credit_page) {
            card.usd_per_credit = value;
            card.credit_value_source_url = Some(CREDIT_VALUE_URL.trim_end_matches(".json").into());
        }
    }
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

pub fn estimate_usage_cost(db: &AppDb, model: &str, usage: &AiUsage) -> Option<(f64, f64)> {
    let card = get_rate_card(db).ok()?;
    let normalized = normalize_model(model);
    let rate = card
        .rates
        .iter()
        .find(|item| normalize_model(&item.model) == normalized)?;
    let cached = usage.cached_input_tokens.min(usage.input_tokens);
    let uncached = usage.input_tokens.saturating_sub(cached);
    let credits = (uncached as f64 * rate.input_credits_per_million
        + cached as f64 * rate.cached_input_credits_per_million
        + usage.output_tokens as f64 * rate.output_credits_per_million)
        / 1_000_000.0;
    Some((credits, credits * card.usd_per_credit))
}

async fn fetch_page(client: &Client, url: &str, label: &str) -> Result<String> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("无法读取 {label}"))?;
    if !response.status().is_success() {
        bail!("{label}返回 {}", response.status());
    }
    Ok(response.text().await?)
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
        usd_per_credit: DEFAULT_USD_PER_CREDIT,
        credit_value_source_url: Some(CREDIT_VALUE_URL.trim_end_matches(".json").into()),
        rates,
    })
}

fn parse_usd_per_credit(page: &str) -> Option<f64> {
    let text = strip_tags(page);
    let marker = "credits, which is equivalent to $";
    let marker_start = text.find(marker)?;
    let credits = text[..marker_start]
        .split_whitespace()
        .next_back()?
        .trim_matches(|character: char| !character.is_ascii_digit() && character != ',' && character != '.')
        .replace(',', "")
        .parse::<f64>()
        .ok()?;
    let dollars = text[marker_start + marker.len()..]
        .split_whitespace()
        .next()?
        .trim_matches(|character: char| !character.is_ascii_digit() && character != '.')
        .parse::<f64>()
        .ok()?;
    let value = dollars / credits;
    (credits > 0.0 && value.is_finite() && value > 0.0 && value <= 1.0).then_some(value)
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
    card.usd_per_credit.is_finite()
        && card.usd_per_credit > 0.0
        && card.usd_per_credit <= 1.0
        && card.rates.len() >= 3
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
        usd_per_credit: DEFAULT_USD_PER_CREDIT,
        credit_value_source_url: Some(CREDIT_VALUE_URL.trim_end_matches(".json").into()),
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

    #[test]
    fn parses_official_credit_dollar_equivalent() {
        let page = r#"
          <p>Once the offer is applied, you should see <strong>2,500 credits</strong>,
          which is <strong>equivalent to $100</strong>.</p>
        "#;
        assert_eq!(parse_usd_per_credit(page), Some(0.04));
    }

    #[test]
    fn converts_luna_tokens_to_usd_equivalent() {
        let data_dir = std::env::temp_dir().join(format!(
            "screenuse-pricing-test-{}",
            uuid::Uuid::new_v4()
        ));
        let db = AppDb::open_in(data_dir.clone()).expect("open test database");
        let usage = AiUsage {
            input_tokens: 79_308,
            output_tokens: 2_354,
            total_tokens: 81_662,
            ..AiUsage::default()
        };
        let (credits, usd) = estimate_usage_cost(&db, "gpt-5.6-luna", &usage).expect("estimate");
        assert!((credits - 2.3358).abs() < 0.000_001);
        assert!((usd - 0.093_432).abs() < 0.000_001);
        drop(db);
        let _ = std::fs::remove_dir_all(data_dir);
    }
}
