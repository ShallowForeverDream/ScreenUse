import { useCallback, useEffect, useMemo, useState } from 'react';
import { Activity, BarChart3, CalendarClock, CheckCircle2, Database, Download, FolderKanban, HardDrive, Pause, Play, RefreshCcw, Search, Settings, Sparkles, SplitSquareHorizontal, TimerReset } from 'lucide-react';
import { Bar, BarChart, CartesianGrid, Cell, Legend, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';
import { api } from './api';
import type { AppSettings, DashboardData, Project, Task, WorkSession } from './types';

const tabs = [
  { id: 'timeline', label: '时间轴', icon: Activity },
  { id: 'projects', label: '项目', icon: FolderKanban },
  { id: 'reports', label: '报告', icon: BarChart3 },
  { id: 'settings', label: '设置', icon: Settings },
] as const;

type TabId = (typeof tabs)[number]['id'];
const categoryColors: Record<string, string> = { 开发: '#38bdf8', 学习: '#a78bfa', 写作: '#f0abfc', 沟通: '#34d399', 娱乐: '#fb7185', 杂务: '#facc15', 离开: '#94a3b8' };

export default function App() {
  const [activeTab, setActiveTab] = useState<TabId>('timeline');
  const [data, setData] = useState<DashboardData | null>(null);
  const [loading, setLoading] = useState(true);
  const [toast, setToast] = useState('');
  const [selected, setSelected] = useState<Set<string>>(() => new Set());

  const load = useCallback(async () => {
    const dashboard = await api.dashboard();
    setData(dashboard);
    setLoading(false);
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), 15_000);
    return () => window.clearInterval(timer);
  }, [load]);

  const showToast = useCallback((message: string) => {
    setToast(message);
    window.setTimeout(() => setToast(''), 2800);
  }, []);

  const runAction = useCallback(async (fn: () => Promise<unknown>, message: string) => {
    try { await fn(); showToast(message); await load(); }
    catch (error) { showToast(error instanceof Error ? error.message : String(error)); }
  }, [load, showToast]);

  const totals = useMemo(() => summarize(data), [data]);

  if (loading || !data) {
    return <div className="boot"><Sparkles />正在启动 ScreenUse…</div>;
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">SU</div>
          <div><strong>ScreenUse</strong><span>个人工作时间账本</span></div>
        </div>
        <nav>
          {tabs.map((tab) => {
            const Icon = tab.icon;
            return <button key={tab.id} className={activeTab === tab.id ? 'active' : ''} onClick={() => setActiveTab(tab.id)}><Icon size={18} />{tab.label}</button>;
          })}
        </nav>
        <div className="collector-card">
          <div className="collector-state"><span className={data.collectorRunning ? 'dot ok' : 'dot muted'} />{data.collectorRunning ? '采集中' : '已暂停'}</div>
          <div className="small">全显示器 · {data.settings.fps} FPS · {data.settings.chunkMinutes} 分钟切片</div>
          <div className="button-row">
            <button onClick={() => runAction(api.startCollector, '采集已启动')}><Play size={14} />启动</button>
            <button onClick={() => runAction(api.stopCollector, '采集已暂停')}><Pause size={14} />暂停</button>
          </div>
        </div>
      </aside>

      <main className="main">
        <header className="topbar">
          <div>
            <h1>{tabs.find(t => t.id === activeTab)?.label}</h1>
            <p>自动把窗口、录屏切片、标签页、VS Code 与 DDL 数据归因成项目/任务。</p>
          </div>
          <div className="top-actions">
            <button onClick={() => runAction(api.runAnalysisOnce, '已执行一次 AI/规则分析队列')}><Sparkles size={16} />分析一次</button>
            <button onClick={() => runAction(api.compactSessions, '已智能合并连续同类会话')}><RefreshCcw size={16} />整理会话</button>
            <button onClick={() => runAction(api.retryFailedJobs, '已把失败/降级任务放回待分析队列')}><RefreshCcw size={16} />重试分析</button>
            <button onClick={() => runAction(api.cleanupMediaCache, '已按缓存上限清理临时切片')}><Database size={16} />清理缓存</button>
            <button onClick={() => runAction(() => api.backupNow(), '数据库已备份')}><HardDrive size={16} />备份</button>
          </div>
        </header>

        <section className="kpi-grid">
          <Kpi title="今日有效工作" value={`${totals.workMinutes} 分钟`} hint="不含离开/空闲" />
          <Kpi title="自动归因置信度" value={`${Math.round(totals.avgConfidence * 100)}%`} hint="目标 80% 以上无需手改" />
          <Kpi title="AI 队列" value={`${data.queue.pending + data.queue.failed + data.queue.downgraded}`} hint={`失败 ${data.queue.failed} · 降级 ${data.queue.downgraded}`} />
          <Kpi title="临时缓存" value={`${data.queue.tempStorageGb.toFixed(2)} GB`} hint={`上限 ${data.queue.tempStorageLimitGb} GB，分析后删除原始切片`} />
        </section>

        {activeTab === 'timeline' && <TimelineView data={data} selected={selected} setSelected={setSelected} runAction={runAction} />}
        {activeTab === 'projects' && <ProjectsView data={data} />}
        {activeTab === 'reports' && <ReportsView data={data} runAction={runAction} />}
        {activeTab === 'settings' && <SettingsView data={data} runAction={runAction} />}
      </main>
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

function TimelineView({ data, selected, setSelected, runAction }: { data: DashboardData; selected: Set<string>; setSelected: (next: Set<string>) => void; runAction: (fn: () => Promise<unknown>, message: string) => Promise<void>; }) {
  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id); else next.add(id);
    setSelected(next);
  };
  const mergeSelected = async () => {
    const ids = [...selected];
    if (ids.length < 2) return;
    const summary = window.prompt('合并后的会话名称', '合并后的工作会话') || '合并后的工作会话';
    await api.mergeSessions(ids, summary);
    setSelected(new Set());
  };
  return (
    <div className="content-grid timeline-grid">
      <section className="panel timeline-panel">
        <div className="panel-title"><div><h2>AI 合并工作会话</h2><p>5 分钟切片只是底层数据，这里显示自然工作会话。</p></div><button disabled={selected.size < 2} onClick={() => runAction(mergeSelected, '已合并选中会话')}>合并选中</button></div>
        <div className="timeline-list">
          {data.sessions.map((session) => <SessionRow key={session.id} session={session} projects={data.projects} tasks={data.tasks} selected={selected.has(session.id)} onToggle={() => toggle(session.id)} runAction={runAction} />)}
        </div>
      </section>
      <aside className="panel review-panel">
        <div className="panel-title"><h2>复盘提醒</h2></div>
        <div className="queue-box">
          <CalendarClock size={28} />
          <strong>{data.queue.failed + data.queue.downgraded > 0 ? '有待复核分析' : '今天状态正常'}</strong>
          <p>{data.queue.pending} 个待分析切片，{data.queue.failed} 个失败，{data.queue.downgraded} 个规则降级。</p>
          <button onClick={() => runAction(api.retryFailedJobs, '已开始重试失败/低置信分析')}>一键重试</button>
        </div>
        <div className="plan-list">
          <h3>DDL / 日历计划</h3>
          {data.planItems.slice(0, 7).map(item => <div className="plan-item" key={item.id}><span>{item.source}</span><strong>{item.title}</strong><small>{item.dueAt ? formatTime(item.dueAt) : '无截止时间'}</small></div>)}
        </div>
      </aside>
    </div>
  );
}

function SessionRow({ session, projects, tasks, selected, onToggle, runAction }: { session: WorkSession; projects: Project[]; tasks: Task[]; selected: boolean; onToggle: () => void; runAction: (fn: () => Promise<unknown>, message: string) => Promise<void>; }) {
  const duration = minutesBetween(session.startedAt, session.endedAt);
  const color = categoryColors[session.category] ?? '#e2e8f0';
  const rename = async () => {
    const summary = window.prompt('新的会话摘要', session.summary);
    if (summary) await api.updateSession(session.id, { summary, userConfirmed: true });
  };
  const recategorize = async () => {
    const category = window.prompt('新的分类：学习/写作/开发/沟通/娱乐/杂务/离开', session.category);
    if (category) await api.updateSession(session.id, { category, userConfirmed: true });
  };
  const split = async () => {
    const splitAt = window.prompt('拆分时间 ISO，例如 2026-07-04T12:30:00Z', midpointIso(session.startedAt, session.endedAt));
    if (splitAt) await api.splitSession(session.id, splitAt);
  };
  const confirm = () => api.updateSession(session.id, { userConfirmed: true, confidence: Math.max(session.confidence, 0.96) });
  const learnRule = () => api.learnRuleFromSession(session.id);
  return (
    <article className={`session-row ${selected ? 'selected' : ''}`} style={{ borderLeftColor: color }}>
      <input type="checkbox" checked={selected} onChange={onToggle} />
      <div className="time-cell"><strong>{formatClock(session.startedAt)}</strong><span>{duration} 分钟</span></div>
      <div className="session-main">
        <div className="session-head"><span className="category" style={{ background: `${color}22`, color }}>{session.category}</span><strong>{session.summary}</strong>{session.userConfirmed && <CheckCircle2 size={16} className="confirmed" />}</div>
        <div className="hierarchy"><span>{session.projectName || '未归类项目'}</span><span>›</span><span>{session.taskTitle || '未归类任务'}</span></div>
        <div className="evidence-row">{session.evidence.slice(0, 4).map((e, idx) => <span key={`${e.kind}-${idx}`}>{e.label}: {e.value}</span>)}</div>
      </div>
      <div className="confidence"><span>{Math.round(session.confidence * 100)}%</span><meter min={0} max={1} value={session.confidence} /></div>
      <div className="row-actions">
        <button onClick={() => runAction(confirm, '已确认，后续 AI 不会覆盖')}>确认</button>
        <button onClick={() => runAction(learnRule, '已从该会话学习归因规则')}>学习规则</button>
        <button onClick={() => runAction(rename, '已改名并生成纠错信号')}>改名</button>
        <button onClick={() => runAction(recategorize, '已重归因')}>重归因</button>
        <button onClick={() => runAction(split, '已拆分会话')}><SplitSquareHorizontal size={14} /></button>
      </div>
    </article>
  );
}

function ProjectsView({ data }: { data: DashboardData }) {
  const tasksByProject = useMemo(() => groupBy(data.tasks, task => task.projectId), [data.tasks]);
  return <div className="content-grid two-col">
    <section className="panel"><div className="panel-title"><h2>自动发现项目</h2><p>优先由目录、Git 仓库、文档簇生成。</p></div><div className="project-grid">{data.projects.map(project => <div className="project-card" key={project.id} style={{ '--accent': project.color } as React.CSSProperties}><span>{project.category}</span><h3>{project.name}</h3><p>{project.description || '暂无描述'}</p><small>{project.source}</small></div>)}</div></section>
    <section className="panel"><div className="panel-title"><h2>任务与规则</h2></div><div className="task-list">{data.projects.map(project => <div key={project.id} className="task-group"><h3>{project.name}</h3>{(tasksByProject.get(project.id) || []).map(task => <div className="task-row" key={task.id}><TimerReset size={15} /><span>{task.title}</span><small>{task.source}</small></div>)}</div>)}</div></section>
  </div>;
}

function ReportsView({ data, runAction }: { data: DashboardData; runAction: (fn: () => Promise<unknown>, message: string) => Promise<void>; }) {
  return <div className="content-grid reports-grid">
    <section className="panel chart-panel"><div className="panel-title"><div><h2>项目/任务趋势</h2><p>回答“时间花在哪些工作上”。</p></div><div className="button-row"><button onClick={() => runAction(() => api.exportData('csv'), 'CSV 已导出')}>CSV</button><button onClick={() => runAction(() => api.exportData('excel'), 'Excel 已导出')}>Excel</button><button onClick={() => runAction(() => api.exportData('markdown'), 'Markdown 已导出')}>Markdown</button></div></div><ResponsiveContainer width="100%" height={320}><BarChart data={data.trends}><CartesianGrid strokeDasharray="3 3" stroke="#223047" /><XAxis dataKey="label" stroke="#94a3b8" /><YAxis stroke="#94a3b8" /><Tooltip contentStyle={{ background: '#101827', border: '1px solid #263244', color: '#e2e8f0' }} /><Legend /><Bar dataKey="value" name="分钟" radius={[10, 10, 0, 0]}>{data.trends.map((entry, index) => <Cell key={`cell-${index}`} fill={categoryColors[entry.group] ?? '#7dd3fc'} />)}</Bar></BarChart></ResponsiveContainer></section>
    <section className="panel chart-panel"><div className="panel-title"><h2>分类占比</h2></div><ResponsiveContainer width="100%" height={260}><BarChart data={data.categories} layout="vertical"><CartesianGrid strokeDasharray="3 3" stroke="#223047" /><XAxis type="number" stroke="#94a3b8" /><YAxis type="category" dataKey="label" stroke="#94a3b8" /><Tooltip contentStyle={{ background: '#101827', border: '1px solid #263244', color: '#e2e8f0' }} /><Bar dataKey="value" name="分钟" radius={[0, 10, 10, 0]}>{data.categories.map((entry, index) => <Cell key={`cat-${index}`} fill={categoryColors[entry.group] ?? '#7dd3fc'} />)}</Bar></BarChart></ResponsiveContainer></section>
    <section className="panel report-copy"><h2>Markdown 日报草稿</h2><pre>{buildMarkdownReport(data)}</pre></section>
  </div>;
}

function SettingsView({ data, runAction }: { data: DashboardData; runAction: (fn: () => Promise<unknown>, message: string) => Promise<void>; }) {
  const [settings, setSettings] = useState<AppSettings>(data.settings);
  const [secret, setSecret] = useState('');
  const update = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => setSettings(prev => ({ ...prev, [key]: value }));
  return <div className="content-grid settings-grid">
    <section className="panel form-panel"><div className="panel-title"><h2>采集与分析</h2></div><label>采集范围<input value={settings.captureScope} onChange={e => update('captureScope', e.target.value)} /></label><label>FPS（默认 1，保护性能）<input type="number" value={settings.fps} onChange={e => update('fps', Number(e.target.value))} /></label><label>切片分钟<input type="number" value={settings.chunkMinutes} onChange={e => update('chunkMinutes', Number(e.target.value))} /></label><label>分析模式<input value={settings.analysisTiming} onChange={e => update('analysisTiming', e.target.value)} placeholder="near-realtime / idle-batch / daily" /></label><label>空闲阈值秒<input type="number" value={settings.idleThresholdSeconds} onChange={e => update('idleThresholdSeconds', Number(e.target.value))} /></label><label>临时缓存上限 GB<input type="number" value={settings.tempStorageLimitGb} onChange={e => update('tempStorageLimitGb', Number(e.target.value))} /></label><label className="check-row"><input type="checkbox" checked={settings.autoStart} onChange={e => update('autoStart', e.target.checked)} />启动 ScreenUse 后自动采集</label><label className="check-row"><input type="checkbox" checked={settings.quickPauseEnabled} onChange={e => update('quickPauseEnabled', e.target.checked)} />显示快捷暂停入口</label><button onClick={() => runAction(() => api.saveSettings(settings), '设置已保存')}>保存设置</button></section>
    <section className="panel form-panel"><div className="panel-title"><h2>OpenAI 兼容模型</h2></div><label>API Base<input value={settings.aiBaseUrl} onChange={e => update('aiBaseUrl', e.target.value)} /></label><label>模型名<input value={settings.aiModel} onChange={e => update('aiModel', e.target.value)} /></label><label>凭据名称<input value={settings.aiSecretRef || 'openai-compatible'} onChange={e => update('aiSecretRef', e.target.value)} /></label><label>API Key<input type="password" value={secret} onChange={e => setSecret(e.target.value)} placeholder="保存到系统凭据库，不进数据库" /></label><div className="button-row"><button onClick={() => runAction(async () => { const name = settings.aiSecretRef || 'openai-compatible'; await api.saveSecret(name, secret); update('aiSecretRef', name); await api.saveSettings({ ...settings, aiSecretRef: name }); }, '密钥已写入系统凭据库')}>保存密钥</button><button onClick={() => runAction(() => api.testAiConfig(settings, settings.aiSecretRef || 'openai-compatible'), 'AI 配置可读取')}>测试配置</button></div></section>
    <section className="panel form-panel"><div className="panel-title"><h2>外部计划源</h2></div><label>DDL-Manager 数据库<input value={settings.ddlManagerDbPath} onChange={e => update('ddlManagerDbPath', e.target.value)} /></label><button onClick={() => runAction(() => api.importDdlManager(settings.ddlManagerDbPath), '已只读导入 DDL-Manager 计划')}>导入 DDL-Manager</button><label>ICS 文件路径<input placeholder="D:\\calendar.ics" id="ics-path" /></label><button onClick={() => { const value = (document.getElementById('ics-path') as HTMLInputElement | null)?.value || ''; void runAction(() => api.importIcs(value), '已导入 ICS'); }}>导入 ICS</button></section>
    <section className="panel form-panel"><div className="panel-title"><h2>数据目录</h2></div><p>SQLite、导出文件、备份和临时切片都放在本地数据目录。原始屏幕帧在 AI 成功分析后删除；降级结果会保留到缓存上限清理或手动重试。</p><button onClick={() => runAction(async () => { const path = await api.revealDataDir(); window.alert(path); }, '已显示数据目录')}>查看数据目录</button><button onClick={() => runAction(api.cleanupMediaCache, '已清理临时缓存')}>清理临时缓存</button><button onClick={() => runAction(() => api.backupNow(settings.backupDir || undefined), '已手动备份')}>立即备份</button></section>
    <section className="panel form-panel"><div className="panel-title"><h2>插件入口</h2></div><p>浏览器扩展会推送标签页标题/URL；VS Code 扩展会推送 workspace、active file、Git branch。两者都只发元数据，不读网页正文或文件正文。</p><code>extensions/chromium</code><code>extensions/vscode</code></section>
  </div>;
}

function Kpi({ title, value, hint }: { title: string; value: string; hint: string }) { return <div className="kpi"><span>{title}</span><strong>{value}</strong><small>{hint}</small></div>; }
function summarize(data: DashboardData | null) { if (!data) return { workMinutes: 0, avgConfidence: 0 }; const valid = data.sessions.filter(s => s.category !== '离开'); const workMinutes = valid.reduce((sum, s) => sum + minutesBetween(s.startedAt, s.endedAt), 0); const avgConfidence = valid.length ? valid.reduce((sum, s) => sum + s.confidence, 0) / valid.length : 0; return { workMinutes, avgConfidence }; }
function minutesBetween(start: string, end: string) { return Math.max(0, Math.round((new Date(end).getTime() - new Date(start).getTime()) / 60_000)); }
function midpointIso(start: string, end: string) { return new Date((new Date(start).getTime() + new Date(end).getTime()) / 2).toISOString(); }
function formatClock(value: string) { return new Date(value).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }); }
function formatTime(value: string) { return new Date(value).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }); }
function groupBy<T>(items: T[], key: (item: T) => string) { const map = new Map<string, T[]>(); for (const item of items) { const k = key(item); map.set(k, [...(map.get(k) || []), item]); } return map; }
function buildMarkdownReport(data: DashboardData) { const lines = ['# ScreenUse 日报', '', '## 项目耗时']; for (const trend of data.trends.slice(0, 8)) lines.push(`- ${trend.label}：${trend.value} 分钟（${trend.group}）`); lines.push('', '## 主要工作会话'); for (const s of data.sessions.slice(0, 8)) lines.push(`- ${formatClock(s.startedAt)}-${formatClock(s.endedAt)} ${s.projectName || '未归类'} / ${s.taskTitle || '未归类'}：${s.summary}（置信度 ${Math.round(s.confidence * 100)}%）`); return lines.join('\n'); }
