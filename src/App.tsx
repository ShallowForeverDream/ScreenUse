import {
  Fragment,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from 'react';
import { createPortal } from 'react-dom';
import {
  Activity,
  BarChart3,
  CalendarDays,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  Cloud,
  Clock3,
  Copy,
  Database,
  Download,
  FolderKanban,
  HardDrive,
  Github,
  KeyRound,
  Laptop,
  LayoutDashboard,
  Merge,
  Monitor,
  Moon,
  Pause,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Search,
  Settings,
  ShieldCheck,
  Sparkles,
  SplitSquareHorizontal,
  Sun,
  Tags,
  TimerReset,
  Trash2,
  WandSparkles,
  X,
  ZoomIn,
  ZoomOut,
} from 'lucide-react';
import { api } from './api';
import type {
  AppSettings,
  CategoryOption,
  DashboardData,
  GithubSyncConfig,
  GithubSyncResult,
  GithubSyncStatus,
  Project,
  SessionPatch,
  Task,
  ThemeMode,
  WorkSession,
} from './types';

const tabs = [
  { id: 'today', label: '今日', icon: LayoutDashboard },
  { id: 'timeline', label: '时间轴', icon: Activity },
  { id: 'projects', label: '项目', icon: FolderKanban },
  { id: 'settings', label: '设置', icon: Settings },
] as const;

type TabId = (typeof tabs)[number]['id'];

const categoryColors: Record<string, string> = {
  开发: '#60a5fa',
  学习: '#a78bfa',
  写作: '#f472b6',
  沟通: '#34d399',
  娱乐: '#fb7185',
  杂务: '#fbbf24',
  离开: '#94a3b8',
};

const THEME_STORAGE_KEY = 'screenuse-theme';

const TIMELINE_GRID_WIDTH = 64;
const TIMELINE_SCALES = [
  { secondsPerGrid: 3600, label: '1 小时/格' },
  { secondsPerGrid: 1800, label: '30 分钟/格' },
  { secondsPerGrid: 600, label: '10 分钟/格' },
  { secondsPerGrid: 300, label: '5 分钟/格' },
  { secondsPerGrid: 60, label: '1 分钟/格' },
  { secondsPerGrid: 30, label: '30 秒/格' },
  { secondsPerGrid: 10, label: '10 秒/格' },
  { secondsPerGrid: 5, label: '5 秒/格' },
  { secondsPerGrid: 1, label: '1 秒/格' },
] as const;
const DEFAULT_TIMELINE_ZOOM = 2;

function normalizeTheme(value: unknown): ThemeMode {
  return value === 'system' || value === 'light' || value === 'dark' ? value : 'light';
}

function readStoredTheme(): ThemeMode {
  try {
    return normalizeTheme(window.localStorage.getItem(THEME_STORAGE_KEY));
  } catch {
    return 'light';
  }
}

function applyTheme(mode: ThemeMode) {
  const resolved =
    mode === 'system'
      ? window.matchMedia('(prefers-color-scheme: dark)').matches
        ? 'dark'
        : 'light'
      : mode;
  document.documentElement.dataset.theme = resolved;
  document.documentElement.style.colorScheme = resolved;
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, mode);
  } catch {
    // A disabled WebView storage backend should not prevent theme switching.
  }
}

applyTheme(readStoredTheme());

export default function App() {
  const [activeTab, setActiveTab] = useState<TabId>('today');
  const [data, setData] = useState<DashboardData | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState('');
  const [toast, setToast] = useState('');
  const [selectedDate, setSelectedDate] = useState(localDateKey(new Date()));
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [editing, setEditing] = useState<WorkSession[]>([]);
  const [themeMode, setThemeMode] = useState<ThemeMode>(readStoredTheme);
  const [globalSearchOpen, setGlobalSearchOpen] = useState(false);
  const [projectFocusId, setProjectFocusId] = useState('');

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

  useEffect(() => {
    applyTheme(themeMode);
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const syncSystemTheme = () => {
      if (themeMode === 'system') applyTheme('system');
    };
    media.addEventListener('change', syncSystemTheme);
    return () => media.removeEventListener('change', syncSystemTheme);
  }, [themeMode]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setGlobalSearchOpen((open) => !open);
      }
      if (event.key === 'Escape') setGlobalSearchOpen(false);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  useEffect(() => {
    if (data) setThemeMode(normalizeTheme(data.settings.theme));
  }, [data?.settings.theme]);

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
                aria-current={activeTab === tab.id ? 'page' : undefined}
                aria-label={tab.label}
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
          {data.activeContext && (
            <div className="context-pin">
              <div>
                <strong>{data.activeContext.projectName}</strong>
                <span>{data.activeContext.taskTitle || '当前项目'}</span>
              </div>
              <button
                onClick={() => void runAction(api.clearContextPin, '已恢复自动判断')}
                type="button"
                aria-label="取消固定当前事务"
                title="取消固定"
              >
                <X size={14} />
              </button>
            </div>
          )}
          <div className="collector-actions">
            {data.collectorRunning ? (
              <button
                onClick={() => void runAction(api.stopCollector, '自动记录已暂停')}
                type="button"
              >
                <Pause size={14} />暂停记录
              </button>
            ) : (
              <button
                className="primary"
                onClick={() => void runAction(api.startCollector, '自动记录已启动')}
                type="button"
              >
                <Play size={14} />开始记录
              </button>
            )}
          </div>
        </div>
        <div className="sidebar-foot">v0.2 · 本机优先 · 可选加密同步</div>
      </aside>

      <main className="main">
        <header className="topbar">
          <div className="page-heading">
            <h1>{currentTab.label}</h1>
            <p>{pageDescription(activeTab)}</p>
          </div>
          <div className="topbar-right">
            <button
              className="global-search-trigger"
              onClick={() => setGlobalSearchOpen(true)}
              type="button"
              aria-label="全局搜索"
            >
              <Search size={15} /><span>搜索</span><kbd>Ctrl K</kbd>
            </button>
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
                <RefreshCw size={16} />整理会话
              </button>
              <button
                onClick={() =>
                  void runAction(api.cleanupMediaCache, '数据库与旧缓存已优化')
                }
                type="button"
                title="清理过期原始事件并压缩数据库"
              >
                <Database size={16} />清理存储
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
              hint={stats.reviewCount ? '活动切换后生成，确认一次即可' : '暂无已结束的新时间块'}
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
            onEdit={(session) => setEditing([session])}
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
            runAction={runAction}
            categoryOptions={data.categoryOptions}
            focusProjectId={projectFocusId}
          />
        )}
        {activeTab === 'settings' && (
          <SettingsView data={data} runAction={runAction} onThemeChange={setThemeMode} />
        )}
      </main>

      {editing.length > 0 && (
        <EditSessionModal
          sessions={editing}
          projects={data.projects}
          tasks={data.tasks}
          categoryOptions={data.categoryOptions}
          onClose={() => setEditing([])}
          onSave={async (sessions, patch, options) => {
            await runAction(async () => {
              await api.updateSessions(sessions.map((session) => session.id), patch);
              if (options.remember) {
                for (const session of sessions) {
                  await api.learnRuleFromSession(session.id, options.keyword);
                }
              }
              if (options.pin && patch.projectId) {
                await api.pinContext(patch.projectId, patch.taskId, 30);
              }
            }, sessions.length > 1
              ? `已统一修正 ${sessions.length} 条会话`
              : options.pin
                ? '已修正，并固定当前事务 30 分钟'
                : options.remember
                  ? '已修正并记住规则'
                  : '会话已修正');
            setEditing([]);
            if (sessions.length > 1) setSelected(new Set());
          }}
          runAction={runAction}
        />
      )}
      {globalSearchOpen && (
        <GlobalSearch
          data={data}
          onClose={() => setGlobalSearchOpen(false)}
          onOpenSession={(session) => {
            setGlobalSearchOpen(false);
            setActiveTab('timeline');
            setEditing([session]);
          }}
          onOpenProject={(projectId) => {
            setProjectFocusId(projectId);
            setGlobalSearchOpen(false);
            setActiveTab('projects');
          }}
          onNavigate={(tab) => {
            setGlobalSearchOpen(false);
            setActiveTab(tab);
          }}
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

type GlobalSearchResult =
  | { id: string; kind: 'project'; title: string; meta: string; projectId: string; search: string }
  | { id: string; kind: 'task'; title: string; meta: string; projectId: string; search: string }
  | { id: string; kind: 'session'; title: string; meta: string; session: WorkSession; search: string };

function GlobalSearch({
  data,
  onClose,
  onOpenSession,
  onOpenProject,
  onNavigate,
}: {
  data: DashboardData;
  onClose: () => void;
  onOpenSession: (session: WorkSession) => void;
  onOpenProject: (projectId: string) => void;
  onNavigate: (tab: TabId) => void;
}) {
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => inputRef.current?.focus(), []);

  const results = useMemo<GlobalSearchResult[]>(() => {
    const needle = normalizeSearchText(query);
    const projectsById = new Map(data.projects.map((project) => [project.id, project]));
    const all: GlobalSearchResult[] = [
      ...data.projects.map((project) => ({
        id: `project:${project.id}`,
        kind: 'project' as const,
        title: project.name,
        meta: `${project.category} · 项目`,
        projectId: project.id,
        search: normalizeSearchText(`${project.name} ${project.category} ${project.description || ''}`),
      })),
      ...data.tasks.map((task) => {
        const project = projectsById.get(task.projectId);
        return {
          id: `task:${task.id}`,
          kind: 'task' as const,
          title: task.title,
          meta: `${project?.name || '未知项目'} · 任务`,
          projectId: task.projectId,
          search: normalizeSearchText(`${task.title} ${project?.name || ''} ${project?.category || ''}`),
        };
      }),
      ...data.sessions.map((session) => ({
        id: `session:${session.id}`,
        kind: 'session' as const,
        title: session.summary,
        meta: `${formatDateTime(session.startedAt)} · ${session.projectName || session.category}`,
        session,
        search: normalizeSearchText([
          session.summary,
          session.projectName,
          session.taskTitle,
          session.category,
          ...session.evidence.map((item) => item.value),
        ].filter(Boolean).join(' ')),
      })),
    ];
    if (!needle) {
      return all.filter((item) => item.kind === 'session').slice(0, 6);
    }
    return all
      .filter((item) => item.search.includes(needle))
      .sort((left, right) => {
        const leftExact = normalizeSearchText(left.title).startsWith(needle) ? 1 : 0;
        const rightExact = normalizeSearchText(right.title).startsWith(needle) ? 1 : 0;
        return rightExact - leftExact;
      })
      .slice(0, 12);
  }, [data, query]);

  useEffect(() => setActiveIndex(0), [query]);

  const open = (result: GlobalSearchResult) => {
    if (result.kind === 'session') onOpenSession(result.session);
    else onOpenProject(result.projectId);
  };

  return (
    <div className="command-backdrop" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <section className="command-palette" role="dialog" aria-modal="true" aria-label="全局搜索">
        <div className="command-input">
          <Search size={19} />
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'ArrowDown') {
                event.preventDefault();
                setActiveIndex((index) => results.length ? Math.min(results.length - 1, index + 1) : 0);
              } else if (event.key === 'ArrowUp') {
                event.preventDefault();
                setActiveIndex((index) => Math.max(0, index - 1));
              } else if (event.key === 'Enter' && results[activeIndex]) {
                event.preventDefault();
                open(results[activeIndex]);
              }
            }}
            placeholder="搜索会话、项目、任务、应用或页面…"
            aria-label="搜索 ScreenUse"
          />
          <kbd>Esc</kbd>
        </div>
        {!query && (
          <div className="command-shortcuts">
            {tabs.map(({ id, label, icon: Icon }) => (
              <button key={id} onClick={() => onNavigate(id)} type="button">
                <Icon size={15} />{label}
              </button>
            ))}
          </div>
        )}
        <div className="command-results" role="listbox" aria-label={query ? '搜索结果' : '最近会话'}>
          <span className="command-section-label">{query ? `${results.length} 个结果` : '最近会话'}</span>
          {results.map((result, index) => {
            const Icon = result.kind === 'session' ? Clock3 : result.kind === 'task' ? TimerReset : FolderKanban;
            return (
              <button
                key={result.id}
                className={index === activeIndex ? 'active' : ''}
                onMouseEnter={() => setActiveIndex(index)}
                onClick={() => open(result)}
                role="option"
                aria-selected={index === activeIndex}
                type="button"
              >
                <span className={`command-result-icon ${result.kind}`}><Icon size={16} /></span>
                <span><strong>{result.title}</strong><small>{result.meta}</small></span>
                <ChevronRight size={15} />
              </button>
            );
          })}
          {!results.length && <EmptyState title="没有匹配结果" detail="可以搜索项目名、任务名、窗口标题、应用或当前页面。" />}
        </div>
        <footer><span><kbd>↑</kbd><kbd>↓</kbd> 选择</span><span><kbd>Enter</kbd> 打开</span><span><kbd>Ctrl K</kbd> 随时搜索</span></footer>
      </section>
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
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null);
  const [timelineZoom, setTimelineZoom] = useState(DEFAULT_TIMELINE_ZOOM);
  const review = sessions.filter(needsReview).slice(0, 4);
  const projectRows = projectBreakdown(sessions, selectedDate).slice(0, 6);
  const projectTotal = projectRows.reduce((sum, row) => sum + row.minutes, 0);
  const visiblePlanItems = planItems
    .filter((item) => item.status !== 'done')
    .slice(0, 5);

  return (
    <div className="dashboard-grid">
      <section className="panel span-2">
        <PanelTitle title="时间分布" />
        {stats.activeMinutes === 0 && stats.idleMinutes === 0 ? (
          <EmptyState title="这一天还没有记录" detail="保持 ScreenUse 在托盘运行即可自动出现数据。" />
        ) : (
          <>
            <div className="distribution-bar" aria-label="分类时间分布">
              {stats.categories
                .filter((item) => item.minutes > 0 && item.category !== '离开')
                .map((item) => (
                  <button
                    key={item.category}
                    title={`${item.category} ${formatDuration(item.minutes)}`}
                    aria-label={`查看${item.category}的具体时间段`}
                    onClick={() => setSelectedCategory(item.category)}
                    type="button"
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
                  <button
                    key={item.category}
                    className="distribution-row"
                    onClick={() => setSelectedCategory(item.category)}
                    type="button"
                  >
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
                    <ChevronRight size={15} />
                  </button>
                ))}
            </div>
          </>
        )}
      </section>

      <section className="panel">
        <PanelTitle
          title="项目投入"
          subtitle={projectTotal ? `${projectRows.length} 个项目 · ${formatDuration(projectTotal)}` : undefined}
        />
        <div className="project-investment-list">
          {projectRows.length ? (
            projectRows.map((row) => {
              const project = projects.find((item) => item.id === row.id);
              const color = project?.color || categoryColor(row.category);
              const percent = Math.max(2, Math.round((row.minutes / Math.max(1, projectTotal)) * 100));
              return (
                <div className="project-investment" key={row.id || row.name}>
                  <div className="project-investment-head">
                    <span className="project-mark" style={{ '--project-color': color } as CSSProperties}>
                      <FolderKanban size={15} />
                    </span>
                    <div>
                      <strong>{row.name}</strong>
                      <span>{row.category}</span>
                    </div>
                    <b>{formatDuration(row.minutes)}</b>
                  </div>
                  <div className="project-progress-row">
                    <div className="project-progress">
                      <span style={{ width: `${percent}%`, background: color }} />
                    </div>
                    <small>{percent}%</small>
                  </div>
                </div>
              );
            })
          ) : (
            <EmptyState title="暂无项目时间" detail="修正一条会话后，相似活动会自动归入项目。" />
          )}
        </div>
      </section>

      <section className="panel span-3 day-track-panel">
        <PanelTitle
          title="今日时间段"
          action={
            <div className="timeline-zoom" aria-label="时间刻度缩放">
              <button
                onClick={() => setTimelineZoom((value) => Math.max(0, value - 1))}
                disabled={timelineZoom === 0}
                title="缩小时间刻度（也可按 Ctrl 向下滚轮）"
                type="button"
              >
                <ZoomOut size={15} />
              </button>
              <span>{TIMELINE_SCALES[timelineZoom].label}</span>
              <button
                onClick={() => setTimelineZoom((value) => Math.min(TIMELINE_SCALES.length - 1, value + 1))}
                disabled={timelineZoom === TIMELINE_SCALES.length - 1}
                title="放大时间刻度（也可按 Ctrl 向上滚轮）"
                type="button"
              >
                <ZoomIn size={15} />
              </button>
            </div>
          }
        />
        <DayActivityTimeline
          sessions={sessions}
          selectedDate={selectedDate}
          zoom={timelineZoom}
          onZoomChange={setTimelineZoom}
          onEdit={onEdit}
        />
      </section>

      <section className="panel span-3 review-panel">
        <PanelTitle
          title="待复核"
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
            <strong>暂无待确认时间块</strong>
            <span>继续使用即可，活动切换后会自动生成。</span>
          </div>
        )}
      </section>

      {visiblePlanItems.length > 0 && (
        <section className="panel span-3">
          <PanelTitle title="计划线索" />
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
      {selectedCategory && (
        <CategoryDetailModal
          category={selectedCategory}
          sessions={sessions.filter((session) => session.category === selectedCategory)}
          selectedDate={selectedDate}
          onClose={() => setSelectedCategory(null)}
          onEdit={(session) => {
            setSelectedCategory(null);
            onEdit(session);
          }}
        />
      )}
    </div>
  );
}

function DayActivityTimeline({
  sessions,
  selectedDate,
  zoom,
  onZoomChange,
  onEdit,
}: {
  sessions: WorkSession[];
  selectedDate: string;
  zoom: number;
  onZoomChange: (zoom: number) => void;
  onEdit: (session: WorkSession) => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const centerRef = useRef<{ date: string; seconds: number } | null>(null);
  const wheelAtRef = useRef(0);
  const [viewport, setViewport] = useState({ left: 0, width: 0 });
  const [tooltip, setTooltip] = useState<{
    session: WorkSession;
    app: string;
    timeRange: string;
    x: number;
    y: number;
    placement: 'above' | 'below';
  } | null>(null);
  const scale = TIMELINE_SCALES[zoom] || TIMELINE_SCALES[DEFAULT_TIMELINE_ZOOM];
  const pixelsPerSecond = TIMELINE_GRID_WIDTH / scale.secondsPerGrid;
  const width = 24 * 60 * 60 * pixelsPerSecond;
  const sorted = useMemo(
    () => [...sessions].sort(
      (left, right) => new Date(left.startedAt).getTime() - new Date(right.startedAt).getTime(),
    ),
    [sessions],
  );
  const initialCenterSeconds = useMemo(() => {
    if (selectedDate === localDateKey(new Date())) {
      const now = new Date();
      return now.getHours() * 3600 + now.getMinutes() * 60 + now.getSeconds();
    }
    const latest = sorted[sorted.length - 1];
    if (!latest) return 12 * 60 * 60;
    const bounds = sessionBoundsOnDate(latest, selectedDate);
    return Math.min(24 * 60 * 60, bounds.startSeconds + bounds.durationSeconds / 2);
  }, [selectedDate, sorted]);

  const updateViewport = useCallback(() => {
    const element = scrollRef.current;
    if (!element) return;
    centerRef.current = {
      date: selectedDate,
      seconds: (element.scrollLeft + element.clientWidth / 2) / pixelsPerSecond,
    };
    setViewport({ left: element.scrollLeft, width: element.clientWidth });
  }, [pixelsPerSecond, selectedDate]);

  const showSessionTooltip = useCallback((
    element: HTMLButtonElement,
    session: WorkSession,
    app: string,
    timeRange: string,
  ) => {
    const bounds = element.getBoundingClientRect();
    const placement = bounds.top >= 140 ? 'above' : 'below';
    setTooltip({
      session,
      app,
      timeRange,
      x: Math.min(window.innerWidth - 144, Math.max(144, bounds.left + bounds.width / 2)),
      y: placement === 'above' ? bounds.top - 10 : bounds.bottom + 10,
      placement,
    });
  }, []);

  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const centerSeconds = centerRef.current?.date === selectedDate
      ? centerRef.current.seconds
      : initialCenterSeconds;
    const maximum = Math.max(0, width - element.clientWidth);
    element.scrollLeft = Math.min(
      maximum,
      Math.max(0, centerSeconds * pixelsPerSecond - element.clientWidth / 2),
    );
    updateViewport();
  }, [initialCenterSeconds, pixelsPerSecond, selectedDate, updateViewport, width]);

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const observer = new ResizeObserver(updateViewport);
    observer.observe(element);
    return () => observer.disconnect();
  }, [updateViewport]);

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const handleWheel = (event: WheelEvent) => {
      if (!event.ctrlKey) return;
      event.preventDefault();
      const now = performance.now();
      if (now - wheelAtRef.current < 80 || event.deltaY === 0) return;
      wheelAtRef.current = now;
      onZoomChange(Math.max(
        0,
        Math.min(TIMELINE_SCALES.length - 1, zoom + (event.deltaY < 0 ? 1 : -1)),
      ));
    };
    element.addEventListener('wheel', handleWheel, { passive: false });
    return () => element.removeEventListener('wheel', handleWheel);
  }, [onZoomChange, zoom]);

  const firstTick = Math.max(
    0,
    Math.floor((viewport.left / pixelsPerSecond) / scale.secondsPerGrid) - 2,
  );
  const lastTick = Math.min(
    Math.floor((24 * 60 * 60) / scale.secondsPerGrid),
    Math.ceil(((viewport.left + viewport.width) / pixelsPerSecond) / scale.secondsPerGrid) + 2,
  );
  const visibleTicks = Array.from(
    { length: Math.max(0, lastTick - firstTick + 1) },
    (_, index) => (firstTick + index) * scale.secondsPerGrid,
  );

  if (!sessions.length) {
    return <EmptyState title="暂无活动" detail="应用切换后会自动生成可修正的时间段。" />;
  }

  return (
    <>
      <div
        className="day-track-scroll"
        ref={scrollRef}
        onScroll={() => {
          updateViewport();
          setTooltip(null);
        }}
        title="拖动滚动条查看全天；按住 Ctrl 滚动鼠标滚轮缩放"
      >
        <div className="day-track" style={{ width }}>
          <div className="day-track-axis">
            {visibleTicks.map((second) => (
              <span
                className={second === 0 ? 'day-start' : second === 24 * 60 * 60 ? 'day-end' : undefined}
                key={second}
                style={{ left: second * pixelsPerSecond }}
              >
                {formatAxisTime(second, scale.secondsPerGrid < 60)}
              </span>
            ))}
          </div>
          <div
            className="day-track-lane"
            style={{ backgroundSize: `${TIMELINE_GRID_WIDTH}px 100%` }}
          >
            {sorted.map((session) => {
              const bounds = sessionBoundsOnDate(session, selectedDate);
              const app = sessionApplication(session);
              const blockWidth = Math.max(5, bounds.durationSeconds * pixelsPerSecond);
              const showSeconds = scale.secondsPerGrid < 60;
              const timeRange = `${formatTimelineClock(session.startedAt, showSeconds)}–${formatTimelineClock(session.endedAt, showSeconds)}`;
              return (
                <button
                  aria-label={`${timeRange}，${session.summary}，${app}，点击修正`}
                  className={`day-track-block${needsReview(session) ? ' needs-review' : ''}`}
                  key={session.id}
                  onBlur={() => setTooltip(null)}
                  onClick={() => {
                    setTooltip(null);
                    onEdit(session);
                  }}
                  onFocus={(event) => showSessionTooltip(event.currentTarget, session, app, timeRange)}
                  onMouseEnter={(event) => showSessionTooltip(event.currentTarget, session, app, timeRange)}
                  onMouseLeave={() => setTooltip(null)}
                  type="button"
                  style={
                    {
                      left: bounds.startSeconds * pixelsPerSecond,
                      width: blockWidth,
                      '--block-color': categoryColor(session.category),
                    } as CSSProperties
                  }
                />
              );
            })}
          </div>
        </div>
      </div>
      {tooltip && createPortal(
        <div
          className={`timeline-session-tooltip ${tooltip.placement}`}
          role="tooltip"
          style={{ left: tooltip.x, top: tooltip.y }}
        >
          <strong>{tooltip.session.summary}</strong>
          <span>{tooltip.timeRange}</span>
          <div>
            <i style={{ '--tooltip-color': categoryColor(tooltip.session.category) } as CSSProperties} />
            <b>{tooltip.session.category}</b>
            <em>{tooltip.session.projectName || '未归类项目'}{tooltip.session.taskTitle ? ` · ${tooltip.session.taskTitle}` : ''}</em>
          </div>
          <small>{tooltip.app}</small>
        </div>,
        document.body,
      )}
    </>
  );
}

function CategoryDetailModal({
  category,
  sessions,
  selectedDate,
  onClose,
  onEdit,
}: {
  category: string;
  sessions: WorkSession[];
  selectedDate: string;
  onClose: () => void;
  onEdit: (session: WorkSession) => void;
}) {
  const sorted = [...sessions].sort(
    (left, right) => new Date(left.startedAt).getTime() - new Date(right.startedAt).getTime(),
  );
  const total = sorted.reduce(
    (sum, session) => sum + sessionMinutesOnDate(session, selectedDate),
    0,
  );
  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <section className="modal category-detail" role="dialog" aria-modal="true" aria-label={`${category}时间段`}>
        <div className="modal-head">
          <div className="category-detail-title">
            <span style={{ background: categoryColor(category) }} />
            <div>
              <h2>{category}</h2>
              <p>{formatDuration(total)} · {sorted.length} 个时间段</p>
            </div>
          </div>
          <button className="icon-button" onClick={onClose} type="button" aria-label="关闭">
            <X size={17} />
          </button>
        </div>
        <div className="category-session-list">
          {sorted.map((session) => (
            <button key={session.id} onClick={() => onEdit(session)} type="button">
              <span className="category-session-time">
                <strong>{formatClock(session.startedAt)}</strong>
                <small>{formatClock(session.endedAt)}</small>
              </span>
              <span className="category-session-main">
                <strong>{session.summary}</strong>
                <small>{session.projectName || '未归类'}{session.taskTitle ? ` · ${session.taskTitle}` : ''}</small>
              </span>
              <span className="category-session-app">{sessionApplication(session)}</span>
              <Pencil size={15} />
            </button>
          ))}
        </div>
      </section>
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
  onEdit: (sessions: WorkSession[]) => void;
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
  const selectedSessions = sessions.filter((session) => selected.has(session.id));

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

  const editSession = (session: WorkSession) => {
    onEdit(selectedSessions.length > 1 ? selectedSessions : [session]);
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
            disabled={!selectedSessions.length}
            onClick={() => onEdit(selectedSessions)}
            type="button"
          >
            <Pencil size={15} />修正 {selectedSessions.length || ''}
          </button>
          <button
            disabled={selected.size < 2}
            onClick={() => void runAction(mergeSelected, '已合并所选会话')}
            type="button"
          >
            <Merge size={15} />合并 {selected.size || ''}
          </button>
        </div>

        <div className="timeline-list">
          {filtered.map((session, index) => {
            const newer = index > 0 ? filtered[index - 1] : null;
            const newerIndex = newer ? sessions.findIndex((item) => item.id === newer.id) : -1;
            const currentIndex = sessions.findIndex((item) => item.id === session.id);
            const hidden = newerIndex >= 0 && currentIndex > newerIndex + 1
              ? sessions.slice(newerIndex + 1, currentIndex)
              : [];
            const hiddenApps = [...new Set(hidden.map(sessionApplication))];
            const hiddenMinutes = hidden.reduce(
              (total, item) => total + minutesBetween(item.startedAt, item.endedAt),
              0,
            );
            return (
              <Fragment key={session.id}>
                {hidden.length > 0 && (
                  <div className="filtered-gap">
                    <span>中间切换至 {hiddenApps.slice(0, 2).join('、')}{hiddenApps.length > 2 ? ` 等 ${hiddenApps.length} 个应用` : ''}</span>
                    <b>{formatDuration(hiddenMinutes)}</b>
                  </div>
                )}
                <SessionRow
                  session={session}
                  selected={selected.has(session.id)}
                  onToggle={() => toggle(session.id)}
                  onEdit={() => editSession(session)}
                  onConfirm={() =>
                    void runAction(
                      () =>
                        api.updateSession(session.id, {
                          confidence: Math.max(0.96, session.confidence),
                          userConfirmed: true,
                        }),
                      '分类已确认',
                    )
                  }
                />
              </Fragment>
            );
          })}
          {!filtered.length && (
            <EmptyState
              title={reviewOnly ? '没有待复核记录' : '没有匹配的活动'}
              detail={reviewOnly ? '本地规则已经处理完这一天。' : '尝试清空搜索条件。'}
            />
          )}
        </div>
      </section>

    </div>
  );
}

function SessionRow({
  session,
  selected,
  onToggle,
  onEdit,
  onConfirm,
}: {
  session: WorkSession;
  selected: boolean;
  onToggle: () => void;
  onEdit: () => void;
  onConfirm: () => void;
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
      <div className="session-actions">
        {needsReview(session) && (
          <button className="confirm-button" onClick={onConfirm} type="button">
            <Check size={15} />确认
          </button>
        )}
        <button className="edit-button" onClick={onEdit} type="button">
          <Pencil size={15} />修正
        </button>
      </div>
    </article>
  );
}

function ProjectsView({
  projects,
  tasks,
  sessions,
  selectedDate,
  runAction,
  categoryOptions,
  focusProjectId,
}: {
  projects: Project[];
  tasks: Task[];
  sessions: WorkSession[];
  selectedDate: string;
  runAction: ActionRunner;
  categoryOptions: CategoryOption[];
  focusProjectId: string;
}) {
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState('');
  const [category, setCategory] = useState('开发');
  const [busyProjectId, setBusyProjectId] = useState('');
  const [busyTaskId, setBusyTaskId] = useState('');
  const [query, setQuery] = useState('');
  const [selectedProjectId, setSelectedProjectId] = useState(focusProjectId || projects[0]?.id || '');
  const [taskName, setTaskName] = useState('');
  const rows = projectBreakdown(sessions, selectedDate);
  const minutesByProject = new Map(rows.map((row) => [row.id, row.minutes]));
  const tasksByProject = groupBy(tasks, (task) => task.projectId);
  const needle = normalizeSearchText(query);
  const visibleProjects = projects.filter((project) => !needle || normalizeSearchText([
    project.name,
    project.category,
    project.description,
    ...(tasksByProject.get(project.id) || []).map((task) => task.title),
  ].filter(Boolean).join(' ')).includes(needle));
  const selectedProject = projects.find((project) => project.id === selectedProjectId) || visibleProjects[0] || projects[0];
  const selectedTasks = selectedProject ? tasksByProject.get(selectedProject.id) || [] : [];

  useEffect(() => {
    if (focusProjectId && projects.some((project) => project.id === focusProjectId)) {
      setSelectedProjectId(focusProjectId);
    }
  }, [focusProjectId, projects]);

  useEffect(() => {
    if (!selectedProjectId && projects[0]) setSelectedProjectId(projects[0].id);
    if (selectedProjectId && !projects.some((project) => project.id === selectedProjectId)) {
      setSelectedProjectId(projects[0]?.id || '');
    }
  }, [projects, selectedProjectId]);

  const createProject = async () => {
    const projectName = name.trim();
    if (!projectName) return;
    await runAction(() => api.createProject(projectName, category), `项目“${projectName}”已创建`);
    setName('');
    setCreating(false);
  };

  const deleteProject = async (project: Project) => {
    if (!window.confirm(`删除项目“${project.name}”？\n\n相关任务会一并删除，历史会话仍保留但会取消项目归属。`)) return;
    setBusyProjectId(project.id);
    try {
      await runAction(() => api.deleteProject(project.id), `项目“${project.name}”已删除`);
    } finally {
      setBusyProjectId('');
    }
  };

  const deleteTask = async (task: Task) => {
    if (!window.confirm(`删除任务“${task.title}”？历史会话会保留，但取消任务归属。`)) return;
    setBusyTaskId(task.id);
    try {
      await runAction(() => api.deleteTask(task.id), `任务“${task.title}”已删除`);
    } finally {
      setBusyTaskId('');
    }
  };

  const createTask = async () => {
    const title = taskName.trim();
    if (!selectedProject || !title) return;
    await runAction(
      () => api.createTask(selectedProject.id, title),
      `任务“${title}”已创建`,
    );
    setTaskName('');
  };

  return (
    <div className="projects-layout">
      <section className="panel">
        <PanelTitle
          title="项目账本"
          subtitle={`${projects.length} 个项目 · 点击卡片查看和管理任务`}
          action={(
            <button className="primary" onClick={() => setCreating((current) => !current)} type="button" aria-expanded={creating}>
              <Plus size={15} />新建项目
            </button>
          )}
        />
        <div className="project-search">
          <Search size={16} />
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索项目或任务" />
          {query && <button onClick={() => setQuery('')} type="button" aria-label="清空项目搜索"><X size={14} /></button>}
        </div>
        {creating && (
          <div className="project-create-row">
            <input
              value={name}
              onChange={(event) => setName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault();
                  void createProject();
                }
              }}
              placeholder="项目名称"
              autoFocus
            />
            <select value={category} onChange={(event) => setCategory(event.target.value)} aria-label="项目分类">
              {categoryOptions.filter((item) => item.name !== '离开').map((item) => <option key={item.name}>{item.name}</option>)}
            </select>
            <button className="primary" disabled={!name.trim()} onClick={() => void createProject()} type="button">
              <Check size={15} />创建
            </button>
          </div>
        )}
        <div className="project-grid">
          {visibleProjects.map((project) => {
            const minutes = minutesByProject.get(project.id) || 0;
            return (
              <article
                className={`project-card ${selectedProject?.id === project.id ? 'active' : ''}`}
                key={project.id}
                style={{ '--project-color': project.color || categoryColor(project.category) } as CSSProperties}
              >
                <div className="project-card-head">
                  <span>{project.category}</span>
                  <div>
                    <b>{formatDuration(minutes)}</b>
                    <button
                      className="project-delete"
                      disabled={busyProjectId === project.id}
                      onClick={() => void deleteProject(project)}
                      type="button"
                      aria-label={`删除项目 ${project.name}`}
                      title="删除项目"
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                </div>
                <button className="project-card-main" onClick={() => setSelectedProjectId(project.id)} type="button">
                  <span><strong>{project.name}</strong><small>{(tasksByProject.get(project.id) || []).length} 个任务</small></span>
                  <ChevronRight size={16} />
                </button>
                <div className="project-progress"><span style={{ width: `${Math.min(100, Math.max(4, (minutes / Math.max(1, rows[0]?.minutes || 1)) * 100))}%` }} /></div>
              </article>
            );
          })}
          {!visibleProjects.length && (
            <EmptyState title={projects.length ? '没有匹配项目' : '还没有项目'} detail={projects.length ? '换个项目名、分类或任务名试试。' : '打开一个代码工作区或修正一条会话即可自动建立。'} />
          )}
        </div>
      </section>

      <section className="panel project-detail-panel">
        <PanelTitle title={selectedProject?.name || '项目任务'} subtitle={selectedProject ? `${selectedProject.category} · ${formatDuration(minutesByProject.get(selectedProject.id) || 0)}` : '先新建一个项目'} />
        {selectedProject && (
          <div className="task-create-row">
            <input
              value={taskName}
              onChange={(event) => setTaskName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault();
                  void createTask();
                }
              }}
              placeholder="输入任务名，Enter 创建"
            />
            <button className="primary" onClick={() => void createTask()} disabled={!taskName.trim()} type="button"><Plus size={15} />添加</button>
          </div>
        )}
        <div className="task-list">
          {selectedProject && (
            <div className="task-group" key={selectedProject.id}>
              {selectedTasks.map((task) => (
                <div className="task-row" key={task.id}>
                  <TimerReset size={15} />
                  <span>{task.title}</span>
                  <button
                    className="task-delete"
                    disabled={busyTaskId === task.id}
                    onClick={() => void deleteTask(task)}
                    type="button"
                    aria-label={`删除任务 ${task.title}`}
                  >
                    <Trash2 size={13} />
                  </button>
                </div>
              ))}
              {!selectedTasks.length && <EmptyState title="还没有任务" detail="直接在上方输入任务名并按 Enter。" />}
            </div>
          )}
          {!selectedProject && <EmptyState title="还没有项目" detail="创建项目后即可添加任务。" />}
        </div>
      </section>
    </div>
  );
}

function SettingsView({
  data,
  runAction,
  onThemeChange,
}: {
  data: DashboardData;
  runAction: ActionRunner;
  onThemeChange: (theme: ThemeMode) => void;
}) {
  const [settings, setSettings] = useState<AppSettings>(data.settings);
  const [secret, setSecret] = useState('');

  useEffect(() => setSettings(data.settings), [data.settings]);

  const update = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => {
    setSettings((current) => ({ ...current, [key]: value }));
  };

  const updateTheme = (theme: ThemeMode) => {
    update('theme', theme);
    onThemeChange(theme);
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
      <section className="panel settings-panel appearance-panel">
        <PanelTitle
          title="外观"
          subtitle="主题会立即预览；保存后在下次启动时继续使用。"
        />
        <div className="theme-selector" role="radiogroup" aria-label="界面主题">
          {([
            { value: 'system', label: '跟随系统', detail: '自动适应 Windows', icon: Monitor },
            { value: 'light', label: '浅色', detail: '明亮、清晰', icon: Sun },
            { value: 'dark', label: '深色', detail: '弱光环境', icon: Moon },
          ] as const).map(({ value, label, detail, icon: Icon }) => (
            <button
              className={settings.theme === value ? 'active' : ''}
              key={value}
              onClick={() => updateTheme(value)}
              role="radio"
              aria-checked={settings.theme === value}
              type="button"
            >
              <Icon size={18} />
              <span>
                <strong>{label}</strong>
                <small>{detail}</small>
              </span>
              {settings.theme === value && <Check size={16} />}
            </button>
          ))}
        </div>
      </section>

      <GithubSyncPanel runAction={runAction} />

      <section className="panel settings-panel">
        <PanelTitle
          title="自动记录"
          subtitle="默认配置优先降低 CPU、写盘和打扰。"
        />
        <div className="field-grid">
          <Field label="观察与更新间隔" hint="每次只延长当前时间块；检测到稳定切换才新建一块。">
            <NumberInput
              value={settings.pollIntervalSeconds}
              min={1}
              max={60}
              suffix="秒"
              onChange={(value) => update('pollIntervalSeconds', value)}
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
          checked={settings.passiveContentCountsAsActive}
          onChange={(value) => update('passiveContentCountsAsActive', value)}
          title="视频播放不计为离开"
          detail="只在网页视频确认正在播放或前台为本地视频播放器时继续计时。"
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
        <PanelTitle title="日历线索" subtitle="可选导入日历计划，用于核对项目投入。" />
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

function GithubSyncPanel({ runAction }: { runAction: ActionRunner }) {
  const [status, setStatus] = useState<GithubSyncStatus | null>(null);
  const [config, setConfig] = useState<GithubSyncConfig | null>(null);
  const [token, setToken] = useState('');
  const [syncKey, setSyncKey] = useState('');
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    const next = await api.githubSyncStatus();
    setStatus(next);
    setConfig(next.config);
    return next;
  }, []);

  useEffect(() => {
    let active = true;
    void api.githubSyncStatus().then((next) => {
      if (!active) return;
      setStatus(next);
      setConfig(next.config);
    });
    return () => {
      active = false;
    };
  }, []);

  const update = <K extends keyof GithubSyncConfig>(key: K, value: GithubSyncConfig[K]) => {
    setConfig((current) => current ? { ...current, [key]: value } : current);
  };

  const save = async () => {
    if (!config || busy) return;
    setBusy(true);
    try {
      const next = await runAction(
        () => api.saveGithubSyncConfig(config, token.trim() || undefined, syncKey.trim() || undefined),
        '同步设置已保存',
      ) as GithubSyncStatus;
      setStatus(next);
      setConfig(next.config);
      setToken('');
      setSyncKey('');
    } catch {
      // runAction already surfaces the backend message.
    } finally {
      setBusy(false);
    }
  };

  const generateKey = async () => {
    if (busy) return;
    setBusy(true);
    try {
      const key = await runAction(api.generateGithubSyncKey, '已生成端侧加密密钥') as string;
      setSyncKey(key);
      await refresh();
    } catch {
      // The toast is enough; keep the form intact for correction.
    } finally {
      setBusy(false);
    }
  };

  const revealKey = async () => {
    if (busy) return;
    setBusy(true);
    try {
      const key = await api.readGithubSyncKey();
      setSyncKey(key);
    } catch {
      try {
        const key = await runAction(api.generateGithubSyncKey, '已生成端侧加密密钥') as string;
        setSyncKey(key);
        await refresh();
      } catch {
        // The toast contains the actionable backend error.
      }
    } finally {
      setBusy(false);
    }
  };

  const copyKey = async () => {
    if (!syncKey) return;
    try {
      await navigator.clipboard.writeText(syncKey);
      await runAction(async () => undefined, '同步密钥已复制');
    } catch {
      const input = document.getElementById('github-sync-key') as HTMLInputElement | null;
      input?.focus();
      input?.select();
    }
  };

  const syncNow = async () => {
    if (busy) return;
    setBusy(true);
    try {
      const result = await runAction(api.syncGithubNow, 'GitHub 同步完成') as GithubSyncResult;
      const next = await refresh();
      setStatus({ ...next, counts: result.counts });
    } catch {
      await refresh().catch(() => undefined);
    } finally {
      setBusy(false);
    }
  };

  const disconnect = async () => {
    if (busy) return;
    setBusy(true);
    try {
      const next = await runAction(
        () => api.disconnectGithubSync(false),
        '已停止自动同步，凭据仍保存在系统凭据库',
      ) as GithubSyncStatus;
      setStatus(next);
      setConfig(next.config);
    } catch {
      // runAction already reported the error.
    } finally {
      setBusy(false);
    }
  };

  if (!config || !status) {
    return (
      <section className="panel settings-panel sync-panel sync-panel-loading">
        <Cloud size={20} />正在读取同步状态…
      </section>
    );
  }

  const repoUrl = config.owner && config.repo
    ? `https://github.com/${config.owner}/${config.repo}`
    : '';
  const recordCount = status.counts.categories + status.counts.projects
    + status.counts.tasks + status.counts.sessions + status.counts.rules;

  return (
    <section className="panel settings-panel sync-panel">
      <div className="sync-panel-head">
        <PanelTitle
          title="GitHub 多端同步"
          subtitle="独立 Private 仓库 · AES-256-GCM 加密 · 自动合并最新修改"
        />
        <span className={`sync-state ${status.ready ? 'ready' : ''}`}>
          <span />{status.ready ? '可以同步' : config.enabled ? '还差凭据' : '未开启'}
        </span>
      </div>

      <div className="sync-overview">
        <div>
          <Github size={18} />
          <span><strong>{config.owner && config.repo ? `${config.owner}/${config.repo}` : '尚未设置仓库'}</strong><small>仓库不存在时首次同步自动创建为 Private</small></span>
        </div>
        <div>
          <ShieldCheck size={18} />
          <span><strong>{status.keyConfigured ? '端侧加密已就绪' : '需要同步密钥'}</strong><small>GitHub 只保存压缩后的密文</small></span>
        </div>
        <div>
          <Cloud size={18} />
          <span><strong>{config.lastSyncedAt ? formatDateTime(config.lastSyncedAt) : '尚未同步'}</strong><small>{recordCount.toLocaleString()} 条结构化记录待同步</small></span>
        </div>
      </div>

      {config.lastError && (
        <div className="sync-error"><CircleAlert size={16} /><span>{config.lastError}</span></div>
      )}

      <div className="sync-columns">
        <div className="sync-form-group">
          <h3>仓库</h3>
          <div className="field-grid sync-fields">
            <Field label="GitHub 用户名">
              <input value={config.owner} onChange={(event) => update('owner', event.target.value)} placeholder="ShallowForeverDream" />
            </Field>
            <Field label="Private 仓库名">
              <input value={config.repo} onChange={(event) => update('repo', event.target.value)} placeholder="ScreenUse-Data" />
            </Field>
            <Field label="分支">
              <input value={config.branch} onChange={(event) => update('branch', event.target.value)} placeholder="main" />
            </Field>
            <Field label="设备名称">
              <input value={config.deviceName} onChange={(event) => update('deviceName', event.target.value)} placeholder="我的电脑" />
            </Field>
          </div>
          <Toggle
            checked={config.enabled}
            onChange={(value) => update('enabled', value)}
            title="启用 GitHub 同步"
            detail="开启后可手动同步；保存配置不会立即上传。"
          />
          <Toggle
            checked={config.autoSync}
            onChange={(value) => update('autoSync', value)}
            title="后台自动同步"
            detail={`每 ${config.intervalMinutes} 分钟检查一次远端更新。`}
          />
          {config.autoSync && (
            <Field label="自动同步间隔" hint="最短 5 分钟。">
              <NumberInput
                value={config.intervalMinutes}
                min={5}
                max={1440}
                suffix="分钟"
                onChange={(value) => update('intervalMinutes', value)}
              />
            </Field>
          )}
        </div>

        <div className="sync-form-group">
          <h3>凭据与密钥</h3>
          <Field label="GitHub Token" hint={status.tokenConfigured ? '已保存；留空不会覆盖。需要 repo 权限。' : '需要 repo 权限，用于读写 Private 仓库。'}>
            <input
              type="password"
              value={token}
              onChange={(event) => setToken(event.target.value)}
              placeholder={status.tokenConfigured ? '已安全保存' : 'github_pat_…'}
            />
          </Field>
          <Field label="同步密钥" hint="所有设备必须使用同一把密钥；密钥不会上传 GitHub。">
            <div className="sync-key-row">
              <input
                id="github-sync-key"
                type={syncKey ? 'text' : 'password'}
                value={syncKey}
                onChange={(event) => setSyncKey(event.target.value)}
                placeholder={status.keyConfigured ? '密钥已保存在系统凭据库' : '生成新密钥或粘贴已有密钥'}
              />
              <button onClick={() => void (syncKey ? copyKey() : revealKey())} type="button" aria-label={syncKey ? '复制同步密钥' : '显示同步密钥'}>
                {syncKey ? <Copy size={16} /> : <KeyRound size={16} />}
              </button>
            </div>
          </Field>
          <div className="sync-key-actions">
            <button onClick={() => void generateKey()} disabled={busy} type="button">
              <KeyRound size={15} />生成新密钥
            </button>
            {status.keyConfigured && !syncKey && (
              <button onClick={() => void revealKey()} disabled={busy} type="button">
                <Copy size={15} />复制到其他设备
              </button>
            )}
          </div>
          <div className="setting-callout good compact">
            <ShieldCheck size={18} />
            <div><strong>Token 和密钥只进系统凭据库</strong><span>数据库与界面状态中都不保存明文。</span></div>
          </div>
        </div>
      </div>

      <div className="sync-bottom">
        <div className="sync-devices">
          <Laptop size={17} />
          <div>
            <strong>{status.devices.length ? `${status.devices.length} 台设备` : '当前设备尚未登记'}</strong>
            <span>{status.devices.slice(0, 3).map((device) => device.name).join(' · ') || config.deviceName}</span>
          </div>
        </div>
        <div className="sync-actions">
          {repoUrl && config.lastSyncedAt && (
            <button onClick={() => window.open(repoUrl, '_blank')} type="button"><Github size={15} />查看仓库</button>
          )}
          {config.enabled && <button onClick={() => void disconnect()} disabled={busy} type="button">停止同步</button>}
          <button onClick={() => void save()} disabled={busy} type="button">保存同步设置</button>
          <button className="primary" onClick={() => void syncNow()} disabled={busy || !status.ready} type="button">
            <Cloud size={16} />{busy ? '处理中…' : '立即同步'}
          </button>
        </div>
      </div>
    </section>
  );
}

interface SearchSelectOption {
  value: string;
  label: string;
  meta?: string;
  keywords?: string[];
  color?: string;
}

function normalizeSearchText(value: string) {
  return value.trim().toLocaleLowerCase('zh-CN');
}

function SearchCreateSelect({
  value,
  options,
  inputLabel,
  placeholder,
  emptyText = '没有匹配项',
  busy = false,
  canCreate,
  createLabel,
  onChange,
  onCreate,
}: {
  value: string;
  options: SearchSelectOption[];
  inputLabel: string;
  placeholder: string;
  emptyText?: string;
  busy?: boolean;
  canCreate?: (query: string) => boolean;
  createLabel?: (query: string) => string;
  onChange: (value: string) => void;
  onCreate?: (query: string) => Promise<void>;
}) {
  const listboxId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const selected = options.find((option) => option.value === value);
  const normalizedQuery = normalizeSearchText(query);
  const trimmedQuery = query.trim();
  const filtered = useMemo(() => {
    if (!normalizedQuery) return options.slice(0, 80);
    return options
      .filter((option) => normalizeSearchText([
        option.label,
        option.meta,
        ...(option.keywords || []),
      ].filter(Boolean).join(' ')).includes(normalizedQuery))
      .slice(0, 80);
  }, [normalizedQuery, options]);
  const showCreate = Boolean(
    trimmedQuery
      && onCreate
      && (canCreate ? canCreate(trimmedQuery) : true),
  );
  const itemCount = filtered.length + (showCreate ? 1 : 0);

  useEffect(() => {
    if (!open) setQuery(selected?.label || '');
  }, [open, selected?.label]);

  useEffect(() => {
    setActiveIndex((current) => Math.min(current, Math.max(0, itemCount - 1)));
  }, [itemCount]);

  const close = () => {
    setOpen(false);
    setQuery(selected?.label || '');
  };

  const selectOption = (option: SearchSelectOption) => {
    onChange(option.value);
    setQuery(option.label);
    setOpen(false);
  };

  const createCurrent = async () => {
    if (!showCreate || !onCreate || busy) return;
    await onCreate(trimmedQuery);
    setOpen(false);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      event.stopPropagation();
      if (!open) setOpen(true);
      if (itemCount) {
        setActiveIndex((current) => (
          event.key === 'ArrowDown'
            ? (current + 1) % itemCount
            : (current - 1 + itemCount) % itemCount
        ));
      }
      return;
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      event.stopPropagation();
      if (!open) {
        setOpen(true);
        return;
      }
      const option = filtered[activeIndex];
      if (option) {
        selectOption(option);
      } else if (showCreate && activeIndex === filtered.length) {
        void createCurrent();
      }
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      close();
    }
  };

  return (
    <div
      className={`search-create-select${open ? ' open' : ''}`}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) close();
      }}
    >
      <div className="search-create-input">
        <Search size={14} />
        <input
          ref={inputRef}
          aria-activedescendant={open && itemCount ? `${listboxId}-option-${activeIndex}` : undefined}
          aria-autocomplete="list"
          aria-controls={listboxId}
          aria-expanded={open}
          aria-label={inputLabel}
          autoComplete="off"
          disabled={busy}
          onChange={(event) => {
            setQuery(event.target.value);
            setActiveIndex(0);
            setOpen(true);
          }}
          onClick={() => {
            if (!open) {
              setQuery('');
              setActiveIndex(0);
              setOpen(true);
            }
          }}
          onFocus={() => {
            if (!open) {
              setQuery('');
              setActiveIndex(0);
              setOpen(true);
            }
          }}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          role="combobox"
          value={open ? query : selected?.label || ''}
        />
        <button
          aria-label={`${open ? '收起' : '展开'}${inputLabel}`}
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => {
            if (open) {
              close();
            } else {
              setQuery('');
              setActiveIndex(0);
              setOpen(true);
              inputRef.current?.focus();
            }
          }}
          type="button"
        >
          <ChevronDown size={15} />
        </button>
      </div>
      {open && (
        <div className="search-create-menu" id={listboxId} role="listbox">
          {filtered.map((option, index) => (
            <button
              aria-selected={option.value === value}
              className={`${index === activeIndex ? 'active' : ''}${option.value === value ? ' selected' : ''}`}
              id={`${listboxId}-option-${index}`}
              key={option.value}
              onClick={() => selectOption(option)}
              onMouseDown={(event) => event.preventDefault()}
              onMouseEnter={() => setActiveIndex(index)}
              role="option"
              type="button"
            >
              <i
                aria-hidden="true"
                className={option.color ? undefined : 'empty'}
                style={option.color ? { '--option-color': option.color } as CSSProperties : undefined}
              />
              <span>
                <strong>{option.label}</strong>
                {option.meta && <small>{option.meta}</small>}
              </span>
              {option.value === value && <Check size={14} />}
            </button>
          ))}
          {showCreate && (
            <button
              aria-selected={false}
              className={`create-option${activeIndex === filtered.length ? ' active' : ''}`}
              id={`${listboxId}-option-${filtered.length}`}
              onClick={() => void createCurrent()}
              onMouseDown={(event) => event.preventDefault()}
              onMouseEnter={() => setActiveIndex(filtered.length)}
              role="option"
              type="button"
            >
              <Plus size={14} />
              <span>
                <strong>{createLabel ? createLabel(trimmedQuery) : `新建“${trimmedQuery}”`}</strong>
                <small>按 Enter 直接创建</small>
              </span>
            </button>
          )}
          {!filtered.length && !showCreate && <p>{emptyText}</p>}
        </div>
      )}
    </div>
  );
}

function EditSessionModal({
  sessions,
  projects,
  tasks,
  categoryOptions,
  onClose,
  onSave,
  runAction,
}: {
  sessions: WorkSession[];
  projects: Project[];
  tasks: Task[];
  categoryOptions: CategoryOption[];
  onClose: () => void;
  onSave: (
    sessions: WorkSession[],
    patch: SessionPatch,
    options: { remember: boolean; keyword?: string; pin: boolean },
  ) => Promise<void>;
  runAction: ActionRunner;
}) {
  const session = sessions[0];
  const isBulk = sessions.length > 1;
  const sharedValue = <T,>(read: (item: WorkSession) => T) => {
    const first = read(session);
    return sessions.every((item) => read(item) === first) ? first : undefined;
  };
  const [summary, setSummary] = useState(session.summary);
  const [category, setCategory] = useState(sharedValue((item) => item.category) || '');
  const [projectId, setProjectId] = useState(sharedValue((item) => item.projectId || '') || '');
  const [taskId, setTaskId] = useState(sharedValue((item) => item.taskId || '') || '');
  const [categoryTouched, setCategoryTouched] = useState(false);
  const [projectTouched, setProjectTouched] = useState(false);
  const [taskTouched, setTaskTouched] = useState(false);
  const [projectOptions, setProjectOptions] = useState(projects);
  const [taskOptions, setTaskOptions] = useState(tasks);
  const [localCategories, setLocalCategories] = useState(categoryOptions);
  const [projectBusy, setProjectBusy] = useState(false);
  const [taskBusy, setTaskBusy] = useState(false);
  const [categoryBusy, setCategoryBusy] = useState(false);
  const [remember, setRemember] = useState(false);
  const [keyword, setKeyword] = useState('');
  const [pin, setPin] = useState(false);
  const categorySearchOptions = useMemo<SearchSelectOption[]>(
    () => localCategories.map((item) => ({
      value: item.name,
      label: item.name,
      color: item.color,
      meta: item.isBuiltin ? '内置分类' : '自定义分类',
    })),
    [localCategories],
  );
  const projectSearchOptions = useMemo<SearchSelectOption[]>(
    () => [
      { value: '', label: '暂不指定', meta: '不关联项目' },
      ...[...projectOptions]
        .filter((project) => !category || project.category === category)
        .sort((left, right) => left.name.localeCompare(right.name, 'zh-CN'))
        .map((project) => ({
          value: project.id,
          label: project.name,
          meta: project.category,
          color: project.color || categoryColor(project.category),
          keywords: [project.category],
        })),
    ],
    [category, projectOptions],
  );
  const taskSearchOptions = useMemo<SearchSelectOption[]>(
    () => [
      { value: '', label: '暂不指定', meta: '不关联任务' },
      ...[...taskOptions]
        .filter((task) => {
          if (projectId) return task.projectId === projectId;
          if (!category) return true;
          return projectOptions.find((project) => project.id === task.projectId)?.category === category;
        })
        .sort((left, right) => left.title.localeCompare(right.title, 'zh-CN'))
        .map((task) => {
          const project = projectOptions.find((item) => item.id === task.projectId);
          return {
            value: task.id,
            label: task.title,
            meta: project ? `${project.name} · ${project.category}` : '项目已删除',
            color: project?.color || categoryColor(project?.category || '杂务'),
            keywords: project ? [project.name, project.category] : [],
          };
        }),
    ],
    [category, projectId, projectOptions, taskOptions],
  );
  const selectedProject = projectOptions.find((project) => project.id === projectId);
  const projectPlaceholder = category ? `搜索“${category}”中的项目` : '搜索全部项目';
  const taskPlaceholder = selectedProject
    ? `搜索“${selectedProject.name}”中的任务`
    : category
      ? `搜索“${category}”项目中的任务`
      : '搜索全部项目中的任务';

  useEffect(() => setProjectOptions(projects), [projects]);
  useEffect(() => setTaskOptions(tasks), [tasks]);
  useEffect(() => setLocalCategories(categoryOptions), [categoryOptions]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [onClose]);

  const selectCategory = (name: string) => {
    setCategory(name);
    setCategoryTouched(true);
    const selectedProject = projectOptions.find((project) => project.id === projectId);
    if (selectedProject && selectedProject.category !== name) {
      setProjectId('');
      setTaskId('');
      setProjectTouched(true);
      setTaskTouched(true);
    }
  };

  const selectProject = (id: string) => {
    setProjectId(id);
    setTaskId('');
    setProjectTouched(true);
    setTaskTouched(true);
    const selectedProject = projectOptions.find((project) => project.id === id);
    if (selectedProject) {
      setCategory(selectedProject.category);
      setCategoryTouched(true);
    }
  };

  const selectTask = (id: string) => {
    setTaskId(id);
    setTaskTouched(true);
    const selectedTask = taskOptions.find((task) => task.id === id);
    if (!selectedTask) return;
    setProjectId(selectedTask.projectId);
    setProjectTouched(true);
    const selectedProject = projectOptions.find((project) => project.id === selectedTask.projectId);
    if (selectedProject) {
      setCategory(selectedProject.category);
      setCategoryTouched(true);
    }
  };

  const createProject = async (rawName: string) => {
    const name = rawName.trim();
    if (!name || projectBusy) return;
    setProjectBusy(true);
    try {
      const created = (await runAction(
        () => api.createProject(name, category),
        `项目“${name}”已创建`,
      )) as Project;
      setProjectOptions((current) => [created, ...current.filter((item) => item.id !== created.id)]);
      setProjectId(created.id);
      setTaskId('');
      setProjectTouched(true);
      setTaskTouched(true);
    } catch {
      // runAction already reports the error in the app toast.
    } finally {
      setProjectBusy(false);
    }
  };

  const deleteSelectedProject = async () => {
    const project = projectOptions.find((item) => item.id === projectId);
    if (!project || projectBusy) return;
    if (!window.confirm(`删除项目“${project.name}”？\n\n相关任务会一并删除，历史会话会保留但取消项目归属。`)) {
      return;
    }
    setProjectBusy(true);
    try {
      await runAction(() => api.deleteProject(project.id), `项目“${project.name}”已删除`);
      setProjectOptions((current) => current.filter((item) => item.id !== project.id));
      setTaskOptions((current) => current.filter((item) => item.projectId !== project.id));
      setProjectId('');
      setTaskId('');
      setProjectTouched(true);
      setTaskTouched(true);
    } catch {
      // runAction already reports the error in the app toast.
    } finally {
      setProjectBusy(false);
    }
  };

  const createCategory = async (rawName: string) => {
    const name = rawName.trim();
    if (!name || categoryBusy) return;
    setCategoryBusy(true);
    try {
      const created = (await runAction(() => api.createCategory(name), `分类“${name}”已创建`)) as CategoryOption;
      setLocalCategories((current) => [...current, created]);
      selectCategory(created.name);
    } catch {
      // runAction already reports the error in the app toast.
    } finally {
      setCategoryBusy(false);
    }
  };

  const deleteSelectedCategory = async () => {
    const selected = localCategories.find((item) => item.name === category);
    if (!selected || selected.isBuiltin || categoryBusy) return;
    if (!window.confirm(`删除分类“${selected.name}”？使用它的项目和会话会改为“杂务”。`)) return;
    setCategoryBusy(true);
    try {
      await runAction(() => api.deleteCategory(selected.name), `分类“${selected.name}”已删除`);
      setLocalCategories((current) => current.filter((item) => item.name !== selected.name));
      setProjectOptions((current) => current.map((project) => (
        project.category === selected.name ? { ...project, category: '杂务' } : project
      )));
      setCategory('杂务');
      setCategoryTouched(true);
      if (projectId && projectOptions.find((project) => project.id === projectId)?.category === selected.name) {
        setProjectId('');
        setTaskId('');
        setProjectTouched(true);
        setTaskTouched(true);
      }
    } finally {
      setCategoryBusy(false);
    }
  };

  const createTask = async (rawTitle: string) => {
    const title = rawTitle.trim();
    if (!projectId || !title || taskBusy) return;
    setTaskBusy(true);
    try {
      const created = (await runAction(() => api.createTask(projectId, title), `任务“${title}”已创建`)) as Task;
      setTaskOptions((current) => [created, ...current]);
      setTaskId(created.id);
      setTaskTouched(true);
    } catch {
      // runAction already reports the error in the app toast.
    } finally {
      setTaskBusy(false);
    }
  };

  const deleteSelectedTask = async () => {
    const task = taskOptions.find((item) => item.id === taskId);
    if (!task || taskBusy) return;
    if (!window.confirm(`删除任务“${task.title}”？历史会话会保留，但取消任务归属。`)) return;
    setTaskBusy(true);
    try {
      await runAction(() => api.deleteTask(task.id), `任务“${task.title}”已删除`);
      setTaskOptions((current) => current.filter((item) => item.id !== task.id));
      setTaskId('');
      setTaskTouched(true);
    } finally {
      setTaskBusy(false);
    }
  };

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
      <section
        className="modal"
        onMouseDown={(event) => event.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="edit-session-title"
      >
        <div className="modal-head">
          <div>
            <h2 id="edit-session-title">{isBulk ? '批量修正' : '修正会话'}</h2>
            <p>
              {isBulk
                ? `${sessions.length} 条会话 · ${formatDuration(sessions.reduce(
                  (total, item) => total + minutesBetween(item.startedAt, item.endedAt),
                  0,
                ))}`
                : `${formatDateTime(session.startedAt)} · ${formatDuration(minutesBetween(session.startedAt, session.endedAt))}`}
            </p>
          </div>
          <button className="icon-button" onClick={onClose} type="button" aria-label="关闭">
            <X size={18} />
          </button>
        </div>

        {!isBulk && (
          <Field label="摘要">
            <input value={summary} onChange={(event) => setSummary(event.target.value)} autoFocus />
          </Field>
        )}
        <div className="field-grid">
          <Field label="分类">
            <div className="project-picker">
              <SearchCreateSelect
                busy={categoryBusy}
                canCreate={(query) => !localCategories.some(
                  (item) => normalizeSearchText(item.name) === normalizeSearchText(query),
                )}
                createLabel={(query) => `新建分类“${query}”`}
                inputLabel="搜索或新建分类"
                onChange={selectCategory}
                onCreate={createCategory}
                options={categorySearchOptions}
                placeholder="输入分类名称"
                value={category}
              />
              <div className="project-picker-actions single">
                <button
                  className="danger-button"
                  disabled={localCategories.find((item) => item.name === category)?.isBuiltin !== false || categoryBusy}
                  onClick={() => void deleteSelectedCategory()}
                  type="button"
                >
                  <Trash2 size={15} />删除
                </button>
              </div>
            </div>
          </Field>
          <Field label="项目">
            <div className="project-picker">
              <SearchCreateSelect
                busy={projectBusy}
                canCreate={(query) => Boolean(category) && !projectOptions.some(
                  (project) => project.category === category
                    && normalizeSearchText(project.name) === normalizeSearchText(query),
                )}
                createLabel={(query) => `在“${category}”中新建“${query}”`}
                inputLabel="搜索或新建项目"
                onChange={selectProject}
                onCreate={createProject}
                options={projectSearchOptions}
                placeholder={projectPlaceholder}
                value={projectId}
              />
              <div className="project-picker-actions single">
                <button
                  className="danger-button"
                  disabled={!projectId || projectBusy}
                  onClick={() => void deleteSelectedProject()}
                  type="button"
                >
                  <Trash2 size={15} />删除
                </button>
              </div>
            </div>
          </Field>
          <Field label="任务">
            <div className="project-picker">
              <SearchCreateSelect
                busy={taskBusy}
                canCreate={(query) => Boolean(projectId) && !taskOptions.some(
                  (task) => task.projectId === projectId
                    && normalizeSearchText(task.title) === normalizeSearchText(query),
                )}
                createLabel={(query) => {
                  const project = projectOptions.find((item) => item.id === projectId);
                  return `在“${project?.name || '当前项目'}”中新建“${query}”`;
                }}
                emptyText={projectId ? '没有匹配任务' : '没有匹配任务；选择已有任务会自动带出项目和分类'}
                inputLabel="搜索或新建任务"
                onChange={selectTask}
                onCreate={createTask}
                options={taskSearchOptions}
                placeholder={taskPlaceholder}
                value={taskId}
              />
              <div className="project-picker-actions single">
                <button className="danger-button" disabled={!taskId || taskBusy} onClick={() => void deleteSelectedTask()} type="button">
                  <Trash2 size={15} />删除
                </button>
              </div>
            </div>
          </Field>
        </div>

        <div className="correction-options">
          <label>
            <input type="checkbox" checked={remember} onChange={(event) => setRemember(event.target.checked)} />
            <span><strong>记住规则</strong><small>以后按上下文识别，不按应用名硬归类</small></span>
          </label>
          <label className={!projectId ? 'disabled' : ''}>
            <input type="checkbox" checked={pin} disabled={!projectId} onChange={(event) => setPin(event.target.checked)} />
            <span><strong>固定 30 分钟</strong><small>适合 ChatGPT、终端等上下文不明确的应用</small></span>
          </label>
          {remember && (
            <input
              value={keyword}
              onChange={(event) => setKeyword(event.target.value)}
              placeholder="识别词：ICPC、icpc-trainer、网站开发"
            />
          )}
        </div>

        {!isBulk && (
          <>
            <details className="modal-evidence">
              <summary>判断依据</summary>
              {session.evidence.length ? (
                session.evidence.map((item, index) => (
                  <span key={`${item.kind}-${index}`}>
                    <b>{item.label}</b>{item.value}
                  </span>
                ))
              ) : (
                <span>没有附加元数据</span>
              )}
            </details>

            <div className="modal-secondary">
              <button onClick={() => void runAction(split, '会话已拆分')} type="button">
                <SplitSquareHorizontal size={16} />拆分
              </button>
            </div>
          </>
        )}

        <div className="modal-actions">
          <button onClick={onClose} type="button">取消</button>
          <button
            className="primary"
            disabled={isBulk && !categoryTouched && !projectTouched && !taskTouched}
            onClick={() => {
              const patch: SessionPatch = isBulk
                ? {
                    ...(categoryTouched ? { category } : {}),
                    ...(projectTouched
                      ? { projectId: projectId || undefined, clearProject: !projectId }
                      : {}),
                    ...(taskTouched ? { taskId: taskId || undefined, clearTask: !taskId } : {}),
                    userConfirmed: true,
                  }
                : {
                    summary: summary.trim() || session.summary,
                    projectId: projectId || undefined,
                    taskId: taskId || undefined,
                    clearProject: !projectId,
                    clearTask: !taskId,
                    category,
                    confidence: Math.max(0.96, session.confidence),
                    userConfirmed: true,
                  };
              void onSave(sessions, patch, { remember, keyword: keyword.trim() || undefined, pin });
            }}
            type="button"
          >
            <Check size={17} />{isBulk ? `统一修正 ${sessions.length} 条` : '保存并确认'}
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
    <div className={`kpi ${attention ? 'attention' : ''}`} title={hint}>
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
    (session.source === 'context-complete' || !session.projectId || session.confidence < 0.72)
  );
}

function sessionMinutesOnDate(session: WorkSession, dateKey: string) {
  const dayStart = dateFromKey(dateKey).getTime();
  const dayEnd = dayStart + 24 * 60 * 60 * 1000;
  const start = Math.max(dayStart, new Date(session.startedAt).getTime());
  const end = Math.min(dayEnd, new Date(session.endedAt).getTime());
  return Math.max(0, Math.round((end - start) / 60_000));
}

function sessionBoundsOnDate(session: WorkSession, dateKey: string) {
  const dayStart = dateFromKey(dateKey).getTime();
  const dayEnd = dayStart + 24 * 60 * 60 * 1000;
  const start = Math.max(dayStart, new Date(session.startedAt).getTime());
  const end = Math.min(dayEnd, new Date(session.endedAt).getTime());
  return {
    startSeconds: Math.max(0, (start - dayStart) / 1000),
    durationSeconds: Math.max(0, (end - start) / 1000),
  };
}

function sessionApplication(session: WorkSession) {
  return (
    session.evidence.find((item) => item.kind === 'app' || item.label === '应用')?.value ||
    '未知应用'
  );
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

function formatTimelineClock(value: string, showSeconds: boolean) {
  return new Date(value).toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    second: showSeconds ? '2-digit' : undefined,
  });
}

function formatAxisTime(totalSeconds: number, showSeconds: boolean) {
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = Math.floor(totalSeconds % 60);
  return showSeconds
    ? `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`
    : `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}`;
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
  if (categoryColors[category]) return categoryColors[category];
  const palette = ['#8b5cf6', '#ec4899', '#06b6d4', '#14b8a6', '#f97316', '#6366f1', '#84cc16', '#d946ef'];
  const hash = [...category].reduce((value, character) => (value * 31 + character.charCodeAt(0)) >>> 0, 0);
  return palette[hash % palette.length];
}

function sourceLabel(source: string) {
  const labels: Record<string, string> = {
    'workspace-auto': '工作区自动发现',
    'metadata-auto': '元数据自动发现',
    manual: '手动创建',
    seed: '示例',
    'ai-review': 'AI 复核',
    'context-complete': '切换后待确认',
  };
  return labels[source] || source;
}

function pageDescription(tab: TabId) {
  const descriptions: Record<TabId, string> = {
    today: '今天的时间去向',
    timeline: '查看并修正记录',
    projects: '项目与任务投入',
    settings: '记录、主题与数据',
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
