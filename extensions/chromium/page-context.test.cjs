const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const sandbox = {};
vm.createContext(sandbox);
vm.runInContext(
  fs.readFileSync(path.join(__dirname, 'page-context.js'), 'utf8'),
  sandbox,
  { filename: 'page-context.js' },
);

const { conversationTitleFromSnapshot } = sandbox.ScreenUsePageContext;

assert.deepEqual(
  JSON.parse(JSON.stringify(conversationTitleFromSnapshot({
    hostname: 'chatgpt.com',
    pathname: '/c/current-id',
    documentTitle: 'ChatGPT',
    links: [
      { pathname: '/c/another-id', title: '其他对话', selected: false },
      { pathname: '/c/current-id', title: 'ICPC刷题网站功能需求', selected: false },
    ],
  }))),
  { title: 'ICPC刷题网站功能需求', type: 'chatgpt-conversation' },
);

assert.deepEqual(
  JSON.parse(JSON.stringify(conversationTitleFromSnapshot({
    hostname: 'chatgpt.com',
    pathname: '/c/current-id',
    documentTitle: '自动记录时间优化 - ChatGPT',
    links: [],
  }))),
  { title: '自动记录时间优化', type: 'chatgpt-conversation' },
);

assert.deepEqual(
  JSON.parse(JSON.stringify(conversationTitleFromSnapshot({
    hostname: 'chatgpt.com',
    pathname: '/',
    documentTitle: 'ChatGPT',
    links: [],
  }))),
  { title: '新对话', type: 'chatgpt-new-chat' },
);

assert.equal(conversationTitleFromSnapshot({
  hostname: 'example.com',
  pathname: '/c/current-id',
  documentTitle: '普通网页',
  links: [],
}), null);

assert.deepEqual(
  JSON.parse(JSON.stringify(conversationTitleFromSnapshot({
    hostname: 'claude.ai',
    pathname: '/chat/claude-id',
    documentTitle: 'Claude',
    links: [
      { pathname: '/chat/claude-id', title: '论文实验设计', selected: true },
    ],
  }))),
  { title: '论文实验设计', type: 'claude-conversation' },
);

assert.deepEqual(
  JSON.parse(JSON.stringify(conversationTitleFromSnapshot({
    hostname: 'gemini.google.com',
    pathname: '/app/gemini-id',
    documentTitle: 'Gemini',
    links: [
      { pathname: '/app/gemini-id', title: 'IOT week1 复盘', selected: false },
    ],
  }))),
  { title: 'IOT week1 复盘', type: 'gemini-conversation' },
);

assert.deepEqual(
  JSON.parse(JSON.stringify(conversationTitleFromSnapshot({
    hostname: 'chat.deepseek.com',
    pathname: '/a/chat/s/deepseek-id',
    documentTitle: '保研材料修改 - DeepSeek',
    links: [],
  }))),
  { title: '保研材料修改', type: 'deepseek-conversation' },
);

assert.deepEqual(
  JSON.parse(JSON.stringify(conversationTitleFromSnapshot({
    hostname: 'chat.qwen.ai',
    pathname: '/c/qwen-id',
    documentTitle: '数据库复习 - Qwen',
    links: [],
  }))),
  { title: '数据库复习', type: 'qwen-conversation' },
);

assert.deepEqual(
  JSON.parse(JSON.stringify(conversationTitleFromSnapshot({
    hostname: 'copilot.microsoft.com',
    pathname: '/chats/copilot-id',
    documentTitle: 'ScreenUse Windows 打包 - Microsoft Copilot',
    links: [],
  }))),
  { title: 'ScreenUse Windows 打包', type: 'copilot-conversation' },
);

for (const [hostname, pathname, documentTitle, type] of [
  ['www.perplexity.ai', '/search/research-id', '论文检索 - Perplexity', 'perplexity-conversation'],
  ['kimi.moonshot.cn', '/chat/kimi-id', '推免材料整理 - Kimi', 'kimi-conversation'],
  ['www.doubao.com', '/chat/doubao-id', 'UI 视觉复盘 - 豆包', 'doubao-conversation'],
  ['poe.com', '/chat/poe-id', 'API 设计 - Poe', 'poe-conversation'],
  ['grok.com', '/c/grok-id', '热点资料核对 - Grok', 'grok-conversation'],
  ['chat.mistral.ai', '/chat/mistral-id', 'Rust 性能审计 - Le Chat', 'mistral-conversation'],
  ['yuanbao.tencent.com', '/chat/yuanbao-id', '会议纪要 - 腾讯元宝', 'yuanbao-conversation'],
  ['huggingface.co', '/chat/conversation/hf-id', '模型实验 - HuggingChat', 'huggingchat-conversation'],
]) {
  assert.deepEqual(
    JSON.parse(JSON.stringify(conversationTitleFromSnapshot({
      hostname,
      pathname,
      documentTitle,
      links: [],
    }))),
    { title: documentTitle.split(' - ')[0], type },
  );
}

assert.equal(conversationTitleFromSnapshot({
  hostname: 'gemini.google.com',
  pathname: '/settings',
  documentTitle: 'Settings - Gemini',
  links: [],
}), null);

console.log('AI conversation page context tests passed');
