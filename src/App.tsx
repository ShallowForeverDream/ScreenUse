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
  Ellipsis,
  EyeOff,
  FolderKanban,
  HardDrive,
  Github,
  KeyRound,
  Laptop,
  LayoutDashboard,
  ListFilter,
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
  Undo2,
  WandSparkles,
  X,
  ZoomIn,
  ZoomOut,
} from 'lucide-react';
import { api } from './api';
import type {
  AnalysisJob,
  AppSettings,
  CategoryOption,
  CodexRateCard,
  DashboardData,
  GithubSyncConfig,
  GithubSyncResult,
  GithubSyncStatus,
  Project,
  SessionPatch,
  Task,
  ThemeMode,
  UndoStatus,
  WorkSession,
} from './types';

const tabs = [
  { id: 'today', label: '今日', icon: LayoutDashboard },
  { id: 'timeline', label: '时间轴', icon: Activity },
  { id: 'projects', label: '项目', icon: FolderKanban },
  { id: 'ai', label: 'AI复核', icon: WandSparkles },
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
  无效: '#94a3b8',
  离开: '#94a3b8',
  未记录: '#cbd5e1',
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
const DEFAULT_REVIEW_CONFIDENCE_THRESHOLD = 0.8;
const TASK_CHART_COLORS = ['#8b5cf6', '#60a5fa', '#ec4899', '#22c55e', '#f59e0b', '#06b6d4', '#f97316', '#a3e635'];
const MAX_TIMELINE_GAP_SNAP_SECONDS = 10;

type ProjectRange = 'today' | 'week' | 'month' | 'quarter' | 'year' | 'all' | 'custom';
type ProjectRangePreset = Exclude<ProjectRange, 'custom'>;

const PROJECT_RANGE_OPTIONS: { id: ProjectRangePreset; label: string }[] = [
  { id: 'today', label: '今天' },
  { id: 'week', label: '本周' },
  { id: 'month', label: '本月' },
  { id: 'quarter', label: '本季度' },
  { id: 'year', label: '本年' },
  { id: 'all', label: '全部' },
];

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

interface ConfirmationOptions {
  title: string;
  detail: string;
  confirmLabel?: string;
}

interface PendingConfirmation extends ConfirmationOptions {
  resolve: (accepted: boolean) => void;
}

function ConfirmationDialog({
  request,
  onChoose,
}: {
  request: PendingConfirmation;
  onChoose: (accepted: boolean) => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    cancelRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopImmediatePropagation();
      onChoose(false);
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [onChoose]);

  return createPortal(
    <div
      className="confirm-backdrop"
      onMouseDown={(event) => {
        event.stopPropagation();
        onChoose(false);
      }}
      role="presentation"
    >
      <section
        aria-describedby="confirm-dialog-detail"
        aria-labelledby="confirm-dialog-title"
        aria-modal="true"
        className="confirm-dialog"
        data-confirm-dialog="true"
        onMouseDown={(event) => event.stopPropagation()}
        role="alertdialog"
      >
        <div className="confirm-dialog-icon"><Trash2 size={19} /></div>
        <div>
          <h2 id="confirm-dialog-title">{request.title}</h2>
          <p id="confirm-dialog-detail">{request.detail}</p>
        </div>
        <div className="confirm-dialog-actions">
          <button className="confirm-cancel" onClick={() => onChoose(false)} ref={cancelRef} type="button">
            取消
          </button>
          <button className="confirm-danger" onClick={() => onChoose(true)} type="button">
            {request.confirmLabel || '确认删除'}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  );
}

function useConfirmation() {
  const [request, setRequest] = useState<PendingConfirmation | null>(null);
  const requestRef = useRef<PendingConfirmation | null>(null);

  useEffect(() => () => requestRef.current?.resolve(false), []);

  const confirm = useCallback((options: ConfirmationOptions) => new Promise<boolean>((resolve) => {
    const pending = { ...options, resolve };
    requestRef.current = pending;
    setRequest(pending);
  }), []);

  const choose = useCallback((accepted: boolean) => {
    const current = requestRef.current;
    requestRef.current = null;
    setRequest(null);
    current?.resolve(accepted);
  }, []);

  return {
    confirm,
    isOpen: Boolean(request),
    dialog: request ? <ConfirmationDialog request={request} onChoose={choose} /> : null,
  };
}

function RenameCategoryDialog({
  currentName,
  busy,
  onCancel,
  onRename,
}: {
  currentName: string;
  busy: boolean;
  onCancel: () => void;
  onRename: (name: string) => void;
}) {
  const [name, setName] = useState(currentName);
  const inputRef = useRef<HTMLInputElement>(null);
  const valid = Boolean(name.trim() && name.trim() !== currentName);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopImmediatePropagation();
      onCancel();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [onCancel]);

  return createPortal(
    <div
      className="confirm-backdrop"
      onMouseDown={(event) => {
        event.stopPropagation();
        onCancel();
      }}
      role="presentation"
    >
      <section
        aria-labelledby="rename-category-title"
        aria-modal="true"
        className="confirm-dialog rename-dialog"
        onMouseDown={(event) => event.stopPropagation()}
        role="dialog"
      >
        <div className="confirm-dialog-icon rename"><Pencil size={19} /></div>
        <div>
          <h2 id="rename-category-title">重命名分类</h2>
          <p>项目、任务归属和历史会话会一起更新。</p>
        </div>
        <input
          aria-label="新的分类名称"
          disabled={busy}
          onChange={(event) => setName(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && valid && !busy) {
              event.preventDefault();
              onRename(name.trim());
            }
          }}
          ref={inputRef}
          value={name}
        />
        <div className="confirm-dialog-actions">
          <button className="confirm-cancel" disabled={busy} onClick={onCancel} type="button">取消</button>
          <button className="primary" disabled={!valid || busy} onClick={() => onRename(name.trim())} type="button">
            保存名称
          </button>
        </div>
      </section>
    </div>,
    document.body,
  );
}

function EditProjectDialog({
  project,
  categoryOptions,
  busy,
  onCancel,
  onSave,
}: {
  project: Project;
  categoryOptions: CategoryOption[];
  busy: boolean;
  onCancel: () => void;
  onSave: (name: string, category: string) => void;
}) {
  const [name, setName] = useState(project.name);
  const [category, setCategory] = useState(project.category);
  const inputRef = useRef<HTMLInputElement>(null);
  const normalizedName = name.trim();
  const valid = Boolean(normalizedName)
    && (normalizedName !== project.name || category !== project.category);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || busy) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      onCancel();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [busy, onCancel]);

  return createPortal(
    <div
      className="confirm-backdrop"
      onMouseDown={(event) => {
        event.stopPropagation();
        if (!busy) onCancel();
      }}
      role="presentation"
    >
      <section
        aria-labelledby="edit-project-title"
        aria-modal="true"
        className="confirm-dialog project-edit-dialog"
        onMouseDown={(event) => event.stopPropagation()}
        role="dialog"
      >
        <div className="confirm-dialog-icon rename"><FolderKanban size={19} /></div>
        <div>
          <h2 id="edit-project-title">编辑项目</h2>
          <p>改名或移动到其他分类，已有任务和时间段会一起保留。</p>
        </div>
        <div className="project-edit-fields">
          <label>
            <span>项目名称</span>
            <input
              disabled={busy}
              onChange={(event) => setName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && valid && !busy) {
                  event.preventDefault();
                  onSave(normalizedName, category);
                }
              }}
              ref={inputRef}
              value={name}
            />
          </label>
          <label>
            <span>所属分类</span>
            <select
              aria-label="项目所属分类"
              disabled={busy}
              onChange={(event) => setCategory(event.target.value)}
              value={category}
            >
              {categoryOptions.map((item) => <option key={item.name} value={item.name}>{item.name}</option>)}
            </select>
          </label>
        </div>
        <div className="confirm-dialog-actions">
          <button className="confirm-cancel" disabled={busy} onClick={onCancel} type="button">取消</button>
          <button className="primary" disabled={!valid || busy} onClick={() => onSave(normalizedName, category)} type="button">
            {busy ? '保存中…' : '保存修改'}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  );
}

function ManageCategoriesDialog({
  categories,
  idleCategory,
  stats,
  busyCategory,
  blocked,
  onCancel,
  onRename,
  onDelete,
}: {
  categories: CategoryOption[];
  idleCategory: string;
  stats: Map<string, { projects: number; minutes: number }>;
  busyCategory: string;
  blocked: boolean;
  onCancel: () => void;
  onRename: (category: CategoryOption) => void;
  onDelete: (category: CategoryOption) => void;
}) {
  const [query, setQuery] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);
  const needle = normalizeSearchText(query);
  const visibleCategories = categories.filter((category) => (
    !needle || normalizeSearchText(category.name).includes(needle)
  ));

  useEffect(() => {
    inputRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || busyCategory || blocked) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      onCancel();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [blocked, busyCategory, onCancel]);

  return createPortal(
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        event.stopPropagation();
        if (!busyCategory && !blocked) onCancel();
      }}
      role="presentation"
    >
      <section
        aria-labelledby="manage-categories-title"
        aria-modal="true"
        className="modal category-manager-dialog"
        onMouseDown={(event) => event.stopPropagation()}
        role="dialog"
      >
        <div className="modal-head category-manager-head">
          <div>
            <h2 id="manage-categories-title">分类管理</h2>
            <p>改名会同步更新项目、规则和历史时间段。</p>
          </div>
          <button className="icon-button" disabled={Boolean(busyCategory)} onClick={onCancel} type="button" aria-label="关闭分类管理">
            <X size={18} />
          </button>
        </div>
        <div className="category-manager-search">
          <Search size={16} />
          <input
            aria-label="搜索分类"
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索分类"
            ref={inputRef}
            value={query}
          />
          {query && (
            <button onClick={() => setQuery('')} type="button" aria-label="清空分类搜索">
              <X size={14} />
            </button>
          )}
        </div>
        <div className="category-manager-list">
          {visibleCategories.map((category) => {
            const categoryStats = stats.get(category.name) || { projects: 0, minutes: 0 };
            const isIdleCategory = category.name === idleCategory;
            const isBusy = busyCategory === category.name;
            const cannotDelete = categories.length <= 1 || isIdleCategory;
            return (
              <article className="category-manager-row" key={category.name}>
                <i style={{ background: category.color || categoryColor(category.name) }} />
                <span>
                  <strong>{category.name}</strong>
                  <small>
                    {isIdleCategory
                      ? `离开时间归属 · ${categoryStats.projects} 个项目`
                      : `${categoryStats.projects} 个项目 · ${formatDuration(categoryStats.minutes)}`}
                  </small>
                </span>
                <div>
                  <button disabled={Boolean(busyCategory)} onClick={() => onRename(category)} type="button">
                    <Pencil size={14} />改名
                  </button>
                  <button
                    className="danger-button"
                    disabled={Boolean(busyCategory) || cannotDelete}
                    onClick={() => onDelete(category)}
                    title={isIdleCategory ? '请先在设置中更换离开时间归属' : categories.length <= 1 ? '至少保留一个分类' : '删除分类'}
                    type="button"
                  >
                    <Trash2 size={14} />{isBusy ? '删除中…' : '删除'}
                  </button>
                </div>
              </article>
            );
          })}
          {!visibleCategories.length && <EmptyState title="没有匹配分类" detail="换个名称试试。" />}
        </div>
      </section>
    </div>,
    document.body,
  );
}

function TextInputDialog({
  title,
  detail,
  initialValue,
  inputLabel,
  confirmLabel,
  type = 'text',
  min,
  max,
  step,
  busy = false,
  isValid,
  onCancel,
  onConfirm,
}: {
  title: string;
  detail: string;
  initialValue: string;
  inputLabel: string;
  confirmLabel: string;
  type?: 'text' | 'datetime-local';
  min?: string;
  max?: string;
  step?: number;
  busy?: boolean;
  isValid?: (value: string) => boolean;
  onCancel: () => void;
  onConfirm: (value: string) => void;
}) {
  const [value, setValue] = useState(initialValue);
  const inputRef = useRef<HTMLInputElement>(null);
  const normalizedValue = type === 'text' ? value.trim() : value;
  const valid = Boolean(normalizedValue) && (isValid ? isValid(normalizedValue) : true);

  useEffect(() => {
    inputRef.current?.focus();
    if (type === 'text') inputRef.current?.select();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || busy) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      onCancel();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [busy, onCancel, type]);

  return createPortal(
    <div
      className="confirm-backdrop"
      onMouseDown={(event) => {
        event.stopPropagation();
        if (!busy) onCancel();
      }}
      role="presentation"
    >
      <section
        aria-labelledby="text-input-dialog-title"
        aria-modal="true"
        className="confirm-dialog rename-dialog"
        onMouseDown={(event) => event.stopPropagation()}
        role="dialog"
      >
        <div className="confirm-dialog-icon rename"><Pencil size={19} /></div>
        <div>
          <h2 id="text-input-dialog-title">{title}</h2>
          <p>{detail}</p>
        </div>
        <input
          aria-label={inputLabel}
          disabled={busy}
          max={max}
          min={min}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && valid && !busy) {
              event.preventDefault();
              onConfirm(normalizedValue);
            }
          }}
          ref={inputRef}
          step={step}
          type={type}
          value={value}
        />
        <div className="confirm-dialog-actions">
          <button className="confirm-cancel" disabled={busy} onClick={onCancel} type="button">取消</button>
          <button className="primary" disabled={!valid || busy} onClick={() => onConfirm(normalizedValue)} type="button">
            {busy ? '处理中…' : confirmLabel}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  );
}

export default function App() {
  const [activeTab, setActiveTab] = useState<TabId>('today');
  const [data, setData] = useState<DashboardData | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState('');
  const [toast, setToast] = useState<{ message: string; tone: 'success' | 'error' } | null>(null);
  const [selectedDate, setSelectedDate] = useState(localDateKey(new Date()));
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [selectionResetKey, setSelectionResetKey] = useState(0);
  const [editing, setEditing] = useState<WorkSession[]>([]);
  const [undoStatus, setUndoStatus] = useState<UndoStatus>({ available: false });
  const [themeMode, setThemeMode] = useState<ThemeMode>(readStoredTheme);
  const [globalSearchOpen, setGlobalSearchOpen] = useState(false);
  const [projectFocusId, setProjectFocusId] = useState('');
  const loadSequenceRef = useRef(0);
  const toastTimerRef = useRef<number | null>(null);

  const load = useCallback(async () => {
    const sequence = ++loadSequenceRef.current;
    try {
      const [dashboard, nextUndoStatus] = await Promise.all([
        api.dashboard(),
        api.undoStatus().catch(() => ({ available: false } as UndoStatus)),
      ]);
      if (sequence !== loadSequenceRef.current) return;
      setData(dashboard);
      setUndoStatus(nextUndoStatus);
      setLoadError('');
    } catch (error) {
      if (sequence !== loadSequenceRef.current) return;
      setLoadError(error instanceof Error ? error.message : String(error));
    } finally {
      if (sequence === loadSequenceRef.current) setLoading(false);
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
    const onFocus = () => void load();
    document.addEventListener('visibilitychange', onVisibility);
    window.addEventListener('focus', onFocus);
    return () => {
      loadSequenceRef.current += 1;
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', onVisibility);
      window.removeEventListener('focus', onFocus);
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

  const showToast = useCallback((message: string, tone: 'success' | 'error' = 'success') => {
    if (toastTimerRef.current !== null) window.clearTimeout(toastTimerRef.current);
    setToast({ message, tone });
    toastTimerRef.current = window.setTimeout(() => {
      setToast(null);
      toastTimerRef.current = null;
    }, 3200);
  }, []);

  useEffect(() => () => {
    if (toastTimerRef.current !== null) window.clearTimeout(toastTimerRef.current);
  }, []);

  const runAction = useCallback(
    (fn: () => Promise<unknown>, successMessage: string) => {
      const action = (async () => {
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
          showToast(error instanceof Error ? error.message : String(error), 'error');
          throw error;
        }
      })();
      // Fire-and-forget toolbar actions still report through the toast without
      // producing an unhandled rejection. Awaiting callers keep normal errors.
      void action.catch(() => undefined);
      return action;
    },
    [load, showToast],
  );

  const undoLastCorrection = useCallback(async () => {
    if (!undoStatus.available) return;
    await runAction(api.undoLastSessionCorrection, '已撤销上一次修正');
    setEditing([]);
    setSelected(new Set());
    setSelectionResetKey((value) => value + 1);
  }, [runAction, undoStatus.available]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const isTyping = Boolean(target?.matches('input, textarea, select, [contenteditable="true"]'));
      if (!isTyping && event.altKey && !event.ctrlKey && !event.metaKey) {
        const index = Number(event.key) - 1;
        if (index >= 0 && index < tabs.length) {
          event.preventDefault();
          setActiveTab(tabs[index].id);
        }
      }
      if (
        !isTyping
        && editing.length === 0
        && undoStatus.available
        && (event.ctrlKey || event.metaKey)
        && event.key.toLowerCase() === 'z'
      ) {
        event.preventDefault();
        void undoLastCorrection().catch(() => undefined);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [editing.length, undoLastCorrection, undoStatus.available]);

  const daySessions = useMemo(
    () =>
      (data?.sessions || [])
        .filter((session) => sessionSecondsOnDate(session, selectedDate) > 0)
        .sort(
          (left, right) =>
            new Date(right.startedAt).getTime() - new Date(left.startedAt).getTime(),
        ),
    [data, selectedDate],
  );
  useEffect(() => {
    const validIds = new Set((data?.sessions || []).map((session) => session.id));
    setSelected((current) => {
      const next = new Set([...current].filter((id) => validIds.has(id)));
      return next.size === current.size ? current : next;
    });
  }, [data?.sessions]);
  const stats = useMemo(
    () => summarizeDay(daySessions, selectedDate, data?.settings),
    [data?.settings, daySessions, selectedDate],
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
                title={`${tab.label}（Alt+${tabs.indexOf(tab) + 1}）`}
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
            {activeTab !== 'settings' && activeTab !== 'ai' && (
              <DateNavigator
                value={selectedDate}
                onChange={setSelectedDate}
                onPrevious={() => goDate(-1)}
                onNext={() => goDate(1)}
              />
            )}
            <div className="top-actions">
              <button
                disabled={!undoStatus.available}
                onClick={() => void undoLastCorrection().catch(() => undefined)}
                title={undoStatus.available
                  ? `撤销：${undoStatus.label || '上一次修正'}（Ctrl+Z）`
                  : '暂无可撤销的修正'}
                type="button"
              >
                <Undo2 size={16} />撤销
              </button>
              <details
                className="top-more"
                onBlur={(event) => {
                  if (!event.currentTarget.contains(event.relatedTarget)) {
                    event.currentTarget.removeAttribute('open');
                  }
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Escape') event.currentTarget.removeAttribute('open');
                }}
              >
                <summary aria-label="更多操作" title="更多操作">
                  <Ellipsis size={17} /><span>更多</span>
                </summary>
                <div className="top-more-menu">
                  <button
                    onClick={(event) => {
                      event.currentTarget.closest('details')?.removeAttribute('open');
                      setActiveTab('ai');
                    }}
                    type="button"
                  >
                    <WandSparkles size={16} />AI 自动复核
                  </button>
                  <button
                    onClick={(event) => {
                      event.currentTarget.closest('details')?.removeAttribute('open');
                      void runAction(api.compactSessions, '已整理连续同类会话');
                    }}
                    type="button"
                    title="合并被短暂切换打断的同类活动"
                  >
                    <RefreshCw size={16} />整理会话
                  </button>
                  <button
                    onClick={(event) => {
                      event.currentTarget.closest('details')?.removeAttribute('open');
                      void runAction(api.cleanupMediaCache, '数据库与旧缓存已优化');
                    }}
                    type="button"
                    title="清理过期原始事件并压缩数据库"
                  >
                    <Database size={16} />清理存储
                  </button>
                </div>
              </details>
            </div>
          </div>
        </header>

        {activeTab !== 'settings' && activeTab !== 'ai' && (
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
              hint={`${formatDuration(stats.classifiedMinutes)} / ${formatDuration(stats.activeMinutes)}`}
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
            tasks={data.tasks}
            stats={stats}
            selectedDate={selectedDate}
            idleCategory={data.settings.idleCategory}
            selectionResetKey={selectionResetKey}
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
            sessions={data.sessions}
            selectedDate={selectedDate}
            runAction={runAction}
            categoryOptions={data.categoryOptions}
            idleCategory={data.settings.idleCategory}
            focusProjectId={projectFocusId}
            selectionResetKey={selectionResetKey}
            onEdit={setEditing}
          />
        )}
        {activeTab === 'ai' && (
          <AiReviewView
            sessions={data.sessions}
            codexPlan={data.settings.codexPlan}
            aiMode={data.settings.aiMode}
            memoryCount={data.queue.personalMemoryCount}
            memoryUses={data.queue.personalMemoryUses}
            runAction={runAction}
            onToggleAuto={(enabled) => runAction(async () => {
              const next: AppSettings = {
                ...data.settings,
                aiMode: enabled ? 'auto' : 'off',
              };
              await api.saveSettings(next);
              if (enabled) await api.startAnalysisQueue();
            }, enabled ? 'AI 自动复核已开启' : 'AI 自动复核已关闭')}
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
              await api.applySessionCorrection(
                sessions.map((session) => session.id),
                patch,
                options.remember,
                options.keyword,
                options.pin ? 30 : undefined,
              );
            }, sessions.length > 1
              ? `已统一修正 ${sessions.length} 条会话`
              : options.pin
                ? '已修正，并固定当前事务 30 分钟'
                : options.remember
                  ? '已修正并记住规则'
                  : '会话已修正');
            setEditing([]);
            setSelected(new Set());
            setSelectionResetKey((value) => value + 1);
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
      {toast && (
        <div className={`toast ${toast.tone}`} role={toast.tone === 'error' ? 'alert' : 'status'}>
          {toast.tone === 'error' ? <CircleAlert size={17} /> : <CheckCircle2 size={17} />}
          <span>{toast.message}</span>
          <button aria-label="关闭提示" onClick={() => setToast(null)} type="button"><X size={14} /></button>
        </div>
      )}
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

type AiJobFilter = 'all' | 'pending' | 'running' | 'completed' | 'skipped' | 'failed';

const aiJobFilters: { id: AiJobFilter; label: string }[] = [
  { id: 'all', label: '全部' },
  { id: 'pending', label: '排队' },
  { id: 'running', label: '运行中' },
  { id: 'completed', label: '已复核' },
  { id: 'skipped', label: '未调用' },
  { id: 'failed', label: '失败' },
];

function AiReviewView({
  sessions,
  codexPlan,
  aiMode,
  memoryCount,
  memoryUses,
  runAction,
  onToggleAuto,
}: {
  sessions: WorkSession[];
  codexPlan: string;
  aiMode: string;
  memoryCount: number;
  memoryUses: number;
  runAction: ActionRunner;
  onToggleAuto: (enabled: boolean) => Promise<unknown>;
}) {
  const [jobs, setJobs] = useState<AnalysisJob[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [selectedJobIds, setSelectedJobIds] = useState<Set<string>>(() => new Set());
  const [detail, setDetail] = useState<AnalysisJob | null>(null);
  const [filter, setFilter] = useState<AiJobFilter>('all');
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [rateCard, setRateCard] = useState<CodexRateCard | null>(null);
  const selectAllJobsRef = useRef<HTMLInputElement>(null);
  const confirmation = useConfirmation();

  const refresh = useCallback(async (quiet = false) => {
    if (!quiet) setLoading(true);
    try {
      const next = await api.listAnalysisJobs(300);
      setJobs(next);
      setSelectedJobIds((current) => {
        const available = new Set(next.map((job) => job.id));
        return new Set([...current].filter((id) => available.has(id)));
      });
      setSelectedId((current) => current && next.some((job) => job.id === current)
        ? current
        : next[0]?.id || null);
    } finally {
      if (!quiet) setLoading(false);
    }
  }, []);

  const hasActiveJobs = jobs.some((job) => job.status === 'pending' || job.status === 'running');

  useEffect(() => {
    void refresh();
    void api.getCodexRateCard().then(setRateCard);
  }, [refresh]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') void refresh(true);
    }, hasActiveJobs ? 3000 : 30_000);
    return () => window.clearInterval(timer);
  }, [hasActiveJobs, refresh]);

  const selectedStatus = jobs.find((job) => job.id === selectedId)?.status;
  useEffect(() => {
    if (!selectedId) {
      setDetail(null);
      return undefined;
    }
    let alive = true;
    const loadDetail = async () => {
      const next = await api.getAnalysisJob(selectedId);
      if (alive) setDetail(next);
    };
    void loadDetail();
    const shouldPoll = selectedStatus === 'pending' || selectedStatus === 'running';
    const timer = shouldPoll
      ? window.setInterval(() => {
          if (document.visibilityState === 'visible') void loadDetail();
        }, 5000)
      : null;
    return () => {
      alive = false;
      if (timer !== null) window.clearInterval(timer);
    };
  }, [selectedId, selectedStatus]);

  const runAndRefresh = async (action: () => Promise<unknown>, message: string) => {
    setBusy(true);
    try {
      await runAction(action, message);
      await refresh(true);
      if (selectedId) setDetail(await api.getAnalysisJob(selectedId));
    } catch {
      // runAction already surfaces the backend message in the app toast.
    } finally {
      setBusy(false);
    }
  };

  const toggleAutoReview = async () => {
    if (busy) return;
    const enable = aiMode !== 'auto';
    setBusy(true);
    try {
      await onToggleAuto(enable);
      await refresh(true);
      if (enable) {
        window.setTimeout(() => void refresh(true), 1200);
      }
    } catch {
      // The shared action runner already displays the backend message.
    } finally {
      setBusy(false);
    }
  };

  const refreshRateCard = async () => {
    setBusy(true);
    try {
      const next = await runAction(api.refreshCodexRateCard, '已对齐 OpenAI 最新 Codex 费率') as CodexRateCard;
      setRateCard(next);
    } catch {
      // runAction already surfaces the backend message.
    } finally {
      setBusy(false);
    }
  };

  const deleteSkippedJob = async () => {
    if (!detail || detail.status !== 'skipped' || busy) return;
    const accepted = await confirmation.confirm({
      title: '删除这条未调用 AI 的记录？',
      detail: `只删除复核历史，不会删除其中 ${detail.chunkIds.length} 个原始时间段。`,
    });
    if (!accepted) return;

    const deletedId = detail.id;
    setBusy(true);
    try {
      await runAction(() => api.deleteAnalysisJob(deletedId), '未调用 AI 的复核记录已删除');
      setDetail(null);
      setSelectedId(null);
      await refresh(true);
    } catch {
      // runAction already surfaces the backend message in the app toast.
    } finally {
      setBusy(false);
    }
  };

  const visibleJobs = useMemo(() => jobs.filter((job) => {
    if (filter === 'all') return true;
    if (filter === 'failed') return job.status === 'failed' || job.status === 'downgraded';
    return job.status === filter;
  }), [filter, jobs]);
  const selectedJobs = useMemo(
    () => jobs.filter((job) => selectedJobIds.has(job.id)),
    [jobs, selectedJobIds],
  );
  const pendingSelectedJobs = selectedJobs.filter((job) => job.status === 'pending');
  const retryableSelectedJobs = selectedJobs.filter(
    (job) => job.status === 'failed' || job.status === 'downgraded',
  );
  const deletableSelectedJobs = selectedJobs.filter((job) => job.status === 'skipped');
  const visibleSelectedCount = visibleJobs.reduce(
    (count, job) => count + Number(selectedJobIds.has(job.id)),
    0,
  );
  const allVisibleSelected = visibleJobs.length > 0 && visibleSelectedCount === visibleJobs.length;

  useEffect(() => {
    if (selectAllJobsRef.current) {
      selectAllJobsRef.current.indeterminate = visibleSelectedCount > 0 && !allVisibleSelected;
    }
  }, [allVisibleSelected, visibleSelectedCount]);

  const toggleSelectedJob = (id: string) => {
    setSelectedJobIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const toggleAllVisibleJobs = (checked: boolean) => {
    setSelectedJobIds((current) => {
      const next = new Set(current);
      visibleJobs.forEach((job) => {
        if (checked) next.add(job.id);
        else next.delete(job.id);
      });
      return next;
    });
  };

  const retrySelectedJobs = async () => {
    const ids = retryableSelectedJobs.map((job) => job.id);
    if (!ids.length || busy) return;
    setBusy(true);
    try {
      await runAction(
        () => api.retryAnalysisJobs(ids),
        `已将 ${ids.length} 条失败记录重新排队`,
      );
      setSelectedJobIds(new Set());
      await refresh(true);
      if (selectedId) setDetail(await api.getAnalysisJob(selectedId));
    } catch {
      // runAction already surfaces the backend message in the app toast.
    } finally {
      setBusy(false);
    }
  };

  const runSelectedJobs = async () => {
    const ids = pendingSelectedJobs.map((job) => job.id);
    if (!ids.length || busy || aiMode !== 'auto') return;
    setBusy(true);
    try {
      await runAction(
        () => api.runAnalysisJobs(ids),
        `已处理 ${ids.length} 条所选复核记录，失败项已保留`,
      );
      setSelectedJobIds(new Set());
      await refresh(true);
      if (selectedId) setDetail(await api.getAnalysisJob(selectedId));
    } catch {
      // runAction already surfaces the backend message in the app toast.
    } finally {
      setBusy(false);
    }
  };

  const deleteSelectedJobs = async () => {
    const ids = deletableSelectedJobs.map((job) => job.id);
    if (!ids.length || busy) return;
    const chunkCount = deletableSelectedJobs.reduce((sum, job) => sum + job.chunkIds.length, 0);
    const accepted = await confirmation.confirm({
      title: `删除 ${ids.length} 条未调用 AI 的记录？`,
      detail: `只删除复核历史，不会删除其中 ${chunkCount} 个原始时间段。`,
    });
    if (!accepted) return;

    setBusy(true);
    try {
      await runAction(
        () => api.deleteAnalysisJobs(ids),
        `已删除 ${ids.length} 条未调用 AI 的复核记录`,
      );
      if (selectedId && ids.includes(selectedId)) {
        setSelectedId(null);
        setDetail(null);
      }
      setSelectedJobIds(new Set());
      await refresh(true);
    } catch {
      // runAction already surfaces the backend message in the app toast.
    } finally {
      setBusy(false);
    }
  };
  const counts = useMemo(() => ({
    pending: jobs.filter((job) => job.status === 'pending').length,
    running: jobs.filter((job) => job.status === 'running').length,
    completed: jobs.filter((job) => job.status === 'completed').length,
    skipped: jobs.filter((job) => job.status === 'skipped').length,
    failed: jobs.filter((job) => job.status === 'failed' || job.status === 'downgraded').length,
  }), [jobs]);
  const usageSummary = useMemo(() => {
    let totalTokens = 0;
    let credits = 0;
    let jobCount = 0;
    let unmatched = false;
    jobs.forEach((job) => {
      if (job.provider !== 'codex-account') return;
      jobCount += 1;
      totalTokens += job.usage.totalTokens || 0;
      const estimate = estimateAiCredits(job, rateCard);
      if (estimate == null) {
        if (job.usage.totalTokens > 0) unmatched = true;
      } else {
        credits += estimate;
      }
    });
    return {
      totalTokens,
      credits,
      jobCount,
      costUsd: rateCard ? credits * rateCard.usdPerCredit : null,
      unmatched,
    };
  }, [jobs, rateCard]);
  const sessionsById = useMemo(
    () => new Map(sessions.map((session) => [session.id, session])),
    [sessions],
  );

  return (
    <div className="ai-review-page">
      <section className="ai-review-summary">
        <div><span>排队</span><strong>{counts.pending}</strong></div>
        <div><span>运行中</span><strong>{counts.running}</strong></div>
        <div><span>已复核</span><strong>{counts.completed}</strong></div>
        <div><span>未调用</span><strong>{counts.skipped}</strong></div>
        <div><span>失败</span><strong>{counts.failed}</strong></div>
        <div className="ai-review-actions">
          <button
            className={`ai-auto-toggle ${aiMode === 'auto' ? 'active' : ''}`}
            disabled={busy}
            onClick={() => void toggleAutoReview()}
            type="button"
            title={aiMode === 'auto'
              ? '关闭后会让当前请求完成，但不会继续处理下一条'
              : '开启后按排队顺序逐条复核，无需重复点击'}
          >
            {aiMode === 'auto' ? <Pause size={15} /> : <Play size={15} />}
            自动复核·{aiMode === 'auto' ? '已开启' : '已关闭'}
          </button>
          <button disabled={busy} onClick={() => void refreshRateCard()} type="button">
            <RefreshCw size={15} />更新费率
          </button>
          <button
            disabled={busy || counts.failed === 0}
            onClick={() => void runAndRefresh(api.retryFailedJobs, '失败任务已重新排队')}
            type="button"
          >
            <RefreshCw size={15} />重试全部失败
          </button>
          <button disabled={loading} onClick={() => void refresh()} type="button" title="刷新列表">
            <RefreshCw className={loading ? 'spin' : ''} size={15} />刷新
          </button>
        </div>
      </section>

      {(usageSummary.totalTokens > 0 || memoryCount > 0) && (
        <section className="ai-usage-strip" aria-label="AI 用量估算">
          <div>
            <span>最近 {usageSummary.jobCount} 条 Codex 记录</span>
            <strong>{formatAiTokens(usageSummary.totalTokens)} Token</strong>
          </div>
          <div>
            <span>信用点</span>
            <strong>{usageSummary.credits.toFixed(usageSummary.credits < 10 ? 4 : 2)} Credits</strong>
          </div>
          <div>
            <span>Token 等值开销</span>
            <strong>{usageSummary.costUsd == null ? '—' : formatUsdEstimate(usageSummary.costUsd)}</strong>
          </div>
          <div>
            <span>套餐</span>
            <strong>{codexPlanLabel(codexPlan)}</strong>
          </div>
          <div>
            <span>个人记忆</span>
            <strong>{memoryCount} 条 · 命中 {memoryUses} 次</strong>
          </div>
          {usageSummary.unmatched && <small>部分历史模型没有匹配费率，合计不包含这些记录。</small>}
        </section>
      )}

      <section className={`ai-review-workspace ${jobs.length === 0 ? 'empty' : ''}`}>
        <aside className="ai-job-browser">
          <div className="ai-job-filters" role="tablist" aria-label="复核状态">
            {aiJobFilters.map((item) => (
              <button
                aria-selected={filter === item.id}
                className={filter === item.id ? 'active' : ''}
                key={item.id}
                onClick={() => setFilter(item.id)}
                role="tab"
                type="button"
              >
                {item.label}
              </button>
            ))}
          </div>
          {jobs.length > 0 && <div className="ai-job-selection-bar">
            <div>
              <label className="selection-toggle" title={`选择当前筛选下的 ${visibleJobs.length} 条记录`}>
                <input
                  className="themed-checkbox"
                  ref={selectAllJobsRef}
                  type="checkbox"
                  checked={allVisibleSelected}
                  disabled={!visibleJobs.length}
                  onChange={(event) => toggleAllVisibleJobs(event.target.checked)}
                />
                全选当前
              </label>
              <span>已选 {selectedJobs.length}</span>
              <button
                className="selection-clear"
                disabled={!selectedJobs.length}
                onClick={() => setSelectedJobIds(new Set())}
                type="button"
              >
                全不选
              </button>
            </div>
            <div className="ai-job-batch-actions">
              <button
                className="primary"
                disabled={busy || aiMode !== 'auto' || !pendingSelectedJobs.length}
                onClick={() => void runSelectedJobs()}
                type="button"
                title={aiMode === 'auto'
                  ? '依次处理所选记录中的排队项'
                  : '请先开启 AI 自动复核'}
              >
                <WandSparkles size={13} />复核 {pendingSelectedJobs.length || ''}
              </button>
              <button
                disabled={busy || !retryableSelectedJobs.length}
                onClick={() => void retrySelectedJobs()}
                type="button"
                title="只重试所选记录中的失败项"
              >
                <RefreshCw size={13} />重试 {retryableSelectedJobs.length || ''}
              </button>
              <button
                className="danger-button"
                disabled={busy || !deletableSelectedJobs.length}
                onClick={() => void deleteSelectedJobs()}
                type="button"
                title="只删除所选记录中的未调用项"
              >
                <Trash2 size={13} />删除 {deletableSelectedJobs.length || ''}
              </button>
            </div>
          </div>}
          <div className="ai-job-list">
            {visibleJobs.map((job) => (
              <div
                className={`ai-job-row ${selectedId === job.id ? 'active' : ''} ${selectedJobIds.has(job.id) ? 'checked' : ''}`}
                key={job.id}
              >
                <label className="ai-job-check" aria-label={`选择 ${job.chunkIds.length} 个时间段的复核记录`}>
                  <input
                    className="themed-checkbox"
                    type="checkbox"
                    checked={selectedJobIds.has(job.id)}
                    onChange={() => toggleSelectedJob(job.id)}
                  />
                </label>
                <button
                  className="ai-job-open"
                  onClick={() => setSelectedId(job.id)}
                  type="button"
                >
                  <span className={`ai-status ${job.status}`}>{aiJobStatusLabel(job.status)}</span>
                  <strong>{job.chunkIds.length} 个时间段</strong>
                  <small>{job.model || '等待读取模型'}</small>
                  <time>{formatAiDateTime(job.queuedAt)}</time>
                </button>
              </div>
            ))}
            {!loading && visibleJobs.length === 0 && (
              <div className="ai-job-empty">
                <WandSparkles size={22} />
                <strong>{filter === 'all' ? '这里还没有记录' : '这个状态下没有记录'}</strong>
                <span>{filter === 'all'
                  ? '未归到具体任务，或低于 80% 且达到最小时长的会话会进入复核。'
                  : '切换上方状态，或刷新列表查看最新结果。'}</span>
              </div>
            )}
          </div>
        </aside>

        <div className="ai-job-detail">
          {detail ? (
            <>
              <header className="ai-job-detail-head">
                <div>
                  <span className={`ai-status ${detail.status}`}>{aiJobStatusLabel(detail.status)}</span>
                  <h2>{detail.chunkIds.length} 个时间段的上下文复核</h2>
                  <p>{formatAiDateTime(detail.metadataRange.startedAt)} — {formatAiDateTime(detail.metadataRange.endedAt)}</p>
                </div>
                <div className="ai-job-detail-tools">
                  {detail.status === 'skipped' && (
                    <button
                      className="danger-button"
                      disabled={busy}
                      onClick={() => void deleteSkippedJob()}
                      type="button"
                    >
                      <Trash2 size={14} />删除记录
                    </button>
                  )}
                  <code>{detail.id.slice(0, 8)}</code>
                </div>
              </header>

              <div className="ai-job-facts">
                <AiFact label="提供方" value={aiProviderLabel(detail.provider)} />
                <AiFact label="模型" value={detail.model || '尚未开始'} />
                <AiFact label="模式" value={detail.mode} />
                <AiFact label="结果" value={`${detail.resultCount} 条`} />
                <AiFact label="排队" value={formatAiDateTime(detail.queuedAt)} />
                <AiFact label="开始" value={formatAiDateTime(detail.processingStartedAt)} />
                <AiFact label="完成" value={formatAiDateTime(detail.completedAt)} />
                <AiFact label="耗时" value={formatAiDuration(detail.durationMs)} />
                <AiFact label="重试" value={`${detail.retryCount} 次`} />
                <AiFact label={detail.retryCount > 0 ? '累计总 Token' : '总 Token'} value={formatAiTokens(detail.usage.totalTokens)} />
                <AiFact label="输入 Token" value={formatAiTokens(detail.usage.inputTokens)} />
                <AiFact label="缓存 Token" value={formatAiTokens(detail.usage.cachedInputTokens)} />
                <AiFact label="输出 Token" value={formatAiTokens(detail.usage.outputTokens)} />
                <AiFact label="推理 Token" value={formatAiTokens(detail.usage.reasoningOutputTokens)} />
                <AiFact label="信用点" value={formatAiCredits(detail, rateCard)} />
                <AiFact
                  label={isEstimatedAiCost(detail) || detail.usage.costUsd == null ? 'Token 等值开销' : '实际开销'}
                  value={formatAiCost(detail, rateCard)}
                />
                {detail.provider === 'codex-account' && (
                  <AiFact label="套餐" value={codexPlanLabel(codexPlan)} />
                )}
                {detail.provider === 'codex-account' && rateCard && (
                  <AiFact label="费率同步" value={formatAiDateTime(rateCard.fetchedAt)} />
                )}
              </div>

              {detail.provider === 'codex-account' && rateCard && (
                <div className="ai-rate-note">
                  <span>
                    按 Token 先换算 Credits，再按 1 Credit ≈ ${rateCard.usdPerCredit.toFixed(2)} 计算美元等值，并累计失败重试；这是用量等值，不摊分每月订阅费。
                  </span>
                  <div>
                    <button onClick={() => window.open(rateCard.sourceUrl, '_blank')} type="button">
                      模型费率
                    </button>
                    {rateCard.creditValueSourceUrl && (
                      <button onClick={() => window.open(rateCard.creditValueSourceUrl || '', '_blank')} type="button">
                        换算依据
                      </button>
                    )}
                  </div>
                </div>
              )}

              {detail.error && <div className="ai-job-error"><CircleAlert size={16} />{detail.error}</div>}

              <section className="ai-targets">
                <h3>待复核时间段</h3>
                <div>
                  {detail.chunkIds.map((id) => {
                    const session = sessionsById.get(id);
                    return (
                      <article key={id}>
                        <time>{session
                          ? `${formatTimelineClock(session.startedAt, true)}–${formatTimelineClock(session.endedAt, true)}`
                          : id.slice(0, 8)}</time>
                        <span>
                          <strong>{session ? displaySessionSummary(session) : '时间段已被整理或删除'}</strong>
                          <small>{session
                            ? [session.category, session.projectName, session.taskTitle].filter(Boolean).join(' · ')
                            : '仍保留原始复核记录'}</small>
                        </span>
                      </article>
                    );
                  })}
                </div>
              </section>

              <AiTraceBlock title="系统提示词" value={detail.systemPrompt} empty={aiTraceEmptyText(detail)} />
              <AiTraceBlock title="发送给 AI 的提示词" value={detail.userPrompt} empty={aiTraceEmptyText(detail)} />
              <AiTraceBlock
                title="AI 原始回复"
                value={detail.response}
                empty={detail.status === 'skipped'
                  ? '未调用 AI，因此没有原始回复'
                  : detail.status === 'running'
                    ? '正在等待回复'
                    : '尚无回复'}
              />
            </>
          ) : (
            <div className="ai-detail-empty">
              <WandSparkles size={28} />
              <strong>{jobs.length === 0 ? '准备好后开始第一次复核' : '选择一条复核记录'}</strong>
              <span>{jobs.length === 0
                ? '复核会保留模型、提示词、原始回复、Token 和估算开销。'
                : '可查看模型、完整提示词、原始回复和运行状态。'}</span>
              {jobs.length === 0 && (
                <button
                  className={aiMode === 'auto' ? 'ai-auto-toggle active' : 'ai-auto-toggle'}
                  disabled={busy}
                  onClick={() => void toggleAutoReview()}
                  type="button"
                >
                  {aiMode === 'auto' ? <Pause size={15} /> : <Play size={15} />}
                  自动复核·{aiMode === 'auto' ? '已开启' : '开启'}
                </button>
              )}
            </div>
          )}
        </div>
      </section>
      {confirmation.dialog}
    </div>
  );
}

function AiFact({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong title={value}>{value}</strong></div>;
}

function AiTraceBlock({
  title,
  value,
  empty,
}: {
  title: string;
  value?: string | null;
  empty: string;
}) {
  const [copied, setCopied] = useState(false);
  const copiedTimerRef = useRef<number | null>(null);
  useEffect(() => () => {
    if (copiedTimerRef.current !== null) window.clearTimeout(copiedTimerRef.current);
  }, []);
  const copy = async () => {
    if (!value) return;
    await navigator.clipboard.writeText(value);
    setCopied(true);
    if (copiedTimerRef.current !== null) window.clearTimeout(copiedTimerRef.current);
    copiedTimerRef.current = window.setTimeout(() => {
      setCopied(false);
      copiedTimerRef.current = null;
    }, 1400);
  };
  return (
    <details className="ai-trace" open={title === 'AI 原始回复' && Boolean(value)}>
      <summary>
        <span>{title}</span>
        {value && (
          <button onClick={(event) => { event.preventDefault(); void copy(); }} type="button">
            {copied ? <Check size={14} /> : <Copy size={14} />}{copied ? '已复制' : '复制'}
          </button>
        )}
      </summary>
      {value ? <pre>{value}</pre> : <p>{empty}</p>}
    </details>
  );
}

function aiJobStatusLabel(status: string) {
  const labels: Record<string, string> = {
    pending: '排队中',
    running: '运行中',
    completed: '已复核',
    skipped: '未调用 AI',
    failed: '失败',
    downgraded: '已降级',
  };
  return labels[status] || status;
}

function aiProviderLabel(provider: string) {
  if (provider === 'codex-account') return '当前 Codex 账号';
  if (provider === 'openai-compatible') return 'OpenAI 兼容接口';
  return provider || '尚未开始';
}

function aiTraceEmptyText(job: AnalysisJob) {
  if (job.status === 'skipped') return '该批时间段已人工修正，因此没有向 AI 发送提示词';
  return job.completedAt ? '详细内容已按轻量保留策略自动清理' : '任务运行后记录';
}

function formatAiDateTime(value?: string | null) {
  if (!value) return '—';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '—' : formatDateTime(value);
}

function formatAiDuration(value?: number | null) {
  if (value == null) return '—';
  if (value < 1000) return `${value} ms`;
  return `${(value / 1000).toFixed(value < 10000 ? 1 : 0)} 秒`;
}

function formatAiTokens(value?: number | null) {
  if (!value) return '—';
  return value.toLocaleString('zh-CN');
}

function normalizeAiModel(value: string) {
  return value.toLocaleLowerCase('en-US').replace(/[^a-z0-9]/g, '');
}

function estimateAiCredits(job: AnalysisJob, rateCard: CodexRateCard | null) {
  if (job.provider !== 'codex-account' || !rateCard || !job.usage.totalTokens) return null;
  const model = normalizeAiModel(job.model);
  const rate = rateCard.rates.find((item) => normalizeAiModel(item.model) === model);
  if (!rate) return null;
  const cached = Math.min(job.usage.cachedInputTokens || 0, job.usage.inputTokens || 0);
  const uncached = Math.max(0, (job.usage.inputTokens || 0) - cached);
  return (
    uncached * rate.inputCreditsPerMillion
    + cached * rate.cachedInputCreditsPerMillion
    + (job.usage.outputTokens || 0) * rate.outputCreditsPerMillion
  ) / 1_000_000;
}

function formatAiCredits(job: AnalysisJob, rateCard: CodexRateCard | null) {
  const credits = estimateAiCredits(job, rateCard);
  if (credits == null) return job.usage.totalTokens > 0 ? '暂无匹配费率' : '—';
  return `${credits.toFixed(credits < 10 ? 4 : 2)} Credits`;
}

function codexPlanLabel(plan: string) {
  const labels: Record<string, string> = {
    plus: 'Plus · $20/月',
    'pro-5x': 'Pro 5x · $100/月',
    'pro-20x': 'Pro 20x · $200/月',
  };
  return labels[plan] || 'Codex 套餐';
}

function formatUsdEstimate(value: number) {
  const decimals = value < 0.01 ? 6 : value < 1 ? 4 : 2;
  return `≈ $${value.toFixed(decimals)}`;
}

function isEstimatedAiCost(job: AnalysisJob) {
  return job.provider === 'codex-account' || Boolean(job.usage.costNote?.includes('估算'));
}

function formatAiCost(job: AnalysisJob, rateCard: CodexRateCard | null) {
  if (job.usage.costUsd != null) {
    const value = `$${job.usage.costUsd.toFixed(job.usage.costUsd < 0.01 ? 6 : 4)}`;
    return isEstimatedAiCost(job) ? `≈ ${value}` : value;
  }
  const credits = estimateAiCredits(job, rateCard);
  if (job.provider === 'codex-account' && credits != null && rateCard) {
    return formatUsdEstimate(credits * rateCard.usdPerCredit);
  }
  return job.usage.costNote || (job.usage.totalTokens > 0 ? '未返回金额' : '—');
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
        title: displaySessionSummary(session),
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
  tasks,
  stats,
  selectedDate,
  idleCategory,
  selectionResetKey,
  onEdit,
  onOpenTimeline,
  planItems,
}: {
  sessions: WorkSession[];
  projects: Project[];
  tasks: Task[];
  stats: DayStats;
  selectedDate: string;
  idleCategory: string;
  selectionResetKey: number;
  onEdit: (sessions: WorkSession[]) => void;
  onOpenTimeline: () => void;
  planItems: DashboardData['planItems'];
}) {
  const [distributionMode, setDistributionMode] = useState<'category' | 'project'>('category');
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null);
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(null);
  const [selectedUnclassifiedCategory, setSelectedUnclassifiedCategory] = useState<string | null>(null);
  const [timelineZoom, setTimelineZoom] = useState(DEFAULT_TIMELINE_ZOOM);
  const [showDetailedSegments, setShowDetailedSegments] = useState(false);
  const review = sessions.filter(needsReview).slice(0, 4);
  const distributionRank = (category: string) => category === '无效'
    ? 2
    : category === '离开' || category === idleCategory
      ? 1
      : 0;
  const categoryDistributionRows = stats.categories
    .filter((item) => item.minutes > 0)
    .map((item, index) => ({ item, index }))
    .sort((left, right) => (
      distributionRank(left.item.category) - distributionRank(right.item.category)
      || left.index - right.index
    ))
    .map(({ item }) => item);
  const projectDistributionRows = projectBreakdown(
    sessions.filter((session) => (
      session.category !== '离开' && session.category !== idleCategory
    )),
    selectedDate,
  )
    .filter((item) => item.minutes > 0)
    .map((item, index) => ({ item, index }))
    .sort((left, right) => (
      distributionRank(left.item.category) - distributionRank(right.item.category)
      || left.index - right.index
    ))
    .map(({ item }) => ({
      ...item,
      color: projects.find((project) => project.id === item.id)?.color
        || categoryColor(item.category),
    }));
  const visibleDistributionRows = distributionMode === 'category'
    ? categoryDistributionRows.map((item) => ({
        key: `category:${item.category}`,
        label: item.category,
        category: item.category,
        minutes: item.minutes,
        color: categoryColor(item.category),
        projectId: '',
      }))
    : projectDistributionRows.map((item) => ({
        key: item.id || `unclassified:${item.category}`,
        label: item.id ? item.name : `${item.name} · ${item.category}`,
        category: item.category,
        minutes: item.minutes,
        color: item.color,
        projectId: item.id,
      }));
  const distributionTotal = visibleDistributionRows.reduce((sum, item) => sum + item.minutes, 0);
  const selectedProject = selectedProjectId
    ? projects.find((project) => project.id === selectedProjectId) || null
    : null;
  const visiblePlanItems = planItems
    .filter((item) => item.status !== 'done')
    .slice(0, 5);

  return (
    <div className="dashboard-grid">
      <section className="panel span-3">
        <PanelTitle
          title="时间分布"
          subtitle={`${visibleDistributionRows.length} 个${distributionMode === 'category' ? '分类' : '项目'} · ${formatDuration(distributionTotal)}`}
          action={(
            <div className="distribution-mode-tabs" role="radiogroup" aria-label="时间分布聚合方式">
              <button
                aria-checked={distributionMode === 'category'}
                className={distributionMode === 'category' ? 'active' : ''}
                onClick={() => setDistributionMode('category')}
                role="radio"
                type="button"
              >
                分类
              </button>
              <button
                aria-checked={distributionMode === 'project'}
                className={distributionMode === 'project' ? 'active' : ''}
                onClick={() => setDistributionMode('project')}
                role="radio"
                type="button"
              >
                项目
              </button>
            </div>
          )}
        />
        {stats.activeMinutes === 0 && stats.idleMinutes === 0 ? (
          <EmptyState title="这一天还没有记录" detail="保持 ScreenUse 在托盘运行即可自动出现数据。" />
        ) : (
          <>
            <div className="distribution-bar" aria-label={`${distributionMode === 'category' ? '分类' : '项目'}时间分布`}>
              {visibleDistributionRows
                .filter((item) => item.category !== '离开' && item.category !== idleCategory)
                .map((item) => (
                  <button
                    key={item.key}
                    title={`${item.label} ${formatDuration(item.minutes)}`}
                    aria-label={`查看${item.label}的具体时间段`}
                    onClick={() => {
                      if (distributionMode === 'category') setSelectedCategory(item.category);
                      else if (item.projectId) setSelectedProjectId(item.projectId);
                      else setSelectedUnclassifiedCategory(item.category);
                    }}
                    type="button"
                    style={
                      {
                        '--segment-color': item.color,
                        flexGrow: item.minutes,
                      } as CSSProperties
                    }
                  />
                ))}
            </div>
            <div className="distribution-list">
              {visibleDistributionRows.map((item) => (
                <button
                  key={item.key}
                  className={`distribution-row ${distributionMode}`}
                  onClick={() => {
                    if (distributionMode === 'category') setSelectedCategory(item.category);
                    else if (item.projectId) setSelectedProjectId(item.projectId);
                    else setSelectedUnclassifiedCategory(item.category);
                  }}
                  type="button"
                >
                  <span
                    className="legend-dot"
                    style={{ background: item.color }}
                  />
                  <strong title={item.label}>{item.label}</strong>
                  <div className="mini-track">
                    <span
                      style={
                        {
                          width: `${Math.max(
                            3,
                            Math.round(
                              (item.minutes /
                                Math.max(1, distributionTotal)) *
                                100,
                            ),
                          )}%`,
                          background: item.color,
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

      <section className="panel span-3 day-track-panel">
        <PanelTitle
          title="今日时间段"
          action={
            <div className="timeline-controls">
              <button
                aria-label={showDetailedSegments ? '合并相邻的相同任务' : '显示每个原始时间段'}
                aria-pressed={showDetailedSegments}
                className={`timeline-detail-toggle${showDetailedSegments ? ' active' : ''}`}
                onClick={() => setShowDetailedSegments((value) => !value)}
                title={showDetailedSegments ? '合并相邻的相同任务' : '显示每个原始时间段'}
                type="button"
              >
                <SplitSquareHorizontal size={15} />
              </button>
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
            </div>
          }
        />
        <DayActivityTimeline
          detailed={showDetailedSegments}
          sessions={sessions}
          selectedDate={selectedDate}
          zoom={timelineZoom}
          onZoomChange={setTimelineZoom}
          selectionResetKey={selectionResetKey}
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
              <button key={session.id} onClick={() => onEdit([session])} type="button">
                <CircleAlert size={16} />
                <span>
                  <strong>{displaySessionSummary(session)}</strong>
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
          selectionResetKey={selectionResetKey}
          onClose={() => setSelectedCategory(null)}
          onEdit={onEdit}
        />
      )}
      {selectedProject && (
        <ProjectTodayDetailModal
          project={selectedProject}
          sessions={sessions.filter((session) => session.projectId === selectedProject.id)}
          tasks={tasks.filter((task) => task.projectId === selectedProject.id)}
          selectedDate={selectedDate}
          selectionResetKey={selectionResetKey}
          onClose={() => setSelectedProjectId(null)}
          onEdit={onEdit}
        />
      )}
      {selectedUnclassifiedCategory && (
        <CategoryDetailModal
          category={`未归类 · ${selectedUnclassifiedCategory}`}
          sessions={sessions.filter((session) => (
            !session.projectId && session.category === selectedUnclassifiedCategory
          ))}
          selectedDate={selectedDate}
          selectionResetKey={selectionResetKey}
          onClose={() => setSelectedUnclassifiedCategory(null)}
          onEdit={onEdit}
        />
      )}
    </div>
  );
}

function DayActivityTimeline({
  detailed,
  sessions,
  selectedDate,
  zoom,
  onZoomChange,
  selectionResetKey,
  onEdit,
}: {
  detailed: boolean;
  sessions: WorkSession[];
  selectedDate: string;
  zoom: number;
  onZoomChange: (zoom: number) => void;
  selectionResetKey: number;
  onEdit: (sessions: WorkSession[]) => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const centerRef = useRef<{ date: string; seconds: number } | null>(null);
  const wheelAtRef = useRef(0);
  const [viewport, setViewport] = useState({ left: 0, width: 0 });
  const [expandedGroup, setExpandedGroup] = useState<{
    sessionIds: string[];
    startedAt: string;
    endedAt: string;
  } | null>(null);
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
  const groups = useMemo(() => {
    type TimelineGroup = {
      id: string;
      sessions: WorkSession[];
      startedAt: string;
      endedAt: string;
      untracked: boolean;
    };
    const result: TimelineGroup[] = [];
    for (const session of sorted) {
      const previous = result[result.length - 1];
      const previousSession = previous?.sessions[previous.sessions.length - 1];
      const touchesPrevious = previous
        && new Date(session.startedAt).getTime() <= new Date(previous.endedAt).getTime() + 5_000;
      if (!detailed && previous && previousSession
        && touchesPrevious && sameTimelineTask(previousSession, session)) {
        previous.sessions.push(session);
        if (new Date(session.endedAt).getTime() > new Date(previous.endedAt).getTime()) {
          previous.endedAt = session.endedAt;
        }
      } else {
        result.push({
          id: session.id,
          sessions: [session],
          startedAt: session.startedAt,
          endedAt: session.endedAt,
          untracked: false,
        });
      }
    }
    const continuous: TimelineGroup[] = [];
    for (const group of result) {
      const previous = continuous[continuous.length - 1];
      if (previous) {
        const gapSeconds = Math.max(
          0,
          (new Date(group.startedAt).getTime() - new Date(previous.endedAt).getTime()) / 1000,
        );
        if (gapSeconds > 0 && gapSeconds <= MAX_TIMELINE_GAP_SNAP_SECONDS) {
          previous.endedAt = group.startedAt;
        } else if (gapSeconds > MAX_TIMELINE_GAP_SNAP_SECONDS) {
          const gapId = `timeline-gap:${previous.endedAt}:${group.startedAt}`;
          continuous.push({
            id: gapId,
            sessions: [{
              id: gapId,
              startedAt: previous.endedAt,
              endedAt: group.startedAt,
              projectName: '采集状态',
              category: '未记录',
              summary: '未记录/采集暂停',
              confidence: 1,
              evidence: [{
                kind: 'system',
                label: '原因',
                value: 'ScreenUse 未运行、正在重启或自动记录曾暂停',
                weight: 1,
              }],
              userConfirmed: true,
              source: 'timeline-gap',
            }],
            startedAt: previous.endedAt,
            endedAt: group.startedAt,
            untracked: true,
          });
        }
      }
      continuous.push(group);
    }
    return continuous;
  }, [detailed, sorted]);
  const expandedSessions = expandedGroup
    ? sorted.filter((session) => expandedGroup.sessionIds.includes(session.id))
    : [];
  useEffect(() => {
    setExpandedGroup(null);
  }, [detailed, selectedDate]);
  useEffect(() => {
    if (expandedGroup && !expandedSessions.length) setExpandedGroup(null);
  }, [expandedGroup, expandedSessions.length]);
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
      if (event.deltaY < 0) {
        const bounds = element.getBoundingClientRect();
        const pointerX = Math.min(
          element.clientWidth,
          Math.max(0, event.clientX - bounds.left),
        );
        centerRef.current = {
          date: selectedDate,
          seconds: (element.scrollLeft + pointerX) / pixelsPerSecond,
        };
      } else {
        centerRef.current = {
          date: selectedDate,
          seconds: (element.scrollLeft + element.clientWidth / 2) / pixelsPerSecond,
        };
      }
      onZoomChange(Math.max(
        0,
        Math.min(TIMELINE_SCALES.length - 1, zoom + (event.deltaY < 0 ? 1 : -1)),
      ));
    };
    element.addEventListener('wheel', handleWheel, { passive: false });
    return () => element.removeEventListener('wheel', handleWheel);
  }, [onZoomChange, pixelsPerSecond, selectedDate, zoom]);

  const axisLabelPadding = 34;
  const firstVisiblePixel = viewport.left > 0 ? viewport.left + axisLabelPadding : 0;
  const lastVisiblePixel = viewport.left + viewport.width < width
    ? viewport.left + viewport.width - axisLabelPadding
    : width;
  const firstTick = Math.max(
    0,
    Math.ceil((firstVisiblePixel / pixelsPerSecond) / scale.secondsPerGrid),
  );
  const lastTick = Math.min(
    Math.floor((24 * 60 * 60) / scale.secondsPerGrid),
    Math.floor((lastVisiblePixel / pixelsPerSecond) / scale.secondsPerGrid),
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
            {groups.map((group) => {
              const primary = group.sessions[0];
              const displaySession = group.sessions.length === 1
                ? primary
                : {
                    ...primary,
                    startedAt: group.startedAt,
                    endedAt: group.endedAt,
                    summary: primary.taskTitle || primary.summary,
                    evidence: group.sessions[group.sessions.length - 1].evidence,
                  };
              const bounds = sessionBoundsOnDate(displaySession, selectedDate);
              const applications = [...new Set(group.sessions.map(sessionApplication))];
              const app = group.untracked
                ? 'ScreenUse 未运行、正在重启或自动记录曾暂停'
                : `${applications.slice(0, 3).join('、')}${applications.length > 3 ? ` 等 ${applications.length} 个应用` : ''}${group.sessions.length > 1 ? ` · 合并 ${group.sessions.length} 段` : ''}`;
              const blockWidth = Math.max(1, Math.max(5, bounds.durationSeconds) * pixelsPerSecond);
              const timeRange = `${formatTimelineClock(group.startedAt, true)}–${formatTimelineClock(group.endedAt, true)}`;
              const actionLabel = group.sessions.length > 1
                ? `点击查看 ${group.sessions.length} 个具体时间段`
                : '点击修正';
              return (
                <button
                  aria-disabled={group.untracked}
                  aria-label={group.untracked
                    ? `${timeRange}，${displaySessionSummary(displaySession)}，${app}，悬浮查看原因`
                    : `${timeRange}，${displaySessionSummary(displaySession)}，${app}，${actionLabel}`}
                  className={`day-track-block${group.untracked ? ' untracked' : ''}${!group.untracked && group.sessions.some(needsReview) ? ' needs-review' : ''}`}
                  key={group.id}
                  onBlur={() => setTooltip(null)}
                  onClick={() => {
                    setTooltip(null);
                    if (group.untracked) return;
                    if (group.sessions.length > 1) {
                      setExpandedGroup({
                        sessionIds: group.sessions.map((session) => session.id),
                        startedAt: group.startedAt,
                        endedAt: group.endedAt,
                      });
                    } else {
                      onEdit(group.sessions);
                    }
                  }}
                  onFocus={(event) => showSessionTooltip(event.currentTarget, displaySession, app, timeRange)}
                  onMouseEnter={(event) => showSessionTooltip(event.currentTarget, displaySession, app, timeRange)}
                  onMouseLeave={() => setTooltip(null)}
                  type="button"
                  style={
                    {
                      left: bounds.startSeconds * pixelsPerSecond,
                      width: blockWidth,
                      '--block-color': categoryColor(group.untracked ? '未记录' : primary.category),
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
          <strong>{displaySessionSummary(tooltip.session)}</strong>
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
      {expandedGroup && expandedSessions.length > 0 && (
        <CategoryDetailModal
          category={expandedSessions[0].category}
          sessions={expandedSessions}
          selectedDate={selectedDate}
          title={expandedSessions[0].taskTitle || expandedSessions[0].projectName || '合并时间段'}
          contextLabel={`${formatTimelineClock(expandedGroup.startedAt, true)}–${formatTimelineClock(expandedGroup.endedAt, true)}`}
          selectionResetKey={selectionResetKey}
          onClose={() => setExpandedGroup(null)}
          onEdit={onEdit}
        />
      )}
    </>
  );
}

function matchesSessionSearch(session: WorkSession, query: string) {
  if (!query) return true;
  const searchable = normalizeSearchText([
    session.summary,
    session.category,
    session.projectName,
    session.taskTitle,
    sessionApplication(session),
    formatSessionMoment(session.startedAt, true),
    formatSessionMoment(session.endedAt, true),
    session.source,
    ...session.evidence.flatMap((item) => [item.label, item.value]),
  ].filter(Boolean).join(' '));
  return query.split(' ').filter(Boolean).every((token) => searchable.includes(token));
}

function SessionDetailSearch({
  value,
  resultCount,
  totalCount,
  onChange,
}: {
  value: string;
  resultCount: number;
  totalCount: number;
  onChange: (value: string) => void;
}) {
  return (
    <div className="detail-search-row">
      <label className="search-field detail-search">
        <Search size={15} />
        <input
          aria-label="搜索时间段"
          placeholder="搜索摘要、当前页面、应用、项目或任务"
          value={value}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Escape' && value) {
              event.preventDefault();
              onChange('');
            }
          }}
        />
        {value && (
          <button aria-label="清空时间段搜索" onClick={() => onChange('')} type="button">
            <X size={14} />
          </button>
        )}
      </label>
      {value && <span className="detail-search-count">{resultCount}/{totalCount} 段</span>}
    </div>
  );
}

function ProjectTodayDetailModal({
  project,
  sessions,
  tasks,
  selectedDate,
  selectionResetKey,
  onClose,
  onEdit,
}: {
  project: Project;
  sessions: WorkSession[];
  tasks: Task[];
  selectedDate: string;
  selectionResetKey: number;
  onClose: () => void;
  onEdit: (sessions: WorkSession[]) => void;
}) {
  const [selectedTaskKey, setSelectedTaskKey] = useState('all');
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [query, setQuery] = useState('');
  const selectAllRef = useRef<HTMLInputElement>(null);
  const taskById = useMemo(() => new Map(tasks.map((task) => [task.id, task])), [tasks]);
  const sortedProjectSessions = useMemo(
    () => [...sessions].sort(
      (left, right) => new Date(left.startedAt).getTime() - new Date(right.startedAt).getTime(),
    ),
    [sessions],
  );
  const taskRows = useMemo(() => {
    const groups = new Map<string, { key: string; title: string; minutes: number; sessions: WorkSession[] }>();
    for (const session of sortedProjectSessions) {
      const key = session.taskId || 'unassigned';
      const title = session.taskId
        ? session.taskTitle || taskById.get(session.taskId)?.title || '已删除任务'
        : '未归属任务';
      const current = groups.get(key) || { key, title, minutes: 0, sessions: [] };
      current.minutes += sessionMinutesOnDate(session, selectedDate);
      current.sessions.push(session);
      groups.set(key, current);
    }
    return [...groups.values()]
      .filter((row) => row.minutes > 0)
      .sort((left, right) => right.minutes - left.minutes)
      .map((row, index) => ({ ...row, color: TASK_CHART_COLORS[index % TASK_CHART_COLORS.length] }));
  }, [selectedDate, sortedProjectSessions, taskById]);
  const total = taskRows.reduce((sum, row) => sum + row.minutes, 0);
  const activeTask = taskRows.find((row) => row.key === selectedTaskKey) || null;
  const taskSessions = activeTask?.sessions || sortedProjectSessions;
  const normalizedQuery = normalizeSearchText(query);
  const visibleSessions = taskSessions.filter((session) => matchesSessionSearch(session, normalizedQuery));
  const selectedSessions = taskSessions.filter((session) => selected.has(session.id));
  const visibleSelectedCount = visibleSessions.reduce(
    (count, session) => count + Number(selected.has(session.id)),
    0,
  );
  const allSelected = visibleSessions.length > 0 && visibleSelectedCount === visibleSessions.length;
  const visibleMinutes = visibleSessions.reduce(
    (sum, session) => sum + sessionMinutesOnDate(session, selectedDate),
    0,
  );
  let pieCursor = 0;
  const pieBackground = taskRows.length
    ? `conic-gradient(${taskRows.map((row) => {
      const start = pieCursor;
      pieCursor += (row.minutes / Math.max(total, 0.001)) * 100;
      return `${row.color} ${start.toFixed(2)}% ${pieCursor.toFixed(2)}%`;
    }).join(', ')})`
    : 'var(--track)';

  useEffect(() => {
    if (selectAllRef.current) {
      selectAllRef.current.indeterminate = visibleSelectedCount > 0 && !allSelected;
    }
  }, [allSelected, visibleSelectedCount]);
  useEffect(() => setSelected(new Set()), [selectionResetKey]);

  const chooseTask = (key: string) => {
    setSelectedTaskKey(key);
    setSelected(new Set());
    setQuery('');
  };
  const toggleAllVisible = (checked: boolean) => {
    setSelected((current) => {
      const next = new Set(current);
      visibleSessions.forEach((session) => {
        if (checked) next.add(session.id);
        else next.delete(session.id);
      });
      return next;
    });
  };
  const toggle = (id: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const editSession = (session: WorkSession) => {
    onEdit(selected.has(session.id) && selectedSessions.length > 1 ? selectedSessions : [session]);
  };

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <section className="modal project-today-detail" role="dialog" aria-modal="true" aria-label={`${project.name}今日投入`}>
        <div className="modal-head project-today-head">
          <div className="category-detail-title">
            <span style={{ background: project.color || categoryColor(project.category) }} />
            <div>
              <h2>{project.name}</h2>
              <p>{project.category} · {formatDuration(total)} · {sortedProjectSessions.length} 个时间段</p>
            </div>
          </div>
          <button className="icon-button" onClick={onClose} type="button" aria-label="关闭">
            <X size={17} />
          </button>
        </div>

        <section className="project-task-summary" aria-label="今日任务分布">
          <div className="project-task-chart">
            <div className="task-pie" style={{ background: pieBackground }} role="img" aria-label={`${project.name}今日任务饼图`}>
              <span><strong>{formatCompactDuration(total)}</strong><small>今日投入</small></span>
            </div>
          </div>
          <div className="project-task-breakdown">
            <div className="project-task-breakdown-head">
              <div><strong>任务分布</strong><span>{taskRows.length} 项</span></div>
              <button className={selectedTaskKey === 'all' ? 'active' : ''} onClick={() => chooseTask('all')} type="button">
                全部时间段
              </button>
            </div>
            <div className="project-task-rows">
              {taskRows.map((row) => {
                const percent = Math.round((row.minutes / Math.max(total, 0.001)) * 100);
                return (
                  <button
                    aria-pressed={selectedTaskKey === row.key}
                    className={selectedTaskKey === row.key ? 'active' : ''}
                    key={row.key}
                    onClick={() => chooseTask(row.key)}
                    type="button"
                  >
                    <i style={{ background: row.color }} />
                    <span><strong>{row.title}</strong><small>{row.sessions.length} 个时间段</small></span>
                    <b>{formatDuration(row.minutes)}</b>
                    <em>{percent}%</em>
                    <ChevronRight size={15} />
                  </button>
                );
              })}
            </div>
          </div>
        </section>

        <section className="project-session-detail">
          <div className="project-session-detail-head">
            <div>
              <h3>{activeTask?.title || '全部时间段'}</h3>
              <p>{formatDuration(normalizedQuery ? visibleMinutes : activeTask?.minutes || total)} · {visibleSessions.length}{normalizedQuery ? `/${taskSessions.length}` : ''} 段</p>
            </div>
            <div className="category-detail-actions">
              <label className="selection-toggle" title={`选择当前 ${visibleSessions.length} 个时间段`}>
                <input
                  className="themed-checkbox"
                  ref={selectAllRef}
                  type="checkbox"
                  checked={allSelected}
                  onChange={(event) => toggleAllVisible(event.target.checked)}
                />
                全选
              </label>
              <button className="selection-clear" disabled={!selectedSessions.length} onClick={() => setSelected(new Set())} type="button">
                全不选
              </button>
              <button disabled={!selectedSessions.length} onClick={() => onEdit(selectedSessions)} type="button">
                <Pencil size={15} />批量修正 {selectedSessions.length || ''}
              </button>
            </div>
          </div>
          <SessionDetailSearch
            value={query}
            resultCount={visibleSessions.length}
            totalCount={taskSessions.length}
            onChange={setQuery}
          />
          <div className="category-session-list">
            {!visibleSessions.length && (
              <EmptyState
                title="没有匹配的时间段"
                detail="可以搜索摘要、当前页面、应用、项目或任务名称。"
              />
            )}
            {visibleSessions.map((session) => (
              <div className={selected.has(session.id) ? 'selected' : ''} key={session.id}>
                <label className="category-session-check" aria-label={`选择 ${displaySessionSummary(session)}`}>
                  <input className="themed-checkbox" checked={selected.has(session.id)} onChange={() => toggle(session.id)} type="checkbox" />
                </label>
                <button className="category-session-open" onClick={() => editSession(session)} type="button">
                  <span className="category-session-time">
                    <strong>{formatTimelineClock(session.startedAt, true)}</strong>
                    <small>{formatTimelineClock(session.endedAt, true)}</small>
                  </span>
                  <span className="category-session-main">
                    <strong>{displaySessionSummary(session)}</strong>
                    <small>{session.taskTitle || '未归属任务'}</small>
                  </span>
                  <span className="category-session-app">{sessionApplication(session)}</span>
                  <Pencil size={15} />
                </button>
              </div>
            ))}
          </div>
        </section>
      </section>
    </div>
  );
}

function CategoryDetailModal({
  category,
  sessions,
  selectedDate,
  title,
  contextLabel,
  durationForSession,
  showDate = false,
  selectionResetKey,
  onClose,
  onEdit,
}: {
  category: string;
  sessions: WorkSession[];
  selectedDate: string;
  title?: string;
  contextLabel?: string;
  durationForSession?: (session: WorkSession) => number;
  showDate?: boolean;
  selectionResetKey: number;
  onClose: () => void;
  onEdit: (sessions: WorkSession[]) => void;
}) {
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [query, setQuery] = useState('');
  const allSorted = useMemo(
    () => [...sessions].sort(
      (left, right) => new Date(left.startedAt).getTime() - new Date(right.startedAt).getTime(),
    ),
    [sessions],
  );
  const normalizedQuery = normalizeSearchText(query);
  const sorted = allSorted.filter((session) => matchesSessionSearch(session, normalizedQuery));
  const selectedSessions = allSorted.filter((session) => selected.has(session.id));
  const visibleSelectedCount = sorted.reduce(
    (count, session) => count + Number(selected.has(session.id)),
    0,
  );
  const total = sorted.reduce(
    (sum, session) => sum + (durationForSession
      ? durationForSession(session)
      : sessionMinutesOnDate(session, selectedDate)),
    0,
  );
  const allSelected = sorted.length > 0 && visibleSelectedCount === sorted.length;
  const selectAllRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (selectAllRef.current) {
      selectAllRef.current.indeterminate = visibleSelectedCount > 0 && !allSelected;
    }
  }, [allSelected, visibleSelectedCount]);
  useEffect(() => setSelected(new Set()), [selectionResetKey]);
  const editSession = (session: WorkSession) => {
    const belongsToSelection = selected.has(session.id);
    onEdit(belongsToSelection && selectedSessions.length > 1 ? selectedSessions : [session]);
  };
  const toggle = (id: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const toggleAllVisible = (checked: boolean) => {
    setSelected((current) => {
      const next = new Set(current);
      sorted.forEach((session) => {
        if (checked) next.add(session.id);
        else next.delete(session.id);
      });
      return next;
    });
  };
  const heading = title || category;
  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <section className={`modal category-detail ${showDate ? 'show-session-date' : ''}`} role="dialog" aria-modal="true" aria-label={`${heading}时间段`}>
        <div className="modal-head category-detail-head">
          <div className="category-detail-title">
            <span style={{ background: categoryColor(category) }} />
            <div>
              <h2>{heading}</h2>
              <p>{formatDuration(total)} · {sorted.length}{normalizedQuery ? `/${allSorted.length}` : ''} 个{title ? '具体' : ''}时间段{contextLabel ? ` · ${contextLabel}` : ''}</p>
            </div>
          </div>
          <div className="category-detail-actions">
            <label className="selection-toggle" title={`选择全部 ${sorted.length} 个时间段`}>
              <input
                className="themed-checkbox"
                ref={selectAllRef}
                type="checkbox"
                checked={allSelected}
                onChange={(event) => toggleAllVisible(event.target.checked)}
              />
              全选
            </label>
            <button
              className="selection-clear"
              disabled={!selected.size}
              onClick={() => setSelected(new Set())}
              type="button"
            >
              全不选
            </button>
            <button
              disabled={!selectedSessions.length}
              onClick={() => onEdit(selectedSessions)}
              type="button"
            >
              <Pencil size={15} />批量修正 {selectedSessions.length || ''}
            </button>
          </div>
          <button className="icon-button" onClick={onClose} type="button" aria-label="关闭">
            <X size={17} />
          </button>
        </div>
        <SessionDetailSearch
          value={query}
          resultCount={sorted.length}
          totalCount={allSorted.length}
          onChange={setQuery}
        />
        <div className="category-session-list">
          {!sorted.length && (
            <EmptyState
              title={normalizedQuery ? '没有匹配的时间段' : '所选区间没有时间段'}
              detail={normalizedQuery
                ? '可以搜索摘要、当前页面、应用、项目或任务名称。'
                : '切换统计范围后可查看该项目或任务的其他记录。'}
            />
          )}
          {sorted.map((session) => (
            <div className={selected.has(session.id) ? 'selected' : ''} key={session.id}>
              <label className="category-session-check" aria-label={`选择 ${displaySessionSummary(session)}`}>
                <input
                  className="themed-checkbox"
                  checked={selected.has(session.id)}
                  onChange={() => toggle(session.id)}
                  type="checkbox"
                />
              </label>
              <button className="category-session-open" onClick={() => editSession(session)} type="button">
              <span className="category-session-time">
                  <strong>{formatSessionMoment(session.startedAt, showDate)}</strong>
                  <small>{formatSessionMoment(session.endedAt, showDate)}</small>
              </span>
              <span className="category-session-main">
                <strong>{displaySessionSummary(session)}</strong>
                <small>{session.projectName || '未归类'}{session.taskTitle ? ` · ${session.taskTitle}` : ''}</small>
              </span>
              <span className="category-session-app">{sessionApplication(session)}</span>
              <Pencil size={15} />
              </button>
            </div>
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
  const [categoryFilter, setCategoryFilter] = useState('all');
  const [projectFilter, setProjectFilter] = useState('all');
  const [taskFilter, setTaskFilter] = useState('all');
  const [appFilter, setAppFilter] = useState('all');
  const [statusFilter, setStatusFilter] = useState<'all' | 'review' | 'confirmed' | 'automatic' | 'idle'>('all');
  const [mergeDialogOpen, setMergeDialogOpen] = useState(false);
  const [mergeBusy, setMergeBusy] = useState(false);
  const normalized = normalizeSearchText(query);
  const categoryFilters = useMemo(
    () => [...new Set(sessions.map((session) => session.category))]
      .sort((left, right) => left.localeCompare(right, 'zh-CN')),
    [sessions],
  );
  const projectFilters = useMemo(() => {
    const names = new Map<string, string>();
    const projectsById = new Map(projects.map((project) => [project.id, project]));
    for (const session of sessions) {
      if (!session.projectId || (categoryFilter !== 'all' && session.category !== categoryFilter)) {
        continue;
      }
      names.set(
        session.projectId,
        projectsById.get(session.projectId)?.name || session.projectName || '已删除项目',
      );
    }
    return [...names.entries()].sort((left, right) => left[1].localeCompare(right[1], 'zh-CN'));
  }, [categoryFilter, projects, sessions]);
  const taskFilters = useMemo(() => {
    const names = new Map<string, string>();
    const tasksById = new Map(tasks.map((task) => [task.id, task]));
    for (const session of sessions) {
      if (!session.taskId
        || (categoryFilter !== 'all' && session.category !== categoryFilter)
        || (projectFilter !== 'all' && projectFilter !== 'unassigned' && session.projectId !== projectFilter)
        || (projectFilter === 'unassigned' && session.projectId)) {
        continue;
      }
      names.set(
        session.taskId,
        tasksById.get(session.taskId)?.title || session.taskTitle || '已删除任务',
      );
    }
    return [...names.entries()].sort((left, right) => left[1].localeCompare(right[1], 'zh-CN'));
  }, [categoryFilter, projectFilter, sessions, tasks]);
  const appFilters = useMemo(
    () => [...new Set(sessions.map(sessionApplication))]
      .sort((left, right) => left.localeCompare(right, 'zh-CN')),
    [sessions],
  );
  const sessionIndexById = useMemo(
    () => new Map(sessions.map((session, index) => [session.id, index])),
    [sessions],
  );
  const filtered = useMemo(() => sessions.filter((session) => {
    if (categoryFilter !== 'all' && session.category !== categoryFilter) return false;
    if (projectFilter === 'unassigned' && session.projectId) return false;
    if (projectFilter !== 'all' && projectFilter !== 'unassigned' && session.projectId !== projectFilter) {
      return false;
    }
    if (taskFilter === 'unassigned' && session.taskId) return false;
    if (taskFilter !== 'all' && taskFilter !== 'unassigned' && session.taskId !== taskFilter) {
      return false;
    }
    if (appFilter !== 'all' && sessionApplication(session) !== appFilter) return false;
    if (statusFilter === 'review' && !needsReview(session)) return false;
    if (statusFilter === 'confirmed' && !session.userConfirmed) return false;
    if (statusFilter === 'automatic'
      && (session.userConfirmed || needsReview(session) || isIdleSession(session))) return false;
    if (statusFilter === 'idle' && !isIdleSession(session)) return false;
    if (!normalized) return true;
    return normalizeSearchText([
      session.summary,
      session.category,
      session.projectName,
      session.taskTitle,
      ...session.evidence.map((item) => item.value),
    ].filter(Boolean).join(' ')).includes(normalized);
  }), [
    appFilter,
    categoryFilter,
    normalized,
    projectFilter,
    sessions,
    statusFilter,
    taskFilter,
  ]);
  const activeFilterCount = Number(Boolean(normalized))
    + Number(categoryFilter !== 'all')
    + Number(projectFilter !== 'all')
    + Number(taskFilter !== 'all')
    + Number(appFilter !== 'all')
    + Number(statusFilter !== 'all');
  const selectedSessions = sessions.filter((session) => selected.has(session.id));
  const filteredSelectedCount = filtered.reduce(
    (count, session) => count + Number(selected.has(session.id)),
    0,
  );
  const allFilteredSelected = filtered.length > 0 && filteredSelectedCount === filtered.length;
  const selectAllRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (selectAllRef.current) {
      selectAllRef.current.indeterminate = filteredSelectedCount > 0 && !allFilteredSelected;
    }
  }, [allFilteredSelected, filteredSelectedCount]);

  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  };

  const toggleAllFiltered = (checked: boolean) => {
    const next = new Set(selected);
    filtered.forEach((session) => {
      if (checked) next.add(session.id);
      else next.delete(session.id);
    });
    setSelected(next);
  };

  const mergeSelected = async (summary: string) => {
    const ids = [...selected];
    if (ids.length < 2) return;
    setMergeBusy(true);
    try {
      await runAction(() => api.mergeSessions(ids, summary), '已合并所选会话');
      setSelected(new Set());
      setMergeDialogOpen(false);
    } finally {
      setMergeBusy(false);
    }
  };

  const editSession = (session: WorkSession) => {
    const belongsToSelection = selected.has(session.id);
    onEdit(belongsToSelection && selectedSessions.length > 1 ? selectedSessions : [session]);
  };

  const clearFilters = () => {
    setQuery('');
    setCategoryFilter('all');
    setProjectFilter('all');
    setTaskFilter('all');
    setAppFilter('all');
    setStatusFilter('all');
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
          <div className="selection-controls">
            <label className="selection-toggle" title={`选择当前显示的 ${filtered.length} 个时间段`}>
              <input
                className="themed-checkbox"
                ref={selectAllRef}
                type="checkbox"
                checked={allFilteredSelected}
                disabled={!filtered.length}
                onChange={(event) => toggleAllFiltered(event.target.checked)}
              />
              全选
            </label>
            <button
              className="selection-clear"
              disabled={!selected.size}
              onClick={() => setSelected(new Set())}
              type="button"
            >
              全不选
            </button>
          </div>
          <button
            disabled={!selectedSessions.length}
            onClick={() => onEdit(selectedSessions)}
            type="button"
          >
            <Pencil size={15} />修正 {selectedSessions.length || ''}
          </button>
          <button
            disabled={selected.size < 2}
            onClick={() => setMergeDialogOpen(true)}
            type="button"
          >
            <Merge size={15} />合并 {selected.size || ''}
          </button>
        </div>

        <div className="timeline-list">
          {filtered.map((session, index) => {
            const newer = index > 0 ? filtered[index - 1] : null;
            const newerIndex = newer ? sessionIndexById.get(newer.id) ?? -1 : -1;
            const currentIndex = sessionIndexById.get(session.id) ?? -1;
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
              title="没有匹配的活动"
              detail={activeFilterCount ? '清空或调整右侧筛选条件后再试。' : '当天还没有可显示的时间段。'}
            />
          )}
        </div>
      </section>
      <aside className="panel timeline-filter-panel">
        <div className="timeline-filter-head">
          <div>
            <span><ListFilter size={16} /></span>
            <div>
              <h2>筛选条件</h2>
              <p>显示 {filtered.length}/{sessions.length} 个时间段</p>
            </div>
          </div>
          <button disabled={!activeFilterCount} onClick={clearFilters} type="button">
            <X size={13} />清空
          </button>
        </div>
        <div className="timeline-filter-fields">
          <label>
            <span>状态</span>
            <select aria-label="状态筛选" value={statusFilter} onChange={(event) => setStatusFilter(event.target.value as typeof statusFilter)}>
              <option value="all">全部状态</option>
              <option value="review">待复核</option>
              <option value="confirmed">已确认</option>
              <option value="automatic">自动判断</option>
              <option value="idle">离开/空闲</option>
            </select>
          </label>
          <label>
            <span>分类</span>
            <select aria-label="分类筛选" value={categoryFilter} onChange={(event) => {
              setCategoryFilter(event.target.value);
              setProjectFilter('all');
              setTaskFilter('all');
            }}>
              <option value="all">全部分类</option>
              {categoryFilters.map((category) => <option key={category} value={category}>{category}</option>)}
            </select>
          </label>
          <label>
            <span>项目</span>
            <select aria-label="项目筛选" value={projectFilter} onChange={(event) => {
              setProjectFilter(event.target.value);
              setTaskFilter('all');
            }}>
              <option value="all">全部项目</option>
              <option value="unassigned">未归类项目</option>
              {projectFilters.map(([id, name]) => <option key={id} value={id}>{name}</option>)}
            </select>
          </label>
          <label>
            <span>任务</span>
            <select aria-label="任务筛选" value={taskFilter} onChange={(event) => setTaskFilter(event.target.value)}>
              <option value="all">全部任务</option>
              <option value="unassigned">未归类任务</option>
              {taskFilters.map(([id, title]) => <option key={id} value={id}>{title}</option>)}
            </select>
          </label>
          <label>
            <span>应用</span>
            <select aria-label="应用筛选" value={appFilter} onChange={(event) => setAppFilter(event.target.value)}>
              <option value="all">全部应用</option>
              {appFilters.map((app) => <option key={app} value={app}>{app}</option>)}
            </select>
          </label>
        </div>
        <div className={`timeline-filter-state ${activeFilterCount ? 'active' : ''}`}>
          {activeFilterCount ? `已启用 ${activeFilterCount} 项筛选` : '当前显示全部记录'}
        </div>
      </aside>
      {mergeDialogOpen && (
        <TextInputDialog
          busy={mergeBusy}
          confirmLabel="确认合并"
          detail={`将 ${selectedSessions.length} 条记录合并为一个连续时间段。`}
          initialValue={selectedSessions[0]?.summary || '连续工作会话'}
          inputLabel="合并后的会话名称"
          onCancel={() => {
            if (!mergeBusy) setMergeDialogOpen(false);
          }}
          onConfirm={(value) => void mergeSelected(value)}
          title="合并会话"
        />
      )}
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
        <input className="themed-checkbox" type="checkbox" checked={selected} onChange={onToggle} />
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
          <strong>{displaySessionSummary(session)}</strong>
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
  idleCategory,
  focusProjectId,
  selectionResetKey,
  onEdit,
}: {
  projects: Project[];
  tasks: Task[];
  sessions: WorkSession[];
  selectedDate: string;
  runAction: ActionRunner;
  categoryOptions: CategoryOption[];
  idleCategory: string;
  focusProjectId: string;
  selectionResetKey: number;
  onEdit: (sessions: WorkSession[]) => void;
}) {
  const [creating, setCreating] = useState(false);
  const [creatingCategory, setCreatingCategory] = useState(false);
  const [name, setName] = useState('');
  const [category, setCategory] = useState('开发');
  const [categoryName, setCategoryName] = useState('');
  const [managingCategories, setManagingCategories] = useState(false);
  const [renamingCategory, setRenamingCategory] = useState<CategoryOption | null>(null);
  const [busyCategory, setBusyCategory] = useState('');
  const [busyProjectId, setBusyProjectId] = useState('');
  const [editingProject, setEditingProject] = useState<Project | null>(null);
  const [busyTaskId, setBusyTaskId] = useState('');
  const [query, setQuery] = useState('');
  const [projectRange, setProjectRange] = useState<ProjectRange>('week');
  const [customRangeStart, setCustomRangeStart] = useState(selectedDate);
  const [customRangeEnd, setCustomRangeEnd] = useState(selectedDate);
  const [hideZeroProjects, setHideZeroProjects] = useState(false);
  const [selectedProjectId, setSelectedProjectId] = useState(focusProjectId || projects[0]?.id || '');
  const [taskName, setTaskName] = useState('');
  const [sessionDetail, setSessionDetail] = useState<
    { kind: 'project' | 'task' | 'unassigned'; id: string } | null
  >(null);
  const confirmation = useConfirmation();
  const rangeBounds = useMemo(
    () => projectRange === 'custom'
      ? customProjectRangeBounds(customRangeStart, customRangeEnd)
      : projectRangeBounds(projectRange, selectedDate),
    [customRangeEnd, customRangeStart, projectRange, selectedDate],
  );
  const allSessionDateRange = useMemo(() => {
    if (!sessions.length) return { start: selectedDate, end: selectedDate };
    let startedAt = Number.POSITIVE_INFINITY;
    let endedAt = Number.NEGATIVE_INFINITY;
    for (const session of sessions) {
      const start = new Date(session.startedAt).getTime();
      const end = new Date(session.endedAt).getTime();
      if (Number.isFinite(start)) startedAt = Math.min(startedAt, start);
      if (Number.isFinite(end)) endedAt = Math.max(endedAt, end);
    }
    return {
      start: Number.isFinite(startedAt) ? localDateKey(new Date(startedAt)) : selectedDate,
      end: Number.isFinite(endedAt) ? localDateKey(new Date(endedAt)) : selectedDate,
    };
  }, [selectedDate, sessions]);
  const displayedRange = useMemo(() => {
    if (projectRange === 'custom') {
      return { start: customRangeStart, end: customRangeEnd };
    }
    if (!rangeBounds) return allSessionDateRange;
    return {
      start: localDateKey(new Date(rangeBounds.startedAt)),
      end: localDateKey(new Date(rangeBounds.endedAt - 1)),
    };
  }, [allSessionDateRange, customRangeEnd, customRangeStart, projectRange, rangeBounds]);
  const rangePresetLabel = PROJECT_RANGE_OPTIONS.find((item) => item.id === projectRange)?.label
    || '自定义';
  const rangeLabel = `${rangePresetLabel} · ${formatProjectDateRange(displayedRange.start, displayedRange.end)}`;
  const rangeSessions = useMemo(
    () => sessions.filter((session) => sessionMinutesInRange(session, rangeBounds) > 0),
    [rangeBounds, sessions],
  );
  const minutesByProject = useMemo(() => {
    const result = new Map<string, number>();
    for (const session of rangeSessions) {
      if (!session.projectId) continue;
      result.set(
        session.projectId,
        (result.get(session.projectId) || 0) + sessionMinutesInRange(session, rangeBounds),
      );
    }
    return result;
  }, [rangeBounds, rangeSessions]);
  const categoryStats = useMemo(() => {
    const result = new Map<string, { projects: number; minutes: number }>();
    for (const option of categoryOptions) {
      result.set(option.name, { projects: 0, minutes: 0 });
    }
    for (const project of projects) {
      const current = result.get(project.category) || { projects: 0, minutes: 0 };
      current.projects += 1;
      current.minutes += minutesByProject.get(project.id) || 0;
      result.set(project.category, current);
    }
    return result;
  }, [categoryOptions, minutesByProject, projects]);
  const taskStats = useMemo(() => {
    const result = new Map<string, { count: number; minutes: number }>();
    for (const session of rangeSessions) {
      if (!session.taskId) continue;
      const current = result.get(session.taskId) || { count: 0, minutes: 0 };
      current.count += 1;
      current.minutes += sessionMinutesInRange(session, rangeBounds);
      result.set(session.taskId, current);
    }
    return result;
  }, [rangeBounds, rangeSessions]);
  const tasksByProject = useMemo(() => groupBy(tasks, (task) => task.projectId), [tasks]);
  const needle = normalizeSearchText(query);
  const visibleProjects = projects.filter((project) => {
    const matchesSearch = !needle || normalizeSearchText([
      project.name,
      project.category,
      project.description,
      ...(tasksByProject.get(project.id) || []).map((task) => task.title),
    ].filter(Boolean).join(' ')).includes(needle);
    return matchesSearch && (!hideZeroProjects || (minutesByProject.get(project.id) || 0) > 0);
  });
  const maxProjectMinutes = Math.max(1, ...minutesByProject.values());
  const categoryOrder = new Map(categoryOptions.map((item, index) => [item.name, index]));
  const projectGroups = [...groupBy(visibleProjects, (project) => project.category).entries()]
    .map(([groupCategory, groupProjects]) => ({
      category: groupCategory,
      color: categoryOptions.find((item) => item.name === groupCategory)?.color
        || categoryColor(groupCategory),
      projects: [...groupProjects].sort((left, right) => (
        (minutesByProject.get(right.id) || 0) - (minutesByProject.get(left.id) || 0)
        || left.name.localeCompare(right.name, 'zh-CN')
      )),
      minutes: groupProjects.reduce(
        (sum, project) => sum + (minutesByProject.get(project.id) || 0),
        0,
      ),
    }))
    .sort((left, right) => (
      (categoryOrder.get(left.category) ?? Number.MAX_SAFE_INTEGER)
      - (categoryOrder.get(right.category) ?? Number.MAX_SAFE_INTEGER)
      || left.category.localeCompare(right.category, 'zh-CN')
    ));
  const selectedProject = visibleProjects.find((project) => project.id === selectedProjectId)
    || visibleProjects[0];
  const selectedTasks = selectedProject
    ? [...(tasksByProject.get(selectedProject.id) || [])]
      .filter((task) => !hideZeroProjects || (taskStats.get(task.id)?.minutes || 0) > 0)
      .sort((left, right) => (
          (taskStats.get(right.id)?.minutes || 0) - (taskStats.get(left.id)?.minutes || 0)
          || left.title.localeCompare(right.title, 'zh-CN')
        ))
    : [];
  const unassignedSessions = selectedProject
    ? rangeSessions.filter((session) => session.projectId === selectedProject.id && !session.taskId)
    : [];
  const unassignedMinutes = unassignedSessions.reduce(
    (sum, session) => sum + sessionMinutesInRange(session, rangeBounds),
    0,
  );
  const detailTask = sessionDetail?.kind === 'task'
    ? tasks.find((task) => task.id === sessionDetail.id)
    : undefined;
  const detailProject = sessionDetail?.kind === 'project' || sessionDetail?.kind === 'unassigned'
    ? projects.find((project) => project.id === sessionDetail.id)
    : detailTask
      ? projects.find((project) => project.id === detailTask.projectId)
      : undefined;
  const detailSessions = sessionDetail?.kind === 'task'
    ? rangeSessions.filter((session) => session.taskId === sessionDetail.id)
    : sessionDetail?.kind === 'unassigned'
      ? rangeSessions.filter((session) => session.projectId === sessionDetail.id && !session.taskId)
    : sessionDetail?.kind === 'project'
      ? rangeSessions.filter((session) => session.projectId === sessionDetail.id)
      : [];

  useEffect(() => {
    if (focusProjectId && projects.some((project) => project.id === focusProjectId)) {
      setQuery('');
      setHideZeroProjects(false);
      setSelectedProjectId(focusProjectId);
    }
  }, [focusProjectId]);

  useEffect(() => {
    if (!visibleProjects.length) {
      if (selectedProjectId) setSelectedProjectId('');
      return;
    }
    if (!visibleProjects.some((project) => project.id === selectedProjectId)) {
      setSelectedProjectId(visibleProjects[0].id);
    }
  }, [selectedProjectId, visibleProjects]);

  useEffect(() => {
    if (categoryOptions.some((item) => item.name === category)) return;
    setCategory(categoryOptions[0]?.name || '');
  }, [category, categoryOptions]);

  const createCategory = async () => {
    const nextCategoryName = categoryName.trim();
    if (!nextCategoryName) return;
    const created = (await runAction(
      () => api.createCategory(nextCategoryName),
      `分类“${nextCategoryName}”已创建`,
    )) as CategoryOption;
    setCategory(created.name);
    setCategoryName('');
    setCreatingCategory(false);
  };

  const renameCategory = async (newName: string) => {
    const selected = renamingCategory;
    if (!selected || busyCategory) return;
    setBusyCategory(selected.name);
    try {
      const renamed = (await runAction(
        () => api.renameCategory(selected.name, newName),
        `分类“${selected.name}”已改名为“${newName}”`,
      )) as CategoryOption;
      if (category === selected.name) setCategory(renamed.name);
      setRenamingCategory(null);
    } finally {
      setBusyCategory('');
    }
  };

  const deleteCategory = async (selected: CategoryOption) => {
    if (busyCategory || selected.name === idleCategory) return;
    const fallback = categoryOptions.find((item) => item.name === '杂务' && item.name !== selected.name)
      || categoryOptions.find((item) => item.name !== selected.name);
    if (!fallback) return;
    const accepted = await confirmation.confirm({
      title: `删除分类“${selected.name}”？`,
      detail: `该分类下的项目、规则和历史时间段会转到“${fallback.name}”。`,
    });
    if (!accepted) return;
    setBusyCategory(selected.name);
    try {
      const fallbackName = (await runAction(
        () => api.deleteCategory(selected.name),
        `分类“${selected.name}”已删除，内容已转入`,
      )) as string;
      if (category === selected.name) setCategory(fallbackName);
      if (renamingCategory?.name === selected.name) setRenamingCategory(null);
    } finally {
      setBusyCategory('');
    }
  };

  const createProject = async () => {
    const projectName = name.trim();
    if (!projectName || !category) return;
    await runAction(() => api.createProject(projectName, category), `项目“${projectName}”已创建`);
    setName('');
    setCreating(false);
  };

  const saveProject = async (project: Project, projectName: string, projectCategory: string) => {
    setBusyProjectId(project.id);
    try {
      await runAction(
        () => api.updateProject(project.id, projectName, projectCategory),
        `项目“${projectName}”已更新`,
      );
      setEditingProject(null);
    } finally {
      setBusyProjectId('');
    }
  };

  const deleteProject = async (project: Project) => {
    const accepted = await confirmation.confirm({
      title: `删除项目“${project.name}”？`,
      detail: '相关任务会一并删除；历史会话仍会保留，但会取消项目和任务归属。',
    });
    if (!accepted) return;
    setBusyProjectId(project.id);
    try {
      await runAction(() => api.deleteProject(project.id), `项目“${project.name}”已删除`);
    } finally {
      setBusyProjectId('');
    }
  };

  const deleteTask = async (task: Task) => {
    const accepted = await confirmation.confirm({
      title: `删除任务“${task.title}”？`,
      detail: '历史会话会继续保留，但会取消这项任务的归属。',
    });
    if (!accepted) return;
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
          subtitle={`${rangeLabel} · 显示 ${visibleProjects.length}/${projects.length} 个项目`}
          action={(
            <div className="project-header-actions">
              <button
                aria-expanded={managingCategories}
                onClick={() => {
                  setManagingCategories(true);
                  setCreating(false);
                  setCreatingCategory(false);
                }}
                type="button"
              >
                <Tags size={15} />管理分类
              </button>
              <button
                aria-expanded={creatingCategory}
                onClick={() => {
                  setCreatingCategory((current) => !current);
                  setCreating(false);
                }}
                type="button"
              >
                <Plus size={15} />新建分类
              </button>
              <button
                aria-expanded={creating}
                className="primary"
                onClick={() => {
                  setCreating((current) => !current);
                  setCreatingCategory(false);
                }}
                type="button"
              >
                <Plus size={15} />新建项目
              </button>
            </div>
          )}
        />
        <div className="project-search">
          <Search size={16} />
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索项目或任务" />
          {query && <button onClick={() => setQuery('')} type="button" aria-label="清空项目搜索"><X size={14} /></button>}
        </div>
        <div className="project-date-range" aria-label="项目统计起止时间">
          <label>
            <span>开始</span>
            <input
              type="date"
              value={displayedRange.start}
              onChange={(event) => {
                const nextStart = event.target.value;
                if (!nextStart) return;
                setCustomRangeStart(nextStart);
                setCustomRangeEnd(displayedRange.end < nextStart ? nextStart : displayedRange.end);
                setProjectRange('custom');
              }}
            />
          </label>
          <span className="project-date-separator">至</span>
          <label>
            <span>结束</span>
            <input
              type="date"
              value={displayedRange.end}
              onChange={(event) => {
                const nextEnd = event.target.value;
                if (!nextEnd) return;
                setCustomRangeEnd(nextEnd);
                setCustomRangeStart(displayedRange.start > nextEnd ? nextEnd : displayedRange.start);
                setProjectRange('custom');
              }}
            />
          </label>
        </div>
        <div className="project-range-toolbar">
          <div className="project-range-tabs" role="radiogroup" aria-label="项目统计时间范围">
            {PROJECT_RANGE_OPTIONS.map((item) => (
              <button
                aria-checked={projectRange === item.id}
                className={projectRange === item.id ? 'active' : ''}
                key={item.id}
                onClick={() => setProjectRange(item.id)}
                role="radio"
                type="button"
              >
                {item.label}
              </button>
            ))}
          </div>
          <button
            aria-pressed={hideZeroProjects}
            className={`zero-project-toggle ${hideZeroProjects ? 'active' : ''}`}
            onClick={() => setHideZeroProjects((current) => !current)}
            type="button"
          >
            <EyeOff size={14} />隐藏 0 秒
          </button>
        </div>
        {creatingCategory && (
          <div className="project-create-row category-create-row">
            <input
              autoFocus
              onChange={(event) => setCategoryName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault();
                  void createCategory();
                }
              }}
              placeholder="分类名称"
              value={categoryName}
            />
            <button
              className="primary"
              disabled={!categoryName.trim()}
              onClick={() => void createCategory()}
              type="button"
            >
              <Check size={15} />创建分类
            </button>
          </div>
        )}
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
              {categoryOptions.map((item) => <option key={item.name}>{item.name}</option>)}
            </select>
            <button className="primary" disabled={!name.trim() || !category} onClick={() => void createProject()} type="button">
              <Check size={15} />创建
            </button>
          </div>
        )}
        <div className="project-category-groups">
          {projectGroups.map((group) => (
            <section className="project-category-group" key={group.category}>
              <header>
                <span className="project-category-name">
                  <i style={{ background: group.color }} />
                  <strong>{group.category}</strong>
                </span>
                <small>{group.projects.length} 个项目 · {formatDuration(group.minutes)}</small>
              </header>
              <div className="project-grid">
                {group.projects.map((project) => {
                  const minutes = minutesByProject.get(project.id) || 0;
                  return (
                    <article
                      className={`project-card ${selectedProject?.id === project.id ? 'active' : ''}`}
                      key={project.id}
                      style={{ '--project-color': project.color || group.color } as CSSProperties}
                    >
                      <button className="project-card-main" onClick={() => setSelectedProjectId(project.id)} type="button">
                        <span>
                          <strong>{project.name}</strong>
                          <small>{project.description === '在修正归类时手动创建' ? '个人项目' : project.description || '个人项目'}</small>
                        </span>
                        <ChevronRight size={16} />
                      </button>
                      <div className="project-card-head">
                        <span>{(tasksByProject.get(project.id) || []).length} 个任务</span>
                        <div>
                          <b>{formatDuration(minutes)}</b>
                          <button
                            className="project-edit"
                            disabled={busyProjectId === project.id}
                            onClick={() => setEditingProject(project)}
                            type="button"
                            aria-label={`编辑项目 ${project.name}`}
                            title="改名或更换分类"
                          >
                            <Pencil size={13} />
                          </button>
                          <button
                            className="project-delete"
                            disabled={busyProjectId === project.id}
                            onClick={() => void deleteProject(project)}
                            type="button"
                            aria-label={`删除项目 ${project.name}`}
                            title="删除项目"
                          >
                            <Trash2 size={13} />
                          </button>
                        </div>
                      </div>
                      <div className="project-progress"><span style={{ width: `${minutes > 0 ? Math.min(100, Math.max(4, (minutes / maxProjectMinutes) * 100)) : 0}%` }} /></div>
                    </article>
                  );
                })}
              </div>
            </section>
          ))}
          {!visibleProjects.length && (
            <EmptyState
              title={projects.length ? '没有匹配项目' : '还没有项目'}
              detail={projects.length
                ? hideZeroProjects
                  ? `${rangeLabel}没有非零项目，可关闭“隐藏 0 秒”或切换范围。`
                  : '换个项目名、分类或任务名试试。'
                : '打开一个代码工作区或修正一条会话即可自动建立。'}
            />
          )}
        </div>
      </section>

      <section className="panel project-detail-panel">
        <PanelTitle
          title={selectedProject?.name || '项目任务'}
          subtitle={selectedProject ? `${rangePresetLabel} · ${selectedProject.category} · ${formatDuration(minutesByProject.get(selectedProject.id) || 0)}` : '先新建一个项目'}
          action={selectedProject ? (
            <div className="project-detail-actions">
              <button
                onClick={() => setEditingProject(selectedProject)}
                type="button"
                title="改名或更换分类"
              >
                <Pencil size={14} />编辑
              </button>
              <button
                onClick={() => setSessionDetail({ kind: 'project', id: selectedProject.id })}
                type="button"
              >
                <Clock3 size={14} />时间段
              </button>
            </div>
          ) : undefined}
        />
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
              {unassignedSessions.length > 0 && (
                <button
                  className="unassigned-task-card"
                  onClick={() => setSessionDetail({ kind: 'unassigned', id: selectedProject.id })}
                  type="button"
                >
                  <span className="unassigned-task-icon"><TimerReset size={16} /></span>
                  <span>
                    <strong>未归属任务</strong>
                    <small>{unassignedSessions.length} 个时间段</small>
                  </span>
                  <b>{formatDuration(unassignedMinutes)}</b>
                  <ChevronRight size={15} />
                </button>
              )}
              {selectedTasks.map((task) => (
                <div className="task-row" key={task.id}>
                  <button
                    className="task-open"
                    onClick={() => setSessionDetail({ kind: 'task', id: task.id })}
                    type="button"
                  >
                    <TimerReset size={15} />
                    <span>{task.title}</span>
                    <small>{taskStats.get(task.id)?.count || 0} 段 · {formatDuration(taskStats.get(task.id)?.minutes || 0)}</small>
                  </button>
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
              {!selectedTasks.length && !unassignedSessions.length && (
                <EmptyState
                  title={hideZeroProjects ? '当前区间没有投入' : '还没有任务'}
                  detail={hideZeroProjects ? '关闭“隐藏 0 秒”可查看全部任务。' : '直接在上方输入任务名并按 Enter。'}
                />
              )}
            </div>
          )}
          {!selectedProject && <EmptyState title="还没有项目" detail="创建项目后即可添加任务。" />}
        </div>
      </section>
      {managingCategories && (
        <ManageCategoriesDialog
          blocked={Boolean(renamingCategory) || confirmation.isOpen}
          busyCategory={busyCategory}
          categories={categoryOptions}
          idleCategory={idleCategory}
          onCancel={() => setManagingCategories(false)}
          onDelete={(selected) => void deleteCategory(selected)}
          onRename={setRenamingCategory}
          stats={categoryStats}
        />
      )}
      {renamingCategory && (
        <RenameCategoryDialog
          busy={busyCategory === renamingCategory.name}
          currentName={renamingCategory.name}
          onCancel={() => {
            if (!busyCategory) setRenamingCategory(null);
          }}
          onRename={(newName) => void renameCategory(newName)}
        />
      )}
      {confirmation.dialog}
      {editingProject && (
        <EditProjectDialog
          project={editingProject}
          categoryOptions={categoryOptions}
          busy={busyProjectId === editingProject.id}
          onCancel={() => setEditingProject(null)}
          onSave={(projectName, projectCategory) => {
            void saveProject(editingProject, projectName, projectCategory);
          }}
        />
      )}
      {sessionDetail && detailProject && (
        <CategoryDetailModal
          category={detailTask
            ? detailTask.title
            : sessionDetail.kind === 'unassigned'
              ? '未归属任务'
              : detailProject.name}
          sessions={detailSessions}
          selectedDate={selectedDate}
          contextLabel={rangeLabel}
          durationForSession={(session) => sessionMinutesInRange(session, rangeBounds)}
          showDate={projectRange !== 'today'}
          selectionResetKey={selectionResetKey}
          onClose={() => setSessionDetail(null)}
          onEdit={onEdit}
        />
      )}
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
  const [savedSettings, setSavedSettings] = useState<AppSettings>(data.settings);
  const [secret, setSecret] = useState('');
  const savedFingerprintRef = useRef(JSON.stringify(data.settings));

  useEffect(() => {
    const fingerprint = JSON.stringify(data.settings);
    if (fingerprint === savedFingerprintRef.current) return;
    savedFingerprintRef.current = fingerprint;
    setSavedSettings(data.settings);
    setSettings(data.settings);
  }, [data.settings]);

  const update = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => {
    setSettings((current) => ({ ...current, [key]: value }));
  };

  const updateTheme = (theme: ThemeMode) => {
    update('theme', theme);
    onThemeChange(theme);
  };

  const saveAll = useCallback(async () => {
    let next = { ...settings };
    if (settings.aiProvider === 'openai-compatible' && secret.trim()) {
      const secretName = settings.aiSecretRef?.trim() || 'openai-compatible';
      await api.saveSecret(secretName, secret.trim());
      next = { ...next, aiSecretRef: secretName };
    }
    await api.saveSettings(next);
    if (next.aiMode === 'auto') await api.startAnalysisQueue();
    savedFingerprintRef.current = JSON.stringify(next);
    setSavedSettings(next);
    setSettings(next);
    setSecret('');
  }, [secret, settings]);

  const hasChanges = JSON.stringify(settings) !== JSON.stringify(savedSettings) || Boolean(secret.trim());
  const resetChanges = useCallback(() => {
    setSettings(savedSettings);
    setSecret('');
    onThemeChange(savedSettings.theme);
  }, [onThemeChange, savedSettings]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
        event.preventDefault();
        if (hasChanges) void runAction(saveAll, '设置已保存').catch(() => undefined);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [hasChanges, runAction, saveAll]);

  const jumpToSettings = (id: string) => {
    document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  };

  return (
    <div className="settings-grid">
      <nav className="settings-nav" aria-label="设置分区">
        {[
          ['settings-appearance', '外观'],
          ['settings-sync', '同步'],
          ['settings-tracking', '记录'],
          ['settings-ai', 'AI'],
          ['settings-data', '数据'],
        ].map(([id, label]) => (
          <button key={id} onClick={() => jumpToSettings(id)} type="button">{label}</button>
        ))}
        <span>{hasChanges ? '有未保存修改' : '设置已同步'}</span>
      </nav>

      <section className="panel settings-panel appearance-panel" id="settings-appearance">
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

      <section className="panel settings-panel" id="settings-tracking">
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
          <Field label="离开归属" hint="默认归入“无效 → 离开”，保存后会同步迁移自动离开记录。">
            <div className="idle-target-fields">
              <select
                value={settings.idleCategory}
                onChange={(event) => update('idleCategory', event.target.value)}
                aria-label="离开记录分类"
              >
                {data.categoryOptions.map((item) => (
                  <option key={item.name} value={item.name}>{item.name}</option>
                ))}
              </select>
              <input
                value={settings.idleProjectName}
                onChange={(event) => update('idleProjectName', event.target.value)}
                placeholder="离开项目名称"
                aria-label="离开记录项目"
              />
            </div>
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
          title="会议和视频不计为离开"
          detail="前台会议、确认播放的网页视频和本地播放器即使没有键鼠输入也继续计时。"
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

      <section className="panel settings-panel" id="settings-ai">
        <PanelTitle
          title="可选 AI 复核"
          subtitle="开启后按排队顺序逐条运行；同一时间只调用一个模型。"
        />
        <Toggle
          checked={settings.aiMode === 'auto'}
          onChange={(enabled) => update('aiMode', enabled ? 'auto' : 'off')}
          title="AI 自动复核"
          detail="开启后自动发现待复核会话，一个完成后继续下一个；关闭时当前请求会安全结束，但不再开始下一条。"
        />
        {settings.aiMode === 'auto' && (
          <div className="ai-fields">
            <Field label="AI 来源" hint="Codex 模式直接复用当前 ChatGPT 登录，不读取或复制账号令牌。">
              <select
                value={settings.aiProvider}
                onChange={(event) => {
                  const provider = event.target.value;
                  setSettings((current) => ({
                    ...current,
                    aiProvider: provider,
                    aiModel: provider === 'codex-account' ? 'gpt-5.6-luna' : current.aiModel,
                  }));
                }}
              >
                <option value="codex-account">当前 Codex / ChatGPT 账号</option>
                <option value="openai-compatible">OpenAI-compatible API</option>
              </select>
            </Field>
            <Field label="最低会话时长" hint="设为 0 分钟时，所有尚未人工确认的会话都进入复核。">
              <NumberInput
                value={settings.minAiSessionMinutes}
                min={0}
                max={240}
                suffix="分钟"
                onChange={(value) => update('minAiSessionMinutes', value)}
              />
            </Field>
            {settings.aiProvider === 'codex-account' && (
              <Field label="Codex 套餐" hint="用于区分固定订阅费与单次信用点消耗。">
                <select value={settings.codexPlan} onChange={(event) => update('codexPlan', event.target.value)}>
                  <option value="plus">Plus · $20/月</option>
                  <option value="pro-5x">Pro 5x · $100/月</option>
                  <option value="pro-20x">Pro 20x · $200/月</option>
                </select>
              </Field>
            )}
            <Field
              label="模型名"
              hint={settings.aiProvider === 'codex-account' ? 'Luna 适合高频、结构化的分类任务。' : '填写服务端支持的模型 ID。'}
            >
              <input
                value={settings.aiModel}
                onChange={(event) => update('aiModel', event.target.value)}
                placeholder="gpt-5.6-luna"
                readOnly={settings.aiProvider === 'codex-account'}
              />
            </Field>
            {settings.aiProvider === 'openai-compatible' && (
              <>
                <Field label="API Base">
                  <input
                    value={settings.aiBaseUrl}
                    onChange={(event) => update('aiBaseUrl', event.target.value)}
                    placeholder="https://api.openai.com/v1"
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
              </>
            )}
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
            {settings.aiProvider === 'codex-account' && (
              <button
                onClick={() => void runAction(api.refreshCodexRateCard, '已对齐 OpenAI 最新 Codex 费率')}
                type="button"
              >
                <RefreshCw size={16} />对齐最新费率
              </button>
            )}
          </div>
        )}
        <div className="setting-callout">
          <WandSparkles size={19} />
          <div>
            <strong>一次结合整段工作上下文</strong>
            <span>每批最多 8 个目标，附带前后 30 分钟时间段及全部分类、项目、任务；URL 查询参数会去除。</span>
          </div>
        </div>
      </section>

      <section className="panel settings-panel" id="settings-integrations">
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

      <section className="panel settings-panel" id="settings-data">
        <PanelTitle title="数据管理" subtitle="SQLite 会话长期保留，原始事件按保留期轮转。" />
        <div className="data-actions">
          <button
            onClick={() => void runAction(api.revealDataDir, '已打开数据目录')}
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
          <strong>{hasChanges ? '有未保存修改' : '所有设置已保存'}</strong>
          <span>{hasChanges ? '按 Ctrl+S 快速保存；采集器会自动应用新配置。' : '周期刷新不会再覆盖正在编辑的表单。'}</span>
        </div>
        <div className="settings-save-actions">
          {hasChanges && (
            <button onClick={resetChanges} type="button">
              <Undo2 size={16} />放弃修改
            </button>
          )}
          <button
            className="primary"
            disabled={!hasChanges}
            onClick={() => void runAction(saveAll, '设置已保存')}
            type="button"
          >
            <Check size={17} />保存更改 <kbd>Ctrl S</kbd>
          </button>
        </div>
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
      <section className="panel settings-panel sync-panel sync-panel-loading" id="settings-sync">
        <Cloud size={20} />正在读取同步状态…
      </section>
    );
  }

  const repoUrl = config.owner && config.repo
    ? `https://github.com/${config.owner}/${config.repo}`
    : '';
  const historyUrl = repoUrl
    ? `${repoUrl}/commits/${encodeURIComponent(config.branch)}/${config.filePath.split('/').map(encodeURIComponent).join('/')}`
    : '';
  const recordCount = status.counts.categories + status.counts.projects
    + status.counts.tasks + status.counts.sessions + status.counts.rules;

  return (
    <section className="panel settings-panel sync-panel" id="settings-sync">
      <div className="sync-panel-head">
        <PanelTitle
          title="GitHub 多端同步"
          subtitle="拉取 → 按更新时间合并 → 推送 · 独立 Private 仓库 · 端侧加密"
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
          {historyUrl && config.lastSyncedAt && (
            <button
              onClick={() => window.open(historyUrl, '_blank')}
              title="GitHub 为每次同步保留 commit；可在历史中查看或撤销上一版"
              type="button"
            >
              <Undo2 size={15} />同步历史/撤销
            </button>
          )}
          {repoUrl && config.lastSyncedAt && (
            <button onClick={() => window.open(repoUrl, '_blank')} type="button"><Github size={15} />查看仓库</button>
          )}
          {config.enabled && <button onClick={() => void disconnect()} disabled={busy} type="button">停止同步</button>}
          <button onClick={() => void save()} disabled={busy} type="button">保存同步设置</button>
          <button className="primary" onClick={() => void syncNow()} disabled={busy || !status.ready} type="button">
            <Cloud size={16} />{busy ? '处理中…' : '拉取并推送'}
          </button>
        </div>
      </div>
      <div className="setting-callout compact">
        <RefreshCw size={18} />
        <div>
          <strong>同一数据协议可供后续 Android、平板端复用</strong>
          <span>分类、项目、任务、会话、学习规则和删除记录会按 ID 合并；每次远端写入都是一个可追溯的 Git commit。</span>
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
  priority?: number;
}

function normalizeSearchText(value: string) {
  return value.trim().toLocaleLowerCase('zh-CN').replace(/\s+/g, ' ');
}

function searchTextQuality(value: string, query: string) {
  const normalized = normalizeSearchText(value);
  if (!normalized || !normalized.includes(query)) return null;
  if (normalized === query) return 0;
  if (normalized.startsWith(query)) return 1;
  return 2;
}

function searchOptionRank(option: SearchSelectOption, query: string) {
  const labelQuality = searchTextQuality(option.label, query);
  if (labelQuality !== null) return { relation: 0, quality: labelQuality };

  const relatedValues = [option.meta, ...(option.keywords || [])].filter(Boolean) as string[];
  const relatedQualities = relatedValues.flatMap((value) => {
    const quality = searchTextQuality(value, query);
    return quality === null ? [] : [quality];
  });
  if (relatedQualities.length) {
    return { relation: 1, quality: Math.min(...relatedQualities) };
  }

  const tokens = query.split(' ').filter(Boolean);
  const searchable = normalizeSearchText([option.label, ...relatedValues].join(' '));
  if (tokens.length > 1 && tokens.every((token) => searchable.includes(token))) {
    return { relation: 2, quality: 0 };
  }
  return null;
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
      .flatMap((option, index) => {
        const rank = searchOptionRank(option, normalizedQuery);
        return rank ? [{ option, index, rank }] : [];
      })
      .sort((left, right) => left.rank.relation - right.rank.relation
        || (left.option.priority ?? 0) - (right.option.priority ?? 0)
        || left.rank.quality - right.rank.quality
        || left.index - right.index)
      .map((item) => item.option)
      .slice(0, 80);
  }, [normalizedQuery, options]);
  const showCreate = Boolean(
    trimmedQuery
      && onCreate
      && (canCreate ? canCreate(trimmedQuery) : true),
  );
  const createIsFirst = showCreate && !filtered.some(
    (option) => normalizeSearchText(option.label) === normalizedQuery,
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
      if (createIsFirst && activeIndex === 0) {
        void createCurrent();
        return;
      }
      const optionIndex = createIsFirst ? activeIndex - 1 : activeIndex;
      const option = filtered[optionIndex];
      if (option) {
        selectOption(option);
      } else if (showCreate && !createIsFirst && activeIndex === filtered.length) {
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

  const renderCreateOption = (index: number) => showCreate && (
    <button
      aria-selected={false}
      className={`create-option${activeIndex === index ? ' active' : ''}`}
      id={`${listboxId}-option-${index}`}
      key="create-option"
      onClick={() => void createCurrent()}
      onMouseDown={(event) => event.preventDefault()}
      onMouseEnter={() => setActiveIndex(index)}
      role="option"
      type="button"
    >
      <Plus size={14} />
      <span>
        <strong>{createLabel ? createLabel(trimmedQuery) : `新建“${trimmedQuery}”`}</strong>
        <small>按 Enter 直接创建</small>
      </span>
    </button>
  );

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
          {createIsFirst && renderCreateOption(0)}
          {filtered.map((option, index) => (
            <button
              aria-selected={option.value === value}
              className={`${index + (createIsFirst ? 1 : 0) === activeIndex ? 'active' : ''}${option.value === value ? ' selected' : ''}`}
              id={`${listboxId}-option-${index + (createIsFirst ? 1 : 0)}`}
              key={option.value}
              onClick={() => selectOption(option)}
              onMouseDown={(event) => event.preventDefault()}
              onMouseEnter={() => setActiveIndex(index + (createIsFirst ? 1 : 0))}
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
          {!createIsFirst && renderCreateOption(filtered.length)}
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
  const [renamingCategory, setRenamingCategory] = useState<CategoryOption | null>(null);
  const [splitDialogOpen, setSplitDialogOpen] = useState(false);
  const [splitBusy, setSplitBusy] = useState(false);
  const confirmation = useConfirmation();
  const projectById = useMemo(
    () => new Map(projectOptions.map((project) => [project.id, project])),
    [projectOptions],
  );
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
      { value: '', label: '暂不指定', meta: '不关联项目', priority: -1 },
      ...[...projectOptions]
        .sort((left, right) => {
          const priority = (project: Project) => {
            if (category) {
              if (project.category !== category) return 2;
              return projectId && project.id === projectId ? 0 : 1;
            }
            return projectId && project.id === projectId ? 0 : 1;
          };
          return priority(left) - priority(right)
            || left.name.localeCompare(right.name, 'zh-CN');
        })
        .map((project) => {
          const priority = projectId && project.id === projectId
            ? 0
            : category && project.category === category
              ? 1
              : 2;
          return {
            value: project.id,
            label: project.name,
            meta: project.category,
            color: project.color || categoryColor(project.category),
            keywords: [
              project.category,
              ...taskOptions.filter((task) => task.projectId === project.id).map((task) => task.title),
            ],
            priority,
          };
        }),
    ],
    [category, projectId, projectOptions, taskOptions],
  );
  const taskSearchOptions = useMemo<SearchSelectOption[]>(
    () => [
      { value: '', label: '暂不指定', meta: '不关联任务', priority: -1 },
      ...[...taskOptions]
        .sort((left, right) => {
          const priority = (task: Task) => {
            const project = projectById.get(task.projectId);
            if (projectId) {
              if (task.projectId === projectId) return taskId && task.id === taskId ? 0 : 1;
              if (category && project?.category === category) return 2;
              return 3;
            }
            if (category) {
              if (project?.category !== category) return 2;
              return taskId && task.id === taskId ? 0 : 1;
            }
            return taskId && task.id === taskId ? 0 : 1;
          };
          return priority(left) - priority(right)
            || left.title.localeCompare(right.title, 'zh-CN');
        })
        .map((task) => {
          const project = projectById.get(task.projectId);
          return {
            value: task.id,
            label: task.title,
            meta: project ? `${project.name} · ${project.category}` : '项目已删除',
            color: project?.color || categoryColor(project?.category || '杂务'),
            keywords: project ? [project.name, project.category] : [],
            priority: projectId && task.projectId === projectId
              ? 0
              : category && project?.category === category
                ? 1
                : 2,
          };
        }),
    ],
    [category, projectById, projectId, taskId, taskOptions],
  );
  const selectedProject = projectById.get(projectId);
  const selectedCategory = localCategories.find((item) => item.name === category);
  const categoryIsEditable = Boolean(selectedCategory);
  const projectPlaceholder = category ? `搜索全部项目，“${category}”优先` : '搜索全部项目';
  const taskPlaceholder = selectedProject
    ? `搜索全部任务，“${selectedProject.name}”优先`
    : category
      ? `搜索全部任务，“${category}”优先`
      : '搜索全部项目中的任务';

  useEffect(() => setProjectOptions(projects), [projects]);
  useEffect(() => setTaskOptions(tasks), [tasks]);
  useEffect(() => setLocalCategories(categoryOptions), [categoryOptions]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !confirmation.isOpen && !renamingCategory) onClose();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [confirmation.isOpen, onClose, renamingCategory]);

  const selectCategory = (name: string) => {
    setCategory(name);
    setCategoryTouched(true);
    const selectedProject = projectById.get(projectId);
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
    const selectedProject = projectById.get(id);
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
    const selectedProject = projectById.get(selectedTask.projectId);
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
    const project = projectById.get(projectId);
    if (!project || projectBusy) return;
    const accepted = await confirmation.confirm({
      title: `删除项目“${project.name}”？`,
      detail: '相关任务会一并删除；历史会话仍会保留，但会取消项目和任务归属。',
    });
    if (!accepted) return;
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
    if (!selected || categoryBusy) return;
    const fallback = localCategories.find((item) => item.name === '杂务' && item.name !== selected.name)
      || localCategories.find((item) => item.name !== selected.name);
    if (!fallback) return;
    const accepted = await confirmation.confirm({
      title: `删除分类“${selected.name}”？`,
      detail: `使用它的项目、规则和历史会话会转到“${fallback.name}”。`,
    });
    if (!accepted) return;
    setCategoryBusy(true);
    try {
      const fallbackName = (await runAction(
        () => api.deleteCategory(selected.name),
        `分类“${selected.name}”已删除`,
      )) as string;
      setLocalCategories((current) => current.filter((item) => item.name !== selected.name));
      setProjectOptions((current) => current.map((project) => (
        project.category === selected.name ? { ...project, category: fallbackName } : project
      )));
      setCategory(fallbackName);
      setCategoryTouched(true);
    } finally {
      setCategoryBusy(false);
    }
  };

  const renameSelectedCategory = async (newName: string) => {
    const selected = renamingCategory;
    if (!selected || categoryBusy) return;
    setCategoryBusy(true);
    try {
      const renamed = (await runAction(
        () => api.renameCategory(selected.name, newName),
        `分类“${selected.name}”已重命名`,
      )) as CategoryOption;
      setLocalCategories((current) => current.map((item) => (
        item.name === selected.name ? renamed : item
      )));
      setProjectOptions((current) => current.map((project) => (
        project.category === selected.name ? { ...project, category: renamed.name } : project
      )));
      if (category === selected.name) {
        setCategory(renamed.name);
        setCategoryTouched(true);
      }
      setRenamingCategory(null);
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
    const accepted = await confirmation.confirm({
      title: `删除任务“${task.title}”？`,
      detail: '历史会话会继续保留，但会取消这项任务的归属。',
    });
    if (!accepted) return;
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

  const split = async (localValue: string) => {
    const splitAt = new Date(localValue);
    if (Number.isNaN(splitAt.getTime())) return;
    setSplitBusy(true);
    try {
      await runAction(() => api.splitSession(session.id, splitAt.toISOString()), '会话已拆分');
      setSplitDialogOpen(false);
      onClose();
    } finally {
      setSplitBusy(false);
    }
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
              <div className="project-picker-actions">
                <button
                  disabled={!categoryIsEditable || categoryBusy}
                  onClick={() => selectedCategory && setRenamingCategory(selectedCategory)}
                  title="重命名分类"
                  type="button"
                >
                  <Pencil size={15} />重命名
                </button>
                <button
                  className="danger-button"
                  disabled={!categoryIsEditable || categoryBusy}
                  onClick={() => void deleteSelectedCategory()}
                  title="删除分类"
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
                  const project = projectById.get(projectId);
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
          <div className="correction-memory-note">
            <Sparkles size={18} />
            <span><strong>个人记忆会自动学习</strong><small>只用于之后的相似页面；遇到冲突会放弃判断</small></span>
          </div>
          <label>
            <input className="themed-checkbox" type="checkbox" checked={remember} onChange={(event) => setRemember(event.target.checked)} />
            <span><strong>额外建立强规则</strong><small>仅在有明确识别词时使用</small></span>
          </label>
          <label className={!projectId ? 'disabled' : ''}>
            <input className="themed-checkbox" type="checkbox" checked={pin} disabled={!projectId} onChange={(event) => setPin(event.target.checked)} />
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
            <details className="modal-evidence" open>
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
              <button
                disabled={new Date(session.endedAt).getTime() - new Date(session.startedAt).getTime() < 10_000}
                onClick={() => setSplitDialogOpen(true)}
                title="拆分后两段都至少保留 5 秒"
                type="button"
              >
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
      {confirmation.dialog}
      {renamingCategory && (
        <RenameCategoryDialog
          busy={categoryBusy}
          currentName={renamingCategory.name}
          onCancel={() => {
            if (!categoryBusy) setRenamingCategory(null);
          }}
          onRename={(name) => void renameSelectedCategory(name)}
        />
      )}
      {splitDialogOpen && (
        <TextInputDialog
          busy={splitBusy}
          confirmLabel="确认拆分"
          detail="选择分界时间；拆分后的两段都至少保留 5 秒。"
          initialValue={toDateTimeLocalValue(new Date(
            (new Date(session.startedAt).getTime() + new Date(session.endedAt).getTime()) / 2,
          ))}
          inputLabel="会话拆分时间"
          isValid={(value) => {
            const timestamp = new Date(value).getTime();
            return timestamp >= new Date(session.startedAt).getTime() + 5_000
              && timestamp <= new Date(session.endedAt).getTime() - 5_000;
          }}
          max={toDateTimeLocalValue(new Date(new Date(session.endedAt).getTime() - 5_000))}
          min={toDateTimeLocalValue(new Date(new Date(session.startedAt).getTime() + 5_000))}
          onCancel={() => {
            if (!splitBusy) setSplitDialogOpen(false);
          }}
          onConfirm={(value) => void split(value)}
          step={5}
          title="拆分会话"
          type="datetime-local"
        />
      )}
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

function summarizeDay(
  sessions: WorkSession[],
  dateKey: string,
  settings?: AppSettings,
): DayStats {
  const result = new Map<string, number>();
  let activeMinutes = 0;
  let idleMinutes = 0;
  let classifiedMinutes = 0;
  let longestMinutes = 0;
  let reviewCount = 0;

  for (const session of sessions) {
    const minutes = sessionMinutesOnDate(session, dateKey);
    result.set(session.category, (result.get(session.category) || 0) + minutes);
    if (isIdleSession(session, settings)) {
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
    contextCount: sessions.filter((session) => !isIdleSession(session, settings)).length,
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
    !isIdleSession(session) &&
    !session.userConfirmed &&
    (!session.taskId || session.confidence < DEFAULT_REVIEW_CONFIDENCE_THRESHOLD)
  );
}

function isIdleSession(session: WorkSession, settings?: AppSettings) {
  if (session.category === '离开') return true;
  if (session.source === 'collector-idle' && !session.userConfirmed) return true;
  return Boolean(
    settings &&
    session.category === settings.idleCategory &&
    session.projectName === settings.idleProjectName,
  );
}

function sessionSecondsOnDate(session: WorkSession, dateKey: string) {
  const dayStart = dateFromKey(dateKey).getTime();
  const dayEnd = dayStart + 24 * 60 * 60 * 1000;
  const start = Math.max(dayStart, new Date(session.startedAt).getTime());
  const end = Math.min(dayEnd, new Date(session.endedAt).getTime());
  return Math.max(0, (end - start) / 1000);
}

function sessionMinutesOnDate(session: WorkSession, dateKey: string) {
  return sessionSecondsOnDate(session, dateKey) / 60;
}

type ProjectRangeBounds = { startedAt: number; endedAt: number } | null;

function projectRangeBounds(range: ProjectRangePreset, anchorDateKey: string): ProjectRangeBounds {
  if (range === 'all') return null;
  const anchor = dateFromKey(anchorDateKey);
  let startedAt: Date;
  let endedAt: Date;

  if (range === 'today') {
    startedAt = new Date(anchor.getFullYear(), anchor.getMonth(), anchor.getDate());
    endedAt = new Date(anchor.getFullYear(), anchor.getMonth(), anchor.getDate() + 1);
  } else if (range === 'week') {
    const mondayOffset = (anchor.getDay() + 6) % 7;
    startedAt = new Date(anchor.getFullYear(), anchor.getMonth(), anchor.getDate() - mondayOffset);
    endedAt = new Date(startedAt.getFullYear(), startedAt.getMonth(), startedAt.getDate() + 7);
  } else if (range === 'month') {
    startedAt = new Date(anchor.getFullYear(), anchor.getMonth(), 1);
    endedAt = new Date(anchor.getFullYear(), anchor.getMonth() + 1, 1);
  } else if (range === 'quarter') {
    const firstMonth = Math.floor(anchor.getMonth() / 3) * 3;
    startedAt = new Date(anchor.getFullYear(), firstMonth, 1);
    endedAt = new Date(anchor.getFullYear(), firstMonth + 3, 1);
  } else {
    startedAt = new Date(anchor.getFullYear(), 0, 1);
    endedAt = new Date(anchor.getFullYear() + 1, 0, 1);
  }

  return { startedAt: startedAt.getTime(), endedAt: endedAt.getTime() };
}

function customProjectRangeBounds(startDateKey: string, endDateKey: string): ProjectRangeBounds {
  const start = dateFromKey(startDateKey);
  const end = dateFromKey(endDateKey);
  if (!Number.isFinite(start.getTime()) || !Number.isFinite(end.getTime())) return null;
  const startedAt = new Date(start.getFullYear(), start.getMonth(), start.getDate()).getTime();
  const endedAt = new Date(end.getFullYear(), end.getMonth(), end.getDate() + 1).getTime();
  return startedAt < endedAt ? { startedAt, endedAt } : null;
}

function formatProjectDateRange(startDateKey: string, endDateKey: string) {
  const format = (value: string) => value.replace(/-/g, '/');
  return startDateKey === endDateKey
    ? format(startDateKey)
    : `${format(startDateKey)} – ${format(endDateKey)}`;
}

function sessionMinutesInRange(session: WorkSession, bounds: ProjectRangeBounds) {
  const sessionStart = new Date(session.startedAt).getTime();
  const sessionEnd = new Date(session.endedAt).getTime();
  if (!Number.isFinite(sessionStart) || !Number.isFinite(sessionEnd)) return 0;
  const startedAt = bounds ? Math.max(sessionStart, bounds.startedAt) : sessionStart;
  const endedAt = bounds ? Math.min(sessionEnd, bounds.endedAt) : sessionEnd;
  return Math.max(0, (endedAt - startedAt) / 60_000);
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

function sameTimelineTask(left: WorkSession, right: WorkSession) {
  if (left.category !== right.category || left.projectId !== right.projectId) return false;
  if (left.taskId || right.taskId) return Boolean(left.taskId && left.taskId === right.taskId);
  return left.summary.trim() === right.summary.trim();
}

function sessionApplication(session: WorkSession) {
  return (
    session.evidence.find((item) => item.kind === 'app' || item.label === '应用')?.value ||
    '未知应用'
  );
}

function sessionCurrentPage(session: WorkSession) {
  const page = [...session.evidence]
    .reverse()
    .find((item) => item.kind === 'page' || item.label === '当前页面')
    ?.value.trim();
  if (!page) return '';
  return page.replace(
    /\s+(?:-|—|–|·|\|)\s+(?:Google Chrome|Microsoft Edge|Mozilla Firefox|Brave|Tabbit(?: Browser)?|WPS Office)\s*$/i,
    '',
  ).trim();
}

function displaySessionSummary(session: WorkSession) {
  const summary = session.summary.trim();
  const page = sessionCurrentPage(session);
  if (!page) return summary;

  const normalizedSummary = normalizeSearchText(summary);
  const normalizedPage = normalizeSearchText(page);
  if (normalizedSummary.includes(normalizedPage)) return summary;
  return `${summary} · ${page}`;
}

function minutesBetween(start: string, end: string) {
  return Math.max(
    0,
    (new Date(end).getTime() - new Date(start).getTime()) / 60_000,
  );
}

function formatDuration(minutes: number) {
  const totalSeconds = Math.max(0, Math.round((minutes * 60) / 5) * 5);
  if (totalSeconds < 60) return `${totalSeconds} 秒`;
  const hours = Math.floor(totalSeconds / 3600);
  const minutePart = Math.floor((totalSeconds % 3600) / 60);
  const secondPart = totalSeconds % 60;
  if (hours > 0) {
    return minutePart ? `${hours} 小时 ${minutePart} 分钟` : `${hours} 小时`;
  }
  return secondPart ? `${minutePart} 分 ${secondPart} 秒` : `${minutePart} 分钟`;
}

function formatCompactDuration(minutes: number) {
  const totalMinutes = Math.max(0, Math.round(minutes));
  if (totalMinutes < 60) return `${totalMinutes}分`;
  return `${Math.floor(totalMinutes / 60)}时${totalMinutes % 60}分`;
}

function formatClock(value: string) {
  return new Date(value).toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
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

function formatSessionMoment(value: string, showDate: boolean) {
  if (!showDate) return formatTimelineClock(value, true);
  const date = new Date(value);
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${month}/${day} ${formatTimelineClock(value, true)}`;
}

function toDateTimeLocalValue(date: Date) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  const hour = String(date.getHours()).padStart(2, '0');
  const minute = String(date.getMinutes()).padStart(2, '0');
  const second = String(date.getSeconds()).padStart(2, '0');
  return `${year}-${month}-${day}T${hour}:${minute}:${second}`;
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
    ai: '复核队列与完整运行记录',
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
