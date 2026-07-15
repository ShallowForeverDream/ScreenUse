# 本地归类与 AI 复核设计

ScreenUse 的目标不是“尽量猜”，而是在个人数据逐渐积累后，以较高精度自动覆盖常见事务；证据冲突时主动进入复核，避免错误规则批量污染时间账本。

## 问题与取舍

旧版把同一任务历次修正的页面标题合并成一个 OR 规则。它可以很快扩大覆盖面，但 `ChatGPT`、`新标签页`、`图片查看器`、`任务切换` 等弱线索也可能变成强规则，使一次修正影响大量无关记录。规则还没有表达“同一页面曾被纠正到不同任务”的冲突能力。

新版将“明确规则”和“个人例子”分开：

1. **固定事务**：用户主动固定的上下文，优先级最高。
2. **强规则**：只在用户勾选并提供明确识别词时建立；旧版自动生成的宽泛规则在一次性迁移中停用。
3. **个人记忆**：每次人工确认自动保存为一条独立例子，不合并成 OR 列表，也不回写其他历史时间段。
4. **页面/工作区匹配**：使用当前对话、页面、文件、目录、域名和项目/任务名称。
5. **时间连续性**：只在很短间隔内继承已经可靠的具体任务。
6. **应用启发式**：应用名只决定粗分类，不能单独决定具体任务。
7. **拒绝判断**：强证据不足或个人记忆冲突时保留待复核状态。

这种“高精度优先、允许拒绝”的方式对应 selective classification / reject option：覆盖率不是唯一目标，低误归类率更重要。参考：[Selective Classification via One-Sided Prediction](https://proceedings.mlr.press/v130/gangrade21a.html)、[Multi-class Classification with Reject Option and Performance Guarantees using Conformal Prediction](https://proceedings.mlr.press/v230/garcia-galindo24a.html)。

## 个人记忆

个人记忆只保存窗口元数据特征和最终层级，不保存截图或正文：

- 应用；
- 当前页面或对话标题；
- 窗口标题；
- URL 域名；
- 工作区和文件路径末三段；
- 分类、项目、任务；
- 确认时间和命中次数。

查询使用无需模型的加权相似度：当前页面精确命中权重最高，工作区、文件和域名其次，应用名仅提供很小加分；中英文关键词及中文二元字符用于近似匹配。候选按最终层级投票，重复确认提高支持度；两个不同任务具有接近的强匹配时直接放弃自动判断。

记忆上限为 2,000 条，采用 SQLite 单表持久化，预计长期占用只有数 MB。删除任务、项目或撤销修正时会同步清理；GitHub 数据同步完成后会从已确认会话重新构建，因此无需复制额外模型文件。

这一方案借鉴了低成本近邻分类和检索示例的思路，但不引入常驻向量模型：[Like a Good Nearest Neighbor](https://aclanthology.org/2024.eacl-long.17/)、[Retrieval-style In-context Learning for Few-shot Hierarchical Text Classification](https://aclanthology.org/2024.tacl-1.67/)。

## AI 复核提示词与成本

AI 输入分成四部分：

- `reviewItems`：目标会话及均匀抽样的首、中、尾元数据；
- `timelineContext`：全批次去重后的邻近时间线；
- `personalMemory`：每个目标最多 3 个相似且由用户确认的例子；
- `catalog`：完整分类、项目、任务层级。

提示词明确规定：旧归类只是建议，个人记忆和当前页面优先，应用名是弱证据，必须选择最具体的已有任务，并使用分档置信度。上下文会话只提供线索，不能被当作待修改目标。

为降低开销：

- 每个目标最多 12 个均匀采样事件；
- 全批次最多 36 个去重上下文；
- 会话证据最多 4 项，个人记忆最多 3 条；
- 最多 8 个目标共享一次 catalog 和系统提示词；
- Codex 输出 schema 将 `sessionId` 限定为本批真实 ID；解析器仍可按原顺序修复重复或轻微错误 ID；
- 结构化输出只保留短摘要和最多 3 条证据。

静态系统提示词放在最前，兼容提供方支持时可利用提示词缓存。OpenAI 对缓存和限制输出长度的说明分别见 [Prompt Caching](https://openai.com/index/api-prompt-caching/) 与 [Controlling the length of model responses](https://help.openai.com/en/articles/5072518-controlling-the-length-of-openai-model-responses)。

成本以每个复核批次真实 Token 记录为准，界面继续显示 Token、Credits 和美元等值。目标不是每次都把预算用满，而是先由个人记忆消化高频场景；只有拒绝判断的剩余会话进入 AI。默认批量复核下，单条平均等值开销应保持在 0.05 美元以内。

## 回归要求

- 单条修正不得改变其他既有会话；
- 撤销必须同时恢复会话、强规则和个人记忆；
- 通用应用标题不能成为具体任务记忆；
- 同一具体页面可跨 Chrome、WPS、QQ、Explorer 等应用命中；
- 冲突的精确记忆必须拒绝判断；
- 本地完整任务且置信度不低于 80% 时，默认不进入 AI 队列；
- AI 输入不得重复复制每个目标的完整邻近时间线。

ActivityWatch 的规则分类和相同 heartbeat 合并仍是基础参考，但 ScreenUse 在其上增加了页面、任务层级、个人纠错记忆和拒绝判断：[ActivityWatch categorization](https://docs.activitywatch.net/en/latest/features/categorization.html)、[ActivityWatch data model](https://docs.activitywatch.net/en/latest/buckets-and-events.html)。
