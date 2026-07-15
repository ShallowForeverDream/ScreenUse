(function exposePageContext(root) {
  const CHATGPT_HOSTS = new Set(['chatgpt.com', 'www.chatgpt.com', 'chat.openai.com']);
  const GENERIC_CHATGPT_TITLES = new Set([
    '',
    'chatgpt',
    'chatgpt pro',
    'new chat',
    'new conversation',
    '新对话',
  ]);

  function normalizePath(value) {
    const path = String(value || '/').replace(/\/+$/, '');
    return path || '/';
  }

  function cleanTitle(value) {
    let title = String(value || '').replace(/\s+/g, ' ').trim();
    title = title.replace(/\s+(?:[-—|·])\s+ChatGPT(?:\s+Pro)?$/i, '').trim();
    title = title.replace(/\s+(?:打开|更多)?对话选项$/i, '').trim();
    return title.slice(0, 320);
  }

  function usefulChatTitle(value) {
    const title = cleanTitle(value);
    return GENERIC_CHATGPT_TITLES.has(title.toLowerCase()) ? null : title;
  }

  function conversationTitleFromSnapshot(snapshot) {
    const hostname = String(snapshot.hostname || '').toLowerCase();
    if (!CHATGPT_HOSTS.has(hostname)) return null;

    const pathname = normalizePath(snapshot.pathname);
    const links = Array.isArray(snapshot.links) ? snapshot.links : [];
    const exact = links.find((link) => normalizePath(link.pathname) === pathname);
    const selected = exact || links.find((link) => link.selected);
    const selectedTitle = usefulChatTitle(selected?.title);
    if (selectedTitle) {
      return { title: selectedTitle, type: 'chatgpt-conversation' };
    }

    const documentTitle = usefulChatTitle(snapshot.documentTitle);
    if (documentTitle) {
      return { title: documentTitle, type: 'chatgpt-conversation' };
    }

    if (pathname === '/') {
      return { title: '新对话', type: 'chatgpt-new-chat' };
    }
    return null;
  }

  function anchorTitle(anchor) {
    return cleanTitle(
      anchor.querySelector?.('[dir="auto"]')?.textContent
      || anchor.getAttribute?.('aria-label')
      || anchor.textContent,
    );
  }

  function readPageContext(doc = document, pageLocation = location) {
    const links = [...doc.querySelectorAll('nav a[href], aside a[href], a[href*="/c/"]')]
      .map((anchor) => {
        try {
          const url = new URL(anchor.href, pageLocation.href);
          return {
            pathname: url.pathname,
            title: anchorTitle(anchor),
            selected: anchor.getAttribute('aria-current') === 'page'
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
    readPageContext,
  };
}(globalThis));
