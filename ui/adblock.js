'use strict';
// Блокировка рекламы: щит в адресной строке со счётчиком заблокированного на
// странице, окошко по клику на него («Блокировать на этом сайте») и блок «Реклама»
// в Настройки → Безопасность. Блокирует и считает Rust (src/adblock), сюда приходят
// его состояние (adblock) и счёт страницы (blocked); обратно — команды adblock_*.

function createAdblock(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const { esc } = ax;
  const btn = $('#abBtn');
  const card = $('#abc');
  const sheet = $('#sheet');

  // Состояние из Rust: настройки, списки, счётчик за всё время.
  let V = { enabled: true, updates: true, sites: [], lists: [], checked: 0, checking: false, error: null, total: 0, ready: false };
  let cardTab = null;

  const num = (n) => Number(n || 0).toLocaleString('ru-RU');
  const hostOf = (url) => { try { const u = new URL(url); return /^https?:$/.test(u.protocol) ? u.hostname.replace(/^www\./, '') : ''; } catch { return ''; } };
  const plural = (n, one, few, many) => {
    const a = n % 10, b = n % 100;
    return a === 1 && b !== 11 ? one : a >= 2 && a <= 4 && (b < 12 || b > 14) ? few : many;
  };
  const when = (secs) => {
    if (!secs) return 'ещё не проверялись';
    const d = new Date(secs * 1000);
    const days = Math.floor((Date.now() - d.getTime()) / 86400000);
    const day = days <= 0 ? 'сегодня' : days === 1 ? 'вчера' : d.toLocaleDateString('ru-RU', { day: 'numeric', month: 'long' });
    return `проверены ${day}`;
  };
  // «27 Nov 2026 19:02 UTC» из заголовка списка → «27 ноября».
  const listDate = (l) => {
    const t = Date.parse(String(l.modified || '').replace(' UTC', 'Z'));
    return Number.isFinite(t) ? new Date(t).toLocaleDateString('ru-RU', { day: 'numeric', month: 'long' }) : '';
  };

  // ---------- щит в адресной строке ----------
  function renderOmni(t) {
    const host = ax.isPage(t) ? hostOf(t.url) : '';
    const show = !!host && V.enabled && !t.sleeping;
    btn.hidden = !show;
    if (cardOpen() && (!show || t.id !== cardTab)) closeCard();
    if (!show) return;
    const on = t.ads_blocked !== false;
    const n = on ? Number(t.blocked || 0) : 0;
    btn.classList.toggle('off', !on);
    btn.innerHTML = `${icon('shield')}${n ? `<b class="ab-n">${n > 99 ? '99+' : n}</b>` : ''}`;
    btn.title = !on ? `Реклама на ${host} не блокируется`
      : n ? `Заблокировано рекламы на странице: ${num(n)}` : 'Блокировка рекламы';
    if (cardOpen()) renderCard();
  }

  // ---------- окошко ----------
  const cardOpen = () => card.classList.contains('on');

  function renderCard() {
    const t = ax.activeTab();
    if (!t) return;
    const host = hostOf(t.url);
    const on = t.ads_blocked !== false;
    const n = Number(t.blocked || 0);
    const head = on
      ? (n ? `${num(n)} ${plural(n, 'запрос заблокирован', 'запроса заблокировано', 'запросов заблокировано')}` : 'Рекламы на странице не найдено')
      : 'Реклама не блокируется';
    const sub = on
      ? `${num(V.total)} за всё время${V.ready ? '' : ' · списки ещё загружаются'}`
      : `На ${esc(host)} выключено — включите, и страница обновится`;
    card.innerHTML = `<div class="trp-head"><span class="trp-ic${on ? '' : ' off'}">${icon('shield')}</span>
        <span class="trp-t"><b>${head}</b><small>${sub}</small></span></div>
      <label class="trp-row ab-row"><span class="pm-name"><span>Блокировать на ${esc(host)}</span></span><input type="checkbox" class="tg" id="abSite" ${on ? 'checked' : ''}></label>
      <div class="pm-foot"><button class="lnk" data-ab-act="settings">${icon('shield')}Настройки блокировки</button></div>`;
    if (cardOpen()) place();
  }

  function place() {
    const r = btn.getBoundingClientRect();
    const w = Math.min(320, innerWidth - 16);
    card.style.width = w + 'px';
    const left = Math.round(Math.min(Math.max(8, r.right - w), innerWidth - 8 - w));
    // Панель у нижнего края окна — окошко над щитом.
    const up = r.top > innerHeight / 2;
    card.classList.toggle('up', up);
    const top = up ? r.top - 8 - card.offsetHeight : r.bottom + 8;
    Object.assign(card.style, { left: left + 'px', top: Math.max(8, Math.round(top)) + 'px' });
  }

  function openCard() {
    const t = ax.activeTab();
    if (!t || btn.hidden) return;
    if (cardOpen()) { closeCard(); return; }
    cardTab = t.id;
    renderCard();
    card.classList.add('on');
    ax.setOverlay('abc', true);
    place();
  }

  function closeCard() {
    if (!cardOpen()) return;
    card.classList.remove('on');
    ax.setOverlay('abc', false);
    cardTab = null;
  }

  btn.addEventListener('click', openCard);
  card.addEventListener('change', (e) => {
    if (e.target.id !== 'abSite') return;
    // Rust запомнит сайт и обновит страницу: изменение видно сразу.
    ax.send({ cmd: 'adblock_site', on: !!e.target.checked });
    closeCard();
  });
  card.addEventListener('click', (e) => {
    const b = e.target.closest('button[data-ab-act]');
    if (b && b.dataset.abAct === 'settings') { closeCard(); ax.openSettings('security'); }
  });
  document.addEventListener('mousedown', (e) => {
    if (cardOpen() && !e.target.closest('#abc') && !e.target.closest('#abBtn')) closeCard();
  }, true);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && cardOpen()) { e.preventDefault(); e.stopImmediatePropagation(); closeCard(); }
  }, true);
  addEventListener('resize', () => { if (cardOpen()) requestAnimationFrame(place); });

  // ---------- Настройки → Безопасность ----------
  function sectionHtml() {
    const row = (title, note, control, extra = '') =>
      `<div class="srow" ${extra}><span class="sl"><b>${title}</b>${note ? `<small>${note}</small>` : ''}</span><span class="sc">${control}</span></div>`;
    return `
      <h4>Реклама</h4>
      <div class="card">
        ${row('Блокировать рекламу', '<span id="abNote"></span>', '<input type="checkbox" class="tg" data-bkey="adblock">', 'data-kw="реклама блокировка блокировщик adblock баннеры ublock фильтры"')}
        ${row('Обновлять списки фильтров', 'Раз в 4 дня Axiom скачивает свежие EasyList и RuAdList с easylist-downloads.adblockplus.org — только сами списки, ничего о вас. Выключите — останутся списки, встроенные в Axiom', '<input type="checkbox" class="tg" data-bkey="adblock_updates">', 'data-kw="списки фильтров easylist ruadlist обновление реклама"')}
        <div class="srow" data-kw="списки фильтров easylist ruadlist версия проверить"><span class="sl"><b>Списки</b><small id="abLists"></small></span><span class="sc"><button class="btn" data-ab-act="update" id="abUpd">Проверить</button></span></div>
      </div>
      <h4>Сайты без блокировки</h4>
      <div class="card">
        ${row('Не блокировать на сайтах', 'Здесь реклама показывается, как без блокировщика — если сайт без неё не работает. example.com — это и www.example.com, и другие его поддомены. Добавить сайт можно и щитом в адресной строке', '', 'data-kw="не блокировать исключения разрешить рекламу сайты белый список"')}
        <div class="srow" data-kw="не блокировать сайт добавить"><span class="inline"><input type="text" id="abIn" placeholder="example.com" spellcheck="false" maxlength="2048"><button class="btn primary" data-ab-act="add">Добавить</button></span></div>
        <div class="ns-list" id="abSites"></div>
      </div>`;
  }

  function syncSheet() {
    const note = $('#abNote', sheet);
    if (!note) return;
    note.textContent = V.enabled
      ? `Реклама не загружается и не видна — во вкладках и встроенных в страницы фреймах. Заблокировано: ${num(V.total)}`
      : 'Выключено: сайты показывают рекламу';
    const lists = (V.lists || []).map((l) => `${l.title}${listDate(l) ? ` от ${listDate(l)}` : ''}`).join(', ');
    $('#abLists', sheet).textContent = V.checking ? 'Проверяем…'
      : V.error ? `${lists ? `${lists} · ` : ''}${V.error}`
      : `${lists || 'Загружаются…'} · ${when(V.checked)}`;
    $('#abUpd', sheet).disabled = !!V.checking;
    const sites = Array.isArray(V.sites) ? V.sites : [];
    $('#abSites', sheet).innerHTML = sites.map((s) => `<div class="srow"><span class="sl"><b>${esc(s)}</b></span>
      <span class="sc"><button class="ib sm" data-ab-forget="${esc(s)}" title="Снова блокировать рекламу">${icon('x')}</button></span></div>`).join('');
  }

  const addSite = () => {
    const input = $('#abIn', sheet);
    const v = (input?.value || '').trim();
    if (!v) return;
    ax.send({ cmd: 'adblock_allow', site: v });
    input.value = '';
  };
  sheet.addEventListener('click', (e) => {
    const forget = e.target.closest('[data-ab-forget]');
    if (forget) { ax.send({ cmd: 'adblock_forget', site: forget.dataset.abForget }); return; }
    const b = e.target.closest('[data-ab-act]');
    if (!b || !sheet.contains(b)) return;
    if (b.dataset.abAct === 'update') ax.send({ cmd: 'adblock_update' });
    else if (b.dataset.abAct === 'add') addSite();
  });
  sheet.addEventListener('keydown', (e) => {
    if (e.target.id === 'abIn' && e.key === 'Enter') { e.preventDefault(); addSite(); }
  });

  function onState(m) {
    V = { ...V, ...m };
    syncSheet();
    renderOmni(ax.activeTab());
  }

  // Счёт страницы: приходит чаще состояния вкладок, меняет только щит.
  function onBlocked(m) {
    const t = ax.tabs().find((x) => x.id === m.id);
    if (t) t.blocked = m.count;
    V.total = m.total;
    if (m.id === ax.activeTab()?.id) renderOmni(t);
  }

  return { renderOmni, sectionHtml, syncSheet, onState, onBlocked, close: closeCard, isOpen: cardOpen };
}
