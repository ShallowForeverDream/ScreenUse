use crate::db::AppDb;
use crate::models::{Project, RawActivityEvent, SessionPatch, Task, WorkSession};
use anyhow::Result;

#[derive(Debug, Clone)]
struct Assignment {
    project_id: String,
    task_id: Option<String>,
    category: String,
    confidence: f32,
}

pub fn ingest_event(db: &AppDb, event: &RawActivityEvent) -> Result<Option<WorkSession>> {
    db.ingest_raw_event(event.clone())?;
    let Some(session) = db.list_sessions(1)?.into_iter().next() else {
        return Ok(None);
    };
    // A generic app rule is not stronger than an exact page/project match. Resolve
    // the current page before accepting a complete automatic attribution.
    if session.user_confirmed {
        return Ok(Some(session));
    }

    let contextual_assignment = resolve_project_task(db, event, &session.category)?;
    let is_complete =
        session.confidence >= 0.84 && session.project_id.is_some() && session.task_id.is_some();
    let page_context_is_stronger = contextual_assignment.as_ref().is_some_and(|assignment| {
        assignment.confidence >= 0.92 && assignment.confidence > session.confidence + 0.01
    });
    if is_complete && !page_context_is_stronger {
        return Ok(Some(session));
    }

    let assignment = match contextual_assignment {
        Some(assignment) => Some(assignment),
        None => recent_task_assignment(db, &session)?,
    };
    let Some(assignment) = assignment else {
        return Ok(Some(session));
    };
    if session.project_id.as_deref() == Some(assignment.project_id.as_str())
        && session.task_id == assignment.task_id
        && session.category == assignment.category
        && session.confidence >= assignment.confidence
    {
        return Ok(Some(session));
    }

    let updated = db.update_session(
        &session.id,
        SessionPatch {
            summary: None,
            project_id: Some(assignment.project_id),
            task_id: assignment.task_id,
            clear_project: Some(false),
            clear_task: Some(false),
            category: Some(assignment.category),
            confidence: Some(session.confidence.max(assignment.confidence)),
            user_confirmed: Some(false),
        },
    )?;
    db.mark_session_awaiting_confirmation(&updated.id)?;
    db.get_session(&updated.id)
}

fn recent_task_assignment(db: &AppDb, session: &WorkSession) -> Result<Option<Assignment>> {
    let Some(context) = db.recent_task_context(&session.id, &session.started_at)? else {
        return Ok(None);
    };
    let (Some(project_id), Some(task_id)) = (context.project_id, context.task_id) else {
        return Ok(None);
    };
    if context.source == "collector-idle" || (!context.user_confirmed && context.confidence < 0.84)
    {
        return Ok(None);
    }
    let started = chrono::DateTime::parse_from_rfc3339(&session.started_at)?;
    let previous_end = chrono::DateTime::parse_from_rfc3339(&context.ended_at)?;
    let gap_seconds = (started - previous_end).num_seconds().max(0);
    if gap_seconds > 30 {
        return Ok(None);
    }
    Ok(Some(Assignment {
        project_id,
        task_id: Some(task_id),
        category: context.category,
        confidence: if context.user_confirmed { 0.92 } else { 0.90 },
    }))
}

pub fn finalize_context(
    db: &AppDb,
    event: &RawActivityEvent,
    session_id: &str,
) -> Result<Option<WorkSession>> {
    let Some(session) = db.get_session(session_id)? else {
        return Ok(None);
    };
    if session.user_confirmed {
        return Ok(Some(session));
    }

    let settings = db.get_settings()?.normalized();
    if event.input_stats.idle_seconds >= settings.idle_threshold_seconds as u64 {
        let project_id = db.configure_idle_target(&settings)?;
        let updated = db.update_session(
            &session.id,
            SessionPatch {
                summary: Some("离开/空闲".into()),
                project_id: Some(project_id),
                task_id: None,
                clear_project: Some(false),
                clear_task: Some(true),
                category: Some(settings.idle_category),
                confidence: Some(0.99),
                user_confirmed: Some(false),
            },
        )?;
        db.mark_session_awaiting_confirmation(&updated.id)?;
        return db.get_session(&updated.id);
    }
    let (local_category, local_confidence) =
        classify_category(event, settings.idle_threshold_seconds);
    let mut category = if session.confidence >= 0.84 {
        session.category.clone()
    } else {
        local_category.to_string()
    };
    let mut project_id = session.project_id.clone();
    let mut task_id = session.task_id.clone();
    let mut confidence = session.confidence.max(local_confidence);

    if let Some(assignment) = resolve_project_task(db, event, &category)? {
        let current_is_complete = project_id.is_some() && task_id.is_some() && confidence >= 0.84;
        if !current_is_complete || assignment.confidence > confidence + 0.01 {
            project_id = Some(assignment.project_id);
            task_id = assignment.task_id;
            category = assignment.category;
            confidence = confidence.max(assignment.confidence);
        }
    } else if category != session.category && session.project_id.is_some() {
        category = session.category.clone();
    }

    let updated = db.update_session(
        &session.id,
        SessionPatch {
            summary: Some(summary_for_event(event, &category)),
            project_id,
            task_id,
            clear_project: Some(false),
            clear_task: Some(false),
            category: Some(category),
            confidence: Some(confidence.clamp(0.0, 0.99)),
            user_confirmed: Some(false),
        },
    )?;
    db.mark_session_awaiting_confirmation(&updated.id)?;
    db.coalesce_session_neighbors(&updated.id).map(Some)
}

fn resolve_project_task(
    db: &AppDb,
    event: &RawActivityEvent,
    category: &str,
) -> Result<Option<Assignment>> {
    if category == "离开" {
        return Ok(None);
    }
    let projects = db.list_projects()?;
    let tasks = db.list_tasks()?;
    let hay = event_hay(event);
    let page = event_current_page_title(event).map(normalize);
    let workspace = event.workspace.as_deref().and_then(path_label);

    let mut best: Option<(&Project, i32)> = None;
    for project in &projects {
        let score = project_score(
            project,
            category,
            &hay,
            page.as_deref(),
            workspace.as_deref(),
        );
        if score > best.map(|(_, current)| current).unwrap_or(i32::MIN) {
            best = Some((project, score));
        }
    }

    let (project_id, project_score, auto_created) = match best.filter(|(_, score)| *score >= 30) {
        Some((project, score)) => (project.id.clone(), score, false),
        None if should_create_workspace_project(event, workspace.as_deref()) => {
            let name = workspace.unwrap_or_else(|| "开发工作区".into());
            (
                db.upsert_project_by_name(&name, category, "workspace-auto")?,
                70,
                true,
            )
        }
        None => return Ok(None),
    };

    let task_id = if auto_created {
        Some(db.upsert_task_by_title(
            &project_id,
            default_task_title(category, event),
            "workspace-auto",
        )?)
    } else {
        best_task(&tasks, &project_id, &hay, page.as_deref()).map(|task| task.id.clone())
    };

    let confidence = if project_score >= 220 {
        0.94
    } else if project_score >= 90 {
        0.86
    } else if project_score >= 60 {
        0.78
    } else {
        0.70
    };
    let assigned_category = projects
        .iter()
        .find(|project| project.id == project_id)
        .map(|project| project.category.clone())
        .unwrap_or_else(|| category.to_string());
    Ok(Some(Assignment {
        project_id,
        task_id,
        category: assigned_category,
        confidence,
    }))
}

fn project_score(
    project: &Project,
    category: &str,
    hay: &str,
    page: Option<&str>,
    workspace: Option<&str>,
) -> i32 {
    let name = normalize(&project.name);
    let mut score = if !name.is_empty() && page == Some(name.as_str()) {
        240
    } else if !name.is_empty()
        && page.is_some_and(|page| page.contains(&name) || name.contains(page))
    {
        180
    } else if !name.is_empty() && hay.contains(&name) {
        100
    } else {
        0
    };
    if project.category == category {
        score += 8;
    }
    if let Some(workspace) = workspace {
        let workspace = normalize(workspace);
        if !workspace.is_empty() && (name.contains(&workspace) || workspace.contains(&name)) {
            score += 72;
        }
    }
    for token in tokens(&project.name) {
        if is_generic_token(&token) {
            continue;
        }
        if page.is_some_and(|page| page.contains(&token)) {
            score += if token.chars().count() >= 6 { 42 } else { 28 };
        } else if hay.contains(&token) {
            score += if token.chars().count() >= 6 { 28 } else { 16 };
        }
    }
    score
}

fn best_task<'a>(
    tasks: &'a [Task],
    project_id: &str,
    hay: &str,
    page: Option<&str>,
) -> Option<&'a Task> {
    let project_tasks: Vec<_> = tasks
        .iter()
        .filter(|task| task.project_id == project_id)
        .collect();
    let scored = project_tasks
        .iter()
        .map(|task| {
            let title = normalize(&task.title);
            let exact_page_score = if !title.is_empty() && page == Some(title.as_str()) {
                180
            } else if !title.is_empty()
                && page.is_some_and(|page| page.contains(&title) || title.contains(page))
            {
                120
            } else {
                0
            };
            let token_score = tokens(&task.title)
                .into_iter()
                .filter(|token| !is_generic_token(token))
                .map(|token| {
                    if page.is_some_and(|page| page.contains(&token)) {
                        if token.chars().count() >= 6 {
                            32
                        } else {
                            20
                        }
                    } else if hay.contains(&token) {
                        if token.chars().count() >= 6 {
                            20
                        } else {
                            10
                        }
                    } else {
                        0
                    }
                })
                .sum::<i32>();
            (*task, exact_page_score + token_score)
        })
        .max_by_key(|(_, score)| *score);
    match scored {
        Some((task, score)) if score > 0 => Some(task),
        _ => {
            let active = project_tasks
                .into_iter()
                .filter(|task| task.status == "active")
                .collect::<Vec<_>>();
            (active.len() == 1).then(|| active[0])
        }
    }
}

fn should_create_workspace_project(event: &RawActivityEvent, workspace: Option<&str>) -> bool {
    let Some(workspace) = workspace else {
        return false;
    };
    if workspace.chars().count() < 2 || is_generic_workspace(workspace) {
        return false;
    }
    let app = event.app.as_deref().unwrap_or_default().to_lowercase();
    event.source.contains("vscode")
        || [
            "code",
            "cursor",
            "windsurf",
            "codium",
            "devenv",
            "idea",
            "pycharm",
            "webstorm",
            "rustrover",
            "clion",
        ]
        .iter()
        .any(|needle| app.contains(needle))
}

fn default_task_title<'a>(category: &str, event: &'a RawActivityEvent) -> &'a str {
    if event.workspace.is_some() {
        return "日常开发";
    }
    match category {
        "开发" => "开发与调试",
        "学习" => "资料阅读",
        "写作" => "文档写作",
        "沟通" => "消息与会议",
        "娱乐" => "休闲娱乐",
        _ => "日常事务",
    }
}

fn classify_category(event: &RawActivityEvent, idle_threshold_seconds: u32) -> (&'static str, f32) {
    if event.input_stats.idle_seconds >= idle_threshold_seconds as u64 {
        return ("离开", 0.99);
    }
    let app = event.app.as_deref().unwrap_or_default().to_lowercase();
    let hay = event_hay(event);

    if [
        "code.exe",
        "cursor",
        "windsurf",
        "codium",
        "devenv",
        "idea",
        "pycharm",
        "webstorm",
        "rustrover",
        "clion",
    ]
    .iter()
    .any(|needle| app.contains(needle))
    {
        return ("开发", 0.88);
    }
    if [
        "wechat", "weixin", "qq.exe", "teams", "slack", "discord", "telegram", "feishu", "lark",
        "zoom",
    ]
    .iter()
    .any(|needle| app.contains(needle))
    {
        return ("沟通", 0.88);
    }
    if ["winword", "wps", "obsidian", "typora", "notion", "powerpnt"]
        .iter()
        .any(|needle| app.contains(needle))
    {
        return ("写作", 0.84);
    }
    if ["steam", "epicgames", "battle.net", "spotify", "music"]
        .iter()
        .any(|needle| app.contains(needle))
    {
        return ("娱乐", 0.90);
    }

    if contains_any(
        &hay,
        &[
            "github.com",
            "gitlab",
            "stackoverflow",
            "docs.rs",
            "localhost",
            "127.0.0.1",
            "developer.",
            "devtools",
            "npmjs",
            "crates.io",
        ],
    ) {
        return ("开发", 0.80);
    }
    if contains_any(
        &hay,
        &[
            "gmail",
            "outlook",
            "mail.",
            "meeting",
            "会议",
            "飞书",
            "腾讯会议",
            "企业微信",
        ],
    ) {
        return ("沟通", 0.80);
    }
    if contains_any(
        &hay,
        &[
            "markdown",
            ".md",
            "document",
            "文档",
            "写作",
            "稿件",
            "论文写作",
        ],
    ) {
        return ("写作", 0.76);
    }
    if contains_any(
        &hay,
        &[
            "course", "lecture", "tutorial", "arxiv", "知网", "课程", "学习", "教材", ".pdf", "pdf",
        ],
    ) {
        return ("学习", 0.76);
    }
    if contains_any(
        &hay,
        &[
            "youtube", "bilibili", "netflix", "douyin", "抖音", "游戏", "game", "video",
        ],
    ) {
        return ("娱乐", 0.72);
    }
    ("杂务", 0.56)
}

pub(crate) fn summary_for_event(event: &RawActivityEvent, category: &str) -> String {
    if category == "离开" {
        return "离开/空闲".into();
    }
    if let Some(title) = chat_conversation_title(event)
        .map(clean_title)
        .filter(|title| !title.is_empty())
    {
        let project = event
            .metadata
            .get("chatgptProject")
            .and_then(serde_json::Value::as_str)
            .or(event.workspace.as_deref())
            .map(str::trim)
            .filter(|project| !project.is_empty());
        return cap(
            &project
                .filter(|project| !normalize(&title).contains(&normalize(project)))
                .map(|project| format!("{title} · {project}"))
                .unwrap_or(title),
            96,
        );
    }
    let workspace = event.workspace.as_deref().and_then(path_label);
    let file = event.file_path.as_deref().and_then(path_label);
    if let Some(workspace) = workspace {
        return cap(
            &match file {
                Some(file) if file != workspace => format!("{workspace} · {file}"),
                _ => workspace,
            },
            96,
        );
    }

    let title = clean_title(event.window_title.as_deref().unwrap_or_default());
    if let Some(host) = event.url.as_deref().and_then(host_label) {
        return cap(
            &if title.is_empty() || normalize(&title) == normalize(&host) {
                host
            } else {
                format!("{host} · {title}")
            },
            96,
        );
    }

    let app = event
        .app
        .as_deref()
        .unwrap_or("电脑活动")
        .trim_end_matches(".exe")
        .trim();
    if app.eq_ignore_ascii_case("qq") && title == "图片查看器" {
        return "QQ".into();
    }
    cap(
        &if title.is_empty() || normalize(&title) == normalize(app) {
            app.to_string()
        } else {
            format!("{app} · {title}")
        },
        96,
    )
}

fn event_hay(event: &RawActivityEvent) -> String {
    normalize(&format!(
        "{} {} {} {} {}",
        event.app.as_deref().unwrap_or_default(),
        event.window_title.as_deref().unwrap_or_default(),
        event.url.as_deref().unwrap_or_default(),
        event.file_path.as_deref().unwrap_or_default(),
        event.workspace.as_deref().unwrap_or_default(),
    ))
}

fn event_current_page_title(event: &RawActivityEvent) -> Option<&str> {
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
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn chat_conversation_title(event: &RawActivityEvent) -> Option<&str> {
    event
        .metadata
        .get("conversationTitle")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            event
                .metadata
                .get("chatgptConversationTitle")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            (matches!(
                event
                    .metadata
                    .get("activePageSource")
                    .and_then(serde_json::Value::as_str),
                Some("chatgpt-conversation" | "qq-conversation-header")
            ))
            .then(|| event_current_page_title(event))
            .flatten()
        })
}

fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .replace(['\r', '\n', '\t', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn tokens(value: &str) -> Vec<String> {
    normalize(value)
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() >= 2)
        .map(ToOwned::to_owned)
        .collect()
}

fn contains_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| hay.contains(needle))
}

fn path_label(value: &str) -> Option<String> {
    value
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .find(|part| !part.trim().is_empty())
        .map(|part| part.trim().to_string())
}

fn host_label(url: &str) -> Option<String> {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = without_scheme
        .split('/')
        .next()?
        .split('@')
        .next_back()?
        .split(':')
        .next()?;
    let host = host.trim_start_matches("www.").trim();
    (!host.is_empty()).then(|| host.to_string())
}

fn clean_title(value: &str) -> String {
    let mut title = value.replace(['\r', '\n', '\t'], " ").trim().to_string();
    for suffix in [
        " - Google Chrome",
        " — Mozilla Firefox",
        " - Microsoft Edge",
        " - Brave",
        " - Visual Studio Code",
        " — Visual Studio Code",
    ] {
        if let Some(stripped) = title.strip_suffix(suffix) {
            title = stripped.trim().to_string();
        }
    }
    title
}

fn cap(value: &str, max_chars: usize) -> String {
    let cleaned = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= max_chars {
        cleaned
    } else {
        let mut output = cleaned
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>();
        output.push('…');
        output
    }
}

fn is_generic_token(token: &str) -> bool {
    matches!(
        token,
        "开发"
            | "学习"
            | "写作"
            | "沟通"
            | "娱乐"
            | "杂务"
            | "项目"
            | "工作"
            | "任务"
            | "daily"
            | "project"
    )
}

fn is_generic_workspace(value: &str) -> bool {
    matches!(
        normalize(value).as_str(),
        "desktop" | "documents" | "downloads" | "home" | "用户" | "桌面" | "文档" | "下载"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::InputStats;
    use serde_json::json;

    fn event(app: &str, title: &str) -> RawActivityEvent {
        RawActivityEvent {
            id: String::new(),
            source: "test".into(),
            timestamp: String::new(),
            app: Some(app.into()),
            window_title: Some(title.into()),
            url: None,
            file_path: None,
            workspace: None,
            input_stats: InputStats::default(),
            metadata: json!({}),
        }
    }

    #[test]
    fn classifies_ide_as_development() {
        assert_eq!(
            classify_category(&event("Code.exe", "ScreenUse"), 180).0,
            "开发"
        );
    }

    #[test]
    fn extracts_windows_workspace_name() {
        assert_eq!(
            path_label(r"C:\\Code\\ScreenUse").as_deref(),
            Some("ScreenUse")
        );
    }

    #[test]
    fn strips_browser_suffix_from_title() {
        assert_eq!(
            clean_title("ScreenUse - GitHub - Google Chrome"),
            "ScreenUse - GitHub"
        );
    }

    #[test]
    fn qq_image_viewer_keeps_the_main_qq_summary() {
        assert_eq!(
            summary_for_event(&event("QQ.exe", "图片查看器"), "杂务"),
            "QQ"
        );
    }

    #[test]
    fn chatgpt_summary_starts_with_the_current_conversation() {
        let mut event = event("ChatGPT.exe", "codex_work_bridge");
        event.workspace = Some("HDU".into());
        event.file_path = Some(r"C:\Program Files\OpenAI\ChatGPT.exe".into());
        event.metadata = json!({
            "activePageTitle": "codex_work_bridge",
            "activePageSource": "chatgpt-conversation",
            "chatgptConversationTitle": "codex_work_bridge",
            "chatgptProject": "HDU"
        });

        assert_eq!(summary_for_event(&event, "开发"), "codex_work_bridge · HDU");
    }

    #[test]
    fn qq_summary_uses_the_current_person_or_group_as_the_primary_label() {
        let mut event = event("QQ.exe", "科研讨论群");
        event.metadata = json!({
            "activePageTitle": "科研讨论群",
            "activePageSource": "qq-conversation-header",
            "conversationTitle": "科研讨论群",
            "qqConversationTitle": "科研讨论群"
        });

        assert_eq!(summary_for_event(&event, "学习"), "科研讨论群");
    }

    #[test]
    fn exact_current_page_project_outranks_workspace_project() {
        let project = |name: &str| Project {
            id: name.into(),
            name: name.into(),
            category: "开发".into(),
            source: "manual".into(),
            color: "#000000".into(),
            description: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let hay = normalize("ChatGPT.exe codex_work_bridge HDU");
        let page = normalize("codex_work_bridge");

        let page_score = project_score(
            &project("codex_work_bridge"),
            "开发",
            &hay,
            Some(&page),
            Some("HDU"),
        );
        let workspace_score =
            project_score(&project("HDU"), "开发", &hay, Some(&page), Some("HDU"));
        assert!(page_score > workspace_score);
        assert!(page_score >= 220);
    }
}
