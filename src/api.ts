import { invoke } from '@tauri-apps/api/core';
import type { AppSettings, AttributionRule, DashboardData, SessionPatch, WorkSession } from './types';
import { fallbackDashboard } from './mock';

const isTauri = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

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
  mergeSessions: (ids: string[], summary?: string) => call<WorkSession>('merge_sessions', { ids, summary }),
  splitSession: (id: string, splitAt: string) => call<WorkSession[]>('split_session', { id, splitAt }),
  retryFailedJobs: () => call<number>('retry_failed_jobs', undefined, 0),
  runAnalysisOnce: () => call<boolean>('run_analysis_once', undefined, false),
  compactSessions: () => call<number>('compact_sessions', undefined, 0),
  learnRuleFromSession: (id: string) => call<AttributionRule>('learn_rule_from_session', { id }),
  cleanupMediaCache: () => call<number>('cleanup_media_cache', undefined, 0),
  saveSettings: (settings: AppSettings) => call<void>('save_settings', { settings }),
  exportData: (format: 'csv' | 'excel' | 'markdown') => call<string>('export_data', { format }, `browser-preview.${format}`),
  backupNow: (targetDir?: string) => call<string>('backup_now', { targetDir }, 'browser-preview-backup.db'),
  revealDataDir: () => call<string>('reveal_data_dir', undefined, '浏览器预览模式'),
  importDdlManager: (dbPath?: string) => call<number>('import_ddl_manager', { dbPath }, 0),
  importIcs: (path: string) => call<number>('import_ics', { path }, 0),
  saveSecret: (name: string, value: string) => call<string>('save_secret', { name, value }, `credential://ScreenUse/${name}`),
  testAiConfig: (settings: AppSettings, secretName: string) => call<string>('test_ai_config', { settings, secretName }, '浏览器预览：未调用真实模型'),
};
