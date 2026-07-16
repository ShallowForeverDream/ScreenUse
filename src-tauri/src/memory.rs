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
    pub user_confirmed: bool,
    pub source_confidence: f32,
}

#[derive(Debug, Clone)]
pub struct MemoryDecision {
    pub category: String,
    pub project_id: String,
    pub task_id: String,
    pub confidence: f32,
    pub matched_label: String,
    pub support: usize,
    pub memory_session_id: String,
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
    pub user_confirmed: bool,
    pub source_confidence: f32,
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
    // A compacted work session can cover several raw foreground events.  Do not
    // combine the page from one application with the workspace/file of another:
    // that creates impossible memories such as `screenuse + IOT week1 + HDU`.
    // The session's original app evidence is the anchor; only raw events from
    // that application may contribute additional fields.
    let evidence_app = first_evidence_value(&session.evidence, "app");
    let primary_app = primary_session_app(session, events);
    let coherent_events = events
        .iter()
        .filter(|event| {
            primary_app.is_empty()
                || event
                    .app
                    .as_deref()
                    .is_some_and(|app| canonical_app(app) == primary_app)
        })
        .collect::<Vec<_>>();
    if let Some(event) = coherent_events
        .iter()
        .enumerate()
        .max_by_key(|(index, event)| (assignment_event_score(session, event), *index))
        .map(|(_, event)| *event)
    {
        let features = features_from_event(event);
        if is_discriminative(&features) {
            return features;
        }
    }
    if !events.is_empty() && !primary_app.is_empty() {
        // Compaction or an old boundary repair can leave only a neighboring
        // application's raw event inside the range. In that case the session's
        // page/workspace evidence may also belong to that neighbor; retain only
        // the anchored app/window instead of learning a cross-application pair.
        return build_features(
            evidence_app.unwrap_or_default(),
            "",
            first_evidence_value(&session.evidence, "window").unwrap_or_default(),
            "",
            "",
            "",
        );
    }
    build_features(
        evidence_app.unwrap_or_default(),
        first_evidence_value(&session.evidence, "page").unwrap_or_default(),
        first_evidence_value(&session.evidence, "window").unwrap_or_default(),
        first_evidence_value(&session.evidence, "url").unwrap_or_default(),
        first_evidence_value(&session.evidence, "workspace").unwrap_or_default(),
        first_evidence_value(&session.evidence, "file").unwrap_or_default(),
    )
}

pub fn has_ambiguous_session_context(session: &WorkSession, events: &[RawActivityEvent]) -> bool {
    let primary_app = primary_session_app(session, events);
    let contexts = events
        .iter()
        .filter(|event| {
            primary_app.is_empty()
                || event
                    .app
                    .as_deref()
                    .is_some_and(|app| canonical_app(app) == primary_app)
        })
        .map(features_from_event)
        .filter_map(|features| {
            if !features.page.is_empty() {
                Some(features.page)
            } else if !features.window.is_empty() && !is_generic(&features.window) {
                Some(features.window)
            } else {
                None
            }
        })
        .collect::<HashSet<_>>();
    contexts.len() > 1
}

fn primary_session_app(session: &WorkSession, events: &[RawActivityEvent]) -> String {
    first_evidence_value(&session.evidence, "app")
        .map(canonical_app)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            events.iter().rev().find_map(|event| {
                event
                    .app
                    .as_deref()
                    .map(canonical_app)
                    .filter(|value| !value.is_empty())
            })
        })
        .unwrap_or_default()
}

fn assignment_event_score(session: &WorkSession, event: &RawActivityEvent) -> u8 {
    let features = features_from_event(event);
    let task_match = session
        .task_title
        .as_deref()
        .is_some_and(|task| !task.trim().is_empty() && relates_to_assignment(&features, "", task));
    let project_match = session.project_name.as_deref().is_some_and(|project| {
        !project.trim().is_empty() && relates_to_assignment(&features, project, "")
    });
    u8::from(task_match) * 2 + u8::from(project_match)
}

fn canonical_app(value: &str) -> String {
    let normalized = normalize(value.trim());
    normalized
        .strip_suffix(".exe")
        .unwrap_or(&normalized)
        .to_string()
}

fn first_evidence_value<'a>(evidence: &'a [EvidenceItem], kind: &str) -> Option<&'a str> {
    evidence
        .iter()
        .find(|item| item.kind == kind && !item.value.trim().is_empty())
        .map(|item| item.value.as_str())
}

pub fn features_from_session_evidence(session: &WorkSession) -> ContextFeatures {
    features_from_session(session, &[])
}

pub fn features_from_primary_session_evidence(session: &WorkSession) -> ContextFeatures {
    let page = first_evidence_value(&session.evidence, "page").unwrap_or_default();
    let window = if page.is_empty() {
        first_evidence_value(&session.evidence, "window").unwrap_or_default()
    } else {
        ""
    };
    // Evidence is stored as a flat list, so workspace/file entries cannot be
    // proven to belong to the first app once a compacted session contains
    // several contexts.  Keep only the visible primary page identity instead
    // of manufacturing combinations such as `ScreenUse + ICPC workspace`.
    build_features(
        first_evidence_value(&session.evidence, "app").unwrap_or_default(),
        page,
        window,
        "",
        "",
        "",
    )
}

pub fn is_discriminative(features: &ContextFeatures) -> bool {
    (!features.page.is_empty() && !is_generic(&features.page))
        || !features.domain.is_empty()
        || !features.workspace.is_empty()
        || !features.file.is_empty()
        || (!features.window.is_empty()
            && features.window != features.app
            && !is_generic(&features.window))
}

pub(crate) fn exact_context_identity(features: &ContextFeatures) -> Option<String> {
    if features.app.is_empty() {
        return None;
    }
    let (kind, value) = if !features.page.is_empty() {
        ("page", features.page.as_str())
    } else if !features.window.is_empty() {
        ("window", features.window.as_str())
    } else if !features.file.is_empty() {
        ("file", features.file.as_str())
    } else if !features.workspace.is_empty() {
        ("workspace", features.workspace.as_str())
    } else if !features.domain.is_empty() {
        ("domain", features.domain.as_str())
    } else {
        return None;
    };
    Some(format!("{}\u{1f}{kind}\u{1f}{value}", features.app))
}

pub fn relates_to_assignment(
    features: &ContextFeatures,
    project_name: &str,
    task_title: &str,
) -> bool {
    let hay = [
        features.page.as_str(),
        features.window.as_str(),
        features.workspace.as_str(),
        features.file.as_str(),
        features.domain.as_str(),
    ]
    .join(" ");
    [project_name, task_title]
        .into_iter()
        .map(canonical_context)
        .filter(|label| !label.is_empty())
        .any(|label| {
            hay.contains(&label) || set_similarity(&text_tokens(&label), &features.tokens) >= 0.72
        })
}

pub fn choose_assignment(
    query: &ContextFeatures,
    records: &[MemoryRecord],
) -> Option<MemoryDecision> {
    if !is_discriminative(query) {
        return None;
    }
    choose_similarity_assignment(query, records)
        .or_else(|| choose_manual_keyword_signature(query, records))
        .or_else(|| choose_manual_token_pair_signature(query, records))
        .or_else(|| choose_stable_app_assignment(query, records))
}

fn choose_stable_app_assignment(
    query: &ContextFeatures,
    records: &[MemoryRecord],
) -> Option<MemoryDecision> {
    if query.app.is_empty() || is_contextual_application(&query.app) {
        return None;
    }
    let matching = records
        .iter()
        .filter(|record| record.user_confirmed && record.features.app == query.app)
        .collect::<Vec<_>>();
    if matching.len() < 2 {
        return None;
    }
    let assignments = matching
        .iter()
        .map(|record| assignment_key(record))
        .collect::<HashSet<_>>();
    if assignments.len() != 1 {
        return None;
    }
    let record = matching
        .iter()
        .copied()
        .max_by(|left, right| left.confirmed_at.cmp(&right.confirmed_at))?;
    Some(MemoryDecision {
        category: record.category.clone(),
        project_id: record.project_id.clone(),
        task_id: record.task_id.clone(),
        confidence: if matching.len() >= 4 { 0.94 } else { 0.90 },
        matched_label: format!("专用应用：{}", query.app),
        support: matching.len(),
        memory_session_id: record.session_id.clone(),
    })
}

fn is_contextual_application(app: &str) -> bool {
    matches!(
        app,
        "chatgpt"
            | "codex"
            | "chrome"
            | "msedge"
            | "firefox"
            | "brave"
            | "tabbit browser"
            | "qq"
            | "weixin"
            | "wechat"
            | "wps"
            | "wpsoffice"
            | "winword"
            | "excel"
            | "powerpnt"
            | "explorer"
            | "windowsterminal"
            | "pwsh"
            | "powershell"
            | "cmd"
            | "conhost"
            | "code"
            | "cursor"
            | "windsurf"
            | "typora"
            | "obsidian"
            | "notepad"
            | "wemeetapp"
            | "zoom"
            | "teams"
            | "msteams"
            | "dingtalk"
            | "feishu"
            | "snipaste"
    )
}

pub(crate) fn supports_surrounding_continuity(features: &ContextFeatures) -> bool {
    if matches!(
        features.app.as_str(),
        "wps"
            | "wpsoffice"
            | "winword"
            | "excel"
            | "powerpnt"
            | "typora"
            | "obsidian"
            | "notepad"
    ) {
        return true;
    }
    if !matches!(
        features.app.as_str(),
        "chrome" | "msedge" | "firefox" | "brave" | "tabbit browser"
    ) {
        return false;
    }
    let label = format!("{} {}", features.page, features.window);
    [
        "认证",
        "登录",
        "验证你的身份",
        "verify your identity",
        "oauth",
        "sso",
        "教务系统",
    ]
    .iter()
    .any(|marker| label.contains(marker))
}

fn choose_similarity_assignment(
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
    candidates.truncate(64);

    // An explicit correction is also how the user changes the meaning of a
    // context over time. For equally exact manual examples, the latest unique
    // correction wins instead of forcing the user to outvote all old history.
    let best_manual_score = candidates
        .iter()
        .filter(|(record, similarity)| {
            record.user_confirmed && similarity.strong && similarity.score >= 0.86
        })
        .map(|(_, similarity)| similarity.score)
        .max_by(f32::total_cmp);
    if let Some(best_manual_score) = best_manual_score {
        let exact_band = candidates
            .iter()
            .filter(|(record, similarity)| {
                record.user_confirmed
                    && similarity.strong
                    && similarity.score >= best_manual_score - 0.02
            })
            .collect::<Vec<_>>();
        let assignments = exact_band
            .iter()
            .map(|(record, _)| assignment_key(record))
            .collect::<HashSet<_>>();
        if assignments.len() == 1 && best_manual_score >= 0.90 {
            let record = exact_band[0].0;
            return Some(MemoryDecision {
                category: record.category.clone(),
                project_id: record.project_id.clone(),
                task_id: record.task_id.clone(),
                confidence: 0.96,
                matched_label: strongest_label(query, &record.features),
                support: exact_band.len(),
                memory_session_id: record.session_id.clone(),
            });
        } else if assignments.len() > 1 {
            let newest_at = exact_band
                .iter()
                .map(|(record, _)| record.confirmed_at.as_str())
                .max()
                .unwrap_or_default();
            let newest = exact_band
                .iter()
                .filter(|(record, _)| record.confirmed_at == newest_at)
                .collect::<Vec<_>>();
            let newest_assignments = newest
                .iter()
                .map(|(record, _)| assignment_key(record))
                .collect::<HashSet<_>>();
            if newest_assignments.len() == 1 {
                let record = newest[0].0;
                let winner_key = assignment_key(record);
                return Some(MemoryDecision {
                    category: record.category.clone(),
                    project_id: record.project_id.clone(),
                    task_id: record.task_id.clone(),
                    confidence: 0.95,
                    matched_label: strongest_label(query, &record.features),
                    support: exact_band
                        .iter()
                        .filter(|(candidate, _)| assignment_key(candidate) == winner_key)
                        .count(),
                    memory_session_id: record.session_id.clone(),
                });
            }
        }
    }

    // A single exceptionally high-confidence AI result may teach only an exact,
    // highly specific repeat. Broader or lower-confidence AI observations still
    // require the existing three-sample consensus below.
    let exact_ai = candidates
        .iter()
        .filter(|(record, similarity)| {
            !record.user_confirmed
                && record.source_confidence >= 0.96
                && stable_for_single_ai_memory(query)
                && similarity.strong
                && similarity.score >= 0.90
        })
        .collect::<Vec<_>>();
    let exact_ai_assignments = exact_ai
        .iter()
        .map(|(record, _)| assignment_key(record))
        .collect::<HashSet<_>>();
    let exact_ai_is_anchored = exact_ai.iter().any(|(record, _)| {
        is_specific_task_label(&record.task_title)
            && relates_to_assignment(query, "", &record.task_title)
    });
    if exact_ai_assignments.len() == 1 && (exact_ai.len() >= 3 || exact_ai_is_anchored) {
        let (record, _) = exact_ai
            .iter()
            .max_by(|left, right| left.0.confirmed_at.cmp(&right.0.confirmed_at))?;
        return Some(MemoryDecision {
            category: record.category.clone(),
            project_id: record.project_id.clone(),
            task_id: record.task_id.clone(),
            confidence: 0.90,
            matched_label: strongest_label(query, &record.features),
            support: exact_ai.len(),
            memory_session_id: record.session_id.clone(),
        });
    }

    // A manual correction is authoritative for the same strong context. AI
    // results may reinforce that assignment, but cannot outvote it by volume.
    let manual_assignments = candidates
        .iter()
        .filter(|(record, similarity)| {
            record.user_confirmed && similarity.strong && similarity.score >= 0.60
        })
        .map(|(record, _)| assignment_key(record))
        .collect::<HashSet<_>>();
    if !manual_assignments.is_empty() {
        candidates.retain(|(record, _)| {
            record.user_confirmed || manual_assignments.contains(&assignment_key(record))
        });
    }

    #[derive(Default)]
    struct Vote<'a> {
        total: f32,
        best: f32,
        strong: bool,
        support: usize,
        manual_support: usize,
        record: Option<&'a MemoryRecord>,
    }
    let mut votes: HashMap<String, Vote<'_>> = HashMap::new();
    for (rank, (record, similarity)) in candidates.iter().enumerate() {
        let key = assignment_key(record);
        let vote = votes.entry(key).or_default();
        let freshness = 0.985_f32.powi(rank as i32);
        let trust = if record.user_confirmed { 1.0 } else { 0.45 };
        vote.total += similarity.score.powi(2) * freshness * trust;
        vote.best = vote.best.max(similarity.score);
        vote.strong |= similarity.strong;
        vote.support += 1;
        vote.manual_support += usize::from(record.user_confirmed);
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
    let vote_total = ranked.iter().map(|vote| vote.total).sum::<f32>();
    let consensus = if vote_total > 0.0 {
        winner.total / vote_total
    } else {
        0.0
    };
    let margin = runner_up
        .map(|runner| winner.total - runner.total)
        .unwrap_or(winner.total);
    if winner.best < 0.60 {
        return None;
    }
    let record = winner.record?;
    let winner_key = assignment_key(record);
    let recent_streak = candidates
        .iter()
        .take_while(|(candidate, similarity)| {
            similarity.score >= winner.best - 0.04 && assignment_key(candidate) == winner_key
        })
        .count();
    let mut confidence: f32 = if consensus >= 0.92
        && winner.support >= 2
        && winner.best >= 0.72
        && (runner_up.is_none() || recent_streak >= 2)
    {
        0.97
    } else if consensus >= 0.80 && winner.support >= 3 && winner.best >= 0.72 && recent_streak >= 2
    {
        0.94
    } else if runner_up.is_none() && winner.best >= 0.68 {
        0.93
    } else if runner_up.is_none() && winner.strong && winner.best >= 0.62 {
        0.90
    } else if winner.manual_support >= 2 && recent_streak >= 2 && consensus >= 0.68 && winner.strong
    {
        0.91
    } else if consensus >= 0.80 && winner.support >= 3 && winner.best >= 0.72 {
        0.89
    } else if winner.best >= 0.86 && consensus >= 0.66 && margin >= 0.12 {
        0.88
    } else {
        return None;
    };
    if winner.manual_support == 0 {
        if winner.support < 3 {
            return None;
        }
        confidence = confidence.min(0.90);
    }
    Some(MemoryDecision {
        category: record.category.clone(),
        project_id: record.project_id.clone(),
        task_id: record.task_id.clone(),
        confidence,
        matched_label: strongest_label(query, &record.features),
        support: winner.support,
        memory_session_id: record.session_id.clone(),
    })
}

pub(crate) fn stable_for_single_ai_memory(features: &ContextFeatures) -> bool {
    if matches!(
        features.app.as_str(),
        "wemeetapp" | "zoom" | "teams" | "msteams" | "dingtalk" | "feishu"
    ) {
        return false;
    }
    let label = format!("{} {}", features.page, features.window);
    !["会议室", "会议纪要", "元宝纪要", "meeting room"]
        .iter()
        .any(|marker| label.contains(marker))
}

pub(crate) fn is_specific_task_label(value: &str) -> bool {
    let normalized = normalize(value).replace([' ', '_', '-'], "");
    !matches!(
        normalized.as_str(),
        ""
            | "微信"
            | "qq"
            | "会议"
            | "沟通"
            | "聊天"
            | "开发"
            | "测试"
            | "开发与测试"
            | "开发与调试"
            | "使用测试"
            | "学习"
            | "科研"
            | "写作"
            | "阅读"
            | "资料阅读"
            | "日常事务"
            | "浪费"
    ) && !crate::ai::is_placeholder_task_title(value)
}

fn choose_manual_keyword_signature(
    query: &ContextFeatures,
    records: &[MemoryRecord],
) -> Option<MemoryDecision> {
    const MIN_REPEATED_TOKEN_SUPPORT: usize = 2;
    let query_tokens = query
        .tokens
        .iter()
        .filter(|token| is_signature_token(token))
        .filter(|token| {
            let token = canonical_context(token);
            token != query.app && !query.app.ends_with(&token)
        })
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if query_tokens.is_empty() {
        return None;
    }

    #[derive(Default)]
    struct TokenAssignment<'a> {
        count: usize,
        latest: Option<&'a MemoryRecord>,
    }

    let mut token_assignments = HashMap::<String, HashMap<String, TokenAssignment<'_>>>::new();
    for record in records.iter().filter(|record| record.user_confirmed) {
        let assignment = assignment_key(record);
        for token in record
            .features
            .tokens
            .iter()
            .filter(|token| query_tokens.contains(token.as_str()))
        {
            let stats = token_assignments
                .entry(token.clone())
                .or_default()
                .entry(assignment.clone())
                .or_default();
            stats.count += 1;
            if stats
                .latest
                .map_or(true, |latest| record.confirmed_at > latest.confirmed_at)
            {
                stats.latest = Some(record);
            }
        }
    }

    #[derive(Default)]
    struct SignatureVote<'a> {
        tokens: Vec<String>,
        max_token_support: usize,
        record: Option<&'a MemoryRecord>,
    }

    let mut votes = HashMap::<String, SignatureVote<'_>>::new();
    for (token, assignments) in token_assignments {
        if assignments.len() != 1 {
            continue;
        }
        let (assignment, stats) = assignments.into_iter().next()?;
        let vote = votes.entry(assignment).or_default();
        vote.tokens.push(token);
        vote.max_token_support = vote.max_token_support.max(stats.count);
        if let Some(record) = stats.latest {
            if vote
                .record
                .map_or(true, |latest| record.confirmed_at > latest.confirmed_at)
            {
                vote.record = Some(record);
            }
        }
    }

    // Conflicting task signatures abstain even when one side has more matching
    // words. This fallback exists to reduce AI calls, never to guess through a
    // genuine personal-history conflict.
    if votes.len() != 1 {
        return None;
    }
    let (_, mut vote) = votes.into_iter().next()?;
    let record = vote.record?;
    let winner = assignment_key(record);
    let supporting_memories = records
        .iter()
        .filter(|candidate| candidate.user_confirmed && assignment_key(candidate) == winner)
        .filter(|candidate| {
            candidate
                .features
                .tokens
                .iter()
                .any(|token| vote.tokens.contains(token))
        })
        .count();
    if supporting_memories == 0 {
        return None;
    }

    let longest_token = vote
        .tokens
        .iter()
        .map(|token| token.chars().count())
        .max()
        .unwrap_or_default();
    let has_semantic_page = !query.page.is_empty() || !query.domain.is_empty();
    let exact_page_anchor = canonical_context(&query.page);
    let one_shot_specific_anchor = has_semantic_page
        && supporting_memories >= 1
        && exact_page_anchor.chars().count() >= 4
        && exact_page_anchor != query.app
        && !is_context_container_anchor(&exact_page_anchor)
        && vote.tokens.contains(&exact_page_anchor);
    let enough_evidence = one_shot_specific_anchor
        || (vote.max_token_support >= MIN_REPEATED_TOKEN_SUPPORT && vote.tokens.len() >= 4)
        || (has_semantic_page
            && ((vote.tokens.len() >= 3
                && vote.max_token_support >= 3
                && supporting_memories >= 3)
                || (vote.tokens.len() >= 2
                    && vote.max_token_support >= 4
                    && supporting_memories >= 4)
                || (longest_token >= 4
                    && vote.max_token_support >= 6
                    && supporting_memories >= 6)));
    if !enough_evidence {
        return None;
    }

    vote.tokens.sort_by(|left, right| {
        right
            .chars()
            .count()
            .cmp(&left.chars().count())
            .then_with(|| left.cmp(right))
    });
    let matched = vote.tokens.iter().take(3).cloned().collect::<Vec<_>>();
    let confidence = if vote.tokens.len() >= 8 && supporting_memories >= 3 {
        0.95
    } else if vote.tokens.len() >= 6 {
        0.93
    } else {
        0.90
    };
    Some(MemoryDecision {
        category: record.category.clone(),
        project_id: record.project_id.clone(),
        task_id: record.task_id.clone(),
        confidence,
        matched_label: format!("个人关键词：{}", matched.join("、")),
        support: supporting_memories,
        memory_session_id: record.session_id.clone(),
    })
}

fn choose_manual_token_pair_signature(
    query: &ContextFeatures,
    records: &[MemoryRecord],
) -> Option<MemoryDecision> {
    #[derive(Default)]
    struct PairAssignment<'a> {
        count: usize,
        latest: Option<&'a MemoryRecord>,
    }

    #[derive(Default)]
    struct PairVote<'a> {
        pairs: Vec<(String, String)>,
        max_support: usize,
        record: Option<&'a MemoryRecord>,
    }

    let query_tokens = signature_tokens(query);
    if query_tokens.len() < 2 {
        return None;
    }

    let query_pairs = token_pairs(&query_tokens)
        .into_iter()
        .filter(is_specific_token_pair)
        .collect::<HashSet<_>>();
    if query_pairs.is_empty() {
        return None;
    }

    let mut assignments = HashMap::<(String, String), HashMap<String, PairAssignment<'_>>>::new();
    for record in records.iter().filter(|record| record.user_confirmed) {
        let assignment = assignment_key(record);
        let matching_tokens = signature_tokens(&record.features)
            .into_iter()
            .filter(|token| query_tokens.binary_search(token).is_ok())
            .collect::<Vec<_>>();
        for pair in token_pairs(&matching_tokens) {
            if !query_pairs.contains(&pair) {
                continue;
            }
            let stats = assignments
                .entry(pair)
                .or_default()
                .entry(assignment.clone())
                .or_default();
            stats.count += 1;
            if stats
                .latest
                .map_or(true, |latest| record.confirmed_at > latest.confirmed_at)
            {
                stats.latest = Some(record);
            }
        }
    }

    let mut votes = HashMap::<String, PairVote<'_>>::new();
    for (pair, pair_assignments) in assignments {
        if pair_assignments.len() != 1 {
            return None;
        }
        let (assignment, stats) = pair_assignments.into_iter().next()?;
        if stats.count < 2 {
            continue;
        }
        let vote = votes.entry(assignment).or_default();
        vote.pairs.push(pair);
        vote.max_support = vote.max_support.max(stats.count);
        if let Some(record) = stats.latest {
            if vote
                .record
                .map_or(true, |latest| record.confirmed_at > latest.confirmed_at)
            {
                vote.record = Some(record);
            }
        }
    }

    // A pair is useful only when every learned pair visible in the current
    // context agrees on the complete category/project/task path. This keeps a
    // broad workspace word from overruling a more specific conversation or
    // document signal.
    if votes.len() != 1 {
        return None;
    }
    let (_, mut vote) = votes.into_iter().next()?;
    let record = vote.record?;
    vote.pairs.sort_by(|left, right| {
        pair_specificity(right)
            .cmp(&pair_specificity(left))
            .then_with(|| left.cmp(right))
    });
    let matched = vote
        .pairs
        .first()
        .map(|(left, right)| format!("{left} + {right}"))?;
    Some(MemoryDecision {
        category: record.category.clone(),
        project_id: record.project_id.clone(),
        task_id: record.task_id.clone(),
        confidence: if vote.max_support >= 4 { 0.93 } else { 0.90 },
        matched_label: format!("个人组合词：{matched}"),
        support: vote.max_support,
        memory_session_id: record.session_id.clone(),
    })
}

fn signature_tokens(features: &ContextFeatures) -> Vec<String> {
    let mut tokens = features
        .tokens
        .iter()
        .filter(|token| is_signature_token(token))
        .filter(|token| {
            let token = canonical_context(token);
            token != features.app && !features.app.ends_with(&token)
        })
        .cloned()
        .collect::<Vec<_>>();
    tokens.sort();
    tokens.dedup();
    tokens
}

fn token_pairs(tokens: &[String]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for (index, left) in tokens.iter().enumerate() {
        for right in &tokens[index + 1..] {
            pairs.push((left.clone(), right.clone()));
        }
    }
    pairs
}

fn is_specific_token_pair((left, right): &(String, String)) -> bool {
    let longest = left.chars().count().max(right.chars().count());
    !(longest < 4
        || is_context_container_anchor(left)
        || is_context_container_anchor(right)
        || is_weak_pair_token(left) && is_weak_pair_token(right))
}

fn pair_specificity((left, right): &(String, String)) -> usize {
    left.chars().count() + right.chars().count()
}

fn is_weak_pair_token(token: &str) -> bool {
    matches!(
        token,
        "hdu"
            | "college"
            | "runtime"
            | "root"
            | "pdf"
            | "docx"
            | "txt"
            | "exe"
            | "大学"
            | "学校"
            | "校园"
            | "学院"
            | "湖北"
            | "北大"
            | "中心"
            | "平台"
            | "统一"
            | "身份"
            | "认证"
            | "管理员"
            | "administrator"
            | "powershell"
            | "pwsh"
            | "shell"
            | "installer"
            | "isolated"
            | "onekey"
            | "加入"
            | "个人"
            | "老师"
            | "博士"
            | "师兄"
    )
}

fn is_signature_token(token: &str) -> bool {
    !token.chars().all(|character| character.is_ascii_digit())
        && !matches!(
            token,
            "会议"
                | "文档"
                | "开发"
                | "学习"
                | "任务"
                | "项目"
                | "工作"
                | "使用"
                | "查看"
                | "处理"
                | "聊天"
                | "页面"
                | "当前"
                | "其他"
                | "系统"
                | "文件"
                | "资料"
                | "研究"
                | "论文"
                | "复核"
                | "测试"
                | "修复"
                | "浏览"
                | "分析"
                | "时间"
                | "活动"
                | "微信"
                | "qq"
                | "chatgpt"
                | "chrome"
                | "wps"
        )
}

fn is_context_container_anchor(value: &str) -> bool {
    ["小黑屋", "工作区", "workspace", "项目空间", "project space"]
        .iter()
        .any(|marker| value.contains(marker))
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
                user_confirmed: record.user_confirmed,
                source_confidence: record.source_confidence,
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
    let file_name = features.file.rsplit(['/', '\\']).next().unwrap_or_default();
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
    let app = canonical_app(app);
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
            user_confirmed: true,
            source_confidence: 1.0,
        }
    }

    fn observed_session(app: &str, page: &str) -> WorkSession {
        WorkSession {
            id: "session".into(),
            started_at: "2026-07-15T10:00:00Z".into(),
            ended_at: "2026-07-15T10:01:00Z".into(),
            project_id: Some("project".into()),
            project_name: Some("项目".into()),
            task_id: Some("task".into()),
            task_title: Some("任务".into()),
            category: "开发".into(),
            summary: page.into(),
            confidence: 1.0,
            evidence: vec![
                EvidenceItem {
                    kind: "page".into(),
                    label: "当前页面".into(),
                    value: page.into(),
                    weight: 0.82,
                },
                EvidenceItem {
                    kind: "app".into(),
                    label: "应用".into(),
                    value: app.into(),
                    weight: 0.5,
                },
            ],
            user_confirmed: true,
            source: "manual-correction".into(),
        }
    }

    #[test]
    fn session_memory_never_mixes_context_from_different_apps() {
        let session = observed_session("ScreenUse.exe", "ScreenUse");
        let mut chatgpt = event("ChatGPT.exe", "IOT week1");
        chatgpt.workspace = Some("D:/MY_PROJECT/HDU".into());
        let screenuse = event("ScreenUse.exe", "ScreenUse");

        let features = features_from_session(&session, &[chatgpt, screenuse]);

        assert_eq!(features.app, "screenuse");
        assert_ne!(features.page, "iot week1");
        assert!(features.workspace.is_empty());
    }

    #[test]
    fn stale_neighbor_event_cannot_supply_the_session_page() {
        let mut session = observed_session("WeMeetApp.exe", "h1ck0r的个人会议室");
        session.evidence.insert(
            0,
            EvidenceItem {
                kind: "page".into(),
                label: "当前页面".into(),
                value: "分析数据结构平均复杂度".into(),
                weight: 0.82,
            },
        );
        session.evidence.push(EvidenceItem {
            kind: "window".into(),
            label: "窗口".into(),
            value: "h1ck0r的个人会议室".into(),
            weight: 0.7,
        });
        let chatgpt = event("ChatGPT.exe", "分析数据结构平均复杂度");

        let features = features_from_session(&session, &[chatgpt]);

        assert_eq!(features.app, "wemeetapp");
        assert_eq!(features.window, "h1ck0r的个人会议室");
        assert!(features.page.is_empty());
    }

    #[test]
    fn a_generic_raw_event_does_not_erase_specific_session_evidence() {
        let mut session = observed_session("explorer.exe", "大作业 和 5 个其他选项卡 - 文件资源管理器");
        session.evidence[0].kind = "window".into();
        session.evidence[0].label = "窗口".into();
        let generic = event("explorer.exe", "Task Switching");

        let features = features_from_session(&session, &[generic]);

        assert_eq!(features.app, "explorer");
        assert_eq!(features.window, "大作业");
        assert!(is_discriminative(&features));
    }

    #[test]
    fn exact_page_identity_is_not_changed_by_ai_inferred_workspace_noise() {
        let mut first = features_from_event(&event("ChatGPT.exe", "一等奖学金人数统计"));
        let mut second = first.clone();
        first.workspace.clear();
        second.workspace = "unrelated-neighbor".into();

        assert_eq!(exact_context_identity(&first), exact_context_identity(&second));
    }

    #[test]
    fn qq_memory_keeps_qq_page_without_leaking_chatgpt_workspace() {
        let session = observed_session("QQ.exe", "保研成果填报群");
        let mut chatgpt = event("ChatGPT.exe", "ScreenUse开发");
        chatgpt.workspace = Some("D:/MY_PROJECT/ScreenUse".into());
        let qq = event("QQ.exe", "保研成果填报群");

        let features = features_from_session(&session, &[chatgpt, qq]);

        assert_eq!(features.app, "qq");
        assert_eq!(features.page, "保研成果填报群");
        assert!(features.workspace.is_empty());
    }

    #[test]
    fn coherent_chatgpt_event_preserves_conversation_and_workspace() {
        let mut session = observed_session("ChatGPT.exe", "");
        session.evidence.retain(|item| item.kind == "app");
        let mut chatgpt = event("ChatGPT.exe", "codex_work_bridge");
        chatgpt.workspace = Some("D:/MY_PROJECT/HDU".into());

        let features = features_from_session(&session, &[chatgpt]);

        assert_eq!(features.app, "chatgpt");
        assert_eq!(features.page, "codex work bridge");
        assert!(features.workspace.ends_with("my project/hdu"));
    }

    #[test]
    fn multi_page_session_is_not_safe_as_one_permanent_memory() {
        let session = observed_session("ChatGPT.exe", "连接微信QQ聊天记录");
        let first = event("ChatGPT.exe", "连接微信QQ聊天记录");
        let second = event("ChatGPT.exe", "IOT week1");
        assert!(has_ambiguous_session_context(&session, &[first, second]));
    }

    #[test]
    fn assignment_related_event_wins_inside_a_compacted_session() {
        let mut session = observed_session("ChatGPT.exe", "IOT week1");
        session.project_name = Some("IOT".into());
        session.task_title = Some("CVE-2026-44277 复现".into());
        let first = event("ChatGPT.exe", "IOT week1");
        let second = event("ChatGPT.exe", "CVE-2026-44277 复现记录");

        let features = features_from_session(&session, &[first, second]);

        assert_eq!(features.page, "cve-2026-44277 复现记录");
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
    fn repeated_unique_manual_keywords_generalize_to_a_new_page() {
        let records = vec![
            record(
                "one",
                "wps.exe",
                "湖北大学 推免 成果填报 证明材料 第一版",
                "成果填报",
            ),
            record(
                "two",
                "chrome.exe",
                "湖北大学 推免 成果填报 证明材料 第二版",
                "成果填报",
            ),
        ];
        let query = features_from_event(&event("QQ.exe", "湖北大学 推免 成果填报 证明材料 沟通群"));

        let decision = choose_manual_keyword_signature(&query, &records)
            .expect("consistent repeated keywords should resolve the task");
        assert_eq!(decision.task_id, "成果填报");
        assert!(decision.confidence >= 0.90);
        assert!(decision.matched_label.starts_with("个人关键词："));
    }

    #[test]
    fn two_high_support_personal_keywords_can_replace_four_weak_words() {
        let records = (1..=4)
            .map(|index| {
                record(
                    &format!("network-{index}"),
                    "Typora.exe",
                    &format!("计网 刷题 T{index}"),
                    "计算机网络刷题",
                )
            })
            .collect::<Vec<_>>();
        let query = features_from_event(&event("ChatGPT.exe", "计网 刷题 新题"));

        let decision = choose_manual_keyword_signature(&query, &records)
            .expect("two repeatedly confirmed personal terms should be enough");
        assert_eq!(decision.task_id, "计算机网络刷题");
        assert!(decision.support >= 4);
    }

    #[test]
    fn repeated_pure_token_pair_generalizes_to_a_new_page() {
        let records = vec![
            record(
                "aliyun-one",
                "Chrome.exe",
                "root@server-one - Aliyun Workbench",
                "云端开发",
            ),
            record(
                "aliyun-two",
                "Chrome.exe",
                "root@server-two - Aliyun Workbench",
                "云端开发",
            ),
        ];
        let query = features_from_event(&event("Chrome.exe", "root@new-server - Aliyun Workbench"));

        let decision = choose_manual_token_pair_signature(&query, &records)
            .expect("a repeated platform pair should resolve the concrete task");
        assert_eq!(decision.task_id, "云端开发");
        assert_eq!(decision.support, 2);
        assert!(decision.matched_label.contains("aliyun"));
        assert!(decision.matched_label.contains("workbench"));
    }

    #[test]
    fn broad_path_words_cannot_form_a_task_pair() {
        let records = vec![
            record(
                "iot-one",
                "WindowsTerminal.exe",
                "root@host: C:/college/HDU/runtime",
                "IOT复现",
            ),
            record(
                "iot-two",
                "WindowsTerminal.exe",
                "root@other: C:/college/HDU/runtime",
                "IOT复现",
            ),
        ];
        let query = features_from_event(&event(
            "WindowsTerminal.exe",
            "C:/college/HDU/runtime/NapCat/WeChat bridge",
        ));

        assert!(choose_manual_token_pair_signature(&query, &records).is_none());
    }

    #[test]
    fn conflicting_token_pair_history_abstains() {
        let mut records = vec![
            record(
                "aliyun-one",
                "Chrome.exe",
                "Aliyun Workbench server one",
                "任务一",
            ),
            record(
                "aliyun-two",
                "Chrome.exe",
                "Aliyun Workbench server two",
                "任务一",
            ),
        ];
        records.push(record(
            "aliyun-other",
            "Chrome.exe",
            "Aliyun Workbench billing",
            "任务二",
        ));
        let query = features_from_event(&event("Chrome.exe", "Aliyun Workbench new server"));

        assert!(choose_manual_token_pair_signature(&query, &records).is_none());
    }

    #[test]
    fn a_dedicated_application_learns_only_after_consistent_manual_use() {
        let records = vec![
            record("one", "Atlas-Win64-Shipping.exe", "关卡一", "小小梦魇"),
            record("two", "Atlas-Win64-Shipping.exe", "关卡二", "小小梦魇"),
        ];
        let query = features_from_event(&event("Atlas-Win64-Shipping.exe", "Enhanced Edition"));

        let decision = choose_stable_app_assignment(&query, &records)
            .expect("a dedicated executable should learn its stable task");
        assert_eq!(decision.task_id, "小小梦魇");
        assert_eq!(decision.support, 2);
    }

    #[test]
    fn contextual_apps_never_become_permanent_task_anchors() {
        let records = vec![
            record("one", "ChatGPT.exe", "ScreenUse开发一", "ScreenUse"),
            record("two", "ChatGPT.exe", "ScreenUse开发二", "ScreenUse"),
        ];
        let query = features_from_event(&event("ChatGPT.exe", "完全不同的新会话"));

        assert!(choose_stable_app_assignment(&query, &records).is_none());
    }

    #[test]
    fn conflicting_manual_keyword_signatures_abstain() {
        let records = vec![
            record(
                "form-one",
                "wps.exe",
                "湖北大学 推免 成果填报 证明材料 第一版",
                "成果填报",
            ),
            record(
                "form-two",
                "chrome.exe",
                "湖北大学 推免 成果填报 证明材料 第二版",
                "成果填报",
            ),
            record(
                "iot-one",
                "wps.exe",
                "IOT 漏洞复现 测试报告 第一版",
                "漏洞复现",
            ),
            record(
                "iot-two",
                "chrome.exe",
                "IOT 漏洞复现 测试报告 第二版",
                "漏洞复现",
            ),
        ];
        let query = features_from_event(&event(
            "ChatGPT.exe",
            "湖北大学推免成果填报证明材料与IOT漏洞复现测试报告",
        ));

        assert!(choose_manual_keyword_signature(&query, &records).is_none());
    }

    #[test]
    fn exact_manual_context_beats_many_merely_similar_memories() {
        let query = features_from_event(&event("ChatGPT.exe", "IOT week1"));
        let mut records = vec![record("exact", "QQ.exe", "IOT week1", "IOT")];
        for index in 0..20 {
            records.push(record(
                &format!("similar-{index}"),
                "ChatGPT.exe",
                &format!("IOT week{}", index + 2),
                "其他任务",
            ));
        }
        let decision = choose_assignment(&query, &records).expect("exact manual context wins");
        assert_eq!(decision.task_id, "IOT");
        assert_eq!(decision.confidence, 0.96);
    }

    #[test]
    fn one_manual_exact_page_anchor_can_cover_a_small_title_variant() {
        let query = features_from_event(&event("WPS.exe", "修改论文"));
        let decision = choose_assignment(
            &query,
            &[record(
                "manual-paper",
                "WPS.exe",
                "修改论文 - 去 AI 味",
                "论文修改",
            )],
        )
        .expect("specific page anchor should generalize once");
        assert_eq!(decision.task_id, "论文修改");
        assert!(decision.confidence >= 0.86);
    }

    #[test]
    fn one_manual_app_or_container_token_cannot_generalize_a_task() {
        let app_query = features_from_event(&event("Tabbit Browser.exe", "Fortinet SSO"));
        assert!(choose_assignment(
            &app_query,
            &[record(
                "manual-screenuse",
                "Tabbit Browser.exe",
                "ScreenUse 开发",
                "ScreenUse",
            )],
        )
        .is_none());

        let container_query = features_from_event(&event("ChatGPT.exe", "zjh的小黑屋"));
        assert!(choose_assignment(
            &container_query,
            &[record(
                "manual-paper-container",
                "ChatGPT.exe",
                "zjh的小黑屋 · 修改论文",
                "论文修改",
            )],
        )
        .is_none());
    }

    #[test]
    fn surrounding_continuity_is_limited_to_documents_and_auth_helpers() {
        assert!(supports_surrounding_continuity(&features_from_event(&event(
            "WPS.exe",
            "成果填报材料.docx",
        ))));
        assert!(supports_surrounding_continuity(&features_from_event(&event(
            "Tabbit Browser.exe",
            "Fortinet SSO - Tabbit",
        ))));
        assert!(!supports_surrounding_continuity(&features_from_event(&event(
            "ChatGPT.exe",
            "IOT week1",
        ))));
        assert!(!supports_surrounding_continuity(&features_from_event(&event(
            "QQ.exe",
            "校园频道文章",
        ))));
        assert!(!supports_surrounding_continuity(&features_from_event(&event(
            "WeMeetApp.exe",
            "加入会议",
        ))));
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
    fn an_unknown_application_self_title_is_not_discriminative() {
        let mut self_titled = event("AutoProxyWeb.exe", "AutoProxyWeb");
        self_titled.metadata = json!({});
        let query = features_from_event(&self_titled);
        assert_eq!(query.app, query.window);
        assert!(!is_discriminative(&query));
    }

    #[test]
    fn generic_shell_pages_are_not_discriminative_ai_evidence() {
        for page in ["ChatGPT", "系统托盘溢出窗口。", "正在读取本地时间账本…"] {
            let features = build_features("explorer.exe", page, page, "", "", "");
            assert!(!is_discriminative(&features), "generic page: {page}");
        }

        let semantic = build_features(
            "ChatGPT.exe",
            "codex_work_bridge",
            "ChatGPT",
            "",
            "HDU",
            "",
        );
        assert!(is_discriminative(&semantic));
    }

    #[test]
    fn batch_memory_requires_the_observed_context_to_describe_the_assignment() {
        let related = features_from_event(&event("chrome.exe", "IOT week1"));
        let incidental = features_from_event(&event("screenuse.exe", "ScreenUse开发"));
        assert!(relates_to_assignment(&related, "IOT", "会议"));
        assert!(!relates_to_assignment(&incidental, "IOT", "会议"));
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
    fn repeated_corrections_outvote_an_old_exact_outlier() {
        let query = features_from_event(&event("WeMeetApp.exe", "申书豪预定的会议"));
        let mut records = vec![record(
            "old-outlier",
            "WeMeetApp.exe",
            "申书豪预定的会议",
            "校内实习",
        )];
        for index in 0..6 {
            let mut value = record(
                &format!("iot-{index}"),
                "WeMeetApp.exe",
                "申书豪预定的会议",
                "IOT",
            );
            value.confirmed_at = format!("2026-07-15T10:00:{:02}Z", index + 1);
            records.push(value);
        }
        let decision = choose_assignment(&query, &records).expect("consensus should win");
        assert_eq!(decision.task_id, "IOT");
        assert!(decision.confidence >= 0.94);
        assert_eq!(decision.support, 6);
    }

    #[test]
    fn latest_exact_manual_correction_changes_future_classification() {
        let query = features_from_event(&event("ChatGPT.exe", "ScreenUse开发"));
        let mut old = record("old", "ChatGPT.exe", "ScreenUse开发", "旧任务");
        old.confirmed_at = "2026-07-15T10:00:00Z".into();
        let mut newest = record("new", "ChatGPT.exe", "ScreenUse开发", "新任务");
        newest.confirmed_at = "2026-07-16T10:00:00Z".into();
        let decision = choose_assignment(&query, &[old, newest]).expect("latest correction wins");
        assert_eq!(decision.task_id, "新任务");
        assert_eq!(decision.confidence, 0.95);
    }

    #[test]
    fn ai_examples_cannot_outvote_a_manual_exact_context() {
        let query = features_from_event(&event("QQ.exe", "IOT week1"));
        let manual = record("manual", "QQ.exe", "IOT week1", "讨论");
        let mut records = vec![manual];
        for index in 0..12 {
            let mut ai = record(&format!("ai-{index}"), "QQ.exe", "IOT week1", "日常杂务");
            ai.user_confirmed = false;
            records.push(ai);
        }
        let decision = choose_assignment(&query, &records).expect("manual memory should win");
        assert_eq!(decision.task_id, "讨论");
        assert_eq!(decision.support, 1);
    }

    #[test]
    fn one_ai_result_is_prompt_context_not_a_permanent_local_rule() {
        let mut ai = record("ai", "QQ.exe", "在线状态 小组群", "浪费");
        ai.user_confirmed = false;
        ai.source_confidence = 0.93;
        assert!(choose_assignment(
            &features_from_event(&event("QQ.exe", "在线状态 小组群")),
            &[ai]
        )
        .is_none());
    }

    #[test]
    fn one_high_confidence_ai_result_teaches_only_an_exact_repeat() {
        let mut ai = record("ai", "ChatGPT.exe", "漏洞复现 week1", "漏洞复现");
        ai.user_confirmed = false;
        ai.source_confidence = 0.98;

        let exact = choose_assignment(
            &features_from_event(&event("ChatGPT.exe", "漏洞复现 week1")),
            &[ai.clone()],
        )
        .expect("exact high-confidence AI repeat");
        assert_eq!(exact.task_id, "漏洞复现");
        assert_eq!(exact.confidence, 0.90);
        assert!(choose_assignment(
            &features_from_event(&event("ChatGPT.exe", "漏洞复现 week1 周报")),
            &[ai]
        )
        .is_none());
    }

    #[test]
    fn one_ai_meeting_room_result_does_not_become_a_permanent_topic() {
        let mut ai = record(
            "ai-meeting",
            "WeMeetApp.exe",
            "h1ck0r的个人会议室",
            "数据结构讨论",
        );
        ai.user_confirmed = false;
        ai.source_confidence = 0.98;

        assert!(choose_assignment(
            &features_from_event(&event("WeMeetApp.exe", "h1ck0r的个人会议室")),
            &[ai]
        )
        .is_none());
    }

    #[test]
    fn three_consistent_ai_observations_can_resolve_an_exact_repeat() {
        let query = features_from_event(&event("QQ.exe", "成果填报群"));
        let observations = (0..3)
            .map(|index| {
                let mut ai = record(&format!("ai-{index}"), "QQ.exe", "成果填报群", "成果填报");
                ai.user_confirmed = false;
                ai.source_confidence = 0.93;
                ai
            })
            .collect::<Vec<_>>();

        assert!(choose_assignment(&query, &observations[..2]).is_none());
        let decision = choose_assignment(&query, &observations).expect("three AI observations");
        assert_eq!(decision.task_id, "成果填报");
        assert_eq!(decision.support, 3);
        assert!(decision.confidence <= 0.90);
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
