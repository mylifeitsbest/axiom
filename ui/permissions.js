'use strict';
// Разрешения сайтов: окошко «Сайт хочет…» под адресной строкой, карточка сайта
// (клик по замку слева от адреса) и раздел «Безопасность» в настройках.
// Решает и помнит Rust (src/app/permissions.rs): сюда приходят запросы и списки,
// обратно уходят ответы.

// Виды разрешений — те же, что Perm в src/storage/permissions.rs.
const PERMS = {
  microphone: { icon: 'mic', name: 'Микрофон', ask: 'Использовать микрофон' },
  camera: { icon: 'camera', name: 'Камера', ask: 'Использовать камеру' },
  geolocation: { icon: 'pin', name: 'Местоположение', ask: 'Знать, где вы находитесь' },
  notifications: { icon: 'bell', name: 'Уведомления', ask: 'Показывать уведомления' },
  clipboard: { icon: 'clipboard', name: 'Буфер обмена', ask: 'Читать скопированный текст и картинки' },
  midi: { icon: 'midi', name: 'MIDI-устройства', ask: 'Полностью управлять MIDI-устройствами' },
  fonts: { icon: 'font', name: 'Шрифты компьютера', ask: 'Видеть шрифты, установленные на компьютере' },
  windows: { icon: 'windows', name: 'Окна на экранах', ask: 'Открывать и расставлять окна на всех экранах' },
  downloads: { icon: 'download', name: 'Несколько загрузок', ask: 'Скачивать несколько файлов подряд' },
  files: { icon: 'file', name: 'Изменение файлов', ask: 'Изменять файлы и папки, которые вы откроете' },
  screen: { icon: 'monitor', name: 'Экран', ask: 'Показывать ваш экран' },
};
const PERM_ORDER = Object.keys(PERMS);
// «Не спрашивать» в настройках — для самых частых просьб.
const PERM_DEFAULTS = ['notifications', 'geolocation', 'camera', 'microphone', 'clipboard'];
// Ссылки на приложения: понятное имя вместо схемы, где оно известно.
const APP_NAMES = {
  tg: 'Telegram', mailto: 'Почта', steam: 'Steam', discord: 'Discord', zoommtg: 'Zoom', zoomus: 'Zoom',
  spotify: 'Spotify', slack: 'Slack', vscode: 'VS Code', msteams: 'Microsoft Teams', skype: 'Skype',
  whatsapp: 'WhatsApp', viber: 'Viber', magnet: 'Торрент-клиент', tel: 'Звонки', sms: 'Сообщения',
  'ms-settings': 'Параметры Windows', obsidian: 'Obsidian', notion: 'Notion', figma: 'Figma',
};

function createPermissions(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const { esc } = ax;
  const prompt = $('#pmp');
  const card = $('#pmc');
  const sheet = $('#sheet');
  // Кнопки окошка оживают не сразу: клик, начатый на странице, не должен попасть в «Разрешить».
  const ARM_MS = 450;

  // Списки из Rust: сайты с решениями, общее «не спрашивать», «Всегда разрешать» ссылок.
  let L = { sites: [], defaults: {}, app_links: [] };
  let req = null;
  let armedAt = 0;
  // Карточка сайта: для какого origin открыта и меняли ли в ней что-то.
  let cardSite = null;
  let cardChanged = false;
  // Строки, что были в карточке при открытии: «Спрашивать» не убирает строку из-под мыши.
  let cardKinds = [];
  let findQ = '';

  const known = (k) => Object.prototype.hasOwnProperty.call(PERMS, k);
  const permIcon = (k) => icon(PERMS[k]?.icon || 'shield');
  const hostOf = (site) => { try { const u = new URL(site); return u.host; } catch { return site; } };
  const originOf = (url) => { try { const u = new URL(url); return /^https?:$/.test(u.protocol) && u.hostname ? u.origin : null; } catch { return null; } };
  const siteOf = (origin) => L.sites.find((s) => s.site === origin) || null;
  const linksOf = (origin) => L.app_links.filter((l) => l.site === origin);
  const appName = (scheme) => APP_NAMES[scheme] ? `${APP_NAMES[scheme]} (${scheme}:)` : `Ссылки ${scheme}:`;
  const sortKinds = (list) => list.filter(known).sort((a, b) => PERM_ORDER.indexOf(a) - PERM_ORDER.indexOf(b));

  function decisionSelect(site, kind, value) {
    const opt = (v, t) => `<option value="${v}"${v === value ? ' selected' : ''}>${t}</option>`;
    return `<select class="pm-sel ${value}" data-pm-site="${esc(site)}" data-pm-kind="${esc(kind)}" aria-label="${esc(PERMS[kind].name)}">
      ${opt('ask', 'Спрашивать')}${opt('allow', 'Разрешить')}${opt('block', 'Запретить')}</select>`;
  }

  function placeBelowOmni(panel, width, alignLeft) {
    const r = $('#omni')?.getBoundingClientRect();
    const w = Math.min(width, innerWidth - 16);
    panel.style.width = w + 'px';
    const visible = r && r.width > 0 && r.bottom > 0;
    const left = visible ? Math.min(Math.max(8, alignLeft ?? r.left), innerWidth - 8 - w) : (innerWidth - w) / 2;
    // Панель у нижнего края окна — окошко над адресной строкой.
    const up = visible && r.top > innerHeight / 2;
    panel.classList.toggle('up', up);
    const top = !visible ? 56 : up ? r.top - 8 - panel.offsetHeight : r.bottom + 8;
    Object.assign(panel.style, { left: left + 'px', top: Math.max(8, Math.round(top)) + 'px' });
  }

  // ---------- окошко «Сайт хочет…» ----------
  const isOpen = () => prompt.classList.contains('on');

  function renderPrompt() {
    const kinds = sortKinds(req.kinds);
    const screenOnly = kinds.length === 1 && kinds[0] === 'screen';
    const who = req.site || 'Эта страница';
    const sub = req.page ? `<small>Встроен в страницу ${esc(req.page)}</small>`
      : screenOnly ? '<small>Что именно показать, спросит система</small>'
      : !req.remember ? '<small>Решение не запомнится: у страницы нет адреса сайта</small>' : '';
    // «Только сейчас» — когда решение иначе запомнилось бы (и не для экрана: он и так на раз).
    const once = req.remember && !screenOnly;
    prompt.innerHTML = `<div class="trp-head"><span class="trp-ic">${permIcon(kinds[0])}</span>
        <span class="trp-t"><b>${esc(who)} хочет</b>${sub}</span>
        <button class="ib sm pm-x" data-pm="dismiss" title="Не сейчас (Esc)">${icon('x')}</button></div>
      <div class="pm-list">${kinds.map((k) => `<div class="pm-row">${permIcon(k)}<span>${esc(PERMS[k].ask)}</span></div>`).join('')}</div>
      <div class="xap-acts pm-acts"><button class="btn ghost" data-pm="block">Запретить</button><span class="grow"></span>
        ${once ? '<button class="btn" data-pm="once">Только сейчас</button>' : ''}<button class="btn primary" data-pm="allow">Разрешить</button></div>`;
  }

  function openPrompt(m) {
    const fresh = !req || req.id !== m.id;
    req = m;
    closeCard();
    renderPrompt();
    if (!isOpen()) {
      prompt.classList.add('on');
      ax.setOverlay('pmp', true);
    }
    placeBelowOmni(prompt, 380);
    // Новые виды в том же окошке — снова пауза, чтобы их успели прочитать.
    armedAt = performance.now() + ARM_MS;
    if (fresh) prompt.classList.add('arm');
    setTimeout(() => prompt.classList.remove('arm'), ARM_MS);
  }

  function closePrompt() {
    if (!isOpen()) return;
    prompt.classList.remove('on');
    ax.setOverlay('pmp', false);
    req = null;
  }

  function answer(a) {
    if (!req) return;
    ax.send({ cmd: 'permission', id: req.id, answer: a });
    const site = req.site;
    closePrompt();
    if (a === 'dismiss' && site) ax.toast('Передумаете — нажмите на замок слева от адреса');
  }

  prompt.addEventListener('click', (e) => {
    const b = e.target.closest('button[data-pm]');
    if (!b || performance.now() < armedAt) return;
    answer(b.dataset.pm);
  });

  // Из Rust: запрос (новый или с новыми видами) или «закрыть» (id = 0).
  function onAsk(m) {
    if (!m || typeof m.id !== 'number') return;
    if (m.id === 0) { closePrompt(); return; }
    const kinds = Array.isArray(m.kinds) ? m.kinds.filter(known) : [];
    if (!kinds.length) return;
    openPrompt({ id: m.id, site: String(m.site || ''), page: String(m.page || ''), kinds, remember: !!m.remember });
  }

  // ---------- карточка сайта (клик по замку) ----------
  const cardOpen = () => card.classList.contains('on');

  function renderCard() {
    const t = ax.activeTab();
    const secure = /^https:/.test(t?.url || '');
    const host = hostOf(cardSite);
    const own = siteOf(cardSite);
    const decided = own ? Object.keys(own.perms) : [];
    const kinds = sortKinds([...new Set([...cardKinds, ...decided])]);
    cardKinds = kinds;
    const links = linksOf(cardSite);
    const rows = kinds.map((k) => `<div class="trp-row pm-crow"><span class="pm-name">${permIcon(k)}<span>${esc(PERMS[k].name)}</span></span>${decisionSelect(cardSite, k, own?.perms[k] || 'ask')}</div>`).join('');
    const linkRows = links.map((l) => `<div class="trp-row pm-crow"><span class="pm-name">${icon('popout')}<span>${esc(appName(l.scheme))}</span></span>
      <button class="ib sm" data-pm-link="${esc(l.site)}" data-pm-scheme="${esc(l.scheme)}" title="Снова спрашивать перед запуском">${icon('x')}</button></div>`).join('');
    card.innerHTML = `<div class="trp-head"><span class="trp-ic${secure ? '' : ' warn'}">${icon(secure ? 'lock' : 'globe')}</span>
        <span class="trp-t"><b>${esc(host)}</b><small>${secure ? 'Соединение защищено' : 'Соединение не защищено: не вводите здесь пароли и карты'}</small></span></div>
      <div class="pm-h">Разрешения</div>
      <div class="trp-list pm-clist">${rows || '<div class="pm-none">Сайт пока ни о чём не просил</div>'}</div>
      ${links.length ? `<div class="pm-h">Открывать приложения без вопроса</div><div class="trp-list pm-clist">${linkRows}</div>` : ''}
      ${cardChanged ? `<div class="pm-reload"><span>Изменения вступят в силу после обновления страницы</span><button class="btn primary" data-pm-act="reload">Обновить</button></div>` : ''}
      <div class="pm-foot"><button class="lnk" data-pm-act="all">${icon('shield')}Все сайты</button>
        ${decided.length ? '<button class="lnk" data-pm-act="forget">Сбросить разрешения</button>' : ''}</div>`;
    // Высота могла измениться (подсказка «Обновить»): над панелью снизу окошко растёт вверх.
    if (cardOpen()) placeCard();
  }

  const placeCard = () => placeBelowOmni(card, 340, $('#omniIc').getBoundingClientRect().left - 10);

  function openCard() {
    const t = ax.activeTab();
    const origin = ax.isPage(t) ? originOf(t.url) : null;
    if (!origin) return;
    if (cardOpen() && cardSite === origin) { closeCard(); return; }
    cardSite = origin;
    cardChanged = false;
    cardKinds = [];
    renderCard();
    if (!cardOpen()) {
      card.classList.add('on');
      ax.setOverlay('pmc', true);
    }
    placeCard();
  }

  function closeCard() {
    if (!cardOpen()) return;
    card.classList.remove('on');
    ax.setOverlay('pmc', false);
    cardSite = null;
  }

  card.addEventListener('change', (e) => {
    const s = e.target.closest('select[data-pm-site]');
    if (!s) return;
    setDecision(s.dataset.pmSite, s.dataset.pmKind, s.value);
    cardChanged = true;
  });
  card.addEventListener('click', (e) => {
    const b = e.target.closest('button');
    if (!b) return;
    if (b.dataset.pmLink) { ax.send({ cmd: 'forget_app_link', site: b.dataset.pmLink, scheme: b.dataset.pmScheme }); return; }
    switch (b.dataset.pmAct) {
      case 'reload': closeCard(); ax.send({ cmd: 'reload' }); break;
      case 'all': closeCard(); ax.openSettings('security'); break;
      case 'forget': ax.send({ cmd: 'forget_site', site: cardSite }); cardChanged = true; break;
    }
  });

  function setDecision(site, kind, value) {
    if (!known(kind)) return;
    ax.send({ cmd: 'site_permission', site, kind, decision: value === 'allow' || value === 'block' ? value : null });
  }

  // Клик мимо окошек и Esc.
  document.addEventListener('mousedown', (e) => {
    if (cardOpen() && !e.target.closest('#pmc') && !e.target.closest('#omniIc')) closeCard();
    // Переход на другую вкладку вопрос не отклоняет: Rust спрячет окошко и покажет снова,
    // когда вернётесь. Клик по странице — «не сейчас».
    if (isOpen() && !e.target.closest('#pmp, #tabs, #side')) answer('dismiss');
  }, true);
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    if (isOpen()) { e.preventDefault(); e.stopImmediatePropagation(); answer('dismiss'); }
    else if (cardOpen()) { e.preventDefault(); e.stopImmediatePropagation(); closeCard(); }
  }, true);
  const follow = () => requestAnimationFrame(() => {
    if (isOpen()) placeBelowOmni(prompt, 380);
    if (cardOpen()) placeCard();
  });
  addEventListener('resize', follow);
  if (window.ResizeObserver && $('#omni')) new ResizeObserver(follow).observe($('#omni'));

  // Повторный клик по замку закрывает карточку (openCard), клик мимо — тоже (выше).
  $('#omniIc').addEventListener('click', (e) => { e.preventDefault(); e.stopPropagation(); openCard(); });
  $('#permBtn').addEventListener('click', openCard);

  // ---------- адресная строка ----------
  // Замок кликабелен на страницах сайтов; запрещённое сайту — значок справа
  // (перечёркнутый), чтобы было видно, почему микрофон молчит, и где вернуть.
  function renderOmni(t) {
    const origin = ax.isPage(t) ? originOf(t.url) : null;
    $('#omniIc').classList.toggle('pm-click', !!origin);
    $('#omniIc').title = origin ? 'Сведения о сайте и разрешения' : '';
    const btn = $('#permBtn');
    const own = origin ? siteOf(origin) : null;
    const blocked = own ? sortKinds(Object.keys(own.perms)).filter((k) => own.perms[k] === 'block') : [];
    btn.hidden = !blocked.length;
    if (blocked.length) {
      const k = blocked[0];
      if (btn.dataset.k !== k) { btn.innerHTML = permIcon(k); btn.dataset.k = k; }
      btn.title = blocked.length === 1 ? `${PERMS[k].name}: запрещено для этого сайта` : `Запрещено для этого сайта: ${blocked.map((x) => PERMS[x].name.toLowerCase()).join(', ')}`;
    }
    if (cardOpen() && cardSite !== origin) closeCard();
  }

  // ---------- Настройки → Безопасность ----------
  function sectionHtml() {
    const seg = (k) => `<span class="seg" data-pm-def="${k}"><button data-v="ask">Спрашивать</button><button data-v="block">Запрещать</button></span>`;
    return `
      <h4>Сайты</h4>
      <div class="card pm-sites">
        <div class="srow pm-findrow" id="pmFindRow" data-kw="разрешения сайтов микрофон камера местоположение уведомления доступ"><label class="sfind pm-find">${icon('search')}<input id="pmFind" type="text" placeholder="Найти сайт" spellcheck="false"></label></div>
        <div id="pmSites"></div>
      </div>
      <div class="card pm-links" id="pmLinksCard" hidden>
        <div class="srow" data-kw="приложения ссылки telegram mailto steam всегда разрешать"><span class="sl"><b>Открывать приложения без вопроса</b><small>Сайты, которым вы разрешили запускать программы по ссылкам</small></span></div>
        <div class="ns-list" id="pmLinks"></div>
      </div>
      <h4>Не спрашивать</h4>
      <div class="card">
        ${PERM_DEFAULTS.map((k) => `<div class="srow" data-kw="не спрашивать запрещать всем ${esc(PERMS[k].ask.toLowerCase())}"><span class="sl"><b>${esc(PERMS[k].name)}</b><small>${k === 'notifications' ? 'Запрещать — и сайты перестанут спрашивать про уведомления' : 'Запрещать — и сайты не смогут даже спросить'}</small></span><span class="sc">${seg(k)}</span></div>`).join('')}
      </div>
      <div class="note">${icon('shield')}<span>Когда сайт просит микрофон, камеру или что-то ещё, Axiom спрашивает вас под адресной строкой. Решения хранятся только на этом компьютере; исключения из «Запрещать» — разрешённые вами сайты. Отозванное разрешение действует после обновления страницы, а разрешения конкретного сайта видно по клику на замок слева от адреса.</span></div>`;
  }

  function syncSheet() {
    const box = $('#pmSites', sheet);
    if (!box) return;
    const q = findQ.trim().toLowerCase();
    const sites = L.sites.slice().sort((a, b) => b.at - a.at);
    $('#pmFindRow', sheet).hidden = sites.length < 6;
    const shown = q ? sites.filter((s) => s.site.toLowerCase().includes(q)) : sites;
    box.innerHTML = !sites.length
      ? `<div class="srow pm-empty"><span class="sl"><b>Пока ни один сайт ничего не просил</b><small>Когда сайт попросит микрофон, камеру, место или уведомления, ваше решение появится здесь — его можно будет поменять</small></span></div>`
      : !shown.length ? '<div class="srow pm-empty"><span class="sl"><small>Такого сайта в списке нет</small></span></div>'
      : shown.map((s) => {
        const kinds = sortKinds(Object.keys(s.perms));
        return `<div class="pm-site"><div class="pm-shead">${ax.favHtml(s.site)}<b>${esc(hostOf(s.site))}</b><span class="grow"></span>
            <button class="btn ghost" data-pm-forget="${esc(s.site)}" title="Снова спрашивать обо всём">Забыть сайт</button></div>
          ${kinds.map((k) => `<div class="pm-prow"><span class="pm-name">${permIcon(k)}<span>${esc(PERMS[k].name)}</span></span>${decisionSelect(s.site, k, s.perms[k])}</div>`).join('')}</div>`;
      }).join('');
    const links = L.app_links.slice().sort((a, b) => b.at - a.at);
    $('#pmLinksCard', sheet).hidden = !links.length;
    $('#pmLinks', sheet).innerHTML = links.map((l) => `<div class="srow"><span class="sl"><b>${esc(hostOf(l.site))}</b><small>${esc(appName(l.scheme))}</small></span>
      <span class="sc"><button class="ib sm" data-pm-link="${esc(l.site)}" data-pm-scheme="${esc(l.scheme)}" title="Снова спрашивать перед запуском">${icon('x')}</button></span></div>`).join('');
    $$('[data-pm-def]', sheet).forEach((seg) => {
      const v = L.defaults[seg.dataset.pmDef] === 'block' ? 'block' : 'ask';
      $$('button', seg).forEach((b) => b.classList.toggle('on', b.dataset.v === v));
    });
  }

  sheet.addEventListener('input', (e) => {
    if (e.target.id === 'pmFind') { findQ = e.target.value; syncSheet(); }
  });
  sheet.addEventListener('change', (e) => {
    const s = e.target.closest('select[data-pm-site]');
    if (s) setDecision(s.dataset.pmSite, s.dataset.pmKind, s.value);
  });
  sheet.addEventListener('click', (e) => {
    const b = e.target.closest('button');
    if (!b) return;
    if (b.dataset.pmForget) { ax.send({ cmd: 'forget_site', site: b.dataset.pmForget }); return; }
    if (b.dataset.pmLink) { ax.send({ cmd: 'forget_app_link', site: b.dataset.pmLink, scheme: b.dataset.pmScheme }); return; }
    const seg = b.closest('[data-pm-def]');
    if (seg && b.dataset.v) ax.send({ cmd: 'permission_default', kind: seg.dataset.pmDef, block: b.dataset.v === 'block' });
  });

  // Из Rust: списки поменялись (или пришли при запуске).
  function onList(m) {
    const sites = Array.isArray(m.sites) ? m.sites.filter((s) => s && typeof s.site === 'string' && s.perms && typeof s.perms === 'object') : [];
    L = {
      sites: sites.map((s) => ({ site: s.site, at: +s.at || 0, perms: Object.fromEntries(Object.entries(s.perms).filter(([k, v]) => known(k) && (v === 'allow' || v === 'block'))) })),
      defaults: m.defaults && typeof m.defaults === 'object' ? m.defaults : {},
      app_links: Array.isArray(m.app_links) ? m.app_links.filter((l) => l && typeof l.site === 'string' && typeof l.scheme === 'string') : [],
    };
    syncSheet();
    if (cardOpen()) renderCard();
    renderOmni(ax.activeTab());
  }

  return { onAsk, onList, renderOmni, sectionHtml, syncSheet, isOpen, close: () => { if (isOpen()) answer('dismiss'); closeCard(); } };
}
