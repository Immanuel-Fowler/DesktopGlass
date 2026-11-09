// Markdown Preview: renders the Mermaid source in #src into #c. The app sets window.__dgMermaidTheme first
// ('dark' or 'default'); markdown-report-size.js provides report().
(async () => {
  const c = document.getElementById('c');
  try {
    if (!window.mermaid) throw new Error('Mermaid isn\'t available (no bundled copy and no internet).');
    mermaid.initialize({startOnLoad: false, theme: window.__dgMermaidTheme || 'default', securityLevel: 'strict'});
    const { svg } = await mermaid.render('d' + Math.random().toString(36).slice(2), document.getElementById('src').textContent);
    c.innerHTML = svg;
  } catch (e) {
    c.innerHTML = '<div class="err"></div>'; c.firstChild.textContent = 'Diagram error: ' + (e.message || e);
  }
  requestAnimationFrame(report);
})();
