import type { DashboardData } from './types';

const now = new Date();
const iso = (minutesAgo: number) => new Date(now.getTime() - minutesAgo * 60_000).toISOString();

export const fallbackDashboard: DashboardData = {
  collectorRunning: false,
  settings: {
    language: 'zh-CN',
    captureScope: 'all-displays',
    fps: 1,
    chunkMinutes: 5,
    analysisTiming: 'near-realtime',
    aiBaseUrl: 'https://api.openai.com/v1',
    aiModel: 'gpt-4o-mini',
    aiSecretRef: null,
    tempStorageLimitGb: 20,
    idleThresholdSeconds: 180,
    backupDir: null,
    ddlManagerDbPath: `${navigator.platform.includes('Win') ? '%USERPROFILE%\\.ddl-manager\\app.db' : '~/.ddl-manager/app.db'}`,
    autoStart: true,
    quickPauseEnabled: true,
  },
  projects: [
    { id: 'p1', name: 'ScreenUse 开发', category: '开发', source: 'fallback', color: '#7dd3fc', description: '当前产品实现', createdAt: iso(300), updatedAt: iso(10) },
    { id: 'p2', name: '课程与论文', category: '学习', source: 'fallback', color: '#c4b5fd', description: '课程、论文、资料阅读', createdAt: iso(300), updatedAt: iso(20) },
  ],
  tasks: [
    { id: 't1', projectId: 'p1', title: '实现采集与归因闭环', status: 'active', source: 'fallback', plannedDueAt: null, createdAt: iso(280), updatedAt: iso(10) },
    { id: 't2', projectId: 'p2', title: '竞品调研与报告', status: 'active', source: 'fallback', plannedDueAt: iso(-120), createdAt: iso(260), updatedAt: iso(50) },
  ],
  sessions: [
    { id: 's1', startedAt: iso(180), endedAt: iso(112), projectId: 'p1', projectName: 'ScreenUse 开发', taskId: 't1', taskTitle: '实现采集与归因闭环', category: '开发', summary: '搭建 Tauri + React 项目骨架', confidence: 0.86, userConfirmed: false, source: 'fallback', evidence: [{ kind: 'window', label: '窗口', value: 'VS Code / Codex', weight: 0.8 }] },
    { id: 's2', startedAt: iso(100), endedAt: iso(48), projectId: 'p2', projectName: '课程与论文', taskId: 't2', taskTitle: '竞品调研与报告', category: '学习', summary: '阅读 ActivityWatch、Tai、Dayflow 资料', confidence: 0.78, userConfirmed: false, source: 'fallback', evidence: [{ kind: 'url', label: '网页', value: 'GitHub / HN / 文档', weight: 0.7 }] },
    { id: 's3', startedAt: iso(40), endedAt: iso(5), projectId: 'p1', projectName: 'ScreenUse 开发', taskId: 't1', taskTitle: '实现采集与归因闭环', category: '开发', summary: '设计 AI 队列、失败重试和导出', confidence: 0.82, userConfirmed: false, source: 'fallback', evidence: [{ kind: 'ai', label: 'AI摘要', value: '后端架构实现', weight: 0.9 }] },
  ],
  planItems: [
    { id: 'ddl-1', source: 'DDL-Manager', title: 'ScreenUse v1 内测', note: '只读导入示例', startAt: null, dueAt: iso(-360), status: 'todo', tags: ['项目'], matchedSessionIds: [] },
  ],
  trends: [
    { label: 'ScreenUse 开发', value: 103, group: '开发' },
    { label: '课程与论文', value: 52, group: '学习' },
  ],
  categories: [
    { label: '开发', value: 103, group: '开发' },
    { label: '学习', value: 52, group: '学习' },
  ],
  queue: { pending: 2, running: 0, failed: 0, downgraded: 0, tempStorageGb: 0.3, tempStorageLimitGb: 20 },
};
