(function exposePageContext(root) {
  const CHAT_PROFILES = [
    {
      id: 'chatgpt',
      hosts: ['chatgpt.com', 'www.chatgpt.com', 'chat.openai.com'],
      conversationPaths: ['/c/', '/g/'],
      newPaths: ['/'],
    },
    {
      id: 'claude',
      hosts: ['claude.ai'],
      conversationPaths: ['/chat/', '/project/'],
      newPaths: ['/new'],
    },
    {
      id: 'gemini',
      hosts: ['gemini.google.com'],
      conversationPaths: ['/app/'],
      newPaths: ['/app', '/'],
    },
    {
      id: 'perplexity',
      hosts: ['perplexity.ai', 'www.perplexity.ai'],
      conversationPaths: ['/search/', '/spaces/'],
      newPaths: ['/'],
    },
    {
      id: 'deepseek',
      hosts: ['chat.deepseek.com'],
      conversationPaths: ['/a/chat/s/', '/chat/s/'],
      newPaths: ['/'],
    },
    {
      id: 'kimi',
      hosts: ['kimi.moonshot.cn', 'www.kimi.com'],
      conversationPaths: ['/chat/'],
      newPaths: ['/', '/chat'],
    },
    {
      id: 'doubao',
      hosts: ['www.doubao.com', 'doubao.com'],
      conversationPaths: ['/chat/'],
      newPaths: ['/', '/chat'],
    },
    {
      id: 'poe',
      hosts: ['poe.com'],
      conversationPaths: ['/chat/'],
      newPaths: ['/'],
    },
    {
      id: 'copilot',
      hosts: ['copilot.microsoft.com'],
      conversationPaths: ['/chats/'],
      newPaths: ['/', '/chats'],
    },
    {
      id: 'grok',
      hosts: ['grok.com', 'x.com'],
      conversationPaths: ['/c/', '/i/grok'],
      newPaths: ['/', '/i/grok'],
    },
    {
      id: 'qwen',
      hosts: ['chat.qwen.ai', 'tongyi.aliyun.com'],
      conversationPaths: ['/c/', '/qianwen/'],
      newPaths: ['/', '/c', '/qianwen'],
    },
    {
      id: 'mistral',
      hosts: ['chat.mistral.ai'],
      conversationPaths: ['/chat/'],
      newPaths: ['/', '/chat'],
    },
    {
      id: 'yuanbao',
      hosts: ['yuanbao.tencent.com'],
      conversationPaths: ['/chat/'],
      newPaths: ['/', '/chat'],
    },
    {
      id: 'huggingchat',
      hosts: ['huggingface.co'],
      conversationPaths: ['/chat/conversation/'],
      newPaths: ['/chat', '/chat/'],
    },
  ];

  const PRODUCT_TITLES = [
    'chatgpt pro', 'chatgpt', 'claude', 'gemini', 'perplexity', 'deepseek',
    'kimi', '豆包', 'poe', 'microsoft copilot', 'copilot', 'grok', 'qwen',
    '通义千问', 'le chat', 'mistral', '腾讯元宝', 'yuanbao', 'huggingchat',
  ];
  const GENERIC_CONVERSATION_TITLES = new Set([
    '', ...PRODUCT_TITLES, 'new chat', 'new conversation', 'new thread',
    '新对话', '新建对话', '开始新对话',
  ]);

  function normalizePath(value) {
    const path = String(value || '/').replace(/\/+$/, '');
    return path || '/';
  }

  function profileForHost(hostname) {
    const host = String(hostname || '').toLowerCase();
    return CHAT_PROFILES.find((profile) => profile.hosts.includes(host)) || null;
  }

  function isConversationPath(profile, pathname) {
    const path = normalizePath(pathname);
    return profile.conversationPaths.some((prefix) => path.startsWith(prefix));
  }

  function cleanTitle(value) {
    let title = String(value || '').replace(/\s+/g, ' ').trim();
    title = title.replace(/^(?:conversation|chat|thread|对话|会话)\s*[:：]\s*/i, '').trim();
    title = title.replace(/\s+(?:打开|更多)?(?:对话|会话|聊天)选项$/i, '').trim();
    for (const product of PRODUCT_TITLES) {
      const escaped = product.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
      title = title.replace(new RegExp(`\\s+(?:[-—|·])\\s+${escaped}(?:\\s+Pro)?$`, 'i'), '').trim();
    }
    return title.slice(0, 320);
  }

  function usefulConversationTitle(value) {
    const title = cleanTitle(value);
    return GENERIC_CONVERSATION_TITLES.has(title.toLowerCase()) ? null : title;
  }

  function conversationTitleFromSnapshot(snapshot) {
    const profile = profileForHost(snapshot.hostname);
    if (!profile) return null;

    const pathname = normalizePath(snapshot.pathname);
    const isConversation = isConversationPath(profile, pathname);
    const isNew = profile.newPaths.some((path) => normalizePath(path) === pathname);
    if (!isConversation && !isNew) return null;

    const links = Array.isArray(snapshot.links) ? snapshot.links : [];
    const relevantLinks = links.filter((link) => isConversationPath(profile, link.pathname));
    const exact = relevantLinks.find((link) => normalizePath(link.pathname) === pathname);
    const selected = exact || relevantLinks.find((link) => link.selected);
    const selectedTitle = usefulConversationTitle(selected?.title);
    if (selectedTitle) {
      return { title: selectedTitle, type: `${profile.id}-conversation` };
    }

    const documentTitle = usefulConversationTitle(snapshot.documentTitle);
    if (documentTitle) {
      return { title: documentTitle, type: `${profile.id}-conversation` };
    }

    if (isNew) {
      return { title: '新对话', type: `${profile.id}-new-chat` };
    }
    return null;
  }

  function anchorTitle(anchor) {
    return cleanTitle(
      anchor.querySelector?.('[dir="auto"]')?.textContent
      || anchor.getAttribute?.('aria-label')
      || anchor.getAttribute?.('title')
      || anchor.textContent,
    );
  }

  function readPageContext(doc = document, pageLocation = location) {
    if (!profileForHost(pageLocation.hostname)) return null;
    const links = [...doc.querySelectorAll(
      'nav a[href], aside a[href], [role="navigation"] a[href], [role="listitem"] a[href]',
    )]
      .map((anchor) => {
        try {
          const url = new URL(anchor.href, pageLocation.href);
          return {
            pathname: url.pathname,
            title: anchorTitle(anchor),
            selected: anchor.getAttribute('aria-current') === 'page'
              || anchor.getAttribute('aria-selected') === 'true'
              || anchor.dataset?.active === 'true',
          };
        } catch {
          return null;
        }
      })
      .filter(Boolean);
    return conversationTitleFromSnapshot({
      hostname: pageLocation.hostname,
      pathname: pageLocation.pathname,
      documentTitle: doc.title,
      links,
    });
  }

  root.ScreenUsePageContext = {
    cleanTitle,
    conversationTitleFromSnapshot,
    isSupportedConversationHost: (hostname) => Boolean(profileForHost(hostname)),
    readPageContext,
  };
}(globalThis));
