import * as vscode from 'vscode';

const ENDPOINT = 'http://127.0.0.1:51247/integrations/vscode/activity';
const HEARTBEAT_MS = 90_000;
const DEBOUNCE_MS = 700;

let heartbeatTimer: NodeJS.Timeout | undefined;
let debounceTimer: NodeJS.Timeout | undefined;
let pendingKind = 'activate';
let lastSignature = '';
let lastSentAt = 0;

export function activate(context: vscode.ExtensionContext) {
  const schedule = (kind: string, immediate = false) => {
    pendingKind = kind;
    if (debounceTimer) clearTimeout(debounceTimer);
    if (immediate) {
      void sendActivity(kind, true);
      return;
    }
    debounceTimer = setTimeout(() => void sendActivity(pendingKind), DEBOUNCE_MS);
  };

  context.subscriptions.push(
    vscode.commands.registerCommand('screenuse.syncNow', () => schedule('manual', true)),
    vscode.window.onDidChangeActiveTextEditor(() => schedule('active-editor', true)),
    vscode.workspace.onDidChangeWorkspaceFolders(() => schedule('workspace-changed', true)),
    vscode.workspace.onDidSaveTextDocument(() => schedule('save')),
    vscode.window.onDidOpenTerminal(() => schedule('terminal-open')),
    vscode.window.onDidCloseTerminal(() => schedule('terminal-close')),
    vscode.window.onDidChangeActiveTerminal(() => schedule('active-terminal', true)),
    vscode.debug.onDidStartDebugSession(() => schedule('debug-start', true)),
    vscode.debug.onDidTerminateDebugSession(() => schedule('debug-stop', true)),
  );

  heartbeatTimer = setInterval(() => void sendActivity('heartbeat', true), HEARTBEAT_MS);
  schedule('activate', true);
}

export function deactivate() {
  if (heartbeatTimer) clearInterval(heartbeatTimer);
  if (debounceTimer) clearTimeout(debounceTimer);
}

async function sendActivity(eventKind = pendingKind, force = false) {
  const editor = vscode.window.activeTextEditor;
  const activeFolder = editor
    ? vscode.workspace.getWorkspaceFolder(editor.document.uri)
    : vscode.workspace.workspaceFolders?.[0];
  const workspace = activeFolder
    ? [{ name: activeFolder.name, path: activeFolder.uri.fsPath }]
    : [];
  const activeFile = editor?.document.uri.scheme === 'file'
    ? editor.document.uri.fsPath
    : editor?.document.uri.toString();
  const gitBranch = await detectGitBranch(activeFolder?.uri.fsPath);
  const debugActive = vscode.debug.activeDebugSession?.name || null;
  const signature = [
    activeFolder?.uri.fsPath || '',
    activeFile || '',
    editor?.document.languageId || '',
    gitBranch || '',
    debugActive || '',
  ].join('|');

  if (!force && signature === lastSignature && Date.now() - lastSentAt < HEARTBEAT_MS - 5_000) {
    return;
  }

  const payload = {
    source: 'vscode-extension',
    appName: vscode.env.appName,
    capturedAt: new Date().toISOString(),
    eventId: signature || 'vscode:no-editor',
    eventKind,
    workspace,
    activeFile: activeFile || null,
    languageId: editor?.document.languageId || null,
    isDirty: Boolean(editor?.document.isDirty),
    gitBranch,
    terminalCount: vscode.window.terminals.length,
    activeTerminal: vscode.window.activeTerminal?.name || null,
    debugActive,
  };

  try {
    const response = await fetch(ENDPOINT, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(payload),
      signal: AbortSignal.timeout(3_000),
    });
    if (!response.ok) throw new Error(`ScreenUse returned ${response.status}`);
    lastSignature = signature;
    lastSentAt = Date.now();
  } catch {
    // The desktop app may not be running. Stay silent and retry on the next
    // context change or heartbeat so coding is never interrupted.
  }
}

async function detectGitBranch(workspacePath?: string): Promise<string | null> {
  if (!workspacePath) return null;
  const git = vscode.extensions.getExtension('vscode.git')?.exports;
  const api = git?.getAPI?.(1);
  const repository = api?.repositories?.find(
    (candidate: { rootUri?: vscode.Uri }) => candidate.rootUri?.fsPath === workspacePath,
  ) || api?.repositories?.[0];
  return repository?.state?.HEAD?.name || null;
}
