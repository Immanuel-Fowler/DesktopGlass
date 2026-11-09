// DesktopGlass visual HTML editor: selection, highlight, live preview and in-place text editing, injected into
// the page being edited (dg-preview://). Talks to the app through window.webkit.messageHandlers.dg.
(() => {
  if (window.__dg) return; window.__dg = true;
  let mode = 'select', selected = [], boxes = [], editing = null, editOriginal = '', editClean = '', editOutline = ['', ''];
  const PROPS = window.__dgProps || [];   // set by the app (the inspector's property list)
  const INLINE = new Set(['a','abbr','b','bdi','bdo','br','cite','code','data','del','dfn','em','i','ins','kbd','mark','q','s','samp','small','span','strong','sub','sup','time','u','var','wbr']);
  const KEEP_ATTRS = new Set(['href', 'title', 'target', 'rel', 'class', 'id', 'style', 'datetime', 'lang', 'dir', 'cite', 'value']);
  const post = m => window.webkit.messageHandlers.dg.postMessage(m);
  const mk = (color, fill) => { const b = document.createElement('div');
    Object.assign(b.style, {position: 'fixed', pointerEvents: 'none', zIndex: 2147483647, display: 'none',
      border: '1.5px solid ' + color, background: fill, borderRadius: '3px', boxSizing: 'border-box'});
    document.documentElement.appendChild(b); return b; };
  const hover = mk('rgba(10,132,255,.9)', 'rgba(10,132,255,.07)');
  const label = document.createElement('div');
  Object.assign(label.style, {position: 'fixed', pointerEvents: 'none', zIndex: 2147483647, display: 'none',
    font: '600 10px -apple-system, system-ui', color: '#fff', background: 'rgba(10,132,255,.95)', padding: '2px 5px', borderRadius: '4px'});
  document.documentElement.appendChild(label);
  const ours = el => el === hover || el === label || boxes.includes(el);
  const elOf = t => t && t.nodeType === 1 ? t : (t ? t.parentElement : null);
  const tagged = el => { while (el && el.nodeType === 1 && !el.hasAttribute('data-dg-id')) el = el.parentElement; return el && el.nodeType === 1 ? el : null; };
  /// The element a click means: the clicked element itself when JavaScript created it, else its source element.
  const pick = t => { const el = elOf(t); if (!el || ours(el)) return null; const src = tagged(el); if (!src) return null;
    const tn = el.tagName.toLowerCase();
    return (el !== src && tn !== 'html' && tn !== 'body' && tn !== 'head') ? el : src; };
  const place = (b, el) => { if (!el || !el.isConnected) { b.style.display = 'none'; return; } const r = el.getBoundingClientRect();
    Object.assign(b.style, {display: 'block', left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px'}); };
  const name = el => el.tagName.toLowerCase() + (el.id ? '#' + el.id : '') + (el.classList.length ? '.' + Array.from(el.classList).slice(0, 2).join('.') : '')
    + (el.hasAttribute('data-dg-id') ? '' : '  (from JS)');
  const info = el => { const cs = getComputedStyle(el), c = {}; PROPS.forEach(p => c[p] = cs.getPropertyValue(p)); return c; };
  const send = (el, shift, refresh) => { const src = tagged(el); if (!src) return; const dyn = el !== src;
    if (dyn) window.__dgDyn = el;
    post({type: 'select', id: src.getAttribute('data-dg-id'), dynamic: dyn, tag: el.tagName.toLowerCase(), elId: el.id || '',
          classes: Array.from(el.classList), shift: !!shift, refresh: !!refresh, computed: info(el)}); };
  const redraw = () => { boxes.forEach((b, i) => place(b, selected[i])); };
  const refreshAll = () => { redraw(); selected.forEach(el => send(el, false, true)); };
  const byId = id => id === 'dyn' ? window.__dgDyn : document.querySelector('[data-dg-id="' + id + '"]');
  window.__dgHighlight = ids => { boxes.forEach(b => b.remove()); boxes = [];
    selected = ids.map(byId).filter(el => el && el.isConnected);
    selected.forEach((el, i) => { const b = mk(i === 0 ? 'rgba(255,159,10,1)' : 'rgba(255,159,10,.65)', 'rgba(255,159,10,.06)'); boxes.push(b); place(b, el); }); };
  window.__dgSelect = (id, add) => { const el = byId(id); if (!el) return;
    el.scrollIntoView({block: 'nearest', inline: 'nearest'}); send(el, !!add, false); };
  window.__dgSetMode = m => { if (editing) finishEdit(true); mode = m; if (m !== 'select') { hover.style.display = 'none'; label.style.display = 'none'; } };
  window.__dgPreview = (id, prop, value) => { const el = document.querySelector('[data-dg-id="' + id + '"]');
    if (el) { value ? el.style.setProperty(prop, value) : el.style.removeProperty(prop); } refreshAll(); };
  window.__dgSetRule = (sel, prop, value, sheetName) => {
    const sheets = Array.from(document.styleSheets);
    let sheet = sheetName ? sheets.find(s => s.href && decodeURIComponent(s.href).endsWith('/' + sheetName))
                          : sheets.find(s => s.ownerNode && s.ownerNode.tagName === 'STYLE' && !s.ownerNode.hasAttribute('data-dg-live'));
    if (!sheet) { let st = document.querySelector('style[data-dg-live]');
      if (!st) { st = document.createElement('style'); st.setAttribute('data-dg-live', ''); document.head.appendChild(st); } sheet = st.sheet; }
    let rules; try { rules = Array.from(sheet.cssRules); } catch (e) { return; }
    let rule = rules.reverse().find(r => r.selectorText === sel);
    if (!rule) { sheet.insertRule(sel + ' {}', sheet.cssRules.length); rule = sheet.cssRules[sheet.cssRules.length - 1]; }
    value ? rule.style.setProperty(prop, value) : rule.style.removeProperty(prop); refreshAll(); };

  // ---- In-place text editing (double-click) ----
  const clean = root => {
    Array.from(root.querySelectorAll('*')).reverse().forEach(e => {
      const tn = e.tagName.toLowerCase();
      if (tn === 'script' || tn === 'style') { e.remove(); return; }
      if (!INLINE.has(tn)) { e.replaceWith(...e.childNodes); return; }
      Array.from(e.attributes).forEach(a => { if (!KEEP_ATTRS.has(a.name.toLowerCase()) || /^\s*javascript:/i.test(a.value)) e.removeAttribute(a.name); });
    });
    return root.innerHTML.replace(/(\S)&nbsp;/g, '$1 ').replace(/&nbsp;(\S)/g, ' $1');
  };
  const cleanCopy = el => { const c = el.cloneNode(true); c.removeAttribute('contenteditable'); return clean(c); };
  const inlineOnly = el => Array.from(el.querySelectorAll('*')).every(c => INLINE.has(c.tagName.toLowerCase()));
  const finishEdit = save => {
    if (!editing) return; const el = editing; editing = null;
    el.removeAttribute('contenteditable'); el.style.outline = editOutline[0]; el.style.outlineOffset = editOutline[1];
    if (el.getAttribute('style') === '') el.removeAttribute('style');
    if (!save) { el.innerHTML = editOriginal; return; }
    const html = cleanCopy(el);
    if (html !== editClean) post({type: 'editText', id: el.getAttribute('data-dg-id'), html});
  };
  const startEdit = (el, x, y) => {
    // edit the whole paragraph, not just the bold word that was double-clicked
    while (INLINE.has(el.tagName.toLowerCase()) && el.parentElement && el.parentElement.hasAttribute('data-dg-id') && inlineOnly(el.parentElement)) el = el.parentElement;
    if (!el.hasAttribute('data-dg-id') || !el.textContent.trim() || !inlineOnly(el)) { post({type: 'editRefused'}); return; }
    editing = el; editOriginal = el.innerHTML; editClean = cleanCopy(el);
    editOutline = [el.style.outline, el.style.outlineOffset];
    hover.style.display = 'none'; label.style.display = 'none';
    el.contentEditable = 'true'; el.spellcheck = true;
    el.style.outline = '2px solid rgba(48,209,88,.95)'; el.style.outlineOffset = '2px';
    el.focus();
    const r = document.caretRangeFromPoint ? document.caretRangeFromPoint(x, y) : null;
    if (r && el.contains(r.startContainer)) { const s = getSelection(); s.removeAllRanges(); s.addRange(r); }
    el.addEventListener('blur', () => finishEdit(true), {once: true});
  };
  const inEdit = t => editing && editing.contains(elOf(t));
  document.addEventListener('keydown', e => {
    if (!editing) return;
    if (e.key === 'Escape') { e.preventDefault(); finishEdit(false); }
    else if (e.key === 'Enter' && e.shiftKey) { e.preventDefault(); document.execCommand('insertLineBreak'); }
    else if (e.key === 'Enter') { e.preventDefault(); const el = editing; finishEdit(true); el.blur(); }
    else if (e.metaKey && ['b', 'i', 'u'].includes(e.key.toLowerCase())) {
      e.preventDefault(); document.execCommand({b: 'bold', i: 'italic', u: 'underline'}[e.key.toLowerCase()]); }
  }, true);
  document.addEventListener('dblclick', e => { if (mode !== 'select' || inEdit(e.target)) return;
    e.preventDefault(); e.stopPropagation(); const el = elOf(e.target); if (el && !ours(el)) startEdit(el, e.clientX, e.clientY); }, true);

  // ---- Selecting ----
  document.addEventListener('mousemove', e => { if (mode !== 'select' || editing) return; const el = pick(e.target); place(hover, el);
    if (el) { const r = el.getBoundingClientRect(); label.textContent = name(el);
      Object.assign(label.style, {display: 'block', left: r.left + 'px', top: Math.max(0, r.top - 18) + 'px'}); } else label.style.display = 'none'; }, true);
  for (const t of ['mousedown', 'mouseup', 'submit', 'auxclick']) document.addEventListener(t, e => {
    if (mode !== 'select' || inEdit(e.target)) return;
    if (editing && t === 'mousedown') finishEdit(true);
    e.preventDefault(); e.stopPropagation(); }, true);
  document.addEventListener('click', e => { if (mode !== 'select' || inEdit(e.target)) return; const el = pick(e.target); if (!el) return;
    e.preventDefault(); e.stopPropagation(); send(el, e.shiftKey, false); }, true);
  addEventListener('scroll', redraw, true); addEventListener('resize', redraw);
  new MutationObserver(() => requestAnimationFrame(redraw)).observe(document.documentElement, {subtree: true, attributes: true, childList: true});
  post({type: 'loaded'});
})();
