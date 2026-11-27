// Среда плагина в странице. Выполняется в изолированном мире плагина: у него свои
// глобальные переменные, сайт их не видит и не может подменить. DOM общий.
// P — сведения о плагине, S — его скрипты, RUN — их код (функции), см. bootstrap.rs.
(function (P, S, RUN) {
  'use strict';
  const g = globalThis;
  // Мир живёт, пока жив документ: второй запуск в том же документе не нужен.
  if (g.__axiomPluginLoaded) return;
  Object.defineProperty(g, '__axiomPluginLoaded', { value: true });

  // Свои копии до чужого кода: код плагина может переписать что угодно в своём мире.
  const json = JSON.stringify;
  const freeze = Object.freeze;
  const Re = RegExp;

  let top = false;
  try { top = window === window.top; } catch { top = false; }
  const loc = location;
  const scheme = loc.protocol === 'https:' ? 2 : loc.protocol === 'http:' ? 1 : 0;
  const host = loc.hostname.replace(/\.$/, '').toLowerCase();
  const path = loc.pathname + loc.search;
  const hit = (m) => (m.s & scheme) !== 0 && (!m.h || host === m.h || (m.d && host.endsWith('.' + m.h))) && new Re(m.p).test(path);
  const todo = [];
  S.forEach((s, i) => { if ((top || s.all) && s.m.some(hit) && !s.x.some(hit)) todo.push(i); });
  if (!todo.length) return;

  // ---------- канал к браузеру ----------
  // Windows: функция-мост, её Rust ставит в мир после загрузки документа (__axiomUp).
  // Linux: обработчик сообщений WebKit, есть сразу.
  let send = null;
  const queue = [];
  const wait = new Map();
  let seq = 0;
  const on = { action: [], settings: [] };
  let values = freeze(P.settings);

  const connect = () => {
    if (send) return true;
    const wk = g.webkit && g.webkit.messageHandlers && g.webkit.messageHandlers[P.bridge];
    if (wk) {
      send = (s) => wk.postMessage(s);
    } else if (typeof g[P.bridge] === 'function') {
      const bridge = g[P.bridge];
      try { delete g[P.bridge]; } catch { /* не удаляется — не страшно */ }
      send = (s) => bridge(s);
    } else {
      return false;
    }
    queue.splice(0).forEach((s) => send(s));
    return true;
  };
  const post = (m) => {
    const s = json(m);
    if (send) send(s);
    else if (queue.length < 200) queue.push(s);
  };
  const fire = (event, arg) => {
    const list = on[event] || [];
    list.slice().forEach((fn) => {
      try { fn(arg); } catch (err) { console.error(`[${P.name}]`, err); }
    });
    return list.length > 0;
  };
  Object.defineProperty(g, '__axiomUp', { value: connect });
  // Ответы и события от браузера. true — было кому их отдать.
  Object.defineProperty(g, '__axiomRecv', {
    value: (m) => {
      if (!m || typeof m !== 'object') return false;
      if (m.event === 'settings') {
        values = freeze(Object.assign({}, m.values));
        return fire('settings', values);
      }
      if (m.event === 'action') return fire('action');
      const w = wait.get(m.id);
      if (!w) return false;
      wait.delete(m.id);
      if (typeof m.error === 'string') w.reject(new Error(m.error));
      else w.resolve(m.ok);
      return true;
    },
  });
  const call = (op, args) => {
    if (!top) return Promise.reject(new Error('axiom: API работает только в самой странице, не во фрейме'));
    return new Promise((resolve, reject) => {
      const id = ++seq;
      wait.set(id, { resolve, reject });
      post(Object.assign({ id, op }, args));
    });
  };
  const keyList = (keys) => (Array.isArray(keys) ? keys : [keys]).map(String);

  // ---------- API плагина ----------
  const api = freeze({
    plugin: freeze({ id: P.id, name: P.name, version: P.version }),
    // Значения настроек плагина (из «Настройки → Плагины»).
    get settings() { return values; },
    storage: freeze({
      // get() — всё, get('k') — значение, get(['a', 'b']) — { a, b }.
      get(keys) {
        const one = typeof keys === 'string';
        return call('storage.get', { keys: keys == null ? null : keyList(keys) }).then((r) => (one ? r[keys] : r));
      },
      // set('k', v) или set({ k: v, … }). Значения — всё, что переживает JSON.
      set(key, value) {
        const items = key !== null && typeof key === 'object' ? key : { [String(key)]: value };
        return call('storage.set', { items: JSON.parse(json(items)) });
      },
      remove(keys) { return call('storage.remove', { keys: keyList(keys) }); },
      clear() { return call('storage.clear', {}); },
    }),
    // Сообщение внизу окна Axiom. Разрешение "notify".
    notify(text) { return call('notify', { text: String(text) }); },
    tabs: freeze({
      // Новая вкладка рядом с этой. Разрешение "tabs". { background: true } — не переходить на неё.
      open(url, options) {
        let href;
        try { href = new URL(String(url), loc.href).href; } catch { return Promise.reject(new Error('axiom.tabs.open: неверный адрес')); }
        return call('tabs.open', { url: href, background: !!(options && options.background) });
      },
    }),
    // События: 'action' — нажали кнопку плагина на панели, 'settings' — поменяли настройки.
    on(event, fn) {
      if (!Object.prototype.hasOwnProperty.call(on, event)) throw new Error(`axiom.on: нет события «${event}»`);
      if (typeof fn !== 'function') throw new Error('axiom.on: нужна функция');
      on[event].push(fn);
    },
    off(event, fn) {
      const list = on[event];
      const i = list ? list.indexOf(fn) : -1;
      if (i >= 0) list.splice(i, 1);
    },
  });
  Object.defineProperty(g, 'axiom', { value: api });

  if (top) {
    connect();
    post({ op: 'hello' });
  }

  // ---------- стили ----------
  // Через adoptedStyleSheets: элемент <style> на сайтах со строгой CSP не применится.
  const sheets = [];
  todo.forEach((i) => {
    if (!S[i].css) return;
    try {
      const sheet = new CSSStyleSheet();
      sheet.replaceSync(S[i].css);
      sheets.push(sheet);
    } catch (err) { console.error(`[${P.name}] CSS`, err); }
  });
  if (sheets.length) {
    const adopt = () => {
      try {
        const have = document.adoptedStyleSheets;
        const miss = sheets.filter((s) => !have.includes(s));
        if (miss.length) document.adoptedStyleSheets = [...have, ...miss];
      } catch (err) { console.error(`[${P.name}] CSS`, err); }
    };
    adopt();
    // Сайт мог заменить список своим — возвращаем свои стили.
    document.addEventListener('DOMContentLoaded', adopt, { once: true });
    addEventListener('load', adopt, { once: true });
  }

  // ---------- скрипты ----------
  const run = (i) => {
    try { RUN[i].call(g); } catch (err) { console.error(`[${P.name}]`, err); }
  };
  const when = (at, fn) => {
    if (at === 'start') fn();
    else if (at === 'ready' && document.readyState === 'loading') document.addEventListener('DOMContentLoaded', fn, { once: true });
    else if (at === 'load' && document.readyState !== 'complete') addEventListener('load', fn, { once: true });
    else fn();
  };
  todo.forEach((i) => { if (RUN[i]) when(S[i].at, () => run(i)); });
})(P, S, RUN);
