# AI 复核信用点与开销

ScreenUse 对“当前 Codex / ChatGPT 账号”和 OpenAI-compatible API 分开记账。

## Codex / ChatGPT 账号

AI 作业保存输入、缓存输入、输出、推理和总 Token。ScreenUse 使用 OpenAI Codex 官方费率表，将每次作业换算为 Credits：

```text
Credits = 非缓存输入 / 1,000,000 × 输入费率
        + 缓存输入 / 1,000,000 × 缓存输入费率
        + 输出 / 1,000,000 × 输出费率
```

推理 Token 已包含在输出 Token 中，不重复计费。以 GPT-5.6 Luna 当前内置费率快照为例：输入 25、缓存输入 2.5、输出 150 Credits / 1M Token。

同一作业失败后重试时，Token 与 Credits 会累计，不再用最后一次调用覆盖前一次消耗；提示词和原始回复区域显示最后一次尝试，`重试`字段给出尝试关系。

ScreenUse 继续把 Credits 换算为美元等值：OpenAI 的公开说明中 `2,500 Credits = $100`，即 `1 Credit ≈ $0.04`：

```text
美元等值 = Credits × $0.04
```

这项结果用于直观比较不同复核任务的资源消耗。Plus 和 Pro 会先消耗套餐内用量，因此“Token 等值开销”不等同于信用卡当次扣款，也不摊分固定订阅费。固定订阅费仍单独显示：Plus $20/月、Pro 5x $100/月、Pro 20x $200/月。

“更新费率”会同时读取 OpenAI Help Center 的 Codex rate card 和 Credits 美元等值依据并缓存；网络或页面解析失败时继续使用上一次成功费率，不清空审计数据。

- Codex Token 费率：<https://help.openai.com/en/articles/20001106-codex-rate-card>
- Credits 美元等值依据：<https://help.openai.com/en/articles/20001147-codex-credits-for-students-terms-of-service>

## OpenAI-compatible API

如果兼容接口直接返回 `cost`、`total_cost` 或 `cost_usd`，ScreenUse 显示服务端金额。接口只返回 Token 时保留 Token 审计，不擅自套用 Codex 套餐费率。
