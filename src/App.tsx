import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
} from 'react';
import {
  Activity,
  BarChart3,
  CalendarDays,
  Check,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  Clock3,
  Database,
  Download,
  FolderKanban,
  HardDrive,
  LayoutDashboard,
  Merge,
  Pause,
  Pencil,
  Play,
  RefreshCw,
  Search,
  Settings,
  Sparkles,
  SplitSquareHorizontal,
  Tags,
  TimerReset,
  WandSparkles,
  X,
} from 'lucide-react';
import { api } from './api';
import type {
  AppSettings,
  DashboardData,
  Project,
  Task,
  WorkSession,
} from './types';

const tabs = [
  { id: 'today', label: '今日', icon: LayoutDashboard },
  { id: 'timeline', label: '时间轴', icon: Activity },
  { id: 'projects', label: '项目', icon: FolderKanban },
  { id: 'settings', label: '设置', icon: Settings },
] as const;

type TabId = (typeof tabs)[number]['id'];

const categories = ['开发', '学习', '写作', '沟通', '娱乐', '杂务', '离开'];
const categoryColors: Record<string, string> = {
  开发: '#60a5fa',
  学习: '#a78bfa',
  写作: '#f472b6',
  沟通: '#34d399',
  娱乐: '#fb7185',
  杂务: '#fbbf24',
  离开: '#94a3b8',
};

export default function App() {
  const [activeTab, setActiveTab] = useState<TabId>('today');
  const [data, setData] = useState<DashboardData | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState('');
  const [toast, setToast] = useState('');
  const [selectedDate, setSelectedDate] = useState(localDateKey(new Date()));
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [editing, setEditing] = useState<WorkSession | null>(null);

  const load = useCallback(async () => {
    try {
      const dashboard = await api.dashboard();
      setData(dashboard);
      setLoadError('');
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : String(error));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') void load();
    }, 30_000);
    const onVisibility = () => {
      if (document.visibilityState === 'visible') void load();
    };
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [load]);

  const showToast = useCallback((message: string) => {
    setToast(message);
    window.setTimeout(() => setToast(''), 3200);
  }, []);

  const runAction = useCallback(
    async (fn: () => Promise<unknown>, successMessage: string) => {
      try {
        const result = await fn();
        showToast(
          typeof result === 'string' && result.length > 0
            ? `${successMessage}：${result}`
            : successMessage,
        );
        await load();
        return result;
      } catch (error) {
        showToast(error instanceof Error ? error.message : String(error));
        throw error;
      }
    },
    [load, showToast],
  );

  const daySessions = useMemo(
    () =>
      (data?.sessions || [])
        .filter((session) => sessionMinutesOnDate(session, selectedDate) > 0)
        .sort(
          (left, right) =>
            new Date(right.startedAt).getTime() - new Date(left.startedAt).getTime(),
        ),
    [data, selectedDate],
  );
  const stats = useMemo(
    () => summarizeDay(daySessions, selectedDate),
    [daySessions, selectedDate],
  );

  if (loading) {
    return (
      <div className="boot">
        <div className="boot-mark">SU</div>
        <div>
          <strong>ScreenUse</strong>
          <span>正在读取本地时间账本…</span>
        </div>
      </div>
    );
  }

  if (!data) {
    return (
      <div className="boot boot-error">
        <div className="boot-mark">SU</div>
        <div>
          <strong>本地时间账本读取失败</strong>
          <span>{loadError || '未收到后端数据，请重试。'}</span>
          <button className="btn ghost small" onClick={() => void load()}>重新读取</button>
        </div>
      </div>
    );
  }

  const currentTab = tabs.find((tab) => tab.id === activeTab) || tabs[0];
  const goDate = (offset: number) => {
    const date = dateFromKey(selectedDate);
    date.setDate(date.getDate() + offset);
    setSelectedDate(localDateKey(date));
    setSelected(new Set());
  };

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">SU</div>
          <div>
            <strong>ScreenUse</strong>
            <span>个人时间账本</span>
          </div>
        </div>

        <nav className="primary-nav" aria-label="主导航">
          {tabs.map((tab) => {
            const Icon = tab.icon;
            return (
              <button
                key={tab.id}
                className={activeTab === tab.id ? 'active' : ''}
                onClick={() => setActiveTab(tab.id)}
                type="button"
              >
                <Icon size={18} />
                <span>{tab.label}</span>
                {tab.id === 'timeline' && stats.reviewCount > 0 && (
                  <b className="nav-badge">{stats.reviewCount}</b>
                )}
              </button>
            );
          })}
        </nav>

        <div className="collector-card">
          <div className="collector-head">
            <span className={`status-dot ${data.collectorRunning ? 'ok' : ''}`} />
            <div>
              <strong>{data.collectorRunning ? '正在自动记录' : '自动记录已暂停'}</strong>
              <span>窗口元数据 · 无截图</span>
            </div>
          </div>
          <div className="collector-meta">
            <span>{data.settings.pollIntervalSeconds} 秒检测</span>
            <span>{data.settings.aiMode === 'off' ? '本地分类' : '本地优先'}</span>
          </div>
          <div className="collector-actions">
            <button
              className="primary"
              onClick={() => void runAction(api.startCollector, '自动记录已启动')}
              type="button"
            >
              <Play size={14} />启动
            </button>
            <button
              onClick={() => void runAction(api.stopCollector, '自动记录已暂停')}
              type="button"
            >
              <Pause size={14} />暂停
            </button>
          </div>
        </div>
        <div className="sidebar-foot">v0.2 · 数据仅存本机 SQLite</div>
      </aside>

      <main className="main">
        <header className="topbar">
          <div className="page-heading">
            <span className="eyebrow">METADATA-FIRST</span>
            <h1>{currentTab.label}</h1>
            <p>{pageDescription(activeTab)}</p>
          </div>
          <div className="topbar-right">
            {activeTab !== 'settings' && (
              <DateNavigator
                value={selectedDate}
                onChange={setSelectedDate}
                onPrevious={() => goDate(-1)}
                onNext={() => goDate(1)}
              />
            )}
            <div className="top-actions">
              {data.settings.aiMode !== 'off' && (
                <button
                  onClick={() =>
                    void runAction(api.runAnalysisOnce, '已复核一条低置信会话')
                  }
                  type="button"
                >
                  <WandSparkles size={16} />AI 复核
                </button>
              )}
              <button
                onClick={() =>
                  void runAction(api.compactSessions, '已整理连续同类会话')
                }
                type="button"
                title="合并被短暂切换打断的同类活动"
              >
                <RefreshCw size={16} />整理
              </button>
              <button
                onClick={() =>
                  void runAction(api.cleanupMediaCache, '数据库与旧缓存已优化')
                }
                type="button"
                title="清理过期原始事件并压缩数据库"
              >
                <Database size={16} />优化
              </button>
            </div>
          </div>
        </header>

        {activeTab !== 'settings' && (
          <section className="kpi-grid">
            <Kpi
              icon={Clock3}
              title="有效使用"
              value={formatDuration(stats.activeMinutes)}
              hint={`离开 ${formatDuration(stats.idleMinutes)}`}
            />
            <Kpi
              icon={Tags}
              title="已归到项目"
              value={`${stats.classifiedPercent}%`}
              hint={`${stats.classifiedMinutes} / ${stats.activeMinutes} 分钟`}
            />
            <Kpi
              icon={Activity}
              title="上下文"
              value={`${stats.contextCount} 次`}
              hint={`最长连续 ${formatDuration(stats.longestMinutes)}`}
            />
            <Kpi
              icon={CircleAlert}
              title="待复核"
              value={`${stats.reviewCount} 条`}
              hint={stats.reviewCount ? '只处理真正不确定的记录' : '今天无需人工处理'}
              attention={stats.reviewCount > 0}
            />
          </section>
        )}

        {activeTab === 'today' && (
          <TodayView
            sessions={daySessions}
            projects={data.projects}
            stats={stats}
            selectedDate={selectedDate}
            onEdit={setEditing}
            onOpenTimeline={() => setActiveTab('timeline')}
            planItems={data.planItems}
          />
        )}
        {activeTab === 'timeline' && (
          <TimelineView
            sessions={daySessions}
            projects={data.projects}
            tasks={data.tasks}
            selected={selected}
            setSelected={setSelected}
            onEdit={setEditing}
            runAction={runAction}
          />
        )}
        {activeTab === 'projects' && (
          <ProjectsView
            projects={data.projects}
            tasks={data.tasks}
            sessions={daySessions}
            selectedDate={selectedDate}
          />
        )}
        {activeTab === 'settings' && (
          <SettingsView data={data} runAction={runAction} />
        )}
      </main>

      {editing && (
        <EditSessionModal
          session={editing}
          projects={data.projects}
          tasks={data.tasks}
          onClose={() => setEditing(null)}
          onSave={async (session, patch) => {
            await runAction(
              () => api.updateSession(session.id, patch),
              '会话已修正，后续同类活动可继续学习',
            );
            setEditing(null);
          }}
          runAction={runAction}
        />
      )}
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

function DateNavigator({
  value,
  onChange,
  onPrevious,
  onNext,
}: {
  value: string;
  onChange: (value: string) => void;
  onPrevious: () => void;
  onNext: () => void;
}) {
  const today = localDateKey(new Date());
  return (
    <div className="date-navigator">
      <button onClick={onPrevious} type="button" aria-label="前一天">
        <ChevronLeft size={16} />
      </button>
      <label>
        <CalendarDays size={16} />
        <input type="date" value={value} onChange={(event) => onChange(event.target.value)} />
      </label>
      <button onClick={onNext} type="button" aria-label="后一天">
        <ChevronRight size={16} />
      </button>
      {value !== today && (
        <button className="today-button" onClick={() => onChange(today)} type="button">
          今天
        </button>
      )}
    </div>
  );
}

function TodayView({
  sessions,
  projects,
  stats,
  selectedDate,
  onEdit,
  onOpenTimeline,
  planItems,
}: {
  sessions: WorkSession[];
  projects: Project[];
  stats: DayStats;
  selectedDate: string;
  onEdit: (session: WorkSession) => void;
  onOpenTimeline: () => void;
  planItems: DashboardData['planItems'];
}) {
  const review = sessions.filter(needsReview).slice(0, 4);
  const projectRows = projectBreakdown(sessions, selectedDate).slice(0, 6);
  const visiblePlanItems = planItems
    .filter((item) => item.status !== 'done')
    .slice(0, 5);

  return (
    <div className="dashboard-grid">
      <section className="panel span-2">
        <PanelTitle
          title="时间分布"
          subtitle="按真实持续时间汇总；离开时间不会计入有效使用。"
        />
        {stats.activeMinutes === 0 && stats.idleMinutes === 0 ? (
          <EmptyState title="这一天还没有记录" detail="保持 ScreenUse 在托盘运行即可自动出现数据。" />
        ) : (
          <>
            <div className="distribution-bar" aria-label="分类时间分布">
              {stats.categories
                .filter((item) => item.minutes > 0 && item.category !== '离开')
                .map((item) => (
                  <span
                    key={item.category}
                    title={`${item.category} ${formatDuration(item.minutes)}`}
                    style={
                      {
                        '--segment-color': categoryColor(item.category),
                        flexGrow: item.minutes,
                      } as CSSProperties
                    }
                  />
                ))}
            </div>
            <div className="distribution-list">
              {stats.categories
                .filter((item) => item.minutes > 0)
                .map((item) => (
                  <div key={item.category} className="distribution-row">
                    <span
                      className="legend-dot"
                      style={{ background: categoryColor(item.category) }}
                    />
                    <strong>{item.category}</strong>
                    <div className="mini-track">
                      <span
                        style={
                          {
                            width: `${Math.max(
                              3,
                              Math.round(
                                (item.minutes /
                                  Math.max(1, stats.activeMinutes + stats.idleMinutes)) *
                                  100,
                              ),
                            )}%`,
                            background: categoryColor(item.category),
                          }
                        }
                      />
                    </div>
                    <b>{formatDuration(item.minutes)}</b>
                  </div>
                ))}
            </div>
          </>
        )}
      </section>

      <section className="panel">
        <PanelTitle title="项目投入" subtitle="当前日期内最花时间的事务。" />
        <div className="ranking-list">
          {projectRows.length ? (
            projectRows.map((row, index) => (
              <div className="ranking-row" key={row.id || row.name}>
                <span className="rank">{index + 1}</span>
                <div>
                  <strong>{row.name}</strong>
                  <span>{row.category}</span>
                </div>
                <b>{formatDuration(row.minutes)}</b>
              </div>
            ))
          ) : (
            <EmptyState title="暂无项目时间" detail="修正一条会话后，相似活动会自动归入项目。" />
          )}
        </div>
      </section>

      <section className="panel span-2">
        <PanelTitle
          title="最近活动"
          subtitle="窗口切换时自动结束上一段，不需要手动计时。"
          action={
            <button onClick={onOpenTimeline} type="button">
              查看全部
            </button>
          }
        />
        <div className="compact-timeline">
          {sessions.slice(0, 7).map((session) => (
            <button
              className="compact-session"
              key={session.id}
              onClick={() => onEdit(session)}
              type="button"
            >
              <span className="compact-time">{formatClock(session.startedAt)}</span>
              <span
                className="category-line"
                style={{ background: categoryColor(session.category) }}
              />
              <span className="compact-main">
                <strong>{session.summary}</strong>
                <small>
                  {session.projectName || '未归类'} · {session.category}
                </small>
              </span>
              <b>{formatDuration(sessionMinutesOnDate(session, selectedDate))}</b>
              {needsReview(session) && <CircleAlert size={16} className="warning-icon" />}
            </button>
          ))}
          {!sessions.length && (
            <EmptyState title="暂无活动" detail="ScreenUse 会在应用或标签页切换时自动生成时间段。" />
          )}
        </div>
      </section>

      <section className="panel">
        <PanelTitle
          title="待复核"
          subtitle="低置信或尚未匹配项目的记录。"
          action={
            review.length ? (
              <button onClick={onOpenTimeline} type="button">
                处理
              </button>
            ) : undefined
          }
        />
        {review.length ? (
          <div className="review-list">
            {review.map((session) => (
              <button key={session.id} onClick={() => onEdit(session)} type="button">
                <CircleAlert size={16} />
                <span>
                  <strong>{session.summary}</strong>
                  <small>{Math.round(session.confidence * 100)}% 置信度</small>
                </span>
                <Pencil size={15} />
              </button>
            ))}
          </div>
        ) : (
          <div className="all-clear">
            <CheckCircle2 size={30} />
            <strong>今天无需复核</strong>
            <span>常见活动已能自动归类。</span>
          </div>
        )}
      </section>

      {visiblePlanItems.length > 0 && (
        <section className="panel span-3">
          <PanelTitle title="计划线索" subtitle="来自 DDL-Manager 或 ICS，可辅助核对当天投入。" />
          <div className="plan-strip">
            {visiblePlanItems.map((item) => (
              <div key={item.id} className="plan-chip">
                <span>{item.source}</span>
                <strong>{item.title}</strong>
                <small>{item.dueAt ? formatDateTime(item.dueAt) : '未设置截止时间'}</small>
              </div>
            ))}
          </div>
        </section>
      )}
    </div>
  );
}

function TimelineView({
  sessions,
  projects,
  tasks,
  selected,
  setSelected,
  onEdit,
  runAction,
}: {
  sessions: WorkSession[];
  projects: Project[];
  tasks: Task[];
  selected: Set<string>;
  setSelected: (next: Set<string>) => void;
  onEdit: (session: WorkSession) => void;
  runAction: ActionRunner;
}) {
  const [query, setQuery] = useState('');
  const [reviewOnly, setReviewOnly] = useState(false);
  const normalized = query.trim().toLowerCase();
  const filtered = sessions.filter((session) => {
    if (reviewOnly && !needsReview(session)) return false;
    if (!normalized) return true;
    return [
      session.summary,
      session.category,
      session.projectName,
      session.taskTitle,
      ...session.evidence.map((item) => item.value),
    ]
      .filter(Boolean)
      .join(' ')
      .toLowerCase()
      .includes(normalized);
  });

  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  };

  const mergeSelected = async () => {
    const ids = [...selected];
    if (ids.length < 2) return;
    const summary = window.prompt('合并后的会话名称', '连续工作会话')?.trim();
    if (!summary) return;
    await api.mergeSessions(ids, summary);
    setSelected(new Set());
  };

  return (
    <div className="timeline-layout">
      <section className="panel timeline-panel">
        <div className="timeline-toolbar">
          <label className="search-field">
            <Search size={16} />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="搜索项目、摘要、应用或网页"
            />
          </label>
          <label className="switch-label">
            <input
              type="checkbox"
              checked={reviewOnly}
              onChange={(event) => setReviewOnly(event.target.checked)}
            />
            只看待复核
          </label>
          <button
            disabled={selected.size < 2}
            onClick={() => void runAction(mergeSelected, '已合并所选会话')}
            type="button"
          >
            <Merge size={15} />合并 {selected.size || ''}
          </button>
        </div>

        <div className="timeline-list">
          {filtered.map((session) => (
            <SessionRow
              key={session.id}
              session={session}
              selected={selected.has(session.id)}
              onToggle={() => toggle(session.id)}
              onEdit={() => onEdit(session)}
            />
          ))}
          {!filtered.length && (
            <EmptyState
              title={reviewOnly ? '没有待复核记录' : '没有匹配的活动'}
              detail={reviewOnly ? '本地规则已经处理完这一天。' : '尝试清空搜索条件。'}
            />
          )}
        </div>
      </section>

      <aside className="timeline-aside">
        <section className="panel guidance-card">
          <Sparkles size={24} />
          <h2>越用越省事</h2>
          <p>先修正项目和分类，再点一次“学习规则”。以后相似窗口、网址或工作区会优先命中你的选择。</p>
          <div className="guidance-steps">
            <span><b>1</b> 修正少量低置信记录</span>
            <span><b>2</b> 从正确记录学习规则</span>
            <span><b>3</b> 日常只看结果</span>
          </div>
        </section>
        <section className="panel source-card">
          <PanelTitle title="采集信号" subtitle="不会读取网页正文或代码内容。" />
          <div className="source-row"><span>Windows 前台窗口</span><Check size={16} /></div>
          <div className="source-row"><span>活动浏览器标签页</span><Check size={16} /></div>
          <div className="source-row"><span>VS Code 工作区/文件名</span><Check size={16} /></div>
          <div className="source-row muted"><span>截图或录屏</span><X size={16} /></div>
        </section>
      </aside>
    </div>
  );
}

function SessionRow({
  session,
  selected,
  onToggle,
  onEdit,
}: {
  session: WorkSession;
  selected: boolean;
  onToggle: () => void;
  onEdit: () => void;
}) {
  const minutes = minutesBetween(session.startedAt, session.endedAt);
  const evidence = session.evidence.slice(0, 3);
  return (
    <article
      className={`session-row ${selected ? 'selected' : ''} ${needsReview(session) ? 'needs-review' : ''}`}
      style={{ '--session-color': categoryColor(session.category) } as CSSProperties}
    >
      <label className="select-session" title="选择会话">
        <input type="checkbox" checked={selected} onChange={onToggle} />
      </label>
      <div className="time-cell">
        <strong>{formatClock(session.startedAt)}</strong>
        <span>{formatClock(session.endedAt)}</span>
      </div>
      <div className="session-main">
        <div className="session-head">
          <span
            className="category-pill"
            style={
              {
                '--pill-color': categoryColor(session.category),
              } as CSSProperties
            }
          >
            {session.category}
          </span>
          <strong>{session.summary}</strong>
          {session.userConfirmed && <CheckCircle2 size={16} className="confirmed" />}
        </div>
        <div className="session-path">
          <span>{session.projectName || '未归类项目'}</span>
          <i>›</i>
          <span>{session.taskTitle || '未归类任务'}</span>
        </div>
        {evidence.length > 0 && (
          <div className="evidence-row">
            {evidence.map((item, index) => (
              <span key={`${item.kind}-${index}`} title={item.value}>
                {item.label}：{item.value}
              </span>
            ))}
          </div>
        )}
      </div>
      <div className="session-score">
        <strong>{formatDuration(minutes)}</strong>
        <span className={needsReview(session) ? 'low' : ''}>
          {Math.round(session.confidence * 100)}%
        </span>
      </div>
      <button className="edit-button" onClick={onEdit} type="button">
        <Pencil size={15} />修正
      </button>
    </article>
  );
}

function ProjectsView({
  projects,
  tasks,
  sessions,
  selectedDate,
}: {
  projects: Project[];
  tasks: Task[];
  sessions: WorkSession[];
  selectedDate: string;
}) {
  const rows = projectBreakdown(sessions, selectedDate);
  const minutesByProject = new Map(rows.map((row) => [row.id, row.minutes]));
  const tasksByProject = groupBy(tasks, (task) => task.projectId);

  return (
    <div className="projects-layout">
      <section className="panel span-2">
        <PanelTitle
          title="项目账本"
          subtitle="工作区名称、网址和你纠正过的规则会自动把活动归入这些项目。"
        />
        <div className="project-grid">
          {projects.map((project) => {
            const minutes = minutesByProject.get(project.id) || 0;
            return (
              <article
                className="project-card"
                key={project.id}
                style={{ '--project-color': project.color || categoryColor(project.category) } as CSSProperties}
              >
                <div className="project-card-head">
                  <span>{project.category}</span>
                  <b>{formatDuration(minutes)}</b>
                </div>
                <h2>{project.name}</h2>
                <p>{project.description || '根据活动元数据自动维护。'}</p>
                <div className="project-progress">
                  <span
                    style={{
                      width: `${Math.min(100, Math.max(4, (minutes / Math.max(1, rows[0]?.minutes || 1)) * 100))}%`,
                    }}
                  />
                </div>
                <small>{sourceLabel(project.source)}</small>
              </article>
            );
          })}
          {!projects.length && (
            <EmptyState title="还没有项目" detail="打开一个代码工作区或修正一条会话即可自动建立。" />
          )}
        </div>
      </section>

      <section className="panel">
        <PanelTitle title="项目任务" subtitle="用于更细粒度的时间归因。" />
        <div className="task-list">
          {projects.map((project) => (
            <div className="task-group" key={project.id}>
              <h3>{project.name}</h3>
              {(tasksByProject.get(project.id) || []).map((task) => (
                <div className="task-row" key={task.id}>
                  <TimerReset size={15} />
                  <span>{task.title}</span>
                  <small>{sourceLabel(task.source)}</small>
                </div>
              ))}
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}

function SettingsView({ data, runAction }: { data: DashboardData; runAction: ActionRunner }) {
  const [settings, setSettings] = useState<AppSettings>(data.settings);
  const [secret, setSecret] = useState('');

  useEffect(() => setSettings(data.settings), [data.settings]);

  const update = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => {
    setSettings((current) => ({ ...current, [key]: value }));
  };

  const saveAll = async () => {
    let next = { ...settings };
    if (secret.trim()) {
      const secretName = settings.aiSecretRef?.trim() || 'openai-compatible';
      await api.saveSecret(secretName, secret.trim());
      next = { ...next, aiSecretRef: secretName };
      setSettings(next);
      setSecret('');
    }
    await api.saveSettings(next);
  };

  return (
    <div className="settings-grid">
      <section className="panel settings-panel">
        <PanelTitle
          title="自动记录"
          subtitle="默认配置优先降低 CPU、写盘和打扰。"
        />
        <div className="field-grid">
          <Field label="前台检测间隔" hint="应用切换发现速度；推荐 2 秒。">
            <NumberInput
              value={settings.pollIntervalSeconds}
              min={1}
              max={15}
              suffix="秒"
              onChange={(value) => update('pollIntervalSeconds', value)}
            />
          </Field>
          <Field label="稳定上下文心跳" hint="同一活动只覆盖同一条原始事件。">
            <NumberInput
              value={settings.heartbeatSeconds}
              min={10}
              max={300}
              suffix="秒"
              onChange={(value) => update('heartbeatSeconds', value)}
            />
          </Field>
          <Field label="离开判定" hint="多久没有键鼠输入后记为离开。">
            <NumberInput
              value={settings.idleThresholdSeconds}
              min={30}
              max={3600}
              suffix="秒"
              onChange={(value) => update('idleThresholdSeconds', value)}
            />
          </Field>
          <Field label="原始元数据保留" hint="汇总后的会话长期保留；原始事件自动清理。">
            <NumberInput
              value={settings.rawEventRetentionDays}
              min={7}
              max={3650}
              suffix="天"
              onChange={(value) => update('rawEventRetentionDays', value)}
            />
          </Field>
        </div>
        <Toggle
          checked={settings.launchAtLogin}
          onChange={(value) => update('launchAtLogin', value)}
          title="登录 Windows 后静默启动"
          detail="从登录启动项进入时只显示托盘，不弹出主窗口。"
        />
        <Toggle
          checked={settings.autoStart}
          onChange={(value) => update('autoStart', value)}
          title="启动 ScreenUse 后自动记录"
          detail="关闭主窗口后仍在系统托盘静默运行。"
        />
        <Toggle
          checked={settings.autoMaintenance}
          onChange={(value) => update('autoMaintenance', value)}
          title="自动压缩与清理"
          detail="每 6 小时清理旧原始事件和 v0.1 遗留截图缓存。"
        />
        <div className="setting-callout good">
          <CheckCircle2 size={19} />
          <div>
            <strong>截图采集已永久退出默认链路</strong>
            <span>当前只保存应用名、窗口标题、活动标签页和编辑器上下文。</span>
          </div>
        </div>
      </section>

      <section className="panel settings-panel">
        <PanelTitle
          title="可选 AI 复核"
          subtitle="本地规则始终先运行；不开 AI 也能完整使用。"
        />
        <Field label="AI 模式" hint="个人使用推荐关闭或手动复核。">
          <select value={settings.aiMode} onChange={(event) => update('aiMode', event.target.value)}>
            <option value="off">关闭（零费用）</option>
            <option value="manual">手动复核低置信长会话</option>
            <option value="auto">自动复核低置信长会话</option>
          </select>
        </Field>
        {settings.aiMode !== 'off' && (
          <div className="ai-fields">
            <Field label="最低会话时长" hint="短碎片不调用模型。">
              <NumberInput
                value={settings.minAiSessionMinutes}
                min={1}
                max={240}
                suffix="分钟"
                onChange={(value) => update('minAiSessionMinutes', value)}
              />
            </Field>
            <Field label="API Base">
              <input
                value={settings.aiBaseUrl}
                onChange={(event) => update('aiBaseUrl', event.target.value)}
                placeholder="https://api.openai.com/v1"
              />
            </Field>
            <Field label="模型名" hint="选择支持 JSON 输出的小模型即可。">
              <input
                value={settings.aiModel}
                onChange={(event) => update('aiModel', event.target.value)}
                placeholder="例如低价 mini / flash 模型"
              />
            </Field>
            <Field label="凭据名称">
              <input
                value={settings.aiSecretRef || ''}
                onChange={(event) => update('aiSecretRef', event.target.value)}
                placeholder="openai-compatible"
              />
            </Field>
            <Field label="API Key" hint="留空不会覆盖已保存凭据。">
              <input
                type="password"
                value={secret}
                onChange={(event) => setSecret(event.target.value)}
                placeholder="保存到系统凭据库"
              />
            </Field>
            <button
              onClick={() =>
                void runAction(
                  () => api.testAiConfig(settings, settings.aiSecretRef || 'openai-compatible'),
                  'AI 配置可读取',
                )
              }
              type="button"
            >
              <Sparkles size={16} />测试配置
            </button>
          </div>
        )}
        <div className="setting-callout">
          <WandSparkles size={19} />
          <div>
            <strong>单次载荷有硬上限</strong>
            <span>最多发送 80 条精简元数据，URL 查询参数会去除，30 秒自动超时。</span>
          </div>
        </div>
      </section>

      <section className="panel settings-panel">
        <PanelTitle title="外部线索" subtitle="可选导入计划，用于核对项目投入。" />
        <Field label="DDL-Manager 数据库">
          <input
            value={settings.ddlManagerDbPath}
            onChange={(event) => update('ddlManagerDbPath', event.target.value)}
          />
        </Field>
        <button
          onClick={() =>
            void runAction(
              () => api.importDdlManager(settings.ddlManagerDbPath),
              '已只读导入 DDL-Manager',
            )
          }
          type="button"
        >
          <Download size={16} />导入 DDL-Manager
        </button>
        <Field label="ICS 文件路径">
          <input id="ics-path" placeholder="D:\\calendar.ics" />
        </Field>
        <button
          onClick={() => {
            const path =
              (document.getElementById('ics-path') as HTMLInputElement | null)?.value || '';
            if (path) void runAction(() => api.importIcs(path), '已导入 ICS');
          }}
          type="button"
        >
          <CalendarDays size={16} />导入 ICS
        </button>
        <div className="integration-paths">
          <span>浏览器扩展 <code>extensions/chromium</code></span>
          <span>VS Code 扩展 <code>extensions/vscode</code></span>
        </div>
      </section>

      <section className="panel settings-panel">
        <PanelTitle title="数据管理" subtitle="SQLite 会话长期保留，原始事件按保留期轮转。" />
        <div className="data-actions">
          <button
            onClick={() =>
              void runAction(async () => {
                const path = await api.revealDataDir();
                window.alert(path);
                return path;
              }, '数据目录')
            }
            type="button"
          >
            <FolderKanban size={16} />查看数据目录
          </button>
          <button
            onClick={() => void runAction(api.cleanupMediaCache, '数据库已压缩优化')}
            type="button"
          >
            <Database size={16} />立即优化
          </button>
          <button
            onClick={() =>
              void runAction(
                () => api.backupNow(settings.backupDir || undefined),
                '备份已创建',
              )
            }
            type="button"
          >
            <HardDrive size={16} />立即备份
          </button>
          <button
            onClick={() => void runAction(() => api.exportData('csv'), 'CSV 已导出')}
            type="button"
          >
            <Download size={16} />导出 CSV
          </button>
          <button
            onClick={() =>
              void runAction(() => api.exportData('markdown'), 'Markdown 日报已导出')
            }
            type="button"
          >
            <BarChart3 size={16} />导出日报
          </button>
        </div>
        <Field label="自定义备份目录" hint="留空使用 ScreenUse 默认数据目录。">
          <input
            value={settings.backupDir || ''}
            onChange={(event) => update('backupDir', event.target.value || null)}
            placeholder="留空使用默认目录"
          />
        </Field>
      </section>

      <div className="settings-savebar">
        <div>
          <strong>保存后自动生效</strong>
          <span>采集器会在下一次配置刷新时应用间隔和保留策略。</span>
        </div>
        <button className="primary" onClick={() => void runAction(saveAll, '设置已保存')} type="button">
          <Check size={17} />保存全部设置
        </button>
      </div>
    </div>
  );
}

function EditSessionModal({
  session,
  projects,
  tasks,
  onClose,
  onSave,
  runAction,
}: {
  session: WorkSession;
  projects: Project[];
  tasks: Task[];
  onClose: () => void;
  onSave: (
    session: WorkSession,
    patch: {
      summary: string;
      projectId?: string | null;
      taskId?: string | null;
      category: string;
      confidence: number;
      userConfirmed: boolean;
    },
  ) => Promise<void>;
  runAction: ActionRunner;
}) {
  const [summary, setSummary] = useState(session.summary);
  const [category, setCategory] = useState(session.category);
  const [projectId, setProjectId] = useState(session.projectId || '');
  const [taskId, setTaskId] = useState(session.taskId || '');
  const availableTasks = tasks.filter((task) => task.projectId === projectId);

  const split = async () => {
    const splitAt = window.prompt(
      '拆分时间（ISO）',
      new Date(
        (new Date(session.startedAt).getTime() + new Date(session.endedAt).getTime()) / 2,
      ).toISOString(),
    );
    if (!splitAt) return;
    await api.splitSession(session.id, splitAt);
    onClose();
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose} role="presentation">
      <section className="modal" onMouseDown={(event) => event.stopPropagation()}>
        <div className="modal-head">
          <div>
            <span className="eyebrow">CORRECT & LEARN</span>
            <h2>修正会话</h2>
            <p>
              {formatDateTime(session.startedAt)} · {formatDuration(minutesBetween(session.startedAt, session.endedAt))}
            </p>
          </div>
          <button className="icon-button" onClick={onClose} type="button" aria-label="关闭">
            <X size={18} />
          </button>
        </div>

        <Field label="摘要">
          <input value={summary} onChange={(event) => setSummary(event.target.value)} autoFocus />
        </Field>
        <div className="field-grid">
          <Field label="分类">
            <select value={category} onChange={(event) => setCategory(event.target.value)}>
              {categories.map((item) => <option key={item}>{item}</option>)}
            </select>
          </Field>
          <Field label="项目">
            <select
              value={projectId}
              onChange={(event) => {
                setProjectId(event.target.value);
                setTaskId('');
              }}
            >
              <option value="" disabled>选择项目</option>
              {projects.map((project) => (
                <option key={project.id} value={project.id}>{project.name}</option>
              ))}
            </select>
          </Field>
          <Field label="任务">
            <select value={taskId} onChange={(event) => setTaskId(event.target.value)}>
              <option value="">沿用/暂不指定</option>
              {availableTasks.map((task) => (
                <option key={task.id} value={task.id}>{task.title}</option>
              ))}
            </select>
          </Field>
        </div>

        <div className="modal-evidence">
          <strong>本次判断依据</strong>
          {session.evidence.length ? (
            session.evidence.map((item, index) => (
              <span key={`${item.kind}-${index}`}>
                <b>{item.label}</b>{item.value}
              </span>
            ))
          ) : (
            <span>没有附加元数据</span>
          )}
        </div>

        <div className="modal-secondary">
          <button
            onClick={() =>
              void runAction(
                () => api.learnRuleFromSession(session.id),
                '已从当前会话学习规则',
              )
            }
            type="button"
          >
            <Sparkles size={16} />从这条记录学习规则
          </button>
          <button onClick={() => void runAction(split, '会话已拆分')} type="button">
            <SplitSquareHorizontal size={16} />拆分
          </button>
        </div>

        <div className="modal-actions">
          <button onClick={onClose} type="button">取消</button>
          <button
            className="primary"
            onClick={() =>
              void onSave(session, {
                summary: summary.trim() || session.summary,
                projectId: projectId || session.projectId || null,
                taskId: taskId || session.taskId || null,
                category,
                confidence: Math.max(0.96, session.confidence),
                userConfirmed: true,
              })
            }
            type="button"
          >
            <Check size={17} />保存并确认
          </button>
        </div>
      </section>
    </div>
  );
}

function Kpi({
  icon: Icon,
  title,
  value,
  hint,
  attention = false,
}: {
  icon: typeof Activity;
  title: string;
  value: string;
  hint: string;
  attention?: boolean;
}) {
  return (
    <div className={`kpi ${attention ? 'attention' : ''}`}>
      <span className="kpi-icon"><Icon size={18} /></span>
      <div>
        <span>{title}</span>
        <strong>{value}</strong>
        <small>{hint}</small>
      </div>
    </div>
  );
}

function PanelTitle({
  title,
  subtitle,
  action,
}: {
  title: string;
  subtitle?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="panel-title">
      <div>
        <h2>{title}</h2>
        {subtitle && <p>{subtitle}</p>}
      </div>
      {action}
    </div>
  );
}

function EmptyState({ title, detail }: { title: string; detail: string }) {
  return (
    <div className="empty-state">
      <Activity size={24} />
      <strong>{title}</strong>
      <span>{detail}</span>
    </div>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="field">
      <span>{label}</span>
      {children}
      {hint && <small>{hint}</small>}
    </label>
  );
}

function NumberInput({
  value,
  min,
  max,
  suffix,
  onChange,
}: {
  value: number;
  min: number;
  max: number;
  suffix: string;
  onChange: (value: number) => void;
}) {
  return (
    <div className="number-input">
      <input
        type="number"
        value={value}
        min={min}
        max={max}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <span>{suffix}</span>
    </div>
  );
}

function Toggle({
  checked,
  onChange,
  title,
  detail,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  title: string;
  detail: string;
}) {
  return (
    <label className="toggle-row">
      <span>
        <strong>{title}</strong>
        <small>{detail}</small>
      </span>
      <input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} />
      <i />
    </label>
  );
}

type ActionRunner = (
  fn: () => Promise<unknown>,
  successMessage: string,
) => Promise<unknown>;

type DayStats = {
  activeMinutes: number;
  idleMinutes: number;
  classifiedMinutes: number;
  classifiedPercent: number;
  contextCount: number;
  reviewCount: number;
  longestMinutes: number;
  categories: { category: string; minutes: number }[];
};

function summarizeDay(sessions: WorkSession[], dateKey: string): DayStats {
  const result = new Map<string, number>();
  let activeMinutes = 0;
  let idleMinutes = 0;
  let classifiedMinutes = 0;
  let longestMinutes = 0;
  let reviewCount = 0;

  for (const session of sessions) {
    const minutes = sessionMinutesOnDate(session, dateKey);
    result.set(session.category, (result.get(session.category) || 0) + minutes);
    if (session.category === '离开') {
      idleMinutes += minutes;
      continue;
    }
    activeMinutes += minutes;
    if (session.projectId) classifiedMinutes += minutes;
    if (needsReview(session)) reviewCount += 1;
    longestMinutes = Math.max(longestMinutes, minutes);
  }

  return {
    activeMinutes,
    idleMinutes,
    classifiedMinutes,
    classifiedPercent: activeMinutes
      ? Math.round((classifiedMinutes / activeMinutes) * 100)
      : 0,
    contextCount: sessions.filter((session) => session.category !== '离开').length,
    reviewCount,
    longestMinutes,
    categories: [...result.entries()]
      .map(([category, minutes]) => ({ category, minutes }))
      .sort((left, right) => right.minutes - left.minutes),
  };
}

function projectBreakdown(sessions: WorkSession[], dateKey: string) {
  const map = new Map<string, { id: string; name: string; category: string; minutes: number }>();
  for (const session of sessions) {
    if (session.category === '离开') continue;
    const id = session.projectId || '';
    const name = session.projectName || '未归类';
    const key = id || `unclassified:${session.category}`;
    const current = map.get(key) || { id, name, category: session.category, minutes: 0 };
    current.minutes += sessionMinutesOnDate(session, dateKey);
    map.set(key, current);
  }
  return [...map.values()].sort((left, right) => right.minutes - left.minutes);
}

function needsReview(session: WorkSession) {
  return (
    session.category !== '离开' &&
    !session.userConfirmed &&
    (!session.projectId || session.confidence < 0.72)
  );
}

function sessionMinutesOnDate(session: WorkSession, dateKey: string) {
  const dayStart = dateFromKey(dateKey).getTime();
  const dayEnd = dayStart + 24 * 60 * 60 * 1000;
  const start = Math.max(dayStart, new Date(session.startedAt).getTime());
  const end = Math.min(dayEnd, new Date(session.endedAt).getTime());
  return Math.max(0, Math.round((end - start) / 60_000));
}

function minutesBetween(start: string, end: string) {
  return Math.max(
    0,
    Math.round((new Date(end).getTime() - new Date(start).getTime()) / 60_000),
  );
}

function formatDuration(minutes: number) {
  const rounded = Math.max(0, Math.round(minutes));
  if (rounded < 60) return `${rounded} 分钟`;
  const hours = Math.floor(rounded / 60);
  const rest = rounded % 60;
  return rest ? `${hours} 小时 ${rest} 分` : `${hours} 小时`;
}

function formatClock(value: string) {
  return new Date(value).toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
  });
}

function formatDateTime(value: string) {
  return new Date(value).toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function localDateKey(date: Date) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

function dateFromKey(key: string) {
  const [year, month, day] = key.split('-').map(Number);
  return new Date(year, month - 1, day);
}

function categoryColor(category: string) {
  return categoryColors[category] || '#94a3b8';
}

function sourceLabel(source: string) {
  const labels: Record<string, string> = {
    'workspace-auto': '工作区自动发现',
    'metadata-auto': '元数据自动发现',
    manual: '手动创建',
    seed: '示例',
    'ai-review': 'AI 复核',
  };
  return labels[source] || source;
}

function pageDescription(tab: TabId) {
  const descriptions: Record<TabId, string> = {
    today: '一天用了多久、花在哪些事务上，打开就能看清。',
    timeline: '只修正少量不确定记录，其余由本地规则自动完成。',
    projects: '按项目和任务汇总投入，不依赖手动启动计时器。',
    settings: '控制采集频率、数据保留和完全可选的 AI 复核。',
  };
  return descriptions[tab];
}

function groupBy<T>(items: T[], key: (item: T) => string) {
  const map = new Map<string, T[]>();
  for (const item of items) {
    const value = key(item);
    map.set(value, [...(map.get(value) || []), item]);
  }
  return map;
}
