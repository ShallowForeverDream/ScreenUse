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
           ├── ChatGPT / Codex：读取当前选中会话标题和项目名
           ├── WPS / Office：读取当前文档或选中标签页标题
           └── 会议/视频：前台观看时保持为有效活动
           │
           ▼
稳定上下文 ID + 每 1 秒覆盖更新
           │
           ▼
用户学习规则 → 本地分类 → 项目/任务匹配
           │
           ├── 同一活动：持续延长当前时间块
           ├── 稳定切换：结束上一块并等待确认
           └── 低置信长会话：可选 AI 辅助复核
           ▼
SQLite 会话、日期概览、时间轴和导出
```

### 低占用的关键点

- 默认每 1 秒只读取一次前台窗口和系统空闲时间，不采集图像。
- 同一个上下文使用稳定事件 ID；每次更新只覆盖同一条原始事件并延长同一时间块，不会每秒新增一行。
- 应用、标签页、文件或空闲状态连续两次观察都发生变化时，才结束上一段并建立新上下文。
- 只出现一次的加载页、等待响应或短暂弹窗会并回原时间块，避免几秒钟的噪声切碎时间线。
- 同一应用、项目和任务中首尾相接的内部窗口会自动合并，例如 QQ 主窗口与图片查看器；真正切换到其他应用时仍保留边界。
- 新上下文确认后会回填首次观察到切换的时间，检测延迟不会额外吞掉已经观察到的时长。
- 系统待机或长时间调度中断后会切断会话，不把睡眠时间误算为工作时间。
- 空闲状态同样每 1 秒刷新，离开与恢复可以更快反映到时间轴。
- 会议、课程、直播和播放器在前台时不会仅因没有键鼠输入被标为“离开”，可在设置中关闭。
- ChatGPT / Codex 桌面端通过 Windows UI Automation 只读取当前会话标题和项目名，不读取消息正文。
- WPS、Word、Excel、PowerPoint、OneNote、Outlook、Acrobat 等办公软件读取当前文档窗口标题并去掉应用名后缀；窗口标题不可用时再读取选中的文档标签，不读取文档正文。
- Chromium 扩展不再枚举所有窗口和标签，只发送当前活动标签页。
- VS Code 扩展把短时间内的保存、终端和编辑器事件去抖合并，心跳为 90 秒。
- 原始事件默认保留 30 天；已归因的工作会话长期保留。
- SQLite 每 6 小时自动执行清理、checkpoint 和 `PRAGMA optimize`。

### 分类顺序

1. 会议、视频等被动专注场景保持有效；其余活动命中空闲阈值时归为“离开”。
2. 临时固定的当前事务优先，适合 ChatGPT、终端等标题不明确的应用。
3. 用户从已修正会话学习的“应用 + 上下文关键词”规则优先。
4. 使用窗口标题、工作区、URL、项目名和任务名判断真实事务；应用名只作为线索，不直接代表项目。
5. 无可靠上下文时保持未归类，不再把活动塞给同分类下最近使用的项目。
6. IDE 工作区无法匹配时，可自动创建对应项目。
7. 只有启用 AI 后，低置信且达到最短时长的会话才进入复核。

修正时可以直接新建或删除分类、项目和任务，也可以在时间轴多选会话后统一修正分类、项目或任务，并选择“记住规则”或“固定 30 分钟”。分类、项目和任务都支持键入搜索、方向键选择和 Enter 确认；没有匹配项时 Enter 直接创建。任务可跨项目搜索，项目可跨分类搜索，同名项会带父级信息分别显示。相似活动以后会优先命中已确认的上下文，而不是看到 `ChatGPT.exe` 就固定归到某个开发项目。

## 已实现

- Tauri 2 + Rust + React 19 桌面端。
- Windows 前台窗口、进程、ChatGPT/Codex 当前会话、WPS/Office 当前文档标题和系统空闲时间采集。
- 本地 SQLite 项目、任务、会话、规则、计划、导出与备份。
- 日期概览、可点击分类明细、全天时间段、项目投入、上下文次数和待复核收件箱。时间轴默认 10 分钟/格，按住 `Ctrl` 滚轮可保持当前中心缩放，最细支持 1 秒/格。
- 时间轴搜索、人工确认、多选统一修正、改名、重归类、合并、拆分和规则学习。
- Chromium Manifest V3 活动标签页扩展。
- VS Code 工作区、活动文件、语言、Git 分支、终端和调试状态扩展。
- ICS 日历计划导入。
- CSV、Excel 可打开的 `.xls`、Markdown 导出。
- 系统托盘启动、暂停、打开和退出。
- 可选 Windows 登录后静默启动；后台启动时只显示托盘，不弹主窗口。
- Windows Release 使用 GUI 子系统，直接运行或登录启动时不会附带命令行窗口。
- 自动原始事件轮转、旧媒体迁移清理、数据库压缩和备份。
- 可选 OpenAI-compatible API；请求最多 80 条精简元数据、30 秒超时、URL 去查询参数和锚点。

## 运行与构建

> Release 必须使用 `pnpm tauri:build`（内部调用 Tauri CLI），不要直接执行 `cargo build --release`。后者不会启用 Tauri 的 `custom-protocol`，生成的窗口会错误访问开发地址 `127.0.0.1:1420`。仓库现在会直接阻止这种无效构建。

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
| 观察与更新 | 1 秒 | 同一活动只延长当前块；稳定切换才新建一块 |
| 切换防抖 | 连续 2 次观察 | 过滤加载页、等待响应和短暂弹窗 |
| 离开判定 | 180 秒 | 短暂思考不会立即算离开 |
| 会议与视频不计离开 | 开启 | 前台观看或参会时继续计时 |
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
