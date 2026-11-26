'use strict';
// История посещений — вкладка axiom://history. Своего webview у неё нет: пока она
// активна, app.js показывает эту страницу в области контента (как новую вкладку).
// Записи приходят из Rust частями по мере прокрутки, поиск идёт там же — по всей
// истории, а не только по уже показанному. Удалённое пропадает и из подсказок
// адресной строки, и с плиток новой вкладки: они читают ту же историю.
function createHistory(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  // Столько записей за раз; при возврате на вкладку — сколько уже было, но не больше MAX.
  const PAGE = 200;
  const MAX = 1000;
  const RANGES = [
    { id: 'hour', name: 'За последний час' },
    { id: 'today', name: 'За сегодня' },
    { id: 'all', name: 'За всё время' },
  ];

  const page = document.createElement('div');
  page.className = 'hist';
  page.id = 'hist';
  page.innerHTML = `<div class="hist-in">
      <header class="hist-head">
        <span class="hist-t"><h2>История</h2><p>Страницы, которые вы открывали. Из неё же берутся подсказки адресной строки и плитки новой вкладки.</p></span>
        <button class="btn" id="histClearBtn" aria-expanded="false">${icon('trash')}<span>Очистить историю</span></button>
      </header>
      <div class="hist-clear" id="histClear" hidden>
        <div class="hc-t"><b>Очистить историю</b><small>Страницы пропадут из истории, подсказок адресной строки и плиток новой вкладки. Отменить это нельзя.</small></div>
        <span class="seg" role="radiogroup" aria-label="За какое время">${RANGES.map((r) => `<button role="radio" data-range="${r.id}">${r.name}</button>`).join('')}</span>
        <div class="btns"><button class="btn danger" data-h="clear-go">Удалить</button><button class="btn ghost" data-h="clear-cancel">Отмена</button></div>
      </div>
      <div class="hist-bar" id="histBar">
        <label class="hist-find">${icon('search')}<input id="histFind" type="text" spellcheck="false" autocomplete="off" placeholder="Искать в истории" aria-label="Искать в истории"><span class="hist-n" id="histN"></span><button class="ib sm" data-h="find-clear" title="Очистить поиск (Esc)" hidden>${icon('x')}</button></label>
      </div>
      <div class="hist-list" id="histList" role="list"></div>
      <div class="hist-empty" id="histEmpty" hidden></div>
      <div class="hist-more" id="histMore"></div>
    </div>`;
  $('#content').append(page);

  const input = $('#histFind', page);
  const list = $('#histList', page);
  const empty = $('#histEmpty', page);
  const more = $('#histMore', page);
  const clearBox = $('#histClear', page);
  const clearBtn = $('#histClearBtn', page);

  let shown = false;
  // Какая вкладка показана: новая вкладка истории начинается с чистого поиска.
  let tabId = null;
  // Что показано: запрос, записи и сколько их прислал Rust (с этого места — следующая часть).
  let q = '';
  let words = [];
  let items = [];
  let urls = new Set();
  let next = 0;
  let hasMore = false;
  let found = 0;
  let total = null;
  // Ждём ответа: offset запроса; refresh — освежаем уже показанное (вернулись на вкладку).
  let waiting = null;
  let refresh = false;

  // ---------- даты ----------
  const DAY = 86400000;
  const startOfDay = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const dayKey = (d) => `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
  const cap = (s) => s.charAt(0).toUpperCase() + s.slice(1);
  function dayLabel(d) {
    const now = new Date();
    // round: в день перевода часов сутки короче или длиннее.
    const ago = Math.round((startOfDay(now) - startOfDay(d)) / DAY);
    if (ago === 0) return 'Сегодня';
    if (ago === 1) return 'Вчера';
    const weekday = d.toLocaleDateString('ru-RU', { weekday: 'long' });
    const date = d.toLocaleDateString('ru-RU', { day: 'numeric', month: 'long' });
    return `${cap(weekday)}, ${date}${d.getFullYear() !== now.getFullYear() ? ` ${d.getFullYear()}` : ''}`;
  }
  const timeOf = (d) => d.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });

  function plural(n, one, few, many) {
    const a = n % 10, b = n % 100;
    if (a === 1 && b !== 11) return one;
    if (a >= 2 && a <= 4 && (b < 12 || b > 14)) return few;
    return many;
  }
  const num = (n) => n.toLocaleString('ru-RU');
  const pages = (n) => `${num(n)} ${plural(n, 'страница', 'страницы', 'страниц')}`;

  // ---------- строки ----------
  const reEsc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  // Найденные слова подсвечены — и в заголовке, и в адресе.
  function marked(text) {
    if (!words.length) return ax.esc(text);
    const re = new RegExp(words.map(reEsc).join('|'), 'giu');
    let out = '';
    let at = 0;
    for (const m of text.matchAll(re)) {
      if (!m[0]) continue;
      out += `${ax.esc(text.slice(at, m.index))}<mark>${ax.esc(m[0])}</mark>`;
      at = m.index + m[0].length;
    }
    return out + ax.esc(text.slice(at));
  }
  function hostOf(url) {
    try { return new URL(url).host.replace(/^www\./, ''); } catch { return url; }
  }
  function rowHtml(e) {
    const title = e.title.trim() || e.url;
    // Значки строк ниже экрана грузятся, только когда до них долистают.
    const fav = ax.favHtml(e.url).replace('<img ', '<img loading="lazy" ');
    return `<span class="hr-time">${timeOf(new Date(e.last * 1000))}</span>${fav}`
      + `<span class="hr-main"><span class="hr-title">${marked(title)}</span><span class="hr-host">${marked(hostOf(e.url))}</span></span>`
      + `<button class="ib sm hr-x" data-h="delete" title="Удалить из истории" aria-label="Удалить «${ax.esc(title)}» из истории">${icon('x')}</button>`;
  }
  function makeRow(e) {
    const el = document.createElement('div');
    el.className = 'hr';
    el.tabIndex = 0;
    el.setAttribute('role', 'listitem');
    el.dataset.url = e.url;
    el.title = e.title.trim() ? `${e.title.trim()}\n${e.url}` : e.url;
    el.innerHTML = rowHtml(e);
    return el;
  }
  function makeGroup(d) {
    const sec = document.createElement('section');
    sec.className = 'hg';
    sec.dataset.day = dayKey(d);
    sec.innerHTML = `<h4 class="hg-t">${ax.esc(dayLabel(d))}</h4><div class="hg-card"></div>`;
    return sec;
  }

  // Новые записи — в конец списка: тот же день продолжает свою карточку.
  function append(batch) {
    let group = list.lastElementChild;
    let card = group?.querySelector('.hg-card');
    const frag = document.createDocumentFragment();
    for (const e of batch) {
      if (typeof e?.url !== 'string' || urls.has(e.url)) continue;
      urls.add(e.url);
      e.title = typeof e.title === 'string' ? e.title : '';
      e.last = +e.last || 0;
      items.push(e);
      const d = new Date(e.last * 1000);
      if (!group || group.dataset.day !== dayKey(d)) {
        group = makeGroup(d);
        card = group.querySelector('.hg-card');
        frag.append(group);
      }
      card.append(makeRow(e));
    }
    list.append(frag);
  }
  function resetList() {
    list.textContent = '';
    items = [];
    urls = new Set();
    next = 0;
  }

  // ---------- запросы ----------
  function ask(offset, limit) {
    waiting = offset;
    more.classList.toggle('busy', offset > 0);
    ax.send({ cmd: 'history', q, offset, limit });
  }
  function search() {
    q = input.value.trim();
    words = q.toLowerCase().split(/\s+/).filter(Boolean);
    refresh = false;
    ask(0, PAGE);
  }
  let searchTimer = 0;
  input.addEventListener('input', () => {
    clearTimeout(searchTimer);
    syncFind();
    searchTimer = setTimeout(search, 90);
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && input.value) { e.preventDefault(); e.stopPropagation(); input.value = ''; syncFind(); search(); }
    else if (e.key === 'ArrowDown') { e.preventDefault(); list.querySelector('.hr')?.focus(); }
    else if (e.key === 'Enter') { e.preventDefault(); const first = list.querySelector('.hr'); if (first) open(first, e.ctrlKey || e.metaKey); }
  });
  const syncFind = () => { $('[data-h=find-clear]', page).hidden = !input.value; };

  // Ответ Rust на `history`: устаревшие (запрос уже другой) отбрасываем.
  function onPage(m) {
    if (m.q !== q || m.offset !== waiting) return;
    waiting = null;
    more.classList.remove('busy');
    const batch = Array.isArray(m.items) ? m.items : [];
    total = +m.total || 0;
    found = +m.found || 0;
    hasMore = !!m.more;
    if (m.offset === 0) {
      // Вернулись на вкладку: если ничего не поменялось, список и прокрутка остаются.
      const same = refresh && batch.length === items.length
        && batch.every((e, i) => e.url === items[i].url && +e.last === items[i].last && (e.title || '') === items[i].title);
      if (!same) {
        const top = refresh ? page.scrollTop : 0;
        resetList();
        append(batch);
        page.scrollTop = top;
      }
    } else {
      append(batch);
    }
    next = m.offset + batch.length;
    refresh = false;
    renderMeta();
    // Экран длинный, а записей мало: подгружаем дальше, не дожидаясь прокрутки.
    requestAnimationFrame(maybeMore);
  }

  function renderMeta() {
    const none = total === 0;
    const nothing = !none && !!q && found === 0;
    clearBtn.hidden = none;
    $('#histBar', page).hidden = none;
    if (none && !clearBox.hidden) toggleClear(false);
    $('#histN', page).textContent = q && found ? `Найдено: ${num(found)}` : '';
    input.placeholder = total ? `Искать в ${num(total)} ${plural(total, 'странице', 'страницах', 'страницах')}` : 'Искать в истории';
    empty.hidden = !(none || nothing);
    if (none) {
      empty.innerHTML = `<span class="hero-ic">${icon('clock')}</span><b>История пуста</b><span>Здесь появятся страницы, которые вы откроете.</span>`;
    } else if (nothing) {
      empty.innerHTML = `<span class="hero-ic">${icon('search')}</span><b>Ничего не нашлось</b><span>В истории нет страниц, где есть «${ax.esc(q)}»</span>`;
    }
    more.textContent = hasMore ? 'Загружаем…' : '';
  }

  // Подгрузка при прокрутке: метка внизу списка подошла к экрану.
  function maybeMore() {
    if (!shown || !hasMore || waiting !== null) return;
    const r = more.getBoundingClientRect();
    const box = page.getBoundingClientRect();
    if (r.top - box.bottom < 800) ask(next, PAGE);
  }
  page.addEventListener('scroll', maybeMore, { passive: true });
  new ResizeObserver(maybeMore).observe(page);

  // ---------- открыть и удалить ----------
  function open(row, background) {
    ax.send({ cmd: 'history_open', url: row.dataset.url, background: !!background });
  }
  function remove(row) {
    const url = row.dataset.url;
    if (!url || row.classList.contains('out')) return;
    // Фокус переходит на соседнюю строку: Delete можно жать подряд.
    const rows = [...list.querySelectorAll('.hr:not(.out)')];
    const at = rows.indexOf(row);
    const focus = document.activeElement === row || row.contains(document.activeElement);
    const neighbor = rows[at + 1] || rows[at - 1];
    if (focus) neighbor?.focus({ preventScroll: true });
    row.classList.add('out');
    ax.send({ cmd: 'history_delete', url });
    urls.delete(url);
    items = items.filter((e) => e.url !== url);
    next = Math.max(0, next - 1);
    found = Math.max(0, found - 1);
    total = Math.max(0, (total || 0) - 1);
    const group = row.closest('.hg');
    const last = group && group.querySelectorAll('.hr:not(.out)').length === 0;
    const target = last ? group : row;
    target.classList.add('out');
    const done = () => {
      target.remove();
      renderMeta();
      maybeMore();
    };
    if (!ax.speed()) { done(); return; }
    // Строка (или весь день, если она была последней) плавно схлопывается.
    const h = target.getBoundingClientRect().height;
    target.animate(
      [{ height: `${h}px`, opacity: 1 }, { height: '0px', opacity: 0, marginTop: '0px', paddingTop: '0px', paddingBottom: '0px' }],
      { duration: ax.ms(220), easing: 'cubic-bezier(.4,0,.2,1)', fill: 'forwards' },
    ).finished.then(done, done);
  }

  list.addEventListener('mousedown', (e) => { if (e.button === 1 && e.target.closest('.hr')) e.preventDefault(); });
  list.addEventListener('click', (e) => {
    const row = e.target.closest('.hr');
    if (!row || row.classList.contains('out') || e.button !== 0) return;
    if (e.target.closest('[data-h=delete]')) { remove(row); return; }
    open(row, e.ctrlKey || e.metaKey);
  });
  // Средняя кнопка — в новой вкладке в фоне, как у ссылок.
  list.addEventListener('auxclick', (e) => {
    const row = e.target.closest('.hr');
    if (e.button !== 1 || !row || row.classList.contains('out') || e.target.closest('[data-h=delete]')) return;
    e.preventDefault();
    open(row, true);
  });
  list.addEventListener('keydown', (e) => {
    const row = e.target.closest('.hr');
    if (!row || e.target !== row) return;
    const rows = () => [...list.querySelectorAll('.hr:not(.out)')];
    if (e.key === 'Enter') { e.preventDefault(); open(row, e.ctrlKey || e.metaKey); }
    else if (e.key === 'Delete') { e.preventDefault(); remove(row); }
    else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const all = rows();
      const i = all.indexOf(row) + (e.key === 'ArrowDown' ? 1 : -1);
      if (i < 0) input.focus();
      else all[Math.min(i, all.length - 1)]?.focus();
    }
  });

  // ---------- очистить историю ----------
  let range = 'hour';
  // Сколько уйдёт (ответ Rust на `history_count` с тем же `since`).
  let asked = null;
  let counted = null;
  function since(id) {
    if (id === 'all') return 0;
    if (id === 'today') return Math.floor(startOfDay(new Date()) / 1000);
    return Math.floor(Date.now() / 1000) - 3600;
  }
  function toggleClear(on) {
    const open = on ?? clearBox.hidden;
    clearBox.hidden = !open;
    clearBtn.classList.toggle('on', open);
    clearBtn.setAttribute('aria-expanded', String(open));
    if (open) pickRange(range);
  }
  function pickRange(id) {
    range = id;
    clearBox.querySelectorAll('[data-range]').forEach((b) => {
      b.classList.toggle('on', b.dataset.range === id);
      b.setAttribute('aria-checked', String(b.dataset.range === id));
    });
    counted = null;
    asked = since(id);
    syncClear();
    ax.send({ cmd: 'history_count', since: asked });
  }
  // Кнопка подтверждения говорит, что именно удалится: «Удалить 7 страниц».
  function syncClear() {
    const go = $('[data-h=clear-go]', clearBox);
    const n = counted?.range === range ? counted.count : null;
    go.disabled = !n;
    go.textContent = n === null ? 'Удалить'
      : !n ? 'За это время ничего нет'
      : `Удалить ${num(n)} ${plural(n, 'страницу', 'страницы', 'страниц')}`;
  }
  function onCount(m) {
    if (m.since !== asked) return;
    counted = { range, count: Math.max(0, +m.count || 0) };
    syncClear();
  }
  function clearNow() {
    const n = counted?.range === range ? counted.count : 0;
    if (!n) return;
    // Срок — на момент нажатия: панель могла простоять открытой несколько минут.
    ax.send({ cmd: 'history_clear', since: since(range) });
    toggleClear(false);
    ax.toast(range === 'all' ? 'История очищена' : `Удалено из истории: ${pages(n)}`);
    // Rust разбирает сообщения по порядку: этот запрос уже увидит очищенную историю.
    refresh = false;
    page.scrollTop = 0;
    ask(0, PAGE);
  }
  clearBtn.addEventListener('click', () => toggleClear());
  clearBox.addEventListener('click', (e) => {
    const b = e.target.closest('button');
    if (!b) return;
    if (b.dataset.range) pickRange(b.dataset.range);
    else if (b.dataset.h === 'clear-go') clearNow();
    else if (b.dataset.h === 'clear-cancel') toggleClear(false);
  });
  page.addEventListener('click', (e) => {
    if (e.target.closest('[data-h=find-clear]')) { input.value = ''; syncFind(); search(); input.focus(); }
  });
  page.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && !clearBox.hidden) { e.preventDefault(); e.stopPropagation(); toggleClear(false); clearBtn.focus(); }
  });

  // ---------- снаружи ----------
  // `on` — активна вкладка истории `id`. Открыли её — свежий список и курсор в поиске.
  function render(on, id) {
    if (!on) {
      if (shown) toggleClear(false);
      shown = false;
      return;
    }
    if (shown && id === tabId) return;
    shown = true;
    if (id !== tabId) {
      // Другая вкладка истории (прежнюю увели на сайт): с начала и без запроса.
      tabId = id;
      input.value = '';
      syncFind();
      q = '';
      words = [];
      resetList();
      page.scrollTop = 0;
    }
    // Вернулись на ту же вкладку: освежаем то, что уже было показано, не теряя места.
    refresh = items.length > 0;
    ask(0, Math.min(MAX, Math.max(PAGE, next)));
    requestAnimationFrame(() => focus());
  }
  function focus(text) {
    if (typeof text === 'string' && text) { input.value = text; syncFind(); search(); }
    input.focus({ preventScroll: true });
    input.select();
  }

  return { render, focus, onPage, onCount, isShown: () => shown };
}
