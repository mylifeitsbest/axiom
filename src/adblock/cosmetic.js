// Скрытие рекламы на странице (Windows, все фреймы; см. src/adblock/windows.rs).
// Правила даёт браузер: сразу при создании документа — правила его сайта, потом —
// общие правила только для тех классов и id, что есть на странице. Стили — через
// adoptedStyleSheets: строгая CSP сайта их не запрещает, в отличие от <style>.
(() => {
  'use strict';
  const wv = window.chrome && window.chrome.webview;
  if (!wv || typeof wv.postMessage !== 'function') return;
  if (location.protocol !== 'https:' && location.protocol !== 'http:') return;
  // Свои ссылки до скриптов сайта: подменить их потом он уже не сможет.
  const post = wv.postMessage.bind(wv);
  const listen = wv.addEventListener.bind(wv);
  const doc = document;
  const adopted = Object.getOwnPropertyDescriptor(Document.prototype, 'adoptedStyleSheets');
  if (!adopted || typeof CSSStyleSheet !== 'function') return;
  const sheet = new CSSStyleSheet();
  const insertRule = sheet.insertRule.bind(sheet);
  const rules = sheet.cssRules;
  const Obs = MutationObserver;
  const KEY = 'axab';
  // Так много разных имён на странице не бывает: дальше не спрашиваем.
  const MAX_SENT = 20000;

  let attached = false;
  const attach = () => {
    if (attached) return;
    attached = true;
    try { adopted.set.call(doc, [...adopted.get.call(doc), sheet]); } catch { attached = false; }
  };

  // ---------- скрытие по селекторам ----------
  const hidden = new Set();
  // Один неверный селектор сломал бы всё правило: такое делим пополам, пока
  // не останется он один.
  const put = (part) => {
    try {
      insertRule(`${part.join(',')}{display:none!important}`, rules.length);
    } catch {
      if (part.length > 1) {
        const half = part.length >> 1;
        put(part.slice(0, half));
        put(part.slice(half));
      }
    }
  };
  const hide = (list) => {
    if (!Array.isArray(list)) return;
    const fresh = [];
    for (const s of list) {
      if (typeof s === 'string' && s && !hidden.has(s)) {
        hidden.add(s);
        fresh.push(s);
      }
    }
    for (let i = 0; i < fresh.length; i += 300) put(fresh.slice(i, i + 300));
    if (fresh.length) attach();
  };

  // ---------- классы и id страницы → общие правила ----------
  let generic = false;
  let off = false;
  const sent = new Set();
  let queue = [];
  let ids = [];
  let timer = 0;
  let first = true;
  const flush = () => {
    timer = 0;
    if (off || !generic || (!queue.length && !ids.length)) return;
    post({ [KEY]: 1, c: queue, i: ids });
    queue = [];
    ids = [];
  };
  const schedule = () => {
    if (timer || off || !generic || (!queue.length && !ids.length)) return;
    // Первое — сразу, дальше — пачками: страница при загрузке добавляет элементы сотнями.
    timer = setTimeout(flush, first ? 0 : 120);
    first = false;
  };
  const note = (el) => {
    if (sent.size > MAX_SENT) return;
    const id = el.id;
    if (id && typeof id === 'string' && !sent.has('#' + id)) {
      sent.add('#' + id);
      ids.push(id);
    }
    const list = el.classList;
    if (list) {
      for (let i = 0; i < list.length; i++) {
        const c = list[i];
        if (!sent.has(c)) {
          sent.add(c);
          queue.push(c);
        }
      }
    }
  };
  // Заблокированные картинки и фреймы: на их месте не остаётся пустой рамки.
  const gone = new Set();
  const collapse = (el) => {
    const tag = el.tagName;
    if ((tag === 'IMG' || tag === 'IFRAME') && (gone.has(el.src) || gone.has(el.currentSrc))) {
      el.style.setProperty('display', 'none', 'important');
    }
  };
  const scan = (node) => {
    if (node.nodeType !== 1) return;
    if (gone.size) {
      collapse(node);
      if (node.firstElementChild) node.querySelectorAll('img,iframe').forEach(collapse);
    }
    note(node);
    if (node.firstElementChild) {
      const all = node.querySelectorAll('[id],[class]');
      for (let i = 0; i < all.length; i++) note(all[i]);
    }
  };

  // ---------- процедурные правила (`:has-text()`, `:upward()`…) ----------
  let procedural = [];
  let procTimer = 0;
  const textOf = (el) => el.textContent || '';
  const matcher = (arg) => {
    const m = /^\/(.+)\/([a-z]*)$/.exec(arg);
    if (m) {
      try { const re = new RegExp(m[1], m[2]); return (t) => re.test(t); } catch { return null; }
    }
    return (t) => t.includes(arg);
  };
  const step = (els, op, isFirst) => {
    const arg = typeof op.arg === 'string' ? op.arg : '';
    switch (op.type) {
      case 'css-selector': {
        if (isFirst) return [...doc.querySelectorAll(arg)];
        const sel = arg.trimStart();
        const out = new Set();
        for (const el of els) {
          if (sel.startsWith('+') || sel.startsWith('~')) {
            const rest = sel.slice(1).trim();
            for (let n = el.nextElementSibling; n; n = sel[0] === '+' ? null : n.nextElementSibling) {
              if (!rest || n.matches(rest)) out.add(n);
            }
          } else {
            for (const n of el.querySelectorAll(':scope ' + sel)) out.add(n);
          }
        }
        return [...out];
      }
      case 'has-text': {
        const test = matcher(arg);
        return test ? els.filter((el) => test(textOf(el))) : [];
      }
      case 'min-text-length': {
        const n = Number(arg) || 0;
        return els.filter((el) => textOf(el).length >= n);
      }
      case 'matches-path': {
        const test = matcher(arg);
        return test && test(location.pathname + location.search) ? els : [];
      }
      case 'upward': {
        const n = Number(arg);
        return els.map((el) => {
          if (Number.isInteger(n) && n > 0) {
            let p = el;
            for (let i = 0; i < n && p; i++) p = p.parentElement;
            return p;
          }
          try { return el.parentElement && el.parentElement.closest(arg); } catch { return null; }
        }).filter(Boolean);
      }
      default:
        return null;
    }
  };
  const run = () => {
    procTimer = 0;
    for (const f of procedural) {
      if (!Array.isArray(f.selector) || !f.selector.length) continue;
      let els = [];
      try {
        for (let i = 0; i < f.selector.length && els; i++) {
          els = step(els, f.selector[i], i === 0);
          if (els && !els.length) break;
        }
      } catch { els = null; }
      if (!els) continue;
      const a = f.action || null;
      for (const el of els) {
        if (!a) el.style.setProperty('display', 'none', 'important');
        else if (a.type === 'remove') el.remove();
        else if (a.type === 'remove-attr' && typeof a.arg === 'string') el.removeAttribute(a.arg);
        else if (a.type === 'remove-class' && typeof a.arg === 'string') el.classList.remove(a.arg);
        else if (a.type === 'style' && typeof a.arg === 'string') {
          for (const decl of a.arg.split(';')) {
            const at = decl.indexOf(':');
            if (at > 0) el.style.setProperty(decl.slice(0, at).trim(), decl.slice(at + 1).replace(/!important/i, '').trim(), 'important');
          }
        }
      }
    }
  };
  const runSoon = () => {
    if (!procedural.length || procTimer || off) return;
    procTimer = setTimeout(run, 150);
  };

  const observer = new Obs((records) => {
    for (const r of records) {
      if (r.type === 'attributes') note(r.target);
      else for (const n of r.addedNodes) scan(n);
    }
    schedule();
    runSoon();
  });
  observer.observe(doc, { childList: true, subtree: true, attributes: true, attributeFilter: ['class', 'id'] });

  listen('message', (e) => {
    const d = e.data;
    if (!d || typeof d !== 'object' || d[KEY] !== 1) return;
    if (d.off) {
      off = true;
      observer.disconnect();
      return;
    }
    if (Array.isArray(d.x)) {
      d.x.forEach((u) => typeof u === 'string' && gone.add(u));
      doc.querySelectorAll('img,iframe').forEach(collapse);
    }
    hide(d.s);
    if (Array.isArray(d.p) && d.p.length) {
      procedural = procedural.concat(d.p);
      runSoon();
    }
    if (d.g === true && !generic) {
      generic = true;
      if (doc.documentElement) scan(doc.documentElement);
      schedule();
    }
  });
  post({ [KEY]: 1, start: true });
})();
