// browser_click: (selector) => message. "text=…" matches a link/button by its text.
(s) => {
  let el = null;
  if (s.startsWith('text=')) {
    const want = s.slice(5).trim().toLowerCase();
    el = Array.from(document.querySelectorAll('a, button, [role=button], input[type=submit], summary, label'))
      .find(e => (e.innerText || e.value || '').trim().toLowerCase().includes(want));
  } else { el = document.querySelector(s); }
  if (!el) return 'No element matches ' + s;
  el.scrollIntoView({block: 'center'});
  el.click();
  return 'Clicked <' + el.tagName.toLowerCase() + '> ' + (el.innerText || el.value || '').trim().slice(0, 60);
}
