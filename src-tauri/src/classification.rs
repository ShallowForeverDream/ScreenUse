use crate::db::AppDb;
use crate::models::{Project, RawActivityEvent, SessionPatch, Task, WorkSession};
use anyhow::Result;

#[derive(Debug, Clone)]
pub(crate) struct Assignment {
    pub(crate) project_id: String,
    pub(crate) task_id: Option<String>,
    pub(crate) category: String,
    pub(crate) confidence: f32,
    pub(crate) specificity: i32,
}

pub fn ingest_event(db: &AppDb, event: &RawActivityEvent) -> Result<Option<WorkSession>> {
    db.ingest_raw_event(event.clone())?;
    let Some(session) = db.list_sessions(1)?.into_iter().next() else {
        return Ok(None);
    };
    // A generic app rule is not stronger than an exact page/project match. Resolve
    // the current page before accepting a complete automatic attribution.
    if session.user_confirmed
        || session.source == "collector-idle"
        || session.summary.trim() == "离开/空闲"
    {
        return Ok(Some(session));
    }

    let contextual_assignment = resolve_project_task(db, event, &session.category)?;
    let recent_assignment = recent_task_assignment(db, &session)?;
    let assignment = strongest_assignment(contextual_assignment, recent_assignment);
    if !assignment.as_ref().is_some_and(|assignment| {
        assignment_replaces(
            session.project_id.as_deref(),
            session.task_id.as_deref(),
            session.confidence,
            assignment,
        )
    }) {
        return Ok(Some(session));
    }

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

pub(crate) fn recent_task_assignment(
    db: &AppDb,
    session: &WorkSession,
) -> Result<Option<Assignment>> {
    recent_task_assignment_with_policy(db, session, true, 30, 10)
}

pub(crate) fn previous_task_assignment_for_overlay(
    db: &AppDb,
    session: &WorkSession,
) -> Result<Option<Assignment>> {
    // Screenshot overlays are transient tools rather than independent work.
    // Their page text usually says only "截图", so the immediately preceding
    // concrete task is the decisive signal and no lexical task-title match is
    // expected. The caller must first prove that `session` is an overlay.
    recent_task_assignment_with_policy(db, session, false, 5, 120)
}

pub(crate) fn surrounding_task_assignment(
    db: &AppDb,
    session: &WorkSession,
) -> Result<Option<Assignment>> {
    let Some(previous) = db.recent_task_context(&session.id, &session.started_at)? else {
        return Ok(None);
    };
    let Some(next) = db.next_task_context(&session.id, &session.ended_at)? else {
        return Ok(None);
    };
    if !reliable_task_context(&previous) || !reliable_task_context(&next) {
        return Ok(None);
    }
    let (Some(previous_project), Some(previous_task), Some(next_project), Some(next_task)) = (
        previous.project_id.clone(),
        previous.task_id.clone(),
        next.project_id.clone(),
        next.task_id.clone(),
    ) else {
        return Ok(None);
    };
    if previous_project != next_project
        || previous_task != next_task
        || previous.category != next.category
    {
        return Ok(None);
    }
    let target_start = chrono::DateTime::parse_from_rfc3339(&session.started_at)?;
    let target_end = chrono::DateTime::parse_from_rfc3339(&session.ended_at)?;
    let previous_end = chrono::DateTime::parse_from_rfc3339(&previous.boundary_at)?;
    let next_start = chrono::DateTime::parse_from_rfc3339(&next.boundary_at)?;
    if (target_start - previous_end).num_seconds().max(0) > 5
        || (next_start - target_end).num_seconds().max(0) > 5
    {
        return Ok(None);
    }
    Ok(Some(Assignment {
        project_id: previous_project,
        task_id: Some(previous_task),
        category: previous.category,
        confidence: if previous.user_confirmed || next.user_confirmed {
            0.97
        } else {
            0.94
        },
        specificity: 110,
    }))
}

fn recent_task_assignment_with_policy(
    db: &AppDb,
    session: &WorkSession,
    require_task_match: bool,
    max_gap_seconds: i64,
    specificity: i32,
) -> Result<Option<Assignment>> {
    let Some(context) = db.recent_task_context(&session.id, &session.started_at)? else {
        return Ok(None);
    };
    // SQLite stores confidence as REAL while the model uses f32.  A literal
    // 0.84 can round to 0.83999997 after loading; keep the intended boundary
    // inclusive so an immediately following app switch inherits the task.
    if !reliable_task_context(&context) {
        return Ok(None);
    }
    let (Some(project_id), Some(task_id)) =
        (context.project_id.clone(), context.task_id.clone())
    else {
        return Ok(None);
    };
    let features = crate::memory::features_from_session_evidence(session);
    let specific_task_match = !is_generic_task_title(&context.task_title)
        && crate::memory::relates_to_assignment(&features, "", &context.task_title);
    // A project can contain several unrelated tasks.  Matching only the project
    // name (for example `IOT`) must never copy the previous task (`会议`) onto
    // the new page (`CVE 复现`).  Continuity is allowed to fill a concrete task
    // only when the current metadata also identifies that exact task.
    if require_task_match && !specific_task_match {
        return Ok(None);
    }
    let started = chrono::DateTime::parse_from_rfc3339(&session.started_at)?;
    let previous_end = chrono::DateTime::parse_from_rfc3339(&context.boundary_at)?;
    let gap_seconds = (started - previous_end).num_seconds().max(0);
    if gap_seconds > max_gap_seconds {
        return Ok(None);
    }
    Ok(Some(Assignment {
        project_id,
        task_id: Some(task_id),
        category: context.category,
        confidence: if require_task_match {
            if context.user_confirmed {
                0.94
            } else {
                0.91
            }
        } else if context.user_confirmed {
            0.98
        } else {
            0.95
        },
        specificity,
    }))
}

fn reliable_task_context(context: &crate::db::RecentTaskContext) -> bool {
    context.source != "collector-idle"
        && (context.user_confirmed || context.confidence + 0.0001 >= 0.84)
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

    let contextual_assignment = resolve_project_task(db, event, &category)?;
    let recent_assignment = recent_task_assignment(db, &session)?;
    if let Some(assignment) = strongest_assignment(contextual_assignment, recent_assignment) {
        if assignment_replaces(
            project_id.as_deref(),
            task_id.as_deref(),
            confidence,
            &assignment,
        ) {
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

fn strongest_assignment(left: Option<Assignment>, right: Option<Assignment>) -> Option<Assignment> {
    match (left, right) {
        (Some(left), Some(right))
            if (right.specificity, right.confidence) > (left.specificity, left.confidence) =>
        {
            Some(right)
        }
        (Some(left), _) => Some(left),
        (None, right) => right,
    }
}

pub(crate) fn assignment_replaces(
    current_project_id: Option<&str>,
    current_task_id: Option<&str>,
    current_confidence: f32,
    proposed: &Assignment,
) -> bool {
    let current_is_complete =
        current_project_id.is_some() && current_task_id.is_some() && current_confidence >= 0.84;
    let proposed_is_complete = proposed.task_id.is_some();
    (!current_is_complete && (current_task_id.is_none() || proposed_is_complete))
        || (proposed_is_complete
            && (proposed.specificity >= 100 || proposed.confidence > current_confidence + 0.01))
}

pub(crate) fn resolve_project_task(
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
    let page = event_current_page_title(event)
        .or(event.window_title.as_deref())
        .map(normalize);
    let workspace = event.workspace.as_deref().and_then(path_label);

    if let Some((task, project, task_signal, project_signal)) = best_global_task(
        &tasks,
        &projects,
        category,
        &hay,
        page.as_deref(),
        workspace.as_deref(),
    ) {
        let confidence = if task_signal >= 200 {
            0.95
        } else if task_signal >= 140 {
            0.92
        } else if task_signal >= 80 {
            0.89
        } else {
            0.85
        };
        return Ok(Some(Assignment {
            project_id: project.id.clone(),
            task_id: Some(task.id.clone()),
            category: project.category.clone(),
            confidence,
            specificity: 120 + task_signal + project_signal.clamp(0, 100),
        }));
    }

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

    let direct_task_id = (!auto_created)
        .then(|| best_task(&tasks, &project_id, &hay, page.as_deref()))
        .flatten()
        .map(|task| task.id.clone());
    let dominant_task = if auto_created || direct_task_id.is_some() {
        None
    } else {
        db.dominant_confirmed_task_for_project(&project_id)?
    };
    let task_id = if auto_created {
        Some(db.upsert_task_by_title(
            &project_id,
            default_task_title(category, event),
            "workspace-auto",
        )?)
    } else {
        direct_task_id.or_else(|| {
            dominant_task
                .as_ref()
                .map(|(task_id, _, _)| task_id.clone())
        })
    };

    let project_confidence: f32 = if project_score >= 220 {
        0.94
    } else if project_score >= 90 {
        0.86
    } else if project_score >= 60 {
        0.78
    } else {
        0.70
    };
    // A project with one overwhelmingly repeated confirmed task can safely fill
    // the final hierarchy level, but only a strong project signal may skip AI.
    // Weak project matches keep the concrete suggestion below the 0.84 gate.
    let confidence = dominant_task
        .as_ref()
        .map(|(_, memory_confidence, _)| {
            if project_score >= 60 {
                project_confidence.max(*memory_confidence)
            } else {
                project_confidence.max(0.80)
            }
        })
        .unwrap_or(project_confidence);
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
        specificity: project_score + i32::from(dominant_task.is_some()) * 20,
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

fn best_global_task<'a>(
    tasks: &'a [Task],
    projects: &'a [Project],
    category: &str,
    hay: &str,
    page: Option<&str>,
    workspace: Option<&str>,
) -> Option<(&'a Task, &'a Project, i32, i32)> {
    let mut candidates = tasks
        .iter()
        .filter(|task| task.status == "active" && !is_placeholder_task_title(&task.title))
        .filter_map(|task| {
            let project = projects
                .iter()
                .find(|project| project.id == task.project_id)?;
            let project_signal = project_score(project, category, hay, page, workspace);
            if is_generic_task_title(&task.title) && project_signal < 60 {
                return None;
            }
            let task_signal = task_signal(task, hay, page);
            if task_signal < 40 {
                return None;
            }
            let rank = task_signal * 10 + project_signal.clamp(-100, 300);
            Some((task, project, task_signal, project_signal, rank))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.4.cmp(&left.4));
    let best = *candidates.first()?;
    if let Some(second) = candidates.get(1) {
        let same_direct_signal = second.2 >= best.2 - 10;
        let weak_parent_margin = best.3 < second.3 + 30;
        if same_direct_signal && weak_parent_margin {
            return None;
        }
    }
    Some((best.0, best.1, best.2, best.3))
}

fn task_signal(task: &Task, hay: &str, page: Option<&str>) -> i32 {
    let title = normalize(&task.title);
    if title.is_empty() {
        return 0;
    }
    if page == Some(title.as_str()) {
        return 220;
    }
    if page.is_some_and(|page| page.contains(&title) || title.contains(page)) {
        return 150;
    }
    tokens(&task.title)
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
        .sum()
}

fn is_placeholder_task_title(value: &str) -> bool {
    matches!(
        normalize(value).replace([' ', '_', '-'], "").as_str(),
        "" | "other"
            | "others"
            | "none"
            | "nothing"
            | "unknown"
            | "unassigned"
            | "其他"
            | "未指定"
            | "暂不指定"
            | "未归类"
            | "未归类任务"
            | "未归类活动整理"
            | "待分类"
            | "待复核"
    )
}

fn is_generic_task_title(value: &str) -> bool {
    matches!(
        normalize(value).replace([' ', '_', '-'], "").as_str(),
        "会议"
            | "沟通"
            | "聊天"
            | "开发"
            | "测试"
            | "开发与测试"
            | "开发与调试"
            | "学习"
            | "科研"
            | "写作"
            | "阅读"
            | "资料阅读"
            | "日常事务"
    )
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
            "ida",
            "ghidra",
            "sublime_text",
            "zed",
            "rstudio",
            "matlab",
            "unity",
            "unrealeditor",
            "godot",
            "rider",
            "eclipse",
            "netbeans",
            "qtcreator",
            "codeblocks",
            "devcpp",
            "arduino",
            "dbeaver",
            "datagrip",
            "navicat",
            "postman",
            "insomnia",
            "fiddler",
            "wireshark",
            "burpsuite",
            "docker desktop",
            "githubdesktop",
            "gitkraken",
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

pub(crate) fn classify_category(
    event: &RawActivityEvent,
    idle_threshold_seconds: u32,
) -> (&'static str, f32) {
    if event.input_stats.idle_seconds >= idle_threshold_seconds as u64 {
        return ("离开", 0.99);
    }
    let app = event.app.as_deref().unwrap_or_default().to_lowercase();
    let hay = event_hay(event);

    if [
        "code.exe",
        "code - insiders",
        "code-insiders",
        "cursor",
        "windsurf",
        "codium",
        "devenv",
        "idea",
        "pycharm",
        "webstorm",
        "rustrover",
        "clion",
        "ida",
        "ghidra",
        "sublime_text",
        "zed",
        "rstudio",
        "matlab",
        "unity",
        "unrealeditor",
        "godot",
        "rider",
        "eclipse",
        "netbeans",
        "qtcreator",
        "codeblocks",
        "devcpp",
        "arduino",
        "dbeaver",
        "datagrip",
        "navicat",
        "postman",
        "insomnia",
        "fiddler",
        "wireshark",
        "burpsuite",
        "docker desktop",
        "githubdesktop",
        "gitkraken",
    ]
    .iter()
    .any(|needle| app.contains(needle))
    {
        return ("开发", 0.88);
    }
    if [
        "wechat",
        "weixin",
        "qq.exe",
        "teams",
        "slack",
        "discord",
        "telegram",
        "feishu",
        "lark",
        "zoom",
        "dingtalk",
        "wecom",
        "wxwork",
        "wemeet",
        "voovmeeting",
        "webex",
        "outlook",
        "olk.exe",
        "thunderbird",
        "foxmail",
        "tim.exe",
        "messenger",
    ]
    .iter()
    .any(|needle| app.contains(needle))
    {
        return ("沟通", 0.88);
    }
    if [
        "winword", "wps", "et.exe", "wpp", "excel", "obsidian", "typora", "notion", "powerpnt",
        "onenote", "logseq", "joplin", "zettlr", "soffice", "swriter", "scalc", "simpress",
    ]
    .iter()
    .any(|needle| app.contains(needle))
    {
        return ("写作", 0.84);
    }
    if [
        "steam",
        "epicgames",
        "battle.net",
        "spotify",
        "music",
        "bilibili",
        "iqiyi",
        "youku",
        "vlc",
        "potplayer",
        "mpv",
    ]
    .iter()
    .any(|needle| app.contains(needle))
        || app.contains("-win64-shipping")
        || app == "game.exe"
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
            "ghidra",
            "intellij idea",
            "android studio",
            "eclipse ide",
            "apache netbeans",
            "burp suite",
            "dbeaver",
            "datagrip",
            "postman",
            "wireshark",
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
    if let Some(kind) = active_context_type(&event.metadata) {
        if matches!(kind, "document" | "meeting" | "terminal" | "media") {
            if let Some(title) = event_current_page_title(event)
                .map(clean_title)
                .filter(|title| !title.is_empty())
            {
                let workspace = event
                    .workspace
                    .as_deref()
                    .and_then(path_label)
                    .filter(|workspace| !normalize(&title).contains(&normalize(workspace)));
                return cap(
                    &workspace
                        .map(|workspace| format!("{title} · {workspace}"))
                        .unwrap_or(title),
                    96,
                );
            }
        }
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
    let active_page = event_current_page_title(event).unwrap_or_default();
    let conversation = chat_conversation_title(event).unwrap_or_default();
    normalize(&format!(
        "{} {} {} {} {} {} {}",
        event.app.as_deref().unwrap_or_default(),
        event.window_title.as_deref().unwrap_or_default(),
        event.url.as_deref().unwrap_or_default(),
        event.file_path.as_deref().unwrap_or_default(),
        event.workspace.as_deref().unwrap_or_default(),
        active_page,
        conversation,
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

pub(crate) fn active_context_type(metadata: &serde_json::Value) -> Option<&str> {
    metadata
        .get("activeContextType")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            match metadata
                .get("activePageSource")
                .and_then(serde_json::Value::as_str)
            {
                Some(
                    "chatgpt-conversation"
                    | "qq-conversation-header"
                    | "chat-conversation-selection",
                ) => Some("conversation"),
                Some("document-window-title" | "selected-document-tab" | "wps-visible-window") => {
                    Some("document")
                }
                Some(
                    "explorer-address-bar" | "explorer-selected-tab" | "explorer-window-title",
                ) => Some("folder"),
                Some("vscode-extension") => Some("editor"),
                _ => None,
            }
        })
}

pub(crate) fn context_evidence_label(metadata: &serde_json::Value) -> &'static str {
    match active_context_type(metadata) {
        Some("conversation") => "当前会话",
        Some("document") => "当前文档",
        Some("editor") => "当前编辑",
        Some("folder") => "当前目录",
        Some("meeting") => "当前会议",
        Some("terminal") => "当前终端",
        Some("media") => "当前媒体",
        _ => "当前页面",
    }
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
            (active_context_type(&event.metadata) == Some("conversation"))
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
    let chunks = normalize(value)
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() >= 2)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut output = chunks.clone();
    for chunk in chunks {
        output.extend(
            chunk
                .split(['与', '和', '及'])
                .filter(|token| token.chars().count() >= 2)
                .filter(|token| *token != chunk)
                .map(ToOwned::to_owned),
        );
    }
    output.sort();
    output.dedup();
    output
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
    fn classifies_packaged_unreal_games_as_entertainment() {
        assert_eq!(
            classify_category(&event("Atlas-Win64-Shipping.exe", "ATLAS"), 180).0,
            "娱乐"
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
    fn semantic_context_types_use_specific_evidence_labels() {
        for (kind, label) in [
            ("conversation", "当前会话"),
            ("document", "当前文档"),
            ("editor", "当前编辑"),
            ("folder", "当前目录"),
            ("meeting", "当前会议"),
            ("terminal", "当前终端"),
            ("media", "当前媒体"),
            ("browser-page", "当前页面"),
        ] {
            assert_eq!(
                context_evidence_label(&json!({"activeContextType": kind})),
                label,
            );
        }
    }

    #[test]
    fn local_classification_uses_the_semantic_page_not_only_the_app_title() {
        let mut event = event("chrome.exe", "ChatGPT - Google Chrome");
        event.metadata = json!({
            "activePageTitle": "Rust crates.io API 开发",
            "activeContextType": "conversation"
        });
        assert_eq!(classify_category(&event, 180).0, "开发");
    }

    #[test]
    fn document_summary_keeps_the_note_before_its_workspace() {
        let mut event = event("Obsidian.exe", "CVE-2026-44277");
        event.workspace = Some("WorkSpace".into());
        event.metadata = json!({
            "activePageTitle": "CVE-2026-44277",
            "activeContextType": "document"
        });
        assert_eq!(
            summary_for_event(&event, "写作"),
            "CVE-2026-44277 · WorkSpace"
        );
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

    #[test]
    fn an_exact_task_title_can_resolve_its_parent_project() {
        let projects = vec![Project {
            id: "games".into(),
            name: "游戏".into(),
            category: "娱乐".into(),
            source: "manual".into(),
            color: "#000000".into(),
            description: None,
            created_at: String::new(),
            updated_at: String::new(),
        }];
        let tasks = vec![Task {
            id: "steam".into(),
            project_id: "games".into(),
            title: "steam".into(),
            status: "active".into(),
            source: "manual".into(),
            planned_due_at: None,
            created_at: String::new(),
            updated_at: String::new(),
        }];

        let resolved = best_global_task(
            &tasks,
            &projects,
            "娱乐",
            "steamwebhelper steam",
            Some("steam"),
            None,
        )
        .expect("exact task title");
        assert_eq!(resolved.0.id, "steam");
        assert_eq!(resolved.1.id, "games");
        assert!(resolved.2 >= 200);
    }

    #[test]
    fn an_ambiguous_task_title_does_not_guess_a_project() {
        let project = |id: &str, category: &str| Project {
            id: id.into(),
            name: id.into(),
            category: category.into(),
            source: "manual".into(),
            color: "#000000".into(),
            description: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let task = |id: &str, project_id: &str| Task {
            id: id.into(),
            project_id: project_id.into(),
            title: "会议".into(),
            status: "active".into(),
            source: "manual".into(),
            planned_due_at: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let projects = vec![project("IOT", "科研"), project("校内实习", "学习")];
        let tasks = vec![
            task("iot-meeting", "IOT"),
            task("intern-meeting", "校内实习"),
        ];

        assert!(best_global_task(
            &tasks,
            &projects,
            "沟通",
            "腾讯会议",
            Some("腾讯会议"),
            None,
        )
        .is_none());
    }

    #[test]
    fn a_parent_project_signal_disambiguates_same_named_tasks() {
        let project = |id: &str, category: &str| Project {
            id: id.into(),
            name: id.into(),
            category: category.into(),
            source: "manual".into(),
            color: "#000000".into(),
            description: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let task = |id: &str, project_id: &str| Task {
            id: id.into(),
            project_id: project_id.into(),
            title: "会议".into(),
            status: "active".into(),
            source: "manual".into(),
            planned_due_at: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let projects = vec![project("IOT", "科研"), project("校内实习", "学习")];
        let tasks = vec![
            task("iot-meeting", "IOT"),
            task("intern-meeting", "校内实习"),
        ];

        let resolved = best_global_task(
            &tasks,
            &projects,
            "科研",
            &normalize("IOT 项目会议"),
            Some(&normalize("IOT 项目会议")),
            None,
        )
        .expect("project disambiguates task");
        assert_eq!(resolved.0.id, "iot-meeting");
        assert_eq!(resolved.1.id, "IOT");
    }

    #[test]
    fn project_only_context_never_erases_a_concrete_task() {
        let project_only = Assignment {
            project_id: "iot".into(),
            task_id: None,
            category: "科研".into(),
            confidence: 0.96,
            specificity: 220,
        };
        assert!(!assignment_replaces(
            Some("iot"),
            Some("cve-reproduction"),
            0.84,
            &project_only,
        ));

        let concrete_task = Assignment {
            task_id: Some("paper-writing".into()),
            ..project_only
        };
        assert!(assignment_replaces(
            Some("iot"),
            Some("cve-reproduction"),
            0.84,
            &concrete_task,
        ));
    }
}
