// browser_eval: (script) => the result as a string (JSON for non-strings).
(script) => {
  let r;
  try { r = (0, eval)(script); } catch (e) { return 'Error: ' + e; }
  try { return typeof r === 'string' ? r : JSON.stringify(r); } catch (e) { return String(r); }
}
