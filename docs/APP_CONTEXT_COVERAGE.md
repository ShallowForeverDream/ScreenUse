# 应用上下文识别覆盖

ScreenUse 不把“打开了什么软件”直接等同于“正在做什么任务”。采集器按以下优先级选择最具体、同时仍然轻量的上下文：

1. 浏览器或编辑器本机扩展提供的活动标签页、会话、工作区和文件；
2. 当前前台窗口中可访问的选中会话、文档标签或资源管理器地址；
3. 清理产品名和版本号后的窗口标题；
4. 仅在没有更具体信号时使用应用名。

## 当前覆盖

| 场景 | 软件/服务 | 主要信号 |
| --- | --- | --- |
| AI 对话 | ChatGPT/Codex 桌面端，ChatGPT、Claude、Gemini、Perplexity、DeepSeek、Kimi、豆包、Poe、Copilot、Grok、通义千问、Mistral、腾讯元宝、HuggingChat 网页版 | 当前选中的对话标题；ChatGPT 项目名；活动标签页 URL（去查询参数） |
| 即时通信 | QQ、微信、钉钉、飞书/Lark、企业微信、Teams、Slack、Discord、Telegram、Signal、WhatsApp、LINE、Skype | 当前联系人、群聊或频道；QQ 图片/视频查看器继承原会话 |
| 浏览器 | Chrome、Edge、Firefox、Brave、Vivaldi、Opera、Arc、Tabbit、Chromium 及常见衍生浏览器 | 当前标签页标题、紧凑 URL、视频播放状态；扩展上下文必须同时匹配实际浏览器和标签页，避免串台 |
| 邮件、文档与知识库 | Outlook、新 Outlook、Thunderbird、Foxmail、WPS、Office、Acrobat、SumatraPDF、Foxit、LibreOffice、Obsidian、Typora、Notepad、Notion、Logseq、Joplin、Zettlr、calibre、Xournal++ | 当前邮件主题、文档/笔记标题或选中文档标签；移除产品名和版本号 |
| 开发 | VS Code 系、Visual Studio、JetBrains IDE、IDA、Ghidra、Sublime、Zed、Eclipse、NetBeans、Qt Creator、Arduino、DBeaver、DataGrip、Navicat、Postman、Wireshark、Burp、Docker Desktop、Unity、Unreal、Godot 等 | 活动文件、工作区、Git 分支、语言和调试状态；无扩展时使用清理后的文件/项目窗口标题 |
| 终端 | Windows Terminal、PowerShell、CMD、WezTerm、Alacritty、kitty、mintty、ConEmu、MobaXterm、Xshell、PuTTY、SecureCRT、Git Bash、WSL | 活动终端/标签标题；移除终端产品后缀 |
| 文件管理 | Windows 文件资源管理器、Total Commander、Files、Directory Opus、FreeCommander、Double Commander、Everything | 当前可见地址栏/选中标签和目录，而不是多标签窗口的第一个标题 |
| 会议 | 腾讯会议/VooV、Zoom、Webex，以及 Teams、飞书、钉钉等协作应用内会议 | 当前会议标题；前台参会期间即使无键鼠输入仍计时 |
| 视频/媒体 | VLC、mpv、PotPlayer、MPC、Windows Media Player、哔哩哔哩、爱奇艺、优酷 | 当前媒体标题；只有检测到真实播放控件或网页 `videoPlaying` 时才豁免空闲，暂停后恢复 3 分钟空闲规则 |
| 设计创作 | Photoshop、Illustrator、Figma、Blender、Premiere、After Effects、AutoCAD、Krita、Inkscape、DaVinci Resolve、Affinity、SketchUp | 当前作品/文件窗口标题，清理产品后缀 |

## 防误归属

- 浏览器扩展保存的上下文只有在浏览器品牌和原生窗口标签标题一致时才会附加；Chrome 的旧标签不会串到 Edge，另一个标签页也不会沿用旧标题。
- VS Code 扩展上下文只会附加到对应的 VS Code/Cursor/Windsurf/VSCodium 进程，不会串到 JetBrains 或其他编辑器。
- 进程可执行文件路径不再伪装成“当前文件”；升级时会一次性清除旧污染并重建个人记忆，之后只有编辑器等真实集成提供的活动文件才进入文件特征。
- 截图工具以及 QQ/微信的图片、视频预览属于短暂辅助窗口，继承原任务且不会覆盖原会话标题。
- 当前会话、文档、编辑、目录、会议、终端和媒体分别保存语义类型，修正与 AI 复核都优先使用这些具体信号。

## 性能边界

- 仍然每秒只检查当前前台窗口，不枚举后台应用内容。
- 只读取标题、选中项、地址、文件名等元数据，不读取聊天正文、文档正文或屏幕图像。
- 浏览器与编辑器上下文各保留一个有过期时间的内存快照，不形成无界缓存。
- 相同上下文继续复用一个数据库事件和会话，新增识别信号不会变成每秒追加记录。

无法公开标题或无障碍信息的软件仍会退回到应用名，并进入待复核；一次人工修正会形成该页面、会话、工作区或文件的隔离个人记忆。
