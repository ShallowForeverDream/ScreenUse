import { invoke } from '@tauri-apps/api/core';
import type { AnalysisJob, AppSettings, AttributionRule, CategoryOption, CodexRateCard, ContextPin, DashboardData, GithubSyncConfig, GithubSyncResult, GithubSyncStatus, Project, SessionPatch, Task, UndoStatus, WorkSession } from './types';
import { fallbackDashboard } from './mock';

const isTauri = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

const previewSyncStatus = (): GithubSyncStatus => ({
  config: {
    enabled: false,
    owner: 'ShallowForeverDream',
    repo: 'ScreenUse-Data',
    branch: 'main',
    filePath: 'screenuse/snapshot-v1.json.gz.enc',
    autoSync: true,
    intervalMinutes: 15,
    deviceId: 'browser-preview',
    deviceName: '这台电脑',
    tokenSecretRef: 'github-sync-token',
    keySecretRef: 'github-sync-key',
    lastSyncedAt: null,
    lastRemoteSha: null,
    lastError: null,
  },
  tokenConfigured: false,
  keyConfigured: false,
  ready: false,
  counts: {
    categories: fallbackDashboard.categoryOptions.length,
    projects: fallbackDashboard.projects.length,
    tasks: fallbackDashboard.tasks.length,
    sessions: fallbackDashboard.sessions.length,
    rules: 0,
  },
  devices: [],
});

const previewCodexRateCard = (): CodexRateCard => ({
  sourceUrl: 'https://help.openai.com/en/articles/20001106-codex-rate-card',
  fetchedAt: '2026-07-15T00:00:00Z',
  sourceUpdatedLabel: 'ScreenUse 内置官方费率快照',
  usdPerCredit: 0.04,
  creditValueSourceUrl: 'https://help.openai.com/en/articles/20001147-codex-credits-for-students-terms-of-service',
  rates: [
    { model: 'GPT-5.6 Sol', inputCreditsPerMillion: 125, cachedInputCreditsPerMillion: 12.5, outputCreditsPerMillion: 750 },
    { model: 'GPT-5.6 Terra', inputCreditsPerMillion: 62.5, cachedInputCreditsPerMillion: 6.25, outputCreditsPerMillion: 375 },
    { model: 'GPT-5.6 Luna', inputCreditsPerMillion: 25, cachedInputCreditsPerMillion: 2.5, outputCreditsPerMillion: 150 },
  ],
});

async function call<T>(command: string, args?: Record<string, unknown>, fallback?: T): Promise<T> {
  if (!isTauri()) {
    if (fallback !== undefined) return fallback;
    throw new Error(`Tauri command unavailable in browser preview: ${command}`);
  }
  return invoke<T>(command, args);
}

export const api = {
  dashboard: () => call<DashboardData>('get_dashboard_data', undefined, fallbackDashboard),
  startCollector: () => call<void>('start_collector'),
  stopCollector: () => call<void>('stop_collector'),
  updateSession: (id: string, patch: SessionPatch) => call<WorkSession>('update_session', { id, patch }),
  updateSessions: (ids: string[], patch: SessionPatch) => call<WorkSession[]>('update_sessions', { ids, patch }, []),
  applySessionCorrection: (
    ids: string[],
    patch: SessionPatch,
    remember = false,
    keyword?: string,
    pinMinutes?: number,
  ) => call<WorkSession[]>('apply_session_correction', {
    ids,
    patch,
    remember,
    keyword: keyword || null,
    pinMinutes: pinMinutes || null,
  }, []),
  undoStatus: () => call<UndoStatus>('get_undo_status', undefined, { available: false }),
  undoLastSessionCorrection: () => call<string>('undo_last_session_correction'),
  createProject: (name: string, category: string) =>
    call<Project>('create_project', { name, category }, {
      id: `preview-${Date.now()}`,
      name,
      category,
      source: 'manual',
      color: '#a78bfa',
      description: '在修正归类时手动创建',
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
    }),
  updateProject: (id: string, name: string, category: string) =>
    call<Project>('update_project', { id, name, category }, {
      id,
      name,
      category,
      source: 'manual',
      color: '#a78bfa',
      description: '在项目账本中修改',
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
    }),
  deleteProject: async (id: string) => {
    if (!isTauri()) return;
    await call<void>('delete_project', { id });
  },
  createCategory: (name: string) =>
    call<CategoryOption>('create_category', { name }, { name, color: '#a855f7', isBuiltin: false }),
  renameCategory: (oldName: string, newName: string) =>
    call<CategoryOption>('rename_category', { oldName, newName }, {
      name: newName,
      color: '#a855f7',
      isBuiltin: false,
    }),
  deleteCategory: async (name: string) => {
    if (!isTauri()) return name === '杂务' ? '开发' : '杂务';
    return call<string>('delete_category', { name });
  },
  createTask: (projectId: string, title: string) =>
    call<Task>('create_task', { projectId, title }, {
      id: `preview-task-${Date.now()}`,
      projectId,
      title,
      status: 'active',
      source: 'manual',
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
    }),
  deleteTask: async (id: string) => {
    if (!isTauri()) return;
    await call<void>('delete_task', { id });
  },
  pinContext: (projectId: string, taskId?: string | null, minutes = 30) =>
    call<ContextPin>('pin_context', { projectId, taskId: taskId || null, minutes }),
  clearContextPin: () => call<void>('clear_context_pin'),
  mergeSessions: (ids: string[], summary?: string) => call<WorkSession>('merge_sessions', { ids, summary }),
  splitSession: (id: string, splitAt: string) => call<WorkSession[]>('split_session', { id, splitAt }),
  retryFailedJobs: () => call<number>('retry_failed_jobs', undefined, 0),
  listAnalysisJobs: (limit = 200) => call<AnalysisJob[]>('list_analysis_jobs', { limit }, []),
  getAnalysisJob: (id: string) => call<AnalysisJob | null>('get_analysis_job', { id }, null),
  getCodexRateCard: () => call<CodexRateCard>('get_codex_rate_card', undefined, previewCodexRateCard()),
  refreshCodexRateCard: () => call<CodexRateCard>('refresh_codex_rate_card', undefined, previewCodexRateCard()),
  deleteAnalysisJob: async (id: string) => {
    if (!isTauri()) return;
    await call<void>('delete_analysis_job', { id });
  },
  runAnalysisOnce: () => call<boolean>('run_analysis_once', undefined, false),
  compactSessions: () => call<number>('compact_sessions', undefined, 0),
  learnRuleFromSession: (id: string, keyword?: string) => call<AttributionRule>('learn_rule_from_session', { id, keyword: keyword || null }),
  cleanupMediaCache: () => call<number>('cleanup_media_cache', undefined, 0),
  saveSettings: (settings: AppSettings) => call<void>('save_settings', { settings }),
  githubSyncStatus: () => call<GithubSyncStatus>('get_github_sync_status', undefined, previewSyncStatus()),
  saveGithubSyncConfig: (
    config: GithubSyncConfig,
    token?: string,
    encryptionKey?: string,
  ) => call<GithubSyncStatus>('save_github_sync_config', {
    config,
    token: token || null,
    encryptionKey: encryptionKey || null,
  }, {
    ...previewSyncStatus(),
    config,
    tokenConfigured: Boolean(token),
    keyConfigured: Boolean(encryptionKey),
    ready: config.enabled && Boolean(config.owner && token && encryptionKey),
  }),
  generateGithubSyncKey: () => call<string>(
    'generate_github_sync_key',
    undefined,
    'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',
  ),
  readGithubSyncKey: () => call<string>(
    'read_github_sync_key',
    undefined,
    'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',
  ),
  syncGithubNow: () => call<GithubSyncResult>('sync_github_now', undefined, {
    syncedAt: new Date().toISOString(),
    remoteSha: 'browser-preview',
    uploadedBytes: 4096,
    downloadedBytes: 0,
    counts: previewSyncStatus().counts,
    message: '浏览器预览：已模拟同步',
  }),
  disconnectGithubSync: (removeCredentials = false) => call<GithubSyncStatus>(
    'disconnect_github_sync',
    { removeCredentials },
    previewSyncStatus(),
  ),
  exportData: (format: 'csv' | 'excel' | 'markdown') => call<string>('export_data', { format }, `browser-preview.${format}`),
  backupNow: (targetDir?: string) => call<string>('backup_now', { targetDir }, 'browser-preview-backup.db'),
  revealDataDir: () => call<string>('reveal_data_dir', undefined, '浏览器预览模式'),
  importIcs: (path: string) => call<number>('import_ics', { path }, 0),
  saveSecret: (name: string, value: string) => call<string>('save_secret', { name, value }, `credential://ScreenUse/${name}`),
  testAiConfig: (settings: AppSettings, secretName: string) => call<string>('test_ai_config', { settings, secretName }, '浏览器预览：未调用真实模型'),
};
