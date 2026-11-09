// Markdown Preview visuals (Mermaid, SVG): reports the rendered size of #c to the app so the view can fit it.
const report = () => { const el = document.getElementById('c'); const r = el.getBoundingClientRect();
  window.webkit.messageHandlers.size.postMessage({w: r.width, h: Math.ceil(r.height)}); };
addEventListener('resize', report);
