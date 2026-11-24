// Перевод страницы. Rust вставляет этот скрипт, когда человек попросил перевести,
// и сам вызывает: start — начать, take — отдать порцию текста, apply — вставить
// перевод, revert — вернуть оригинал. Сама страница ничего не запрашивает.
(() => {
  if (window.__axiomTr) return;
  // Код, поля ввода, редактируемое и помеченное «не переводить» не трогаем.
  const SKIP = 'script,style,noscript,template,code,pre,kbd,samp,var,textarea,input,svg,math,iframe,object,canvas,'
    + '[translate="no"],.notranslate,[contenteditable]:not([contenteditable="false"])';
  const SKIP_ATTR = 'head,svg,[translate="no"],.notranslate';
  const ATTRS = '[title],[placeholder],[alt],input[type=submit][value],input[type=button][value],input[type=reset][value]';
  // Строчные теги: их текст переводится вместе с абзацем, чтобы не рвать фразу.
  const INLINE = new Set(['A', 'ABBR', 'B', 'BDI', 'BDO', 'BIG', 'CITE', 'DATA', 'DEL', 'DFN', 'EM', 'FONT', 'I', 'INS',
    'LABEL', 'MARK', 'NOBR', 'Q', 'RP', 'RT', 'RUBY', 'S', 'SMALL', 'SPAN', 'STRIKE', 'STRONG', 'SUB', 'SUP', 'TIME', 'TT', 'U']);
  const LETTER = /\p{L}/u;
  const MAX_NODE = 8000;
  const GROUP_ITEMS = 60;
  const GROUP_CHARS = 3000;
  const MAX_ADDED = 3000;
  const MAX_SCAN = 400000;
  const MAX_TRIES = 4;

  let on = false;
  let observer = null;
  let initial = false;
  let rescan = false;
  let queue = [];
  let head = 0;
  let added = [];
  let nextId = 1;
  const refs = new Map();
  // Оригинал и наш перевод — по узлу: страница могла сама поменять текст после нас.
  let orig = new WeakMap();
  let done = new WeakMap();
  let pending = new WeakMap();
  let tries = new WeakMap();
  let origAttr = new WeakMap();
  let doneAttr = new WeakMap();

  const norm = (s) => s.replace(/\s+/g, ' ');
  const meaningful = (v) => !!v && v.length <= MAX_NODE && LETTER.test(v);

  function blockOf(el) {
    while (el && INLINE.has(el.tagName) && el.parentElement) el = el.parentElement;
    return el;
  }

  function visible(el) {
    if (!el || el.tagName === 'TITLE') return true;
    const r = el.getBoundingClientRect();
    return (r.width > 0 || r.height > 0) && r.bottom > -200 && r.top < innerHeight * 1.5;
  }

  // Запятые и числа между ссылками тоже берём: без них Google увидел бы
  // «performancetype safety». Фрагмент уходит, только если в нём есть буквы.
  function wantText(n) {
    const v = n.nodeValue;
    return !!v && v.length <= MAX_NODE && done.get(n) !== v && (tries.get(n) || 0) < MAX_TRIES;
  }

  function attrsOf(el) {
    const list = [];
    for (const a of ['title', 'placeholder', 'alt']) if (el.hasAttribute(a)) list.push(a);
    if (el.tagName === 'INPUT' && /^(submit|button|reset)$/i.test(el.type) && el.hasAttribute('value')) list.push('value');
    return list.filter((a) => {
      const v = el.getAttribute(a);
      return meaningful(v) && doneAttr.get(el)?.[a] !== v;
    });
  }

  const filter = {
    acceptNode(n) {
      if (n.nodeType === 1) return n.matches(SKIP) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_SKIP;
      return wantText(n) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP;
    },
  };

  // Текст поддерева — группами соседних узлов одного абзаца; при первом проходе
  // видимое на экране идёт первым.
  function scan(root, firstPass) {
    const near = [];
    const far = [];
    const top = root.nodeType === 1 ? root : root.parentElement;
    if (!top || top.closest(SKIP)) return;
    let group = null;
    let block = null;
    let total = 0;
    const flush = () => {
      if (group && group.items.some((x) => LETTER.test(x.nodeValue))) (group.near ? near : far).push(group.items);
      group = null;
    };
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, filter);
    let n = root.nodeType === 3 ? (wantText(root) ? root : null) : walker.nextNode();
    for (; n; n = root.nodeType === 3 ? null : walker.nextNode()) {
      const b = blockOf(n.parentElement);
      const len = n.nodeValue.length;
      if (!group || b !== block || group.items.length >= GROUP_ITEMS || group.chars + len > GROUP_CHARS) {
        flush();
        block = b;
        group = { items: [], chars: 0, near: firstPass && visible(b) };
      }
      group.items.push(n);
      group.chars += len;
      total += len;
      if (total > MAX_SCAN) break;
    }
    flush();
    // Подсказки, подписи картинок и кнопки — каждая отдельным фрагментом, в конце.
    if (root.nodeType === 1) {
      const els = root.matches(ATTRS) ? [root, ...root.querySelectorAll(ATTRS)] : root.querySelectorAll(ATTRS);
      for (const el of els) {
        if (el.closest(SKIP_ATTR)) continue;
        for (const a of attrsOf(el)) far.push([{ el, a }]);
      }
    }
    for (const g of near) queue.push(g);
    for (const g of far) queue.push(g);
  }

  function take(limit) {
    if (!on) return null;
    limit = Math.max(200, Math.min(Number(limit) || 0, 50000));
    if (initial || rescan) {
      scan(document.documentElement, initial);
      initial = rescan = false;
      added = [];
    } else if (added.length) {
      const list = added;
      added = [];
      for (const n of list) if (n.isConnected) scan(n, false);
    }
    const out = [];
    let chars = 0;
    while (head < queue.length && chars < limit) {
      const items = queue[head];
      queue[head++] = null;
      const cand = [];
      let space = false;
      for (const it of items) {
        let raw;
        if (it.nodeType === 3) {
          if (!it.isConnected || pending.has(it) || !wantText(it)) continue;
          raw = it.nodeValue;
          // Узел из одних пробелов не переводим, но пробел отдаём соседу: иначе слова слипнутся.
          if (!/\S/.test(raw)) {
            const prev = cand[cand.length - 1];
            if (prev) { if (!prev.text.endsWith(' ')) prev.text += ' '; } else space = true;
            continue;
          }
        } else {
          if (!it.el.isConnected || !attrsOf(it.el).includes(it.a)) continue;
          raw = it.el.getAttribute(it.a);
        }
        let text = norm(raw);
        if (space && !text.startsWith(' ')) text = ' ' + text;
        space = false;
        cand.push({ it, raw, text });
      }
      if (!cand.some((c) => LETTER.test(c.raw))) continue;
      const g = [];
      for (const { it, raw, text } of cand) {
        const id = nextId++;
        if (it.nodeType === 3) {
          refs.set(id, { node: it, raw });
          pending.set(it, id);
          tries.set(it, (tries.get(it) || 0) + 1);
        } else {
          refs.set(id, { el: it.el, a: it.a, raw });
        }
        g.push([id, text]);
        chars += raw.length;
      }
      out.push(g);
    }
    if (head > 1000 && head * 2 > queue.length) {
      queue = queue.slice(head);
      head = 0;
    }
    // Ответы на потерянные запросы не придут: не держим их узлы вечно.
    if (refs.size > 50000) {
      for (const k of refs.keys()) { refs.delete(k); if (refs.size <= 40000) break; }
    }
    return { b: out, more: head < queue.length };
  }

  function apply(list) {
    if (!on || !Array.isArray(list)) return 0;
    let count = 0;
    for (const item of list) {
      if (!Array.isArray(item)) continue;
      const [id, text] = item;
      const r = refs.get(id);
      if (!r || typeof text !== 'string') continue;
      refs.delete(id);
      if (r.node) {
        const node = r.node;
        if (pending.get(node) !== id) continue;
        pending.delete(node);
        // Пока ждали перевода, страница сама поменяла текст — переведём новый.
        if (!node.isConnected || node.nodeValue !== r.raw) continue;
        // Нужен ли пробел по краю, решил Rust; сам пробел берём исходный (перенос, отступ).
        // Пустой перевод — Google влил этот кусок в соседний.
        const t = text.trim();
        const lead = t && text.startsWith(' ') ? (r.raw.match(/^\s+/)?.[0] || ' ') : '';
        const trail = t && text.endsWith(' ') ? (r.raw.match(/\s+$/)?.[0] || ' ') : '';
        const value = lead + t + trail;
        orig.set(node, r.raw);
        done.set(node, value);
        node.nodeValue = value;
      } else {
        const { el, a } = r;
        if (!el.isConnected || el.getAttribute(a) !== r.raw || !text) continue;
        if (!origAttr.has(el)) { origAttr.set(el, Object.create(null)); doneAttr.set(el, Object.create(null)); }
        origAttr.get(el)[a] = r.raw;
        doneAttr.get(el)[a] = text;
        el.setAttribute(a, text);
      }
      count++;
    }
    return count;
  }

  function queueAdded(n) {
    if (rescan) return;
    if (added.length >= MAX_ADDED) { rescan = true; added = []; return; }
    added.push(n);
  }

  function start() {
    if (on) return;
    on = true;
    initial = true;
    queue = [];
    head = 0;
    added = [];
    // Новый текст (лента, подгрузка, одностраничные сайты) только запоминаем:
    // переводить его Rust попросит сам, когда спросит take.
    observer = new MutationObserver((records) => {
      if (!on) return;
      for (const r of records) {
        if (r.type === 'characterData') {
          const t = r.target;
          if (done.get(t) !== t.nodeValue) { pending.delete(t); queueAdded(t); }
        } else {
          for (const n of r.addedNodes) if (n.nodeType === 1 || n.nodeType === 3) queueAdded(n);
        }
      }
    });
    observer.observe(document.documentElement, { childList: true, subtree: true, characterData: true });
  }

  function revert() {
    on = false;
    if (observer) observer.disconnect();
    observer = null;
    queue = [];
    head = 0;
    added = [];
    refs.clear();
    // Возвращаем только то, что после нас никто не менял.
    const walker = document.createTreeWalker(document.documentElement, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      if (orig.has(n) && done.get(n) === n.nodeValue) n.nodeValue = orig.get(n);
    }
    for (const el of document.querySelectorAll(ATTRS)) {
      const o = origAttr.get(el);
      const d = doneAttr.get(el);
      if (!o) continue;
      for (const a of Object.keys(o)) if (el.getAttribute(a) === d[a]) el.setAttribute(a, o[a]);
    }
    orig = new WeakMap();
    done = new WeakMap();
    pending = new WeakMap();
    tries = new WeakMap();
    origAttr = new WeakMap();
    doneAttr = new WeakMap();
  }

  Object.defineProperty(window, '__axiomTr', {
    value: Object.freeze({ start, take, apply, revert, active: () => on }),
  });
})();
