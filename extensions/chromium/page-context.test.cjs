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

console.log('ChatGPT page context tests passed');
