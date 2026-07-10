import * as vscode from 'vscode';

const ENDPOINT = 'http://127.0.0.1:51247/integrations/vscode/activity';
let timer: NodeJS.Timeout | undefined;
let lastEvent = 'activate';

export function activate(context: vscode.ExtensionContext) {
  const sync = (event = 'heartbeat') => { lastEvent = event; void sendActivity(event); };
  context.subscriptions.push(vscode.commands.registerCommand('screenuse.syncNow', sync));
  context.subscriptions.push(vscode.window.onDidChangeActiveTextEditor(() => sync('active-editor')));
  context.subscriptions.push(vscode.workspace.onDidSaveTextDocument(() => sync('save')));
  context.subscriptions.push(vscode.window.onDidOpenTerminal(() => sync('terminal')));
  context.subscriptions.push(vscode.debug.onDidStartDebugSession(() => sync('debug-start')));
  context.subscriptions.push(vscode.debug.onDidTerminateDebugSession(() => sync('debug-stop')));
  timer = setInterval(() => sync('heartbeat'), 60_000);
  sync('activate');
}

export function deactivate() {
  if (timer) clearInterval(timer);
}

async function sendActivity(eventKind = lastEvent) {
  const editor = vscode.window.activeTextEditor;
  const workspace = vscode.workspace.workspaceFolders?.map(folder => ({ name: folder.name, path: folder.uri.fsPath })) || [];
  const gitBranch = await detectGitBranch(workspace[0]?.path);
  const payload = {
    source: 'vscode-extension',
    capturedAt: new Date().toISOString(),
    eventKind,
    workspace,
    activeFile: editor?.document.uri.scheme === 'file' ? editor.document.uri.fsPath : editor?.document.uri.toString(),
    languageId: editor?.document.languageId,
    isDirty: editor?.document.isDirty,
    gitBranch,
    terminalCount: vscode.window.terminals.length,
    debugActive: vscode.debug.activeDebugSession?.name || null,
  };
  try {
    await fetch(ENDPOINT, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(payload) });
  } catch {
    // ScreenUse may not be running; stay silent to avoid interrupting coding.
  }
}

async function detectGitBranch(workspacePath?: string): Promise<string | null> {
  if (!workspacePath) return null;
  const git = vscode.extensions.getExtension('vscode.git')?.exports;
  const api = git?.getAPI?.(1);
  const repo = api?.repositories?.find((r: any) => r.rootUri?.fsPath === workspacePath) || api?.repositories?.[0];
  return repo?.state?.HEAD?.name || null;
}
