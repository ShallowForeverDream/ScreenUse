# ScreenUse

[![ScreenUse CI](https://github.com/ShallowForeverDream/ScreenUse/actions/workflows/ci.yml/badge.svg)](https://github.com/ShallowForeverDream/ScreenUse/actions/workflows/ci.yml)

ScreenUse 是一个 **Windows 优先、低占用、无需手动开计时器** 的个人电脑时间账本。

它根据前台应用、窗口标题、活动浏览器标签页、VS Code 工作区/文件名和空闲状态，在本机自动整理出：

- 今天电脑真正使用了多久；
- 时间花在哪个项目、任务和分类；
- 哪几条记录确实需要人工确认；
- 每次修正后，下一次能否自动识别得更好。

> 0.2 的默认链路不截图、不录屏、不做 OCR，也不要求配置 AI。AI 只是一项可关闭的低置信会话复核功能。

## 为什么重做 0.1

0.1 以 `1 FPS BMP 截图 → 5 分钟切片 → AI 队列 → 20 GB 缓存` 为主流程。这个方向适合“回看屏幕发生过什么”，但不适合个人长期无感记录时间：写盘多、空间大、模型费用持续产生，而且很多分类其实仅靠窗口、网址和工作区就能完成。

0.2 改为 metadata-first：

| 项目 | 0.1 | 0.2 |
| --- | --- | --- |
| 默认采集 | 周期截图 + 窗口元数据 | 前台窗口/标签页/编辑器元数据 |
| 原始数据增长 | 按 FPS 持续增长 | 主要按上下文切换次数增长 |
| 临时缓存 | 默认最高 20 GB | 无媒体缓存 |
| AI | 主归因链路 | 默认关闭，可选复核 |
| 常用交互 | 手动触发分析、管理队列 | 日常只看结果，少量纠错 |
| 自动学习 | 规则降级 | 规则优先 + 项目/工作区匹配 |

第一次启动 0.2 时会自动删除 0.1 遗留的 `media-cache`、媒体记录、截图分析任务和示例会话。

## 工作原理

```text
Windows 前台窗口 + 空闲状态
           │
           ├── Chromium：只补充当前活动标签页
           ├── VS Code：只补充当前工作区/活动文件/Git 分支
           │
           ▼
稳定上下文 ID + 低频心跳覆盖
           │
           ▼
用户学习规则 → 本地分类 → 项目/任务匹配
           │
           ├── 高置信：直接进入时间账本
           └── 低置信长会话：可选 AI 复核
           ▼
SQLite 会话、日期概览、时间轴和导出
```

### 低占用的关键点

- 默认每 2 秒只读取一次前台窗口和系统空闲时间，不采集图像。
- 同一个上下文使用稳定事件 ID；30 秒心跳会覆盖同一条原始事件，不会每次采样都新增一行。
- 应用、标签页、文件或空闲状态变化时才结束上一段并建立新上下文。
- 空闲时检测间隔自动放宽到至少 10 秒。
- Chromium 扩展不再枚举所有窗口和标签，只发送当前活动标签页。
- VS Code 扩展把短时间内的保存、终端和编辑器事件去抖合并，心跳为 90 秒。
- 原始事件默认保留 30 天；已归因的工作会话长期保留。
- SQLite 每 6 小时自动执行清理、checkpoint 和 `PRAGMA optimize`。

### 分类顺序

1. 空闲阈值命中时归为“离开”。
2. 用户从已修正会话学习的规则优先。
3. 本地应用/标题/URL/文件规则判断开发、学习、写作、沟通、娱乐或杂务。
4. 使用工作区名、项目名、任务名和相关关键词匹配项目/任务。
5. IDE 工作区无法匹配时，可自动创建对应项目。
6. 只有启用 AI 后，低置信且达到最短时长的会话才进入复核。

一次人工修正可以把分类、项目、任务和摘要设为已确认；再点击“从这条记录学习规则”，相似活动以后会优先命中该选择。

## 已实现

- Tauri 2 + Rust + React 19 桌面端。
- Windows 前台窗口、进程和系统空闲时间采集。
- 本地 SQLite 项目、任务、会话、规则、计划、导出与备份。
- 日期概览、分类占比、项目排行、上下文次数、最长连续会话和待复核收件箱。
- 时间轴搜索、人工确认、改名、重归类、合并、拆分和规则学习。
- Chromium Manifest V3 活动标签页扩展。
- VS Code 工作区、活动文件、语言、Git 分支、终端和调试状态扩展。
- DDL-Manager 只读导入和 ICS 导入。
- CSV、Excel 可打开的 `.xls`、Markdown 导出。
- 系统托盘启动、暂停、打开和退出。
- 自动原始事件轮转、旧媒体迁移清理、数据库压缩和备份。
- 可选 OpenAI-compatible API；请求最多 80 条精简元数据、30 秒超时、URL 去查询参数和锚点。

## 运行与构建

### 环境

- Windows 10/11；
- Node.js 22；
- pnpm 10；
- Rust 1.77 或更高；
- Microsoft Edge WebView2 Runtime。

### 开发

```powershell
pnpm install
pnpm tauri:dev
```

只预览前端：

```powershell
pnpm dev
```

检查：

```powershell
pnpm check
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
```

打包：

```powershell
pnpm tauri:build
```

## 浏览器扩展

1. 打开 Chrome、Edge、Brave、Vivaldi 等 Chromium 浏览器的扩展管理页。
2. 开启开发者模式。
3. 选择“加载已解压的扩展程序”。
4. 选择 `extensions/chromium`。
5. 保持 ScreenUse 桌面端运行。

扩展只向本机 `http://127.0.0.1:51247/integrations/browser/tabs` 发送当前活动标签页的标题、去除查询参数后的 URL、标签 ID、窗口 ID和音频状态。其他标签页不会进入载荷。

## VS Code 扩展

```powershell
cd extensions/vscode
pnpm install
pnpm compile
```

在 VS Code Extension Host 中加载该目录。扩展向本机 `/integrations/vscode/activity` 发送当前工作区、活动文件路径、语言、Git 分支、终端数量和调试状态，不读取文件正文。

## 推荐个人配置

| 设置 | 建议值 | 说明 |
| --- | ---: | --- |
| 前台检测 | 2 秒 | 切换感知和 CPU 之间的平衡 |
| 稳定上下文心跳 | 30 秒 | 同一 ID 覆盖，不按心跳增加行数 |
| 离开判定 | 180 秒 | 短暂思考不会立即算离开 |
| 原始元数据保留 | 30 天 | 会话长期保留，原始事件可轮转 |
| 自动维护 | 开启 | 每 6 小时轻量清理 |
| AI 模式 | 关闭 | 零费用；本地规则已经能完成主流程 |
| AI 最短会话 | 10 分钟 | 启用时避免为碎片调用模型 |

个人使用时，最有效的训练方式不是持续调用 AI，而是连续使用几天，每天只修正少量待复核记录并学习规则。

## 数据

数据目录可在“设置 → 数据管理 → 查看数据目录”中获取。主要文件为 `screenuse.db`，导出和备份位于同一应用数据目录下的独立文件夹。

- `work_sessions`：长期时间账本；
- `raw_events`：用于解释与再归因的短期元数据；
- `attribution_rules`：从人工修正学习的规则；
- `projects` / `tasks`：归因目标；
- `analysis_jobs`：仅在启用可选 AI 时使用。

## 设计参考

ScreenUse 没有照搬某一个产品，而是组合适合个人电脑时间账本的部分：

- [ActivityWatch](https://docs.activitywatch.net/en/latest/)：watcher、事件、心跳合并、空闲状态和本地数据模型。
- [RescueTime](https://www.rescuetime.com/features)：后台自动记录、按天查看、少量建议复核，而不是依赖手动计时器。
- [Timing](https://timingapp.com/)：时间轴纠错、项目规则和“自动记录后再整理”的工作流。
- [WakaTime](https://wakatime.com/)：编辑器元数据足以识别大量开发工作，不需要读取代码正文。
- [screenpipe](https://github.com/screenpipe/screenpipe)：连续屏幕/音频采集适合可搜索记忆，但对本项目的低空间、低费用目标过重，因此明确不作为默认链路。

详细取舍见 [`docs/METADATA_FIRST.md`](docs/METADATA_FIRST.md)。

## 当前范围

0.2 的系统级前台窗口采集器目前针对 Windows 实现。React/Tauri 数据层、Chromium 扩展、VS Code 扩展和本地 HTTP 上下文接口可跨平台编译，但 macOS/Linux 的原生前台窗口适配器仍属于后续范围。

## License

MIT
