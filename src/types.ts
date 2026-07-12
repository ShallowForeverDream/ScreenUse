export type Category = '学习' | '写作' | '开发' | '沟通' | '娱乐' | '杂务' | '离开' | string;
export type AiMode = 'off' | 'manual' | 'auto' | string;

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
  pollIntervalSeconds: number;
  heartbeatSeconds: number;
  rawEventRetentionDays: number;
  idleThresholdSeconds: number;
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
  ddlManagerDbPath: string;

  // Kept by the backend only to migrate v0.1 settings.
  captureScope: string;
  fps: number;
  chunkMinutes: number;
  analysisTiming: string;
  tempStorageLimitGb: number;
}

export interface DashboardData {
  settings: AppSettings;
  sessions: WorkSession[];
  projects: Project[];
  tasks: Task[];
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
  category?: string;
  confidence?: number;
  userConfirmed?: boolean;
}
