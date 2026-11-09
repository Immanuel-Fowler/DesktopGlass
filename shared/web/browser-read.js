// browser_read: the page as text, links and form fields (a JSON string).
(() => {
  const text = document.body ? document.body.innerText : '';
  const links = Array.from(document.querySelectorAll('a[href]')).slice(0, 80)
    .map(a => ({text: (a.innerText || a.title || '').trim().slice(0, 80), href: a.href}));
  const fields = Array.from(document.querySelectorAll('input, textarea, select, button')).slice(0, 60)
    .map(e => ({tag: e.tagName.toLowerCase(), type: e.type || '', id: e.id || '', name: e.name || '',
                label: (e.labels && e.labels[0] ? e.labels[0].innerText : (e.placeholder || e.innerText || e.value || '')).trim().slice(0, 60)}));
  return JSON.stringify({title: document.title, url: location.href, text, links, fields});
})()
