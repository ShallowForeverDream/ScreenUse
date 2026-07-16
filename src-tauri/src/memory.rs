use crate::models::{EvidenceItem, RawActivityEvent, WorkSession};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const GENERIC_LABELS: &[&str] = &[
    "chatgpt",
    "codex",
    "chrome",
    "google chrome",
    "msedge",
    "microsoft edge",
    "firefox",
    "brave",
    "qq",
    "wechat",
    "weixin",
    "wps",
    "explorer",
    "文件资源管理器",
    "screenuse",
    "new tab",
    "新标签页",
    "desktop",
    "桌面",
    "program manager",
    "task switching",
    "任务切换",
    "任务视图",
    "腾讯会议",
    "会议",
    "图片查看器",
    "开启录音转写",
    "快速设置",
    "录音机",
    "release",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextFeatures {
    pub app: String,
    pub page: String,
    pub window: String,
    pub domain: String,
    pub workspace: String,
    pub file: String,
    pub tokens: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MemoryRecord {
    pub session_id: String,
    pub features: ContextFeatures,
    pub category: String,
    pub project_id: String,
    pub project_name: String,
    pub task_id: String,
    pub task_title: String,
    pub confirmed_at: String,
}

#[derive(Debug, Clone)]
pub struct MemoryDecision {
    pub category: String,
    pub project_id: String,
    pub task_id: String,
    pub confidence: f32,
    pub matched_label: String,
    pub support: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievedMemoryExample {
    pub target_session_id: String,
    pub observed: ContextFeatures,
    pub category: String,
    pub project_id: String,
    pub project_name: String,
    pub task_id: String,
    pub task_title: String,
    pub similarity: f32,
    pub confirmed_at: String,
}

#[derive(Debug, Clone, Copy)]
struct Similarity {
    score: f32,
    strong: bool,
}

pub fn features_from_event(event: &RawActivityEvent) -> ContextFeatures {
    let page = event
        .metadata
        .get("activePageTitle")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            event
                .metadata
                .get("chatgptConversationTitle")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or_default();
    build_features(
        event.app.as_deref().unwrap_or_default(),
        page,
        event.window_title.as_deref().unwrap_or_default(),
        event.url.as_deref().unwrap_or_default(),
        event.workspace.as_deref().unwrap_or_default(),
        event.file_path.as_deref().unwrap_or_default(),
    )
}

pub fn features_from_session(
    session: &WorkSession,
    events: &[RawActivityEvent],
) -> ContextFeatures {
    let latest = events.last();
    let page = events
        .iter()
        .rev()
        .find_map(|event| {
            event
                .metadata
                .get("activePageTitle")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    event
                        .metadata
                        .get("chatgptConversationTitle")
                        .and_then(serde_json::Value::as_str)
                })
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| evidence_value(&session.evidence, "page"))
        .unwrap_or_default();
    let window = latest
        .and_then(|event| event.window_title.as_deref())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| evidence_value(&session.evidence, "window"))
        .unwrap_or_default();
    let app = latest
        .and_then(|event| event.app.as_deref())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| evidence_value(&session.evidence, "app"))
        .unwrap_or_default();
    let url = events
        .iter()
        .rev()
        .find_map(|event| {
            event
                .url
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| evidence_value(&session.evidence, "url"))
        .unwrap_or_default();
    let workspace = events
        .iter()
        .rev()
        .find_map(|event| {
            event
                .workspace
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| evidence_value(&session.evidence, "workspace"))
        .unwrap_or_default();
    let file = events
        .iter()
        .rev()
        .find_map(|event| {
            event
                .file_path
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| evidence_value(&session.evidence, "file"))
        .unwrap_or_default();
    build_features(app, page, window, url, workspace, file)
}

pub fn features_from_session_evidence(session: &WorkSession) -> ContextFeatures {
    features_from_session(session, &[])
}

pub fn is_discriminative(features: &ContextFeatures) -> bool {
    !features.page.is_empty()
        || !features.domain.is_empty()
        || !features.workspace.is_empty()
        || !features.file.is_empty()
        || (!features.window.is_empty() && !is_generic(&features.window))
}

pub fn choose_assignment(
    query: &ContextFeatures,
    records: &[MemoryRecord],
) -> Option<MemoryDecision> {
    if !is_discriminative(query) {
        return None;
    }
    let mut candidates = records
        .iter()
        .filter_map(|record| {
            let similarity = similarity(query, &record.features);
            (similarity.score >= 0.42 && (similarity.strong || similarity.score >= 0.68))
                .then_some((record, similarity))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .1
            .score
            .total_cmp(&left.1.score)
            .then_with(|| right.0.confirmed_at.cmp(&left.0.confirmed_at))
    });
    candidates.truncate(12);

    #[derive(Default)]
    struct Vote<'a> {
        total: f32,
        best: f32,
        strong: bool,
        support: usize,
        record: Option<&'a MemoryRecord>,
    }
    let mut votes: HashMap<String, Vote<'_>> = HashMap::new();
    for (rank, (record, similarity)) in candidates.iter().enumerate() {
        let key = assignment_key(record);
        let vote = votes.entry(key).or_default();
        let decay = match rank {
            0 => 1.0,
            1 => 0.55,
            2 => 0.38,
            _ => 0.24,
        };
        vote.total += similarity.score * decay;
        vote.best = vote.best.max(similarity.score);
        vote.strong |= similarity.strong;
        vote.support += 1;
        if vote.record.is_none() {
            vote.record = Some(record);
        }
    }
    let mut ranked = votes.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .total
            .total_cmp(&left.total)
            .then_with(|| right.best.total_cmp(&left.best))
    });
    let winner = ranked.first()?;
    let runner_up = ranked.get(1);
    let margin = runner_up
        .map(|runner| winner.total - runner.total)
        .unwrap_or(winner.total);
    let conflicting_exact = runner_up
        .is_some_and(|runner| winner.strong && runner.strong && runner.best >= winner.best - 0.04);
    if conflicting_exact || winner.best < 0.60 {
        return None;
    }
    let confidence = if winner.support >= 2 && winner.best >= 0.72 && margin >= 0.18 {
        0.95
    } else if winner.best >= 0.86 && margin >= 0.12 {
        0.93
    } else if winner.strong && winner.best >= 0.62 && margin >= 0.14 {
        0.88
    } else if winner.best >= 0.74 && margin >= 0.20 {
        0.86
    } else {
        return None;
    };
    let record = winner.record?;
    Some(MemoryDecision {
        category: record.category.clone(),
        project_id: record.project_id.clone(),
        task_id: record.task_id.clone(),
        confidence,
        matched_label: strongest_label(query, &record.features),
        support: winner.support,
    })
}

pub fn retrieve_examples(
    targets: &[WorkSession],
    records: &[MemoryRecord],
    per_target: usize,
) -> Vec<RetrievedMemoryExample> {
    let mut output = Vec::new();
    for target in targets {
        let query = features_from_session_evidence(target);
        let mut ranked = records
            .iter()
            .filter_map(|record| {
                let value = similarity(&query, &record.features);
                (value.score >= 0.34).then_some((record, value.score))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| right.0.confirmed_at.cmp(&left.0.confirmed_at))
        });
        let mut seen_assignments = HashSet::new();
        for (record, score) in ranked {
            if !seen_assignments.insert(assignment_key(record)) {
                continue;
            }
            output.push(RetrievedMemoryExample {
                target_session_id: target.id.clone(),
                observed: record.features.clone(),
                category: record.category.clone(),
                project_id: record.project_id.clone(),
                project_name: record.project_name.clone(),
                task_id: record.task_id.clone(),
                task_title: record.task_title.clone(),
                similarity: (score * 1000.0).round() / 1000.0,
                confirmed_at: record.confirmed_at.clone(),
            });
            if output
                .iter()
                .rev()
                .take_while(|item| item.target_session_id == target.id)
                .count()
                >= per_target
            {
                break;
            }
        }
    }
    output
}

pub fn canonical_context(value: &str) -> String {
    let mut value = normalize(value);
    for suffix in [
        " - google chrome",
        " — google chrome",
        " - microsoft edge",
        " — microsoft edge",
        " — mozilla firefox",
        " - brave",
        " - visual studio code",
        " — visual studio code",
        " - 文件资源管理器",
    ] {
        if let Some(stripped) = value.strip_suffix(suffix) {
            value = stripped.trim().to_string();
        }
    }
    if let Some((head, tail)) = value.split_once(" 和 ") {
        if tail.contains("个其他选项卡") {
            value = head.trim().to_string();
        }
    }
    if is_generic(&value) || value.chars().count() < 2 {
        String::new()
    } else {
        value.chars().take(180).collect()
    }
}

pub fn clear_legacy_process_file(features: &mut ContextFeatures) -> bool {
    let file_name = features
        .file
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let Some(stem) = file_name
        .to_lowercase()
        .strip_suffix(".exe")
        .map(ToOwned::to_owned)
    else {
        return false;
    };
    if normalize(&stem) != normalize(&features.app) {
        return false;
    }
    features.file.clear();
    features.tokens.clear();
    for value in [
        &features.page,
        &features.window,
        &features.domain,
        &features.workspace,
    ] {
        features.tokens.extend(text_tokens(value));
    }
    features.tokens.sort();
    features.tokens.dedup();
    true
}

fn build_features(
    app: &str,
    page: &str,
    window: &str,
    url: &str,
    workspace: &str,
    file: &str,
) -> ContextFeatures {
    let app = normalize(app.trim_end_matches(".exe"));
    let page = canonical_context(page);
    let window = canonical_context(window);
    let domain = domain_from_url(url);
    let workspace = canonical_path(workspace);
    let file = canonical_path(file);
    let mut tokens = Vec::new();
    for value in [&page, &window, &domain, &workspace, &file] {
        tokens.extend(text_tokens(value));
    }
    tokens.sort();
    tokens.dedup();
    ContextFeatures {
        app,
        page,
        window,
        domain,
        workspace,
        file,
        tokens,
    }
}

fn evidence_value<'a>(evidence: &'a [EvidenceItem], kind: &str) -> Option<&'a str> {
    evidence
        .iter()
        .filter(|item| item.kind == kind)
        .max_by(|left, right| left.weight.total_cmp(&right.weight))
        .map(|item| item.value.as_str())
        .filter(|value| !value.trim().is_empty())
}

fn similarity(query: &ContextFeatures, memory: &ContextFeatures) -> Similarity {
    let mut score = 0.0_f32;
    let mut strong = false;
    if exact(&query.page, &memory.page) {
        score += 0.58;
        strong = true;
    } else {
        score += 0.30 * text_similarity(&query.page, &memory.page);
    }
    if exact(&query.workspace, &memory.workspace) {
        score += 0.36;
        strong = true;
    } else {
        score += 0.16 * text_similarity(&query.workspace, &memory.workspace);
    }
    if exact(&query.file, &memory.file) {
        score += 0.34;
        strong = true;
    } else {
        score += 0.14 * text_similarity(&query.file, &memory.file);
    }
    if exact(&query.domain, &memory.domain) {
        score += 0.28;
        strong = true;
    }
    if exact(&query.window, &memory.window) {
        score += 0.38;
        strong = true;
    } else {
        score += 0.15 * text_similarity(&query.window, &memory.window);
    }
    if exact(&query.app, &memory.app) {
        score += 0.08;
    }
    score += 0.26 * set_similarity(&query.tokens, &memory.tokens);
    Similarity {
        score: score.min(1.0),
        strong,
    }
}

fn strongest_label(query: &ContextFeatures, memory: &ContextFeatures) -> String {
    for (label, left, right) in [
        ("当前页面", &query.page, &memory.page),
        ("工作区", &query.workspace, &memory.workspace),
        ("文件", &query.file, &memory.file),
        ("网页域名", &query.domain, &memory.domain),
        ("窗口", &query.window, &memory.window),
    ] {
        if exact(left, right) {
            return format!("{label}：{left}");
        }
    }
    "相似的个人修正记录".into()
}

fn assignment_key(record: &MemoryRecord) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        record.category, record.project_id, record.task_id
    )
}

fn exact(left: &str, right: &str) -> bool {
    !left.is_empty() && left == right
}

fn text_similarity(left: &str, right: &str) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    if left == right {
        return 1.0;
    }
    set_similarity(&text_tokens(left), &text_tokens(right))
}

fn set_similarity(left: &[String], right: &[String]) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let left = left.iter().collect::<HashSet<_>>();
    let right = right.iter().collect::<HashSet<_>>();
    let intersection = left.intersection(&right).count() as f32;
    (2.0 * intersection) / (left.len() + right.len()) as f32
}

fn text_tokens(value: &str) -> Vec<String> {
    let mut output = value
        .split(|character: char| !character.is_alphanumeric())
        .map(str::trim)
        .filter(|token| token.chars().count() >= 2)
        .filter(|token| !is_generic(token))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let cjk = value
        .chars()
        .filter(|character| is_cjk(*character))
        .collect::<Vec<_>>();
    for pair in cjk.windows(2) {
        output.push(pair.iter().collect());
    }
    output.sort();
    output.dedup();
    output
}

fn canonical_path(value: &str) -> String {
    let value = value.trim().trim_end_matches(['/', '\\']);
    if value.is_empty() {
        return String::new();
    }
    normalize(
        &value
            .split(['/', '\\'])
            .filter(|part| !part.trim().is_empty())
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("/"),
    )
}

fn domain_from_url(value: &str) -> String {
    let without_scheme = value
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(value);
    let host = without_scheme
        .split('/')
        .next()
        .unwrap_or_default()
        .split('@')
        .next_back()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .trim_start_matches("www.");
    canonical_context(host)
}

fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .replace(['\r', '\n', '\t', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_generic(value: &str) -> bool {
    let value = normalize(value);
    GENERIC_LABELS.contains(&value.as_str())
        || [
            "系统托盘溢出窗口",
            "准备好了，随时开始",
            "正在读取本地时间账本",
        ]
        .iter()
        .any(|needle| value.contains(needle))
}

fn is_cjk(value: char) -> bool {
    matches!(value as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::InputStats;
    use serde_json::json;

    fn event(app: &str, page: &str) -> RawActivityEvent {
        RawActivityEvent {
            id: "event".into(),
            source: "test".into(),
            timestamp: "2026-07-15T10:00:00Z".into(),
            app: Some(app.into()),
            window_title: Some(page.into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({"activePageTitle": page}),
        }
    }

    fn record(id: &str, app: &str, page: &str, task: &str) -> MemoryRecord {
        MemoryRecord {
            session_id: id.into(),
            features: features_from_event(&event(app, page)),
            category: "学习".into(),
            project_id: "research".into(),
            project_name: "科研".into(),
            task_id: task.into(),
            task_title: task.into(),
            confirmed_at: "2026-07-15T10:00:00Z".into(),
        }
    }

    #[test]
    fn exact_personal_page_memory_works_across_apps() {
        let query = features_from_event(&event("wps.exe", "预推免成果填报"));
        let decision = choose_assignment(
            &query,
            &[record("one", "chrome.exe", "预推免成果填报", "成果填报")],
        )
        .expect("exact page should be remembered");
        assert_eq!(decision.task_id, "成果填报");
        assert!(decision.confidence >= 0.88);
    }

    #[test]
    fn generic_application_title_never_becomes_a_task_memory() {
        let query = features_from_event(&event("ChatGPT.exe", "ChatGPT"));
        assert!(!is_discriminative(&query));
        assert!(choose_assignment(
            &query,
            &[record("one", "ChatGPT.exe", "ChatGPT", "ScreenUse")]
        )
        .is_none());
    }

    #[test]
    fn conflicting_exact_memories_abstain() {
        let query = features_from_event(&event("WeMeetApp.exe", "申书豪预定的会议"));
        assert!(choose_assignment(
            &query,
            &[
                record("one", "WeMeetApp.exe", "申书豪预定的会议", "IOT"),
                record("two", "WeMeetApp.exe", "申书豪预定的会议", "校内实习"),
            ],
        )
        .is_none());
    }

    #[test]
    fn canonicalizes_browser_and_explorer_chrome() {
        assert_eq!(canonical_context("ICPC训练 - Google Chrome"), "icpc训练");
        assert_eq!(
            canonical_context("保研 和 3 个其他选项卡 - 文件资源管理器"),
            "保研"
        );
        assert!(canonical_context("ChatGPT - Google Chrome").is_empty());
    }

    #[test]
    fn clears_only_a_process_executable_masquerading_as_the_active_file() {
        let mut polluted = ContextFeatures {
            app: "screenuse".into(),
            page: "时间轴".into(),
            file: "users/me/screenuse.exe".into(),
            tokens: vec!["screenuse".into(), "时间".into()],
            ..Default::default()
        };
        assert!(clear_legacy_process_file(&mut polluted));
        assert!(polluted.file.is_empty());
        assert!(!polluted.tokens.iter().any(|token| token == "screenuse"));

        let mut binary = ContextFeatures {
            app: "ida".into(),
            file: "ctf/challenge.exe".into(),
            ..Default::default()
        };
        assert!(!clear_legacy_process_file(&mut binary));
        assert_eq!(binary.file, "ctf/challenge.exe");
    }
}
