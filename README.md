# ScreenUse

ScreenUse 是个人自用的智能电脑活动时间追踪工具。当前版本已经补齐“采集 → 本地元数据 → AI/规则归因 → 时间轴纠错 → 学习规则 → 报告导出/备份”的闭环，Windows 优先。

## 现在已经完成

- Tauri 桌面端 + React 中文 UI。
- SQLite 本地数据库：项目、任务、工作会话、原始事件、媒体切片、AI 队列、计划项、导出记录、归因规则。
- Windows 前台窗口/空闲检测：采集窗口标题、PID、exe 路径、进程名，默认 10 秒采样，3 分钟空闲归为“离开”。
- 真实低频屏幕采集：默认全显示器、最高 1 FPS、5 分钟切片；帧会缩放到适合 AI/复盘的尺寸并写入本地 `media-cache`。
- AI 队列 worker：后台自动领取 `analysis_jobs`，优先调用 OpenAI-compatible `/chat/completions` 归因；失败自动重试，最终规则降级。
- 分析成功后自动删除原始屏幕帧；降级结果保留到重试或缓存上限清理。
- 规则降级引擎：根据窗口、URL、文件路径、workspace、空闲状态识别开发/学习/写作/沟通/娱乐/杂务/离开。
- 人工确认保护：确认后的会话不会被后续 AI/规则覆盖。
- 一键从会话学习归因规则，后续相似窗口/URL/目录自动命中。
- 智能整理连续同类会话，支持时间轴改名、合并、拆分、重归因、确认。
- 系统托盘：打开主界面、开始采集、暂停采集、分析一次、退出。
- 本地 HTTP 采集端口 `127.0.0.1:51247`：浏览器扩展和 VS Code 扩展推送元数据。
- Chromium 扩展：读取标签标题、URL、标签组，不读网页正文。
- VS Code 扩展：读取 workspace、active file、Git branch、保存/终端/debug 活动，不读文件正文。
- DDL-Manager 只读导入。
- ICS 导入：支持折行、稳定 UID、日期/时间字段规范化。
- 报告中心：项目/任务趋势、分类占比、Markdown 日报草稿。
- CSV、Excel 可打开的 `.xls`、Markdown 导出。
- 手动备份、本地缓存清理。
- API Key 写入系统凭据库，数据库只保存凭据引用。

## 运行

```powershell
pnpm install
pnpm tauri:dev
```

只看前端预览：

```powershell
pnpm dev
```

构建前检查：

```powershell
pnpm check
pnpm build
pnpm cargo:check
```

打包：

```powershell
pnpm tauri:build
```

## 插件

### Chromium 扩展

1. 打开 Chrome / Edge / Tabbit 的扩展管理页。
2. 开启开发者模式。
3. 加载 `extensions/chromium`。
4. ScreenUse 运行时，扩展会把标签元数据 POST 到 `http://127.0.0.1:51247/integrations/browser/tabs`。

### VS Code 扩展

```powershell
cd extensions/vscode
pnpm install
pnpm compile
```

开发调试时用 VS Code Extension Host 加载该目录。插件会把 workspace/file/Git/terminal/debug 元数据 POST 到 `http://127.0.0.1:51247/integrations/vscode/activity`。

## 推荐个人配置

- `FPS`: 1。
- `切片分钟`: 5。
- `空闲阈值秒`: 180。
- `临时缓存上限`: 20GB。
- `分析模式`: `near-realtime`。
- 配好 OpenAI-compatible Base、模型名、凭据名称后，在设置页保存 API Key。
- 每天结束时打开报告页，确认低置信会话，并对常见误判点点击“学习规则”。
