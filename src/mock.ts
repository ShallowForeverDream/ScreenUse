import type { DashboardData } from './types';

const now = new Date();
const iso = (minutesAgo: number) => new Date(now.getTime() - minutesAgo * 60_000).toISOString();
const home = typeof navigator !== 'undefined' && navigator.platform.includes('Win') ? '%USERPROFILE%' : '~';

export const fallbackDashboard: DashboardData = {
  collectorRunning: true,
  activeContext: null,
  categoryOptions: [
    { name: '开发', color: '#60a5fa', isBuiltin: true },
    { name: '学习', color: '#a78bfa', isBuiltin: true },
    { name: '写作', color: '#f472b6', isBuiltin: true },
    { name: '沟通', color: '#34d399', isBuiltin: true },
    { name: '娱乐', color: '#fb7185', isBuiltin: true },
    { name: '杂务', color: '#fbbf24', isBuiltin: true },
    { name: '离开', color: '#94a3b8', isBuiltin: true },
  ],
  settings: {
    language: 'zh-CN',
    theme: 'light',
    pollIntervalSeconds: 10,
    heartbeatSeconds: 10,
    rawEventRetentionDays: 30,
    idleThresholdSeconds: 180,
    autoMaintenance: true,
    autoStart: true,
    launchAtLogin: false,
    quickPauseEnabled: true,
    aiMode: 'off',
    minAiSessionMinutes: 10,
    aiBaseUrl: 'https://api.openai.com/v1',
    aiModel: '',
    aiSecretRef: null,
    backupDir: null,
    ddlManagerDbPath: `${home}\\.ddl-manager\\app.db`,
    captureScope: 'metadata-only',
    fps: 0,
    chunkMinutes: 0,
    analysisTiming: 'local-only',
    tempStorageLimitGb: 1,
  },
  projects: [
    { id: 'p1', name: 'ScreenUse', category: '开发', source: 'workspace-auto', color: '#60a5fa', description: '根据 VS Code 工作区自动识别', createdAt: iso(700), updatedAt: iso(5) },
    { id: 'p2', name: '课程与论文', category: '学习', source: 'manual', color: '#a78bfa', description: '课程、论文和资料阅读', createdAt: iso(1200), updatedAt: iso(45) },
    { id: 'p3', name: '日常事务', category: '杂务', source: 'manual', color: '#fbbf24', description: '零散电脑事务', createdAt: iso(1800), updatedAt: iso(90) },
  ],
  tasks: [
    { id: 't1', projectId: 'p1', title: '日常开发', status: 'active', source: 'workspace-auto', plannedDueAt: null, createdAt: iso(700), updatedAt: iso(5) },
    { id: 't2', projectId: 'p2', title: '资料阅读', status: 'active', source: 'manual', plannedDueAt: null, createdAt: iso(1200), updatedAt: iso(45) },
    { id: 't3', projectId: 'p3', title: '日常事务', status: 'active', source: 'manual', plannedDueAt: null, createdAt: iso(1800), updatedAt: iso(90) },
  ],
  sessions: [
    { id: 's1', startedAt: iso(188), endedAt: iso(126), projectId: 'p1', projectName: 'ScreenUse', taskId: 't1', taskTitle: '日常开发', category: '开发', summary: 'ScreenUse · collectors.rs', confidence: 0.91, userConfirmed: false, source: 'collector-rule', evidence: [{ kind: 'workspace', label: '工作区', value: 'ScreenUse', weight: 0.8 }] },
    { id: 's2', startedAt: iso(118), endedAt: iso(76), projectId: 'p2', projectName: '课程与论文', taskId: 't2', taskTitle: '资料阅读', category: '学习', summary: 'arxiv.org · 时间追踪研究', confidence: 0.79, userConfirmed: false, source: 'collector-rule', evidence: [{ kind: 'url', label: '网页', value: 'https://arxiv.org', weight: 0.7 }] },
    { id: 's3', startedAt: iso(68), endedAt: iso(39), projectId: 'p1', projectName: 'ScreenUse', taskId: 't1', taskTitle: '日常开发', category: '开发', summary: 'ScreenUse · App.tsx', confidence: 0.89, userConfirmed: true, source: 'collector-rule', evidence: [{ kind: 'file', label: '文件', value: 'src/App.tsx', weight: 0.8 }] },
    { id: 's4', startedAt: iso(32), endedAt: iso(8), projectId: null, projectName: null, taskId: null, taskTitle: null, category: '杂务', summary: 'Chrome · 新标签页', confidence: 0.56, userConfirmed: false, source: 'context-complete', evidence: [{ kind: 'window', label: '窗口', value: '新标签页', weight: 0.5 }] },
  ],
  planItems: [
    { id: 'ddl-1', source: 'DDL-Manager', title: '完成 ScreenUse 低占用版本', note: '个人使用', startAt: null, dueAt: iso(-360), status: 'todo', tags: ['项目'], matchedSessionIds: ['s1', 's3'] },
  ],
  trends: [
    { label: 'ScreenUse', value: 91, group: '开发' },
    { label: '课程与论文', value: 42, group: '学习' },
    { label: '未归类', value: 24, group: '杂务' },
  ],
  categories: [
    { label: '开发', value: 91, group: '开发' },
    { label: '学习', value: 42, group: '学习' },
    { label: '杂务', value: 24, group: '杂务' },
  ],
  queue: { pending: 0, running: 0, failed: 0, downgraded: 0, tempStorageGb: 0, tempStorageLimitGb: 1 },
};
