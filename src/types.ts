export type Category = '学习' | '写作' | '开发' | '沟通' | '娱乐' | '杂务' | '离开' | string;
export type AiMode = 'off' | 'manual' | 'auto' | string;
export type ThemeMode = 'system' | 'light' | 'dark';

export interface InputStats {
  idleSeconds: number;
  keyboardEvents: number;
  mouseClicks: number;
  scrollTicks: number;
  shortcutEvents: string[];
}

export interface EvidenceItem {
  kind: string;
  label: string;
  value: string;
  weight: number;
}

export interface Project {
  id: string;
  name: string;
  category: string;
  source: string;
  color: string;
  description?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface Task {
  id: string;
  projectId: string;
  title: string;
  status: string;
  source: string;
  plannedDueAt?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface CategoryOption {
  name: string;
  color: string;
  isBuiltin: boolean;
}

export interface ContextPin {
  projectId: string;
  projectName: string;
  taskId?: string | null;
  taskTitle?: string | null;
  category: string;
  expiresAt: string;
}

export interface WorkSession {
  id: string;
  startedAt: string;
  endedAt: string;
  projectId?: string | null;
  projectName?: string | null;
  taskId?: string | null;
  taskTitle?: string | null;
  category: Category;
  summary: string;
  confidence: number;
  evidence: EvidenceItem[];
  userConfirmed: boolean;
  source: string;
}

export interface PlanItem {
  id: string;
  source: string;
  title: string;
  note?: string | null;
  startAt?: string | null;
  dueAt?: string | null;
  status: string;
  tags: string[];
  matchedSessionIds: string[];
}

export interface TrendPoint {
  label: string;
  value: number;
  group: string;
}

export interface AttributionRule {
  id: string;
  name: string;
  priority: number;
  matcher: unknown;
  projectId?: string | null;
  taskId?: string | null;
  category: string;
  createdFromCorrection: boolean;
  enabled: boolean;
}

export interface QueueHealth {
  pending: number;
  running: number;
  failed: number;
  downgraded: number;
  tempStorageGb: number;
  tempStorageLimitGb: number;
}

export interface AppSettings {
  language: string;
  theme: ThemeMode;
  pollIntervalSeconds: number;
  heartbeatSeconds: number;
  rawEventRetentionDays: number;
  idleThresholdSeconds: number;
  idleCategory: string;
  idleProjectName: string;
  passiveContentCountsAsActive: boolean;
  autoMaintenance: boolean;
  autoStart: boolean;
  launchAtLogin: boolean;
  quickPauseEnabled: boolean;
  aiMode: AiMode;
  minAiSessionMinutes: number;
  aiBaseUrl: string;
  aiModel: string;
  aiSecretRef?: string | null;
  backupDir?: string | null;

  // Kept by the backend only to migrate v0.1 settings.
  captureScope: string;
  fps: number;
  chunkMinutes: number;
  analysisTiming: string;
  tempStorageLimitGb: number;
}

export interface GithubSyncConfig {
  enabled: boolean;
  owner: string;
  repo: string;
  branch: string;
  filePath: string;
  autoSync: boolean;
  intervalMinutes: number;
  deviceId: string;
  deviceName: string;
  tokenSecretRef: string;
  keySecretRef: string;
  lastSyncedAt?: string | null;
  lastRemoteSha?: string | null;
  lastError?: string | null;
}

export interface SyncCounts {
  categories: number;
  projects: number;
  tasks: number;
  sessions: number;
  rules: number;
}

export interface SyncDeviceInfo {
  id: string;
  name: string;
  lastSeenAt: string;
}

export interface GithubSyncStatus {
  config: GithubSyncConfig;
  tokenConfigured: boolean;
  keyConfigured: boolean;
  ready: boolean;
  counts: SyncCounts;
  devices: SyncDeviceInfo[];
}

export interface GithubSyncResult {
  syncedAt: string;
  remoteSha: string;
  uploadedBytes: number;
  downloadedBytes: number;
  counts: SyncCounts;
  message: string;
}

export interface DashboardData {
  settings: AppSettings;
  sessions: WorkSession[];
  projects: Project[];
  tasks: Task[];
  categoryOptions: CategoryOption[];
  activeContext?: ContextPin | null;
  planItems: PlanItem[];
  trends: TrendPoint[];
  categories: TrendPoint[];
  queue: QueueHealth;
  collectorRunning: boolean;
}

export interface SessionPatch {
  summary?: string;
  projectId?: string | null;
  taskId?: string | null;
  clearProject?: boolean;
  clearTask?: boolean;
  category?: string;
  confidence?: number;
  userConfirmed?: boolean;
}
