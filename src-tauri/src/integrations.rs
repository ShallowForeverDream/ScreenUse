#![allow(dead_code)]

use crate::models::PlanItem;
use anyhow::Result;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::collections::hash_map::DefaultHasher;

pub trait IntegrationAdapter {
    fn name(&self) -> &'static str;
    fn pull_plan_items(&self) -> Result<Vec<PlanItem>>;
}

pub struct IcsAdapter {
    pub path: String,
}

impl IntegrationAdapter for IcsAdapter {
    fn name(&self) -> &'static str { "ics" }
    fn pull_plan_items(&self) -> Result<Vec<PlanItem>> {
        parse_ics_file(&self.path)
    }
}

pub fn parse_ics_file(path: &str) -> Result<Vec<PlanItem>> {
    if !Path::new(path).exists() { return Ok(vec![]); }
    let text = fs::read_to_string(path)?;
    let lines = unfold_ics_lines(&text);
    let mut items = Vec::new();
    let mut in_event = false;
    let mut uid = String::new();
    let mut title = String::new();
    let mut start = None;
    let mut end = None;
    let mut note = None;
    let mut status = "planned".to_string();
    for line in lines {
        let line = line.trim();
        match line {
            "BEGIN:VEVENT" => { in_event = true; uid.clear(); title.clear(); start = None; end = None; note = None; status = "planned".into(); },
            "END:VEVENT" if in_event => {
                if !title.is_empty() {
                    let stable = if uid.is_empty() { stable_hash(&format!("{title:?}{start:?}{end:?}{note:?}")) } else { uid.clone() };
                    items.push(PlanItem { id: format!("ics:{}", stable), source: "ICS".into(), title: title.clone(), note: note.clone(), start_at: start.clone(), due_at: end.clone().or_else(|| start.clone()), status: status.clone(), tags: vec!["日历".into()], matched_session_ids: vec![] });
                }
                in_event = false;
            },
            _ if in_event => {
                if let Some(v) = property_value(line, "UID") { uid = sanitize_id(v); }
                else if let Some(v) = property_value(line, "SUMMARY") { title = unescape_ics(v); }
                else if let Some(v) = property_value(line, "DESCRIPTION") { note = Some(unescape_ics(v)); }
                else if let Some(v) = property_value(line, "STATUS") { status = v.to_lowercase(); }
                else if line.starts_with("DTSTART") { start = line.split_once(':').map(|(_, v)| normalize_ics_time(v)); }
                else if line.starts_with("DTEND") { end = line.split_once(':').map(|(_, v)| normalize_ics_time(v)); }
            },
            _ => {}
        }
    }
    Ok(items)
}

pub fn google_calendar_placeholder() -> Vec<PlanItem> { vec![] }
pub fn microsoft_todo_placeholder() -> Vec<PlanItem> { vec![] }

fn unfold_ics_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        if raw.starts_with(' ') || raw.starts_with('\t') {
            if let Some(last) = out.last_mut() { last.push_str(raw.trim_start()); }
        } else {
            out.push(raw.trim_end_matches('\r').to_string());
        }
    }
    out
}

fn property_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    if line.starts_with(&format!("{name}:")) || line.starts_with(&format!("{name};")) {
        return line.split_once(':').map(|(_, v)| v);
    }
    None
}

fn unescape_ics(v: &str) -> String { v.replace("\\n", " ").replace("\\,", ",").replace("\\;", ";").replace("\\\\", "\\") }
fn normalize_ics_time(v: &str) -> String {
    let v = v.trim_end_matches('Z');
    if v.len() == 8 { format!("{}-{}-{}T00:00:00Z", &v[0..4], &v[4..6], &v[6..8]) }
    else if v.len() >= 15 { format!("{}-{}-{}T{}:{}:{}Z", &v[0..4], &v[4..6], &v[6..8], &v[9..11], &v[11..13], &v[13..15]) }
    else { v.to_string() }
}

fn sanitize_id(value: &str) -> String {
    value.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' }).collect()
}

fn stable_hash(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}
