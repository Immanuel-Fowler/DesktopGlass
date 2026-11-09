// browser_type: (selector, text, submit) => message.
(sel, text, submit) => {
  const el = document.querySelector(sel);
  if (!el) return 'No element matches ' + sel;
  el.focus();
  const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, 'value');
  if ((el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') && setter && setter.set) setter.set.call(el, text);
  else if (el.isContentEditable) el.innerText = text;
  else el.value = text;
  el.dispatchEvent(new Event('input', {bubbles: true}));
  el.dispatchEvent(new Event('change', {bubbles: true}));
  if (submit) {
    if (el.form) { el.form.requestSubmit ? el.form.requestSubmit() : el.form.submit(); }
    else { ['keydown', 'keypress', 'keyup'].forEach(t => el.dispatchEvent(new KeyboardEvent(t, {key: 'Enter', code: 'Enter', keyCode: 13, bubbles: true}))); }
  }
  return 'Typed into <' + el.tagName.toLowerCase() + '>';
}
