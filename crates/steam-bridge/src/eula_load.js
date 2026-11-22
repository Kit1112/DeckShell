(async () => {
    const url = new URL({URL});
    if (!['https:', 'http:'].includes(url.protocol)) throw new Error('Invalid EULA URL');
    url.searchParams.set('eulaLang', 'russian');
    url.searchParams.set('json', '1');
    const response = await fetch(url.href);
    if (!response.ok) throw new Error('EULA HTTP ' + response.status);
    const data = await response.json();
    const content = typeof data === 'string' ? data : data?.content;
    if (typeof content !== 'string' || !content.trim()) throw new Error('Empty EULA text');
    // Steam serves BBCode. Remove presentation tags while retaining their text,
    // URLs and unknown tags so that no legal text is silently discarded.
    return content.replace(/\[url=([^\]]+)\]([\s\S]*?)\[\/url\]/gi, '$2 ($1)')
        .replace(/\[\/?(?:b|i|u|h[1-6]|list|olist|code|quote)\]/gi, '')
        .replace(/\[\*\]/g, '• ');
})()
