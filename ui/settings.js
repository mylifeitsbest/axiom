'use strict';
// Меню настроек (полноэкранный лист) и экран первого запуска.
// Вся логика темы живёт в app.js: сюда передаётся объект `ax` с доступом к ней.

// ---------- поисковые системы (те же, что в src/tabs/omnibox.rs) ----------
const SEARCH_ENGINES = [
  { id: 'duckduckgo', name: 'DuckDuckGo', note: 'Без слежки и персональной выдачи' },
  { id: 'google', name: 'Google', note: 'Самая полная выдача, но с персональной подборкой' },
  { id: 'yandex', name: 'Яндекс', note: 'Лучше всех ищет по рунету' },
  { id: 'bing', name: 'Bing', note: 'Поиск Microsoft' },
  { id: 'brave', name: 'Brave Search', note: 'Собственный индекс, без слежки' },
  { id: 'startpage', name: 'Startpage', note: 'Выдача Google без слежки' },
  { id: 'ecosia', name: 'Ecosia', note: 'На доход от рекламы сажают деревья' },
  { id: 'custom', name: 'Свой адрес', note: 'Адрес страницы результатов, %s — место запроса' },
];

// ---------- подмена для сайтов (те же, что в src/tabs/identity.rs) ----------
const AGENTS = [
  { id: 'default', name: 'Как есть', note: 'Сайты видят настоящий Axiom' },
  { id: 'chrome', name: 'Chrome', note: 'Google Chrome на Windows — самый частый браузер' },
  { id: 'firefox', name: 'Firefox', note: 'Firefox на Windows' },
  { id: 'safari', name: 'Safari', note: 'Safari на Mac' },
  { id: 'android', name: 'Android', note: 'Chrome на телефоне: сайты покажут мобильную версию' },
  { id: 'iphone', name: 'iPhone', note: 'Safari на iPhone: сайты покажут мобильную версию' },
  { id: 'custom', name: 'Свой', note: 'Любая строка латиницей, до 512 знаков' },
];
const REFERERS = [
  { id: 'default', name: 'Как есть', note: 'Как в Chrome: другим сайтам — только адрес сайта, без страницы' },
  { id: 'off', name: 'Не отправлять', note: 'Сайты не узнают, откуда вы пришли' },
  { id: 'site', name: 'С того же сайта', note: 'Каждый сайт думает, что вы пришли с него самого. Выручает, когда картинка или файл не открываются по прямой ссылке' },
  { id: 'custom', name: 'Свой адрес', note: 'Все запросы будто идут с этого адреса' },
];
// Свой User-Agent: печатная латиница до 512 знаков (как valid_agent в Rust).
const agentOk = (v) => !!v && v.length <= 512 && /^[\x20-\x7e]+$/.test(v);
// Свой Referer: адрес http(s), без схемы — https:// (как clean_referer в Rust).
function refererOk(v) {
  v = String(v || '').trim();
  if (!v || v.length > 2048) return false;
  try { const u = new URL(v.includes('://') ? v : `https://${v}`); return /^https?:$/.test(u.protocol) && !!u.hostname; } catch { return false; }
}

// ---------- файлы к «Сообщить о проблеме» (как в src/reports/files.rs) ----------
const BUG_MAX_FILES = 10;
const BUG_MAX_MB = 50;
const BUG_SLOTS = 64;
const BUG_CHUNK = 176 * 1024;
const BUG_ACCEPT = 'image/png,image/jpeg,image/gif,image/webp,image/avif,video/*,.mkv,.mov,.3gp,text/plain,.txt,.log,.json,.md,.csv';

// ---------- сайты, которые не усыпляем (как в src/tabs/sleep.rs) ----------
const MAX_SLEEP_SITES = 200;
// Что браузер отправляет сам и как это выключить (её же показывает приветствие).
const PRIVACY_URL = 'https://github.com/mylifeitsbest/axiom/blob/main/PRIVACY.md';
// Сайт для списка «не усыплять» (как sleep::site в Rust): https://www.YouTube.com/watch → youtube.com.
// Без схемы — тоже адрес, «*.» в начале — то же, что без него; одно слово (кроме localhost) — не сайт.
function sleepSite(v) {
  let s = String(v || '').trim().toLowerCase();
  if (!s || s.length > 2048 || /\s/.test(s)) return null;
  s = s.replace(/^(\*\.)+/, '').replace(/^\.+/, '');
  let u;
  try { u = new URL(/^[a-z][a-z0-9+.-]*:\/\//.test(s) ? s : `https://${s}`); } catch { return null; }
  if (!/^https?:$/.test(u.protocol) || !u.hostname) return null;
  let h = u.hostname;
  if (h.startsWith('[') || /^\d+(\.\d+){3}$/.test(h)) return h;
  h = h.replace(/\.$/, '');
  const label = /^[a-z0-9_-]{1,63}$/;
  if (h.length > 253 || !(h.includes('.') || h === 'localhost') || !h.split('.').every((l) => label.test(l))) return null;
  // www.com — сайт, а не «все сайты в .com».
  return h.startsWith('www.') && h.slice(4).includes('.') ? h.slice(4) : h;
}
const sleepSites = (b) => (Array.isArray(b?.no_sleep_sites) ? b.no_sleep_sites : []);

// Свой адрес: http(s), с %s, и запрос не может поменять сайт (как valid_template в Rust).
function searchTemplateOk(t) {
  t = String(t || '').trim();
  if (!t.includes('%s') || t.length > 1024) return false;
  const host = (fill) => {
    try { const u = new URL(t.replaceAll('%s', fill)); return /^https?:$/.test(u.protocol) ? u.hostname : null; } catch { return null; }
  };
  const a = host('a');
  return !!a && a === host('b');
}

// Как называть выбранную систему в подписях: «Запрос уходит в …».
function searchEngineName(b) {
  if (b?.search_engine === 'custom' && searchTemplateOk(b.search_custom)) {
    return new URL(b.search_custom.trim().replaceAll('%s', 'q')).hostname.replace(/^www\./, '');
  }
  return (SEARCH_ENGINES.find((e) => e.id === b?.search_engine) || SEARCH_ENGINES[0]).name;
}

// Запрос со страницы результатов любой системы или своего адреса (как search_query в Rust).
function searchQueryOf(href, custom) {
  let u;
  try { u = new URL(href); } catch { return null; }
  if (!/^https?:$/.test(u.protocol)) return null;
  const host = u.hostname.replace(/^www\./, '');
  const search = u.pathname.replace(/\/+$/, '') === '/search';
  const p = (k) => u.searchParams.get(k);
  const clean = (q) => (q && q.trim()) || null;
  const domain = (d) => host === d || host.endsWith('.' + d);
  const brand = (b) => new RegExp(`^${b}\\.[a-z]+(\\.[a-z]+)?$`).test(host) && host.length - b.length <= 7;
  if (domain('duckduckgo.com')) return clean(p('q'));
  if (search && brand('google')) return p('udm') === '50' ? null : clean(p('q'));
  if (search && (brand('yandex') || host === 'ya.ru')) return clean(p('text'));
  if (search && (domain('bing.com') || domain('ecosia.org') || host === 'search.brave.com')) return clean(p('q'));
  if (domain('startpage.com') && (search || /^\/(sp\/search|do\/d?search)$/.test(u.pathname))) return clean(p('query') || p('q'));
  if (!searchTemplateOk(custom)) return null;
  const MARK = 'axiomquerymark';
  const t = new URL(String(custom).trim().replaceAll('%s', MARK));
  if (t.hostname !== u.hostname) return null;
  const cut = (s, tpl) => {
    const i = tpl.indexOf(MARK);
    const pre = tpl.slice(0, i), post = tpl.slice(i + MARK.length);
    return s.length >= pre.length + post.length && s.startsWith(pre) && s.endsWith(post) ? s.slice(pre.length, s.length - post.length) : null;
  };
  for (const [k, v] of t.searchParams) {
    if (v.includes(MARK)) return t.pathname === u.pathname && p(k) != null ? clean(cut(p(k), v)) : null;
  }
  if (!t.pathname.includes(MARK)) return null;
  const mid = cut(u.pathname, t.pathname);
  try { return mid == null ? null : clean(decodeURIComponent(mid)); } catch { return null; }
}

function createPanels(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const { esc } = ax;

  const SECTIONS = [
    { id: 'look', icon: 'palette', title: 'Внешний вид', desc: 'Тема, цвета и шрифт. Всё меняется сразу.' },
    { id: 'layout', icon: 'layout', title: 'Раскладка', desc: 'Где живут вкладки и сколько воздуха вокруг.' },
    { id: 'search', icon: 'sparkles', title: 'Поиск и ИИ', desc: 'Поисковая система, Google AI Mode и прокси только для ИИ.' },
    { id: 'translate', icon: 'translate', title: 'Перевод', desc: 'Страницы на других языках — в пару кликов.' },
    { id: 'memory', icon: 'cpu', title: 'Память', desc: 'Спящие вкладки не занимают ни мегабайта.' },
    { id: 'data', icon: 'database', title: 'Данные', desc: 'Закладки и история из Chrome, Edge, Firefox и других браузеров.' },
    { id: 'security', icon: 'shield', title: 'Безопасность', desc: 'Что сайтам можно: микрофон, камера, место, уведомления и запуск приложений.' },
    { id: 'system', icon: 'monitor', title: 'Система', desc: 'Установка, обновления и браузер по умолчанию.' },
    { id: 'keys', icon: 'keyboard', title: 'Клавиши', desc: 'Привычные сочетания плюс пара своих.' },
  ];
  const LAYOUTS = [
    { v: 'one', name: 'Одна строка', note: 'Вкладки и адрес рядом. Максимум места.' },
    { v: 'two', name: 'Две строки', note: 'Классика: вкладки сверху, адрес под ними.' },
    { v: 'side', name: 'Сбоку', note: 'Вертикальные вкладки слева.' },
  ];
  const KEYS = [
    ['Ctrl T', 'Новая вкладка'], ['Ctrl W', 'Закрыть вкладку'], ['Ctrl Shift T', 'Вернуть закрытую'],
    ['Ctrl Tab', 'Следующая вкладка'], ['Ctrl 1…9', 'Вкладка по номеру'], ['Ctrl L', 'Адресная строка'],
    ['Ctrl K', 'Панель ИИ'], ['Ctrl Enter', 'Спросить ИИ из адресной строки'], ['Alt Enter', 'Открыть в новой вкладке'],
    ['Ctrl D', 'Закладка'], ['Ctrl H', 'История'], ['Ctrl J', 'Загрузки'], ['Ctrl F', 'Найти на странице'], ['F5', 'Обновить'], ['Alt ← →', 'Назад, вперёд'], ['Ctrl ,', 'Настройки'],
    ['Ctrl Shift R', 'Обновить без кэша'], ['Ctrl + −', 'Масштаб'], ['Ctrl 0', 'Обычный масштаб'], ['Ctrl M', 'Выключить звук вкладки'],
    ['Колесо', 'Громкость вкладки — над её значком звука'],
    ['Ctrl P', 'Печать'], ['Ctrl U', 'Исходный код страницы'], ['F12', 'Инструменты разработчика'],
  ];

  const sheet = $('#sheet');
  const onb = $('#onb');
  let built = false;
  let section = 'look';
  // Перенос из браузеров: какой идёт сейчас и чем закончился каждый.
  let importState = { busy: '', res: {} };
  let onbPick = '';
  // Значок браузера — буква, без чужих логотипов.
  const MARK = { chrome: 'C', edge: 'E', yandex: 'Я', opera: 'O', opera_gx: 'GX', brave: 'B', vivaldi: 'V', chromium: 'C', firefox: 'F', zen: 'Z' };

  // ---------- общие кусочки разметки ----------
  const kbd = (combo) => combo.split(' ').map((k) => `<kbd>${esc(k)}</kbd>`).join('');
  const toggle = (attr, key) => `<input type="checkbox" class="tg" ${attr}="${key}">`;
  const row = (title, note, control, extra = '') =>
    `<div class="srow" ${extra}><span class="sl"><b>${title}</b>${note ? `<small>${note}</small>` : ''}</span><span class="sc">${control}</span></div>`;
  // Ползунок с пузырьком значения над бегунком: виден, пока тянешь, при наведении и фокусе.
  const range = (attrs) => `<span class="rng"><input type="range" ${attrs}><output class="bub" aria-hidden="true"></output></span>`;
  const engineChips = () => `<span class="chips" role="radiogroup" aria-label="Поисковая система">${SEARCH_ENGINES.map((e) =>
    `<button class="chip" role="radio" data-engine="${e.id}">${icon('check')}<span>${e.name}</span></button>`).join('')}</span>`;
  const pickChips = (list, attr, label) => `<span class="chips" role="radiogroup" aria-label="${label}">${list.map((e) =>
    `<button class="chip" role="radio" ${attr}="${e.id}">${icon('check')}<span>${e.name}</span></button>`).join('')}</span>`;
  // «Фоновых вкладок в памяти»: 1–30, последний шаг ползунка — без ограничения (в настройках 0).
  const LIVE_MAX = 30;
  const liveToRange = (n) => (+n > 0 ? Math.min(+n, LIVE_MAX) : LIVE_MAX + 1);
  const rangeToLive = (v) => (v > LIVE_MAX ? 0 : v);
  // Что показывает пузырёк над ползунком.
  const BUBBLE = {
    hue: (v) => `${v}°`,
    tint: (v) => `${Math.round((v / 0.06) * 100)}%`,
    fontSize: (v) => `${v} px`,
    radius: (v) => `${v} px`,
    gap: (v) => `${v} px`,
    speed: (v) => (v ? `×${v}` : 'выкл'),
    aiWidth: (v) => `${v} px`,
    live_tabs: (v) => (v > LIVE_MAX ? 'без ограничения' : `${v} ${plural(v, 'вкладка', 'вкладки', 'вкладок')}`),
    sleep_minutes: (v) => (v ? `${v} мин` : 'никогда'),
  };
  // «Свой адрес» нажат, но годного шаблона ещё нет: поле открыто, система прежняя.
  let customOpen = false;
  const presetCards = (big) => Object.entries(ax.PRESETS).map(([k, p]) =>
    `<button class="preset${big ? ' big' : ''}" data-preset="${k}" data-mode="${p.mode}" data-oled="${p.oled}" style="--h:${p.hue};--c:${p.tint};--acc:${p.accent}">
      <span class="pv"><i class="a"><b></b><b></b><b></b></i><i class="b"><b></b><b></b></i><i class="c"></i></span>
      <span class="pn">${p.name}<i class="ok">${icon('check')}</i></span></button>`).join('');
  const layoutCards = () => LAYOUTS.map((l) =>
    `<button class="lcard" data-layout-pick="${l.v}"><span class="lmini" data-l="${l.v}"><i class="t"></i><i class="s"></i><i class="p"></i></span>
      <span class="ln"><b>${l.name}</b><small>${l.note}</small></span></button>`).join('');
  const swatches = () => ax.ACCENTS.map((c) => `<button class="sw" data-accent="${c}" style="--sw:${c}" title="${c}"></button>`).join('') +
    '<label class="sw own" title="Свой цвет"><input type="color" data-key="accent"></label>';
  const preview = () => `<div class="pvw" id="pvw"><div class="pvw-bar"><i class="pvw-nav"></i><span class="pvw-tabs"><i class="on"></i><i></i><i></i></span><i class="pvw-omni"></i><i class="pvw-dot"></i></div>
    <div class="pvw-main"><i class="pvw-side"><b></b><b></b><b></b></i><i class="pvw-page"><b></b><b></b><b></b><b></b></i><i class="pvw-ai"><b></b><b></b></i></div></div>`;

  // Макет окна для «Где панель»: панель хватают и тянут к верхнему или нижнему краю.
  const barPreview = () => `<span class="bpos" id="bpos" data-bar="top"><i class="bpos-zone t"></i><i class="bpos-zone b"></i>
    <i class="bpos-page"><b></b><b></b><b></b></i>
    <span class="bpos-bar" role="button" tabindex="0" aria-label="Панель. Потяните вверх или вниз, стрелки и Enter тоже переносят"><i class="n"></i><i class="tb on"></i><i class="tb"></i><i class="o"></i><i class="d"></i></span></span>`;

  // ---------- лист настроек ----------
  function build() {
    const S = {
      look: `
        ${preview()}
        <h4>Темы</h4><div class="presets">${presetCards(false)}</div>
        <h4>Цвет</h4><div class="card">
          ${row('Режим', 'Авто — как в Windows', '<span class="seg" data-key="mode"><button data-v="dark">Тёмный</button><button data-v="light">Светлый</button><button data-v="system">Авто</button></span>')}
          ${row('Акцент', 'Кнопки, ссылки, выделение', `<span class="swatches">${swatches()}</span>`)}
          ${row('Оттенок фона', '', range('class="hue" min="0" max="360" step="1" data-key="hue"'))}
          ${row('Насыщенность фона', '0 — чистый серый', range('min="0" max="0.06" step="0.002" data-key="tint"'))}
          ${row('Чистый чёрный', 'Для OLED-экранов', toggle('data-key', 'oled'))}
        </div>
        <h4>Сайты</h4><div class="card">
          ${row('Тема сайтов', 'Сайты получают тему Axiom: в тёмном режиме DuckDuckGo, GitHub, YouTube и другие сами становятся тёмными', '<span class="pill-v" id="siteScheme"></span>')}
          ${row('Тёмные сайты принудительно', 'Затемняет и сайты без своей тёмной темы. Применится после перезапуска Axiom.', toggle('data-bkey', 'force_dark'), 'id="forceDarkRow"')}
        </div>
        <h4>Текст</h4><div class="card">
          ${row('Шрифт', '', '<select data-key="font"><option value="system">Системный</option><option value="rounded">Округлый</option><option value="mono">Моноширинный</option><option value="serif">С засечками</option><option value="custom">Свой…</option></select>')}
          ${row('Название шрифта', 'Любой установленный', '<input type="text" data-key="customFont" placeholder="Inter">', 'data-show="font=custom"')}
          ${row('Размер текста', '', `${range('min="11" max="15" step="0.5" data-key="fontSize"')}<output data-out="fontSize"></output>`)}
        </div>
        <h4>Свой CSS</h4>
        <div class="card pad"><textarea data-key="customCss" spellcheck="false" placeholder=".tab.on { box-shadow: inset 0 -2px var(--acc) }"></textarea>
          <small class="css-cut" id="cssCut" hidden>Загрузки извне (url(), @import, шрифты) не применяются</small>
          <div class="btns"><button class="btn" data-act="copy">Скопировать тему</button><button class="btn" data-act="import-toggle">Вставить тему</button><button class="btn ghost" data-act="reset">Сбросить всё</button></div>
          <div class="import-box" hidden><textarea id="themeJson" spellcheck="false" placeholder='{"accent":"#ff7aa2","radius":14}'></textarea><div class="btns"><button class="btn primary" data-act="import-apply">Применить</button></div></div>
        </div>`,
      layout: `
        <div class="lcards">${layoutCards()}</div>
        <div data-show="layout!=side"><h4>Панель</h4><div class="card">
          <div class="srow bpos-row" data-kw="панель снизу внизу сверху вверху перенести переместить перетащить">
            <span class="sl"><b>Где панель</b><small>Вкладки и адресная строка — у верхнего или нижнего края окна. Потяните панель на макете или выберите здесь.</small>
              <span class="seg" data-key="barPos"><button data-v="top">Сверху</button><button data-v="bottom">Снизу</button></span></span>
            ${barPreview()}
          </div>
        </div></div>
        <h4>Форма</h4><div class="card">
          ${row('Скругления', '', `${range('min="0" max="18" step="1" data-key="radius"')}<output data-out="radius"></output>`)}
          ${row('Отступы', '0 — страница во всё окно', `${range('min="0" max="14" step="1" data-key="gap"')}<output data-out="gap"></output>`)}
        </div>
        <h4>Элементы</h4><div class="card">
          ${row('Кнопки навигации', 'Назад, вперёд, обновить', toggle('data-key', 'showNav'))}
          ${row('Кнопка ИИ', 'Панель открывается и по Ctrl+K', toggle('data-key', 'showAiBtn'))}
          ${row('Островок', 'Чёлка сверху: видео на других вкладках и загрузки', toggle('data-key', 'island'), 'id="islRow"')}
          ${row('Видео в островке', 'Навели мышь на чёлку — из неё вылетает видео той вкладки', toggle('data-key', 'islandVideo'), 'id="islVideoRow"')}
          ${row('Кнопка «Видео в окне»', 'В островке: видео в отдельное окошко, его можно таскать и закрыть', toggle('data-key', 'islandPip'), 'id="islPipRow"')}
          ${row('Островок при вкладках сбоку', 'Сверху — над страницей, в панели — над вкладками', '<span class="seg" data-key="islandSide"><button data-v="top">Сверху</button><button data-v="panel">В панели</button></span>', 'data-show="layout=side"')}
          ${row('Полный адрес', 'Иначе — только домен', toggle('data-key', 'fullUrl'))}
        </div>
        <h4>Движение</h4><div class="card">
          ${row('Скорость анимаций', '0 — выключены', `${range('min="0" max="2" step="0.25" data-key="speed"')}<output data-out="speed"></output>`)}
        </div>`,
      search: `
        <div class="card">
          <div class="srow col" data-kw="поисковик поисковая система поиск по умолчанию google yandex brave"><span class="sl"><b>Поисковая система</b><small id="seNote"></small></span>${engineChips()}</div>
          <div class="srow" id="seCustomRow" data-kw="свой адрес поиска шаблон %s"><span class="inline"><input type="text" id="seCustomIn" placeholder="https://example.com/search?q=%s" spellcheck="false"><button class="btn primary" data-act="search-custom">Применить</button></span></div>
          ${row('Двойной поиск', 'Запрос уходит в <span class="se-name">DuckDuckGo</span> и сразу в AI Mode — и из адресной строки, и из поиска на странице', toggle('data-bkey', 'double_search'))}
        </div>
        <h4>Панель ИИ</h4><div class="card">
          ${row('Прокси только для AI Mode', 'Вкладки ходят напрямую. Пусто — без прокси.', '')}
          <div class="srow"><span class="inline"><input type="text" id="proxyIn" placeholder="socks5://127.0.0.1:1080" spellcheck="false"><button class="btn primary" data-act="proxy">Применить</button></span></div>
          ${row('Ширина панели', 'Или потяните за край панели', `${range('min="280" max="900" step="10" data-key="aiWidth"')}<output data-out="aiWidth"></output>`)}
          ${row('Новый разговор при каждом открытии', 'Закрыли панель — в следующий раз AI Mode начнёт с чистого листа. Выключите, чтобы панель помнила прежний разговор', toggle('data-bkey', 'ai_new_chat'), 'data-kw="разговор чат контекст история очищать сбрасывать новый"')}
        </div>
        <div class="note">${icon('sparkles')}<span>В панели открывается только AI Mode. Ссылки из ответа, обычная выдача Google и другие сайты открываются в новой вкладке.</span></div>`,
      translate: ax.translate.sectionHtml(),
      security: ax.perms.sectionHtml(),
      memory: `
        <div class="card">
          ${row('Фоновых вкладок в памяти', '<span id="liveOut"></span>', range(`min="1" max="${LIVE_MAX + 1}" step="1" data-bkey="live_tabs"`), 'data-kw="живые вкладки лимит без ограничения сколько"')}
          ${row('Усыплять вкладки', '<span id="sleepOut"></span>', range('min="0" max="120" step="5" data-bkey="sleep_minutes"'), 'data-kw="сон спящие таймер минуты"')}
        </div>
        <h4>Сайты без сна</h4><div class="card">
          ${row('Не усыплять на сайтах', 'Их вкладки не засыпают, как отмеченные кружкой. youtube.com — это и www.youtube.com, и m.youtube.com', '', 'data-kw="не усыплять никогда всегда сайты домены список исключения сон"')}
          <div class="srow" data-kw="не усыплять сайт добавить домен"><span class="inline"><input type="text" id="nsIn" placeholder="youtube.com" spellcheck="false" maxlength="2048"><button class="btn primary" data-act="ns-add">Добавить</button></span></div>
          <div class="ns-list" id="nsList"></div>
        </div>
        <div class="note">${icon('moon')}<span>Спящая вкладка полностью выгружается из памяти — остаются адрес, заголовок и место прокрутки. Не засыпают вкладки, где играет звук или остался недописанный текст, те, что вы отметили кружкой в адресной строке, и вкладки сайтов из списка выше. Фоновые вкладки сразу переходят в экономный режим.</span></div>`,
      data: `
        <div class="card imp-card">
          <div class="imp-head"><div class="hero-ic">${icon('download')}</div>
            <div class="hero-t"><b>Перенести из другого браузера</b><small>Закладки и история последнего профиля. Браузер можно не закрывать.</small></div></div>
          <div id="impList"></div></div>
        <div class="card">
          ${row('История', 'Все страницы, которые вы открывали, с поиском и удалением. Из неё берутся подсказки адресной строки и плитки новой вкладки', '<button class="btn" data-act="history">Открыть историю</button>', 'data-kw="история посещений журнал удалить очистить ctrl+h"')}
          ${row('Убирать метки слежки из ссылок', 'utm_*, fbclid, gclid, yclid и другие: сайт не узнает, откуда вы пришли. Формы и кнопку «Назад» это не трогает.', toggle('data-bkey', 'strip_tracking'), 'data-kw="utm приватность трекинг метки слежка"')}
        </div>
        <h4>Подмена для сайтов</h4><div class="card">
          <div class="srow col" data-kw="user agent юзер агент useragent ua браузер подмена маскировка мобильная версия"><span class="sl"><b>User-Agent</b><small id="uaNote"></small></span>${pickChips(AGENTS, 'data-ua', 'User-Agent')}<code class="ua-str" id="uaStr"></code></div>
          <div class="srow" id="uaCustomRow" data-kw="свой user agent строка"><span class="inline"><input type="text" id="uaCustomIn" placeholder="Mozilla/5.0 (…)" spellcheck="false" maxlength="512"><button class="btn primary" data-act="ua-custom">Применить</button></span></div>
          <div class="srow col" id="refRow" data-kw="referer реферер referrer рефер откуда пришли подмена ссылка картинки хотлинк"><span class="sl"><b>Referer</b><small id="refNote"></small></span>${pickChips(REFERERS, 'data-ref', 'Referer')}</div>
          <div class="srow" id="refCustomRow" data-kw="свой referer адрес"><span class="inline"><input type="text" id="refCustomIn" placeholder="https://www.google.com/" spellcheck="false"><button class="btn primary" data-act="ref-custom">Применить</button></span></div>
        </div>
        <div class="note">${icon('globe')}<span id="idNote"></span></div>
        <div class="note">${icon('lock')}<span>Пароли и входы на сайты не переносятся: браузеры шифруют их так, что прочитать их может только сам браузер. Войдите на нужные сайты один раз — Axiom предложит сохранить пароль.</span></div>`,
      system: `
        <div class="card">
          <div class="srow" id="instRow"><span class="sl"><b>Установка</b><small id="instNote"></small></span><span class="sc"><button class="btn primary" data-act="install" id="instBtn">Установить</button></span></div>
          <div class="srow"><span class="sl"><b>Браузер по умолчанию</b><small id="defNote"></small></span><span class="sc"><button class="btn" data-act="default" id="defBtn">Открыть параметры</button></span></div>
          ${row('Отрисовка страниц', 'Если страницы мигают, когда двигаете окно, или Axiom закрывается сам (чаще на NVIDIA). «Без DMA-BUF» и «Без ускорения» — крайние меры: страницы рисует процессор, и на видео (например, YouTube) Axiom может вылетать. После трёх вылетов подряд вернётся «Авто». Применится после перезапуска Axiom', '<select data-bkey="gpu"><option value="auto">Авто</option><option value="no_dmabuf">Без DMA-BUF (может вылетать на видео)</option><option value="no_compositing">Без ускорения (может вылетать на видео)</option><option value="off">Ничего не менять</option></select>', 'id="gpuRow" hidden data-kw="nvidia мигание видеокарта wayland hyprland dmabuf видео вылет краш youtube"')}
        </div>
        <div class="card">
          <div class="srow" id="updRow"><span class="sl"><b>Обновления</b><small id="updNote"></small></span><span class="sc"><button class="btn ghost" data-act="notes" id="notesSet">Что нового</button><button class="btn" data-act="update" id="updSet">Проверить</button></span></div>
          ${row('Проверять автоматически', 'Раз в несколько часов Axiom узнаёт у GitHub номер последней версии', toggle('data-bkey', 'check_updates'))}
          ${row('Бета-версии', 'Исправления и новинки сразу, не дожидаясь выхода версии. Возможны ошибки. Выключите — со следующей версии вернётесь на обычные', toggle('data-bkey', 'beta'), 'id="betaRow" hidden data-kw="бета тест beta ранние сборки"')}
        </div>
        <div class="card">
          ${row('Статистика использования', 'Раз в час Axiom сообщает разработчику номер версии, систему и запускали ли его сегодня, на этой неделе, в этом месяце. Без адресов, истории и номера браузера — по этим данным не узнать, кто вы', toggle('data-bkey', 'usage_stats'), 'data-kw="статистика телеметрия пинг счётчик приватность конфиденциальность данные"')}
          ${row('Что отправляет Axiom', 'Всё, что браузер отправляет сам, куда и как это выключить', '<button class="btn" data-act="privacy">Политика конфиденциальности</button>', 'data-kw="приватность конфиденциальность политика данные privacy телеметрия"')}
          ${row('Отправлять отчёты об ошибках', 'Автоматически отправлять разработчику отчёт, если Axiom вылетит. Без адресов страниц и личных данных.', toggle('data-bkey', 'crash_reports'), 'data-kw="отчёт ошибка вылет краш crash баг падение"')}
          <div class="srow" data-kw="баг ошибка отчёт проблема жалоба не работает ответ разработчика сообщения скриншот снимок экрана фото видео файл"><span class="sl"><b>Сообщить о проблеме</b><small>Что-то сломалось или ведёт себя странно — расскажите, это уйдёт разработчику</small></span><span class="sc"><button class="btn" data-act="bug-open" id="bugOpen">Сообщить о проблеме</button></span></div>
          <div class="bug-box" id="bugBox" hidden>
            <textarea id="bugText" maxlength="4000" placeholder="Что не работает?"></textarea>
            <div class="bug-files" id="bugFiles" hidden></div>
            <div class="bug-attach"><button class="btn" data-act="bug-attach">${icon('clip')}Приложить файлы</button><small id="bugFilesNote">Снимки экрана, фото, видео, текст — до ${BUG_MAX_FILES} файлов и ${BUG_MAX_MB} МБ. Можно перетащить сюда или вставить снимок через Ctrl+V</small></div>
            <input type="file" id="bugPick" multiple hidden accept="${BUG_ACCEPT}">
            <label class="bug-log"><input type="checkbox" class="tg" id="bugLog" checked><span class="sl"><b>Приложить журнал</b><small id="bugLogNote">Последние записи Axiom о его работе</small></span></label>
            <div class="btns"><button class="btn primary" data-act="bug-send" id="bugSend">Отправить</button><button class="btn ghost" data-act="bug-cancel">Отмена</button><small class="bug-hint">Если разработчик ответит, ответ появится здесь же, в Axiom</small></div>
          </div>
          <div class="msgs" id="msgList" hidden></div>
        </div>
        <div class="note">${icon('monitor')}<span id="sysNote"></span></div>
        <div class="about">${logoSvg(44, 'about')}<span><b>Axiom</b><small id="verNote"></small></span></div>`,
      keys: `<div class="card keys">${KEYS.map(([k, v]) => `<div class="krow"><span>${v}</span><span>${kbd(k)}</span></div>`).join('')}</div>`,
    };

    sheet.innerHTML = `<div class="sheet-card">
      <nav class="snav">
        <div class="snav-head">${logoSvg(30, 'nav')}<span><b>Настройки</b><small>Axiom</small></span></div>
        <label class="sfind">${icon('search')}<input id="setFind" type="text" placeholder="Найти настройку" spellcheck="false"></label>
        ${SECTIONS.map((s) => `<button class="snav-b" data-sec="${s.id}">${icon(s.icon)}<span>${s.title}</span></button>`).join('')}
        <span class="grow"></span>
        <span class="snav-foot">${kbd('Esc')} закрыть</span>
      </nav>
      <div class="sbody">
        <header class="shead"><span><h2 id="secTitle"></h2><p id="secDesc"></p></span><button class="ib" data-act="close" title="Закрыть (Esc)">${icon('x')}</button></header>
        <div class="sscroll" id="sscroll">${SECTIONS.map((s) => `<section class="ssec" data-sec="${s.id}"><h3 class="ssec-t">${s.title}</h3>${S[s.id]}</section>`).join('')}
          <div class="nothing" id="nothing">Ничего не нашлось</div></div>
      </div></div>`;
    built = true;
    bindSheet();
    show(section);
    sync();
  }

  function show(id) {
    section = id;
    const s = SECTIONS.find((x) => x.id === id) || SECTIONS[0];
    $$('.snav-b', sheet).forEach((b) => b.classList.toggle('on', b.dataset.sec === s.id));
    $$('.ssec', sheet).forEach((x) => { x.hidden = x.dataset.sec !== s.id; x.classList.toggle('in', x.dataset.sec === s.id); });
    $('#secTitle').textContent = s.title;
    $('#secDesc').textContent = s.desc;
    sheet.classList.remove('searching');
    $('#nothing').hidden = true;
    $('#sscroll').scrollTop = 0;
  }

  // Поиск по настройкам: показываем подходящие строки из всех разделов.
  function filter(q) {
    q = q.trim().toLowerCase();
    if (!q) { $$('.srow,.card,h4,.lcards,.presets,.pvw,.note,.krow', sheet).forEach((e) => e.classList.remove('miss')); show(section); return; }
    sheet.classList.add('searching');
    $('#secTitle').textContent = 'Поиск';
    $('#secDesc').textContent = `«${q}»`;
    let any = false;
    // В data-kw — слова, которых нет в тексте строки, но которыми её ищут.
    const text = (r) => `${r.textContent} ${r.dataset.kw || ''}`.toLowerCase();
    $$('.ssec', sheet).forEach((sec) => {
      let hit = false;
      $$('.srow,.krow', sec).forEach((r) => { const m = text(r).includes(q); r.classList.toggle('miss', !m); hit ||= m && !r.hidden; });
      $$('.card', sec).forEach((c) => c.classList.toggle('miss', !$$('.srow:not(.miss),.krow:not(.miss)', c).length && !c.textContent.toLowerCase().includes(q)));
      $$('h4,.lcards,.presets,.pvw,.note', sec).forEach((e) => e.classList.add('miss'));
      if (!hit && sec.querySelector('.hero-card')?.textContent.toLowerCase().includes(q)) { hit = true; sec.querySelector('.hero-card').classList.remove('miss'); }
      sec.hidden = !hit;
      any ||= hit;
    });
    $('#nothing').hidden = any;
  }

  function sync() {
    if (built) { syncScope(sheet); renderMsgs(); }
    if (onb.classList.contains('on')) syncScope(onb);
    const ds = document.getElementById('dsBtn');
    if (ds) {
      const name = searchEngineName(ax.B);
      ds.title = ax.B.double_search ? `Двойной поиск включён: ${name} + ИИ` : `Двойной поиск выключен: только ${name}`;
    }
    if (built) {
      const sys = ax.system();
      const win = sys.platform === 'windows';
      $('#instRow').hidden = !sys.can_install;
      $('#instNote').textContent = sys.installed ? 'Установлен в профиль пользователя, есть в меню «Пуск»' : 'Сейчас запущена переносная копия. Установка скопирует Axiom в профиль и добавит в «Пуск».';
      $('#instBtn').textContent = sys.installed ? 'Обновить регистрацию' : 'Установить';
      $('#defNote').textContent = sys.default_browser ? 'Axiom — браузер по умолчанию' : 'Сейчас по умолчанию другой браузер';
      $('#defBtn').textContent = win ? 'Открыть параметры' : 'Сделать основным';
      $('#sysNote').textContent = win
        ? 'Windows не разрешает программам самим становиться браузером по умолчанию: Axiom откроет нужную страницу параметров, там достаточно выбрать Axiom.'
        : 'Axiom установлен пакетом (apt или pacman) и обновляется вместе с системой. Браузер по умолчанию меняется через xdg-settings.';
      syncUpdate(sys);
      $('#verNote').textContent = `Версия ${sys.version || ''} · Rust + ${win ? 'WebView2' : sys.platform === 'linux' ? 'WebKitGTK' : 'WebKit'}`;
      syncImportList(sys.browsers || []);
      const pv = $('#pvw');
      if (pv) { pv.dataset.l = ax.A.layout; pv.dataset.bar = ax.A.barPos === 'bottom' ? 'bottom' : 'top'; }
      $('#cssCut').hidden = !ax.cssCut();
      $('#siteScheme').textContent = ax.A.mode === 'system' ? 'Как в системе' : document.documentElement.dataset.mode === 'light' ? 'Светлая' : 'Тёмная';
      $('#forceDarkRow').hidden = sys.platform !== 'windows';
      $('#gpuRow').hidden = sys.platform !== 'linux';
      $('#islVideoRow').hidden = !win && sys.platform !== 'linux';
      // «Картинки в картинке» в WebKitGTK нет.
      $('#islPipRow').hidden = !win;
      ax.translate.syncSheet();
      ax.perms.syncSheet();
    }
  }

  function syncUpdate(sys) {
    const u = ax.update();
    const b = $('#updSet');
    const checking = u?.phase === 'checking';
    b.disabled = checking;
    b.classList.toggle('primary', !!u?.latest);
    // Бета-версии есть для Windows и Arch; на остальных системах переключатель не нужен.
    $('#betaRow').hidden = !u?.beta_available;
    $('#updRow').classList.toggle('has', !!u?.latest);
    if (u?.latest) {
      b.textContent = 'Обновить';
      $('#updNote').textContent = `Доступна версия ${u.latest}, у вас ${u.current}`;
      return;
    }
    b.textContent = checking ? 'Проверяю…' : 'Проверить';
    const current = u?.current || sys.version || '';
    $('#updNote').textContent = checking ? 'Спрашиваю у GitHub…'
      : u?.checked === 'ok' ? `У вас последняя версия — ${current}`
      : u?.checked === 'failed' ? `Не удалось проверить: ${u.check_error}`
      : `Установлена версия ${current}`;
  }

  function syncScope(box) {
    const A = ax.A;
    const B = ax.B;
    $$('[data-key]', box).forEach((el) => {
      const v = A[el.dataset.key];
      if (el.classList.contains('seg')) $$('button', el).forEach((b) => b.classList.toggle('on', b.dataset.v === String(v)));
      else if (el.type === 'checkbox') el.checked = !!v;
      else if (document.activeElement !== el) el.value = v ?? '';
    });
    $$('[data-bkey]', box).forEach((el) => {
      const v = B[el.dataset.bkey];
      if (el.type === 'checkbox') el.checked = !!v;
      else el.value = el.dataset.bkey === 'live_tabs' ? liveToRange(v) : v;
    });
    $$('input[type=range]', box).forEach(syncRange);
    $$('[data-out]', box).forEach((el) => {
      const k = el.dataset.out;
      el.textContent = k === 'speed' ? (ax.speed() ? `×${ax.speed()}` : 'выкл') : k === 'fontSize' ? `${A[k]}px` : String(A[k]);
    });
    const proxy = $('#proxyIn', box);
    if (proxy && document.activeElement !== proxy) proxy.value = B.ai_proxy || '';
    $$('[data-preset]', box).forEach((b) => b.classList.toggle('on', b.dataset.preset === A.preset));
    $$('[data-accent]', box).forEach((b) => b.classList.toggle('on', b.dataset.accent.toLowerCase() === String(A.accent).toLowerCase()));
    $$('[data-layout-pick]', box).forEach((b) => b.classList.toggle('on', b.dataset.layoutPick === A.layout));
    // data-show="ключ=значение" — строка видна при этом значении, «ключ!=значение» — при любом другом.
    $$('[data-show]', box).forEach((el) => {
      const [k, v] = el.dataset.show.split('=');
      const not = k.endsWith('!');
      el.hidden = (String(A[not ? k.slice(0, -1) : k]) === v) === not;
    });
    const bp = $('#bpos', box);
    if (bp && !bp.classList.contains('drag')) bp.dataset.bar = A.barPos === 'bottom' ? 'bottom' : 'top';
    const so = $('#sleepOut', box);
    if (so) { const m = +B.sleep_minutes; so.textContent = m ? `Через ${m} мин без активности` : 'Никогда'; }
    const lo = $('#liveOut', box);
    if (lo) {
      const n = +B.live_tabs;
      lo.textContent = n ? `Не больше ${n}, лишние засыпают сразу`
        : +B.sleep_minutes ? 'Без ограничения: вкладки засыпают только по таймеру' : 'Без ограничения';
    }
    syncSleepSites(box);
    syncSearch(box);
    syncIdentity(box);
  }

  // Заливка до бегунка и пузырёк над ним.
  function syncRange(r) {
    const p = (r.value - r.min) / (r.max - r.min) || 0;
    r.style.setProperty('--fill', `${p * 100}%`);
    const wrap = r.parentElement;
    if (!wrap?.classList.contains('rng')) return;
    wrap.style.setProperty('--p', p);
    const fmt = BUBBLE[r.dataset.key || r.dataset.bkey];
    wrap.querySelector('.bub').textContent = fmt ? fmt(+r.value) : r.value;
  }

  // Сайты без сна: свежие сверху, у каждого крестик «снова усыплять».
  let nsKey = '';
  function syncSleepSites(box) {
    const el = $('#nsList', box);
    if (!el) return;
    const list = sleepSites(ax.B);
    const key = JSON.stringify(list);
    if (key === nsKey) return;
    nsKey = key;
    el.innerHTML = list.length
      ? list.slice().reverse().map((s) => row(esc(s), '', `<button class="ib sm" data-ns-rm="${esc(s)}" title="Снова усыплять на этом сайте">${icon('x')}</button>`)).join('')
      : row('Пока ни одного сайта', 'Или правой кнопкой по вкладке — «Не усыплять на этом сайте»', '');
  }
  function addSleepSite() {
    const input = $('#nsIn');
    const v = input.value.trim();
    const site = sleepSite(v);
    input.classList.toggle('bad', !site);
    if (!site) { ax.toast(v ? 'Не похоже на адрес сайта' : 'Впишите сайт, например youtube.com'); input.focus(); return; }
    const list = sleepSites(ax.B);
    const has = list.find((s) => site === s || site.endsWith(`.${s}`));
    if (has) {
      input.value = '';
      ax.toast(has === site ? `${site} уже в списке` : `${site} и так не засыпает: в списке есть ${has}`);
      return;
    }
    if (list.length >= MAX_SLEEP_SITES) { ax.toast(`Не больше ${MAX_SLEEP_SITES} сайтов: уберите лишние`); return; }
    ax.B.no_sleep_sites = [...list, site];
    input.value = '';
    ax.saveSettings(true);
    ax.refresh();
    ax.toast(`Вкладки ${site} не будут засыпать`);
  }
  function removeSleepSite(site) {
    ax.B.no_sleep_sites = sleepSites(ax.B).filter((s) => s !== site);
    ax.saveSettings(true);
    ax.refresh();
  }

  // Выбранная поисковая система: пилюли, поле своего адреса и подписи с её именем.
  function syncSearch(box) {
    const B = ax.B;
    const id = SEARCH_ENGINES.some((e) => e.id === B.search_engine) ? B.search_engine : 'duckduckgo';
    const custom = id === 'custom' || customOpen;
    // Пока свой адрес не применён, выбранной остаётся прежняя система, а «Свой адрес» лишь обведён.
    $$('[data-engine]', box).forEach((b) => {
      const on = b.dataset.engine === id;
      b.classList.toggle('on', on);
      b.classList.toggle('open', !on && custom && b.dataset.engine === 'custom');
      b.setAttribute('aria-checked', String(on));
    });
    const name = searchEngineName(B);
    $$('.se-name', box).forEach((el) => { el.textContent = name; });
    const note = $('#seNote', box);
    if (note) note.textContent = SEARCH_ENGINES.find((e) => e.id === (custom ? 'custom' : id)).note;
    const rowC = $('#seCustomRow', box);
    if (rowC) rowC.hidden = !custom;
    const input = $('#seCustomIn', box);
    if (input && document.activeElement !== input && !input.classList.contains('bad')) input.value = B.search_custom || '';
  }

  // Подмена для сайтов: пилюли User-Agent и Referer, поля своих значений, подписи.
  // Как со своим поиском: пока своё значение не применено, выбранным остаётся прежнее.
  let uaOpen = false;
  let refOpen = false;
  function syncChoice(box, attr, list, value, open) {
    const id = list.some((e) => e.id === value) ? value : 'default';
    const custom = id === 'custom' || open;
    $$(`[${attr}]`, box).forEach((b) => {
      const v = b.getAttribute(attr);
      const on = v === id;
      b.classList.toggle('on', on);
      b.classList.toggle('open', !on && custom && v === 'custom');
      b.setAttribute('aria-checked', String(on));
    });
    return { id, custom, note: list.find((e) => e.id === (custom ? 'custom' : id)).note };
  }
  function syncIdentity(box) {
    const uaNote = $('#uaNote', box);
    if (!uaNote) return;
    const B = ax.B;
    const ua = syncChoice(box, 'data-ua', AGENTS, B.user_agent, uaOpen);
    const ref = syncChoice(box, 'data-ref', REFERERS, B.referer, refOpen);
    uaNote.textContent = ua.note;
    $('#refNote', box).textContent = ref.note;
    $('#uaCustomRow', box).hidden = !ua.custom;
    const sys = ax.system();
    const agents = Object.fromEntries(sys.agents || []);
    // Что именно увидят сайты; пока своё значение не применено — не показываем.
    const str = ua.id === 'custom' ? B.user_agent_custom : agents[ua.id] || '';
    const code = $('#uaStr', box);
    code.textContent = str;
    code.hidden = !str || (ua.custom && ua.id !== 'custom');
    // Referer меняют перехват запросов WebView2 и расширение веб-процесса WebKitGTK.
    const spoof = sys.platform === 'windows' || sys.platform === 'linux';
    $('#refRow', box).hidden = !spoof;
    $('#refCustomRow', box).hidden = !spoof || !ref.custom;
    $('#idNote', box).textContent = spoof
      ? 'Только для вкладок: панель ИИ и сам Axiom не меняются. Новые запросы идут с подменой сразу, открытые страницы получат новый User-Agent после обновления. Если сайт перестал пускать или показывать картинки — верните «Как есть».'
      : 'Только для вкладок: панель ИИ и сам Axiom не меняются. Открытые страницы получат новый User-Agent после обновления.';
    for (const [id, v] of [['uaCustomIn', B.user_agent_custom], ['refCustomIn', B.referer_custom]]) {
      const input = $(`#${id}`, box);
      if (document.activeElement !== input && !input.classList.contains('bad')) input.value = v || '';
    }
  }
  function pickAgent(id) {
    uaOpen = id === 'custom';
    if (uaOpen && !agentOk(ax.B.user_agent_custom)) { sync(); $('#uaCustomIn')?.focus(); return; }
    ax.B.user_agent = id;
    ax.saveSettings(true);
    ax.refresh();
  }
  function pickReferer(id) {
    refOpen = id === 'custom';
    if (refOpen && !refererOk(ax.B.referer_custom)) { sync(); $('#refCustomIn')?.focus(); return; }
    ax.B.referer = id;
    ax.saveSettings(true);
    ax.refresh();
  }
  function applyCustomAgent() {
    const input = $('#uaCustomIn');
    const v = input.value.trim();
    const ok = agentOk(v);
    input.classList.toggle('bad', !ok);
    if (!ok) { ax.toast(v ? 'Только латиница, цифры и знаки, до 512' : 'Впишите строку User-Agent'); input.focus(); return; }
    Object.assign(ax.B, { user_agent_custom: v, user_agent: 'custom' });
    uaOpen = true;
    ax.saveSettings(true);
    ax.refresh();
    input.blur();
    ax.toast('Свой User-Agent применён');
  }
  function applyCustomReferer() {
    const input = $('#refCustomIn');
    const v = input.value.trim();
    const ok = refererOk(v);
    input.classList.toggle('bad', !ok);
    if (!ok) { ax.toast('Нужен адрес сайта, например https://www.google.com/'); input.focus(); return; }
    Object.assign(ax.B, { referer_custom: v, referer: 'custom' });
    refOpen = true;
    ax.saveSettings(true);
    ax.refresh();
    input.blur();
    ax.toast('Свой Referer применён');
  }

  function pickEngine(id) {
    if (id === 'custom') {
      customOpen = true;
      if (searchTemplateOk(ax.B.search_custom)) { setEngine('custom'); return; }
      sync();
      $('#seCustomIn')?.focus();
      return;
    }
    customOpen = false;
    setEngine(id);
  }
  function setEngine(id) {
    ax.B.search_engine = id;
    ax.saveSettings(true);
    ax.refresh();
  }
  function applyCustomSearch() {
    const input = $('#seCustomIn');
    const v = input.value.trim();
    const ok = searchTemplateOk(v);
    input.classList.toggle('bad', !ok);
    if (!ok) {
      ax.toast(!v.includes('%s') ? 'Поставьте %s на место запроса' : 'Нужен адрес сайта, начиная с https://');
      input.focus();
      return;
    }
    ax.B.search_custom = v;
    customOpen = true;
    setEngine('custom');
    input.blur();
    ax.toast(`Поиск: ${searchEngineName(ax.B)}`);
  }

  // ---------- общие обработчики для листа и приветствия ----------
  const readVal = (el) => (el.type === 'checkbox' ? el.checked : el.type === 'range' ? parseFloat(el.value) : el.value);

  function onInput(e) {
    const el = e.target;
    if (el.id === 'setFind') { filter(el.value); return; }
    if (['seCustomIn', 'uaCustomIn', 'refCustomIn', 'nsIn'].includes(el.id)) { el.classList.remove('bad'); return; }
    if (el.dataset.key) ax.setA(el.dataset.key, readVal(el));
    else if (el.dataset.bkey) {
      const v = readVal(el);
      ax.B[el.dataset.bkey] = el.dataset.bkey === 'live_tabs' ? rangeToLive(v) : v;
      ax.saveSettings();
      ax.refresh();
      if (el.dataset.bkey === 'gpu' && (el.value === 'no_dmabuf' || el.value === 'no_compositing')) ax.toast('Перезапустите Axiom, чтобы применить. Начнёт вылетать на видео — верните «Авто»');
      else if (el.dataset.bkey === 'force_dark' || el.dataset.bkey === 'gpu') ax.toast('Перезапустите Axiom, чтобы применить');
    }
  }

  function onClick(e) {
    const b = e.target.closest('button');
    if (!b) return;
    if (b.dataset.sec) { $('#setFind').value = ''; filter(''); show(b.dataset.sec); return; }
    if (b.dataset.preset) {
      const p = ax.PRESETS[b.dataset.preset];
      ax.swap(() => { Object.assign(ax.A, { mode: p.mode, hue: p.hue, tint: p.tint, accent: p.accent, oled: p.oled, preset: b.dataset.preset }); ax.applyAppearance(); }, e);
      ax.saveSettings();
      return;
    }
    if (b.dataset.layoutPick) { ax.swap(() => { ax.A.layout = b.dataset.layoutPick; ax.applyAppearance(); }, e); ax.saveSettings(); return; }
    const seg = b.closest('.seg');
    if (seg?.dataset.key && b.dataset.v) {
      const key = seg.dataset.key;
      ax.swap(() => { ax.A[key] = b.dataset.v; if (key === 'mode') ax.A.preset = 'custom'; ax.applyAppearance(); }, e);
      ax.saveSettings();
      return;
    }
    if (b.dataset.accent) { ax.setA('accent', b.dataset.accent); return; }
    if (b.dataset.engine) { pickEngine(b.dataset.engine); return; }
    if (b.dataset.ua) { pickAgent(b.dataset.ua); return; }
    if (b.dataset.ref) { pickReferer(b.dataset.ref); return; }
    if (b.dataset.nsRm) { removeSleepSite(b.dataset.nsRm); return; }
    switch (b.dataset.act) {
      case 'close': close(); break;
      case 'search-custom': applyCustomSearch(); break;
      case 'ua-custom': applyCustomAgent(); break;
      case 'ref-custom': applyCustomReferer(); break;
      case 'ns-add': addSleepSite(); break;
      case 'proxy': applyProxy(b.closest('.card, .step') || sheet); break;
      case 'copy': ax.copyText(JSON.stringify(ax.A, null, 2)); ax.toast('Тема скопирована'); break;
      case 'import-toggle': { const box = $('.import-box', sheet); box.hidden = !box.hidden; break; }
      case 'import-apply': importTheme(); break;
      case 'reset': ax.swap(() => { ax.resetAppearance(); }, e); ax.saveSettings(); ax.toast('Настройки вида сброшены'); break;
      case 'import': startImport(b.dataset.browser || onbPick); break;
      case 'install': ax.send({ cmd: 'install' }); b.classList.add('busy'); break;
      case 'default': ax.send({ cmd: 'open_default_apps' }); break;
      case 'notes': ax.openNotes(); break;
      case 'update':
        if (ax.update()?.latest) ax.openUpdate();
        else ax.send({ cmd: 'update_check' });
        break;
      case 'history': ax.openHistory(); break;
      case 'privacy': ax.send({ cmd: 'new_tab', input: PRIVACY_URL }); close(); break;
      case 'bug-open': toggleBug(); break;
      case 'bug-cancel': toggleBug(false); break;
      case 'bug-send': sendBug(); break;
      case 'bug-attach': $('#bugPick').click(); break;
      case 'bug-unattach': unattach(+b.dataset.slot); break;
      default: break;
    }
  }

  // ---------- «Сообщить о проблеме» ----------
  // Журнал читает и чистит Rust: интерфейс знает только, сколько его уйдёт.
  function toggleBug(on) {
    const box = $('#bugBox');
    box.hidden = on === undefined ? !box.hidden : !on;
    if (box.hidden) return;
    ax.send({ cmd: 'report_info' });
    // Форма целиком на виду, с кнопкой «Отправить».
    requestAnimationFrame(() => {
      $('#bugText')?.focus({ preventScroll: true });
      box.scrollIntoView({ block: 'nearest', behavior: ax.speed() ? 'smooth' : 'auto' });
    });
  }
  function sendBug() {
    const input = $('#bugText');
    const text = input.value.trim();
    // Ctrl+Enter, пока отчёт ещё уходит, не шлёт его второй раз.
    if ($('#bugSend').classList.contains('busy')) return;
    if (!text) { ax.toast('Опишите, что не работает'); input.focus(); return; }
    $('#bugSend').classList.add('busy');
    // Файлы ещё переписываются в Axiom — отчёт уйдёт, как только они допишутся.
    if (bugFiles.some((f) => f.state === 'wait' || f.state === 'copy')) { bugSendLater = true; return; }
    const files = bugFiles.filter((f) => f.state === 'ok').map((f) => ({ slot: f.slot, name: f.name }));
    ax.send({ cmd: 'report', text, log: $('#bugLog').checked, files });
  }

  // Файлы к отчёту. Интерфейс читает их сам и передаёт в Rust кусками (base64):
  // следующий кусок — после ответа на предыдущий. Rust ещё раз проверяет тип по
  // первым байтам, а перед отправкой убирает у фото и видео место съёмки.
  let bugFiles = [];
  let bugSlot = 0;
  let bugCopying = null;
  let bugSendLater = false;
  function bugKind(f) {
    const ext = (f.name.match(/\.([^.]+)$/)?.[1] || '').toLowerCase();
    const type = (f.type || '').toLowerCase();
    if (/heic|heif/.test(type) || ext === 'heic' || ext === 'heif') return 'heic';
    if (/^image\/(png|jpeg|gif|webp|avif)$/.test(type) || /^(png|jpe?g|gif|webp|avif)$/.test(ext)) return 'image';
    if (/^video\//.test(type) || /^(mp4|m4v|mov|webm|mkv|3gp)$/.test(ext)) return 'video';
    if (/^text\//.test(type) || /^(txt|log|json|md|csv)$/.test(ext)) return 'text';
    return '';
  }
  function attach(list) {
    const box = $('#bugBox');
    if (!list?.length || !box || box.hidden) return;
    let total = bugFiles.reduce((n, f) => n + f.size, 0);
    for (const file of list) {
      const kind = bugKind(file);
      let why = '';
      if (bugFiles.length >= BUG_MAX_FILES) why = `Не больше ${BUG_MAX_FILES} файлов`;
      else if (kind === 'heic') why = 'HEIC не открыть в браузере — сохраните фото как JPG или PNG';
      else if (!kind) why = `«${file.name}» не приложить: подойдут фото, снимки экрана, видео и текст`;
      else if (!file.size) why = `«${file.name}» пустой`;
      else if (total + file.size > BUG_MAX_MB * 1024 * 1024) why = `Файлы вместе — не больше ${BUG_MAX_MB} МБ`;
      if (why) { ax.toast(why); if (bugFiles.length >= BUG_MAX_FILES) break; continue; }
      // Место по кругу: ответ про убранный файл не спутать с новым на том же месте.
      while (bugFiles.some((f) => f.slot === bugSlot)) bugSlot = (bugSlot + 1) % BUG_SLOTS;
      // Снимок из буфера обмена приходит как image.png.
      const name = /^image\.(png|jpe?g|gif|webp)$/i.test(file.name) ? `Снимок экрана.${file.name.split('.').pop()}` : file.name;
      bugFiles.push({ slot: bugSlot, file, name, size: file.size, kind, sent: 0, state: 'wait', url: kind === 'text' ? '' : URL.createObjectURL(file) });
      bugSlot = (bugSlot + 1) % BUG_SLOTS;
      total += file.size;
    }
    renderBugFiles();
    pumpBugFiles();
  }
  function unattach(slot) {
    const f = bugFiles.find((x) => x.slot === slot);
    if (!f) return;
    if (f.url) URL.revokeObjectURL(f.url);
    bugFiles = bugFiles.filter((x) => x !== f);
    if (bugCopying === f) bugCopying = null;
    if (f.state !== 'wait') ax.send({ cmd: 'report_file_drop', slot });
    renderBugFiles();
    pumpBugFiles();
  }
  function clearBugFiles() {
    for (const f of bugFiles) if (f.url) URL.revokeObjectURL(f.url);
    bugFiles = [];
    bugCopying = null;
    renderBugFiles();
  }
  const b64 = (bytes) => {
    let s = '';
    for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(s);
  };
  async function pumpBugFiles() {
    if (bugCopying) return;
    const f = bugFiles.find((x) => x.state === 'wait');
    if (!f) {
      if (bugSendLater) { bugSendLater = false; $('#bugSend')?.classList.remove('busy'); sendBug(); }
      return;
    }
    bugCopying = f;
    f.state = 'copy';
    sendBugChunk(f);
  }
  async function sendBugChunk(f) {
    let bytes;
    try {
      bytes = new Uint8Array(await f.file.slice(f.sent, f.sent + BUG_CHUNK).arrayBuffer());
    } catch {
      onReportFile({ slot: f.slot, error: `«${f.name}» не прочитать — может, его переместили` });
      return;
    }
    if (bugCopying !== f) return;
    f.expect = f.sent + bytes.length;
    ax.send({ cmd: 'report_file', slot: f.slot, offset: f.sent, data: b64(bytes) });
  }
  function onReportFile(m) {
    const f = bugCopying;
    if (!f || f.slot !== m.slot) return;
    if (m.error) {
      bugCopying = null;
      if (f.url) URL.revokeObjectURL(f.url);
      bugFiles = bugFiles.filter((x) => x !== f);
      ax.toast(m.error);
      // Файл не принят — и отчёт с ним не уйдёт сам: человек решит, отправлять ли без него.
      if (bugSendLater) { bugSendLater = false; $('#bugSend')?.classList.remove('busy'); }
      renderBugFiles();
      pumpBugFiles();
      return;
    }
    if (m.size !== f.expect) return;
    f.sent = m.size;
    if (f.sent >= f.size) { f.state = 'ok'; bugCopying = null; }
    renderBugFile(f);
    if (bugCopying === f) sendBugChunk(f);
    else pumpBugFiles();
  }
  function bugThumb(f) {
    if (f.kind === 'image') return `<img src="${esc(f.url)}" alt="">`;
    if (f.kind === 'video') return `<video src="${esc(f.url)}" muted preload="metadata"></video><span class="play">${icon('play')}</span>`;
    return icon('file');
  }
  function renderBugFiles() {
    const wrap = $('#bugFiles');
    if (!wrap) return;
    wrap.hidden = !bugFiles.length;
    wrap.innerHTML = bugFiles.map((f) => `<div class="bug-file" data-slot="${f.slot}">
        <span class="th">${bugThumb(f)}</span>
        <span class="nm"><b title="${esc(f.name)}">${esc(f.name)}</b><small></small></span>
        <button class="ib sm" data-act="bug-unattach" data-slot="${f.slot}" title="Убрать">${icon('x')}</button>
        <span class="bar"><i></i></span></div>`).join('')
      + (bugFiles.length ? '<p class="bug-files-note">У фото и видео Axiom уберёт место съёмки и данные камеры. То, что видно на самом снимке, уйдёт как есть.</p>' : '');
    for (const f of bugFiles) renderBugFile(f);
    const note = $('#bugFilesNote');
    if (note) {
      const left = BUG_MAX_FILES - bugFiles.length;
      const used = bugFiles.reduce((n, x) => n + x.size, 0);
      note.textContent = bugFiles.length
        ? `${bugFiles.length} из ${BUG_MAX_FILES}, ${size(used)} из ${BUG_MAX_MB} МБ${left ? '' : ' — больше не добавить'}`
        : `Снимки экрана, фото, видео, текст — до ${BUG_MAX_FILES} файлов и ${BUG_MAX_MB} МБ. Можно перетащить сюда или вставить снимок через Ctrl+V`;
    }
  }
  function renderBugFile(f) {
    const el = $(`.bug-file[data-slot="${f.slot}"]`);
    if (!el) return;
    const done = f.state === 'ok';
    el.classList.toggle('copy', !done);
    $('small', el).textContent = done || !f.size ? size(f.size) : `${size(f.size)} · ${Math.floor((f.sent / f.size) * 100)}%`;
    $('.bar i', el).style.width = `${done ? 100 : (f.sent / f.size) * 100}%`;
  }
  function bindBugFiles() {
    const box = $('#bugBox');
    const pick = $('#bugPick');
    pick.addEventListener('change', () => { attach(Array.from(pick.files || [])); pick.value = ''; });
    const hasFiles = (e) => Array.from(e.dataTransfer?.types || []).includes('Files');
    box.addEventListener('dragover', (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'copy';
      box.classList.add('drag');
    });
    box.addEventListener('dragleave', (e) => { if (!box.contains(e.relatedTarget)) box.classList.remove('drag'); });
    box.addEventListener('drop', (e) => {
      box.classList.remove('drag');
      if (!hasFiles(e)) return;
      e.preventDefault();
      attach(Array.from(e.dataTransfer.files || []));
    });
    // Снимок экрана из буфера: Win+Shift+S, PrtSc, «Копировать изображение».
    box.addEventListener('paste', (e) => {
      const files = Array.from(e.clipboardData?.files || []);
      if (!files.length || (e.clipboardData.types || []).includes('text/plain')) return;
      e.preventDefault();
      attach(files);
    });
  }
  const size = (n) => (n < 1024 ? `${n} Б` : n < 1024 * 1024 ? `${Math.max(1, Math.round(n / 1024))} КБ` : `${(n / 1024 / 1024).toFixed(1).replace('.', ',')} МБ`);
  function onReportInfo(m) {
    const note = $('#bugLogNote');
    if (!note) return;
    const n = Math.max(0, +m.log || 0);
    note.textContent = n
      ? `${size(n)} — последние записи Axiom о его работе, без адресов страниц и личных данных`
      : 'Журнал пока пуст';
  }
  // «Написать ещё» из ответа разработчика: открыть форму, когда откроются настройки.
  let wantBug = false;
  const bugSoon = () => { wantBug = true; };

  // «Ваши сообщения»: что отправили и что ответил разработчик (ui/answers.js).
  let msgsShape = '';
  function renderMsgs() {
    const box = $('#msgList');
    const list = ax.answers?.list() || [];
    const shape = JSON.stringify(list);
    if (!box || shape === msgsShape) return;
    msgsShape = shape;
    box.hidden = !list.length;
    const day = (sec) => new Date(sec * 1000).toLocaleDateString('ru-RU', { day: 'numeric', month: 'long' });
    box.innerHTML = list.length ? `<div class="msgs-t">Ваши сообщения</div>${list.map((m) => (m.answer
      ? `<div class="msg">${ax.answers.card(m)}</div>`
      : `<div class="msg"><div class="ans-quote"><small>Вы писали ${day(m.at)} · ответа пока нет</small><p>${esc(m.text)}</p></div></div>`)).join('')}` : '';
  }

  function onReported(m) {
    $('#bugSend')?.classList.remove('busy');
    bugSendLater = false;
    if (!built) return;
    if (!m.done) {
      // Rust уже забрал файлы под неудавшийся отчёт: для новой попытки перепишем их заново.
      for (const f of bugFiles) { f.state = 'wait'; f.sent = 0; }
      bugCopying = null;
      renderBugFiles();
      pumpBugFiles();
      return;
    }
    $('#bugText').value = '';
    clearBugFiles();
    toggleBug(false);
  }

  function applyProxy(scope) {
    const input = $('#proxyIn', scope) || $('#proxyIn');
    ax.B.ai_proxy = input.value.trim();
    ax.saveSettings(true);
  }

  function importTheme() {
    let obj;
    try { obj = JSON.parse($('#themeJson').value); } catch { ax.toast('Это не JSON'); return; }
    if (!obj || typeof obj !== 'object' || Array.isArray(obj)) { ax.toast('Нужен объект темы'); return; }
    // Чужой CSS с загрузкой извне мог бы выдать заголовки вкладок через селекторы атрибутов:
    // такой не берём вовсе (а при применении любого CSS загрузки и так вырезаются).
    const css = typeof obj.customCss === 'string' && obj.customCss.trim() ? obj.customCss : '';
    const external = !!css && ax.cssLoads(css);
    if (external) delete obj.customCss;
    ax.swap(() => { ax.mergeAppearance(obj); });
    ax.saveSettings();
    $('.import-box', sheet).hidden = true;
    // Свой CSS меняет вид самого браузера: о нём стоит знать, даже если он безобиден.
    ax.toast(external ? 'Тема применена без своего CSS: в нём загрузка извне'
      : css ? 'Тема применена вместе со своим CSS — проверьте его, если тема не ваша' : 'Тема применена');
  }

  // ---------- перенос из других браузеров ----------
  const browsers = () => ax.system().browsers || [];
  const mark = (b) => `<span class="bmark">${esc(MARK[b.id] || b.name.slice(0, 1))}</span>`;

  function startImport(id) {
    if (importState.busy || !browsers().some((b) => b.id === id)) return;
    importState.busy = id;
    importState.res[id] = { ok: true, text: 'Переносим…' };
    ax.send({ cmd: 'import', browser: id });
    sync();
    syncOnbImport();
  }

  let listKey = '';
  function syncImportList(list) {
    const key = JSON.stringify([list, importState]);
    if (key === listKey) return;
    listKey = key;
    $('#impList').innerHTML = list.length
      ? list.map((b, i) => {
        const res = importState.res[b.id];
        const sub = [b.profile ? `<small>Профиль «${esc(b.profile)}»</small>` : '', res ? `<small class="imp-res${res.ok ? '' : ' bad'}">${esc(res.text)}</small>` : ''].join('');
        return `<div class="srow imp-row">${mark(b)}<span class="sl"><b>${esc(b.name)}</b>${sub}</span>
          <span class="sc"><button class="btn${i === 0 ? ' primary' : ''}${importState.busy === b.id ? ' busy' : ''}" data-act="import" data-browser="${esc(b.id)}">Перенести</button></span></div>`;
      }).join('')
      : `<div class="srow"><span class="sl"><b>Других браузеров не нашлось</b><small>Axiom переносит закладки и историю из Chrome, Edge, Яндекс Браузера, Opera, Brave, Vivaldi, Firefox и Zen.</small></span></div>`;
  }

  function bindSheet() {
    sheet.addEventListener('input', onInput);
    sheet.addEventListener('click', onClick);
    bindBugFiles();
    sheet.addEventListener('keydown', (e) => {
      if (e.target.id === 'proxyIn' && e.key === 'Enter') applyProxy(sheet);
      if (e.target.id === 'seCustomIn' && e.key === 'Enter') applyCustomSearch();
      if (e.target.id === 'bugText' && e.key === 'Enter' && (e.ctrlKey || e.metaKey)) sendBug();
      if (e.target.id === 'uaCustomIn' && e.key === 'Enter') applyCustomAgent();
      if (e.target.id === 'refCustomIn' && e.key === 'Enter') applyCustomReferer();
      if (e.target.id === 'nsIn' && e.key === 'Enter') addSleepSite();
    });
    sheet.addEventListener('pointerdown', dragBar);
    sheet.addEventListener('keydown', (e) => {
      if (!e.target.matches?.('.bpos-bar')) return;
      const now = ax.A.barPos === 'bottom' ? 'bottom' : 'top';
      const to = e.key === 'ArrowDown' ? 'bottom' : e.key === 'ArrowUp' ? 'top'
        : e.key === 'Enter' || e.key === ' ' ? (now === 'bottom' ? 'top' : 'bottom') : null;
      if (!to) return;
      e.preventDefault();
      setBar(to, e.target);
    });
    // Пузырёк над ползунком держится, пока бегунок тянут, даже если мышь ушла с ползунка.
    sheet.addEventListener('pointerdown', (e) => {
      const wrap = e.target.closest?.('.rng');
      if (!wrap) return;
      wrap.classList.add('drag');
      const up = () => {
        wrap.classList.remove('drag');
        removeEventListener('pointerup', up, true);
        removeEventListener('pointercancel', up, true);
      };
      addEventListener('pointerup', up, true);
      addEventListener('pointercancel', up, true);
    });
  }

  // ---------- где панель: перетаскивание на макете ----------
  // Панель на макете едет за мышью; отпустили ближе к нижнему краю — панель окна
  // переезжает вниз (и наоборот). Щелчок без перетаскивания тоже переносит её.
  function setBar(to, at) {
    if (to === (ax.A.barPos === 'bottom' ? 'bottom' : 'top')) { sync(); return; }
    const r = at.getBoundingClientRect();
    ax.swap(() => { ax.A.barPos = to; ax.applyAppearance(); }, { clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 });
    ax.saveSettings();
  }
  function dragBar(e) {
    const bar = e.target.closest?.('.bpos-bar');
    if (!bar || e.button !== 0) return;
    e.preventDefault();
    bar.focus({ preventScroll: true });
    const box = bar.parentElement;
    const zones = { top: $('.bpos-zone.t', box), bottom: $('.bpos-zone.b', box) };
    const b = box.getBoundingClientRect();
    const start = bar.getBoundingClientRect().top - b.top;
    const min = 6;
    const max = b.height - bar.offsetHeight - 6;
    const y0 = e.clientY;
    let moved = false;
    let to = box.dataset.bar;
    bar.setPointerCapture(e.pointerId);
    const move = (ev) => {
      const dy = ev.clientY - y0;
      if (!moved && Math.abs(dy) < 4) return;
      moved = true;
      box.classList.add('drag');
      const y = Math.min(max, Math.max(min, start + dy));
      bar.style.top = `${y}px`;
      to = y + bar.offsetHeight / 2 > b.height / 2 ? 'bottom' : 'top';
      zones.top.classList.toggle('hot', to === 'top');
      zones.bottom.classList.toggle('hot', to === 'bottom');
    };
    const up = (ev) => {
      bar.removeEventListener('pointermove', move);
      bar.removeEventListener('pointerup', up);
      bar.removeEventListener('pointercancel', up);
      zones.top.classList.remove('hot');
      zones.bottom.classList.remove('hot');
      if (ev.type === 'pointercancel') to = box.dataset.bar;
      else if (!moved) to = box.dataset.bar === 'bottom' ? 'top' : 'bottom';
      // Панель доезжает до своего края плавно: перетаскивание снято, а она пока там,
      // где её отпустили; в следующем кадре место ей даёт класс — и она едет.
      box.classList.remove('drag');
      box.dataset.bar = to;
      requestAnimationFrame(() => { bar.style.top = ''; });
      setBar(to, bar);
    };
    bar.addEventListener('pointermove', move);
    bar.addEventListener('pointerup', up);
    bar.addEventListener('pointercancel', up);
  }

  // Настройки — отдельная вкладка без своего webview (им нужен IPC, а у вкладок его нет).
  // Пока она активна, app.js показывает лист (open), ушли на другую вкладку — прячет (hide).
  // Закрыли саму вкладку — лист уходит плавно (leave), и только потом Rust показывает
  // страницу под ним: нативный webview перекрыл бы анимацию.
  let leaving = null;
  let afterLeave = [];
  function open(id) {
    if (!built) build();
    if (id) show(id);
    if (wantBug) { wantBug = false; toggleBug(true); }
    if (leaving) {
      // Настройки открыли снова, пока прежний лист уходил: он возвращается с начала.
      const a = leaving;
      leaving = null;
      a.cancel();
      sheet.inert = false;
      sheet.classList.remove('on');
      void sheet.offsetWidth;
    }
    if (sheet.classList.contains('on')) return;
    sheet.classList.add('on');
    $('#setBtn').classList.add('on');
    requestAnimationFrame(() => $('#setFind')?.focus({ preventScroll: true }));
  }
  function drop() {
    sheet.inert = false;
    sheet.classList.remove('on');
    $('#setBtn').classList.remove('on');
  }
  // Переключились на другую вкладку: настройки прячутся сразу, как любая вкладка.
  function hide() {
    if (!leaving) drop();
  }
  function leave(done) {
    afterLeave.push(done);
    if (leaving) return;
    const flush = () => { const fns = afterLeave; afterLeave = []; fns.forEach((f) => f()); };
    if (!sheet.classList.contains('on')) { flush(); return; }
    $('#setBtn').classList.remove('on');
    // Уходящий лист уже не принимает ни кликов, ни ввода.
    sheet.inert = true;
    const a = sheet.animate(
      [{ opacity: 1, transform: 'none' }, { opacity: 0, transform: 'translateY(10px) scale(.985)' }],
      { duration: Math.min(ax.ms(300), 800), easing: 'cubic-bezier(.4,0,.2,1)', fill: 'forwards' },
    );
    leaving = a;
    a.finished.then(() => {
      if (leaving !== a) return;
      leaving = null;
      drop();
      a.cancel();
    }, () => {}).finally(flush);
  }
  // × в листе и Esc закрывают саму вкладку настроек.
  function close() {
    if (isOpen()) ax.closeSettings();
  }
  const isOpen = () => sheet.classList.contains('on') && !leaving;

  // ---------- первый запуск ----------
  const STEPS = ['hello', 'look', 'data', 'ai', 'system', 'privacy', 'done'];
  let step = 0;
  let onbBuilt = false;

  function buildOnb() {
    onb.innerHTML = `
      <div class="onb-top" data-drag><span class="grow" data-drag></span>
        <button class="ib" data-win="minimize" title="Свернуть">${icon('min')}</button>
        <button class="ib" data-win="close" title="Закрыть">${icon('x')}</button></div>
      <div class="onb-glow"></div>
      <div class="onb-card">
        <section class="step" data-step="hello">
          <div class="onb-logo">${logoSvg(112, 'onb', true)}</div>
          <h1>Axiom</h1>
          <p class="lead">Лёгкий браузер. Минимум памяти, никакого лишнего шума<br>и ИИ на расстоянии одного сочетания клавиш.</p>
          <div class="onb-actions"><button class="btn primary big" data-onb="next">Начать ${icon('arrowR')}</button></div>
        </section>
        <section class="step" data-step="look">
          <h2>Как будет выглядеть?</h2><p class="lead">Всё это можно поменять потом в настройках.</p>
          <div class="presets onb-presets">${presetCards(true)}</div>
          <div class="lcards compact">${layoutCards()}</div>
        </section>
        <section class="step" data-step="data">
          <div class="step-ic">${icon('download')}</div>
          <h2 id="onbImpTitle">Перенести из другого браузера?</h2>
          <p class="lead" id="onbImpLead"></p>
          <div class="onb-pick" id="onbPick"></div>
          <div class="onb-list" id="onbImpList"><span>${icon('check')}Закладки со всеми папками</span><span>${icon('check')}История — для подсказок и плиток</span><span class="muted">${icon('lock')}Пароли и входы — нет: браузеры не отдают их другим программам</span></div>
          <div class="onb-result" id="onbImport"></div>
          <div class="onb-actions"><button class="btn primary big" data-act="import" id="onbImpBtn">Перенести</button><button class="btn ghost big" data-onb="next" id="onbImpSkip">Пропустить</button></div>
        </section>
        <section class="step" data-step="ai">
          <div class="step-ic">${icon('sparkles')}</div>
          <h2>Поиск и ИИ</h2>
          <p class="lead">Запрос из адресной строки уходит в <span class="se-name">DuckDuckGo</span>, а рядом сразу отвечает Google AI Mode. Поисковую систему можно сменить в настройках.</p>
          <div class="card onb-cardw">
            ${row('Двойной поиск', 'Можно переключать кнопкой ✦ в адресной строке', toggle('data-bkey', 'double_search'))}
            ${row('Прокси для AI Mode', 'Необязательно. Только для панели ИИ.', '<span class="inline"><input type="text" id="proxyIn" placeholder="socks5://127.0.0.1:1080" spellcheck="false"><button class="btn" data-act="proxy">OK</button></span>')}
          </div>
        </section>
        <section class="step" data-step="system">
          <div class="step-ic">${icon('monitor')}</div>
          <h2>Сделать основным?</h2>
          <p class="lead">Ссылки из Telegram, почты и других программ будут открываться в Axiom.</p>
          <div class="card onb-cardw">
            <div class="srow" id="onbInstRow"><span class="sl"><b>Установить в систему</b><small id="onbInst"></small></span><span class="sc"><button class="btn primary" data-act="install" id="onbInstBtn">Установить</button></span></div>
            <div class="srow"><span class="sl"><b>Браузер по умолчанию</b><small id="onbDef"></small></span><span class="sc"><button class="btn" data-act="default" id="onbDefBtn">Выбрать</button></span></div>
          </div>
        </section>
        <section class="step" data-step="privacy">
          <div class="step-ic">${icon('shield')}</div>
          <h2>Что Axiom отправляет</h2>
          <p class="lead">Ваши страницы, история и закладки остаются на компьютере. Сам Axiom отправляет только это — и всё можно выключить.</p>
          <div class="card onb-cardw">
            ${row('Статистика использования', 'Раз в час: версия, система и запускали ли Axiom сегодня. Без адресов и номера браузера', toggle('data-bkey', 'usage_stats'))}
            ${row('Отчёты об ошибках', 'Если Axiom вылетит: что сломалось и конец журнала, без адресов страниц', toggle('data-bkey', 'crash_reports'))}
            ${row('Проверять обновления', 'Раз в несколько часов Axiom узнаёт у GitHub номер последней версии', toggle('data-bkey', 'check_updates'))}
          </div>
          <p class="onb-small">Подробно — в Настройки → Система → «Политика конфиденциальности»</p>
        </section>
        <section class="step" data-step="done">
          <div class="done-mark">${icon('check')}</div>
          <h2>Готово</h2>
          <p class="lead"><kbd>Ctrl</kbd><kbd>K</kbd> — панель ИИ, <kbd>Ctrl</kbd><kbd>,</kbd> — настройки.<br>Приятного сёрфинга.</p>
          <div class="onb-actions"><button class="btn primary big" data-onb="finish">Начать пользоваться</button></div>
        </section>
      </div>
      <div class="onb-nav">
        <button class="btn ghost" data-onb="back">${icon('arrowL')} Назад</button>
        <span class="dots">${STEPS.map((_, i) => `<i data-dot="${i}"></i>`).join('')}</span>
        <button class="btn" data-onb="next">Далее ${icon('arrowR')}</button>
      </div>`;
    onbBuilt = true;
    onb.addEventListener('input', onInput);
    onb.addEventListener('click', (e) => {
      const b = e.target.closest('button');
      if (!b) return;
      if (b.dataset.win) { ax.send({ cmd: 'window', action: b.dataset.win }); return; }
      if (b.dataset.onb === 'next') { go(step + 1); return; }
      if (b.dataset.onb === 'back') { go(step - 1); return; }
      if (b.dataset.onb === 'finish') { finishOnb(); return; }
      if (b.dataset.pick) { onbPick = b.dataset.pick; syncOnbImport(); return; }
      onClick(e);
    });
  }

  function go(i) {
    step = Math.max(0, Math.min(STEPS.length - 1, i));
    $$('.step', onb).forEach((s) => s.classList.toggle('on', s.dataset.step === STEPS[step]));
    $$('[data-dot]', onb).forEach((d) => d.classList.toggle('on', +d.dataset.dot === step));
    const nav = $('.onb-nav', onb);
    nav.classList.toggle('hide', step === 0 || step === STEPS.length - 1);
    $('[data-onb=back]', nav).style.visibility = step > 1 ? 'visible' : 'hidden';
    syncScope(onb);
    syncOnbSystem();
    syncOnbImport();
  }

  function openOnboarding(at = 0) {
    if (!onbBuilt) buildOnb();
    onb.classList.add('on');
    ax.setOverlay('onb', true);
    go(at);
  }
  function finishOnb() {
    onb.classList.add('leaving');
    ax.B.onboarded = true;
    ax.saveSettings(true);
    ax.setOverlay('onb', false);
    setTimeout(() => { onb.classList.remove('on', 'leaving'); }, ax.ms(450) + 30);
  }

  function syncOnbSystem() {
    if (!onbBuilt) return;
    const sys = ax.system();
    const win = sys.platform === 'windows';
    $('#onbInstRow').hidden = !sys.can_install;
    $('#onbInst').textContent = sys.installed ? 'Готово: Axiom в профиле и в меню «Пуск»' : 'Скопирует Axiom в профиль и добавит в меню «Пуск»';
    const btn = $('#onbInstBtn');
    btn.classList.remove('busy');
    btn.innerHTML = sys.installed ? `${icon('check')} Установлен` : 'Установить';
    btn.disabled = !!sys.installed;
    $('#onbDef').textContent = sys.default_browser
      ? 'Axiom уже браузер по умолчанию'
      : win ? 'Windows откроет параметры — выберите там Axiom' : 'Одна кнопка — и ссылки откроются в Axiom';
    $('#onbDefBtn').textContent = win ? 'Выбрать' : 'Сделать основным';
  }
  function syncOnbImport() {
    if (!onbBuilt) return;
    const list = browsers();
    if (!list.some((b) => b.id === onbPick)) onbPick = list[0]?.id || '';
    const one = list.length === 1 ? list[0] : null;
    $('#onbImpTitle').textContent = !list.length ? 'Других браузеров не нашлось'
      : one ? `Перенести из ${one.from}?` : 'Откуда перенести?';
    $('#onbImpLead').textContent = !list.length ? 'Начнём с чистого листа: закладки и история появятся сами, пока вы пользуетесь Axiom.'
      : one ? 'Закладки и история переедут за пару секунд. Браузер можно не закрывать.'
        : 'Нашли несколько браузеров — выберите, где ваши закладки. Браузер можно не закрывать.';
    const pick = $('#onbPick');
    pick.hidden = list.length < 2;
    pick.innerHTML = list.length < 2 ? '' : list.map((b) => {
      const done = importState.res[b.id]?.done ? ' done' : '';
      return `<button class="pick${b.id === onbPick ? ' on' : ''}${done}" data-pick="${esc(b.id)}">${mark(b)}<span>${esc(b.name)}</span><i class="ok">${icon('check')}</i></button>`;
    }).join('');
    $('#onbImpList').hidden = !list.length;
    const btn = $('#onbImpBtn');
    btn.hidden = !list.length;
    btn.classList.toggle('busy', !!importState.busy);
    $('#onbImpSkip').textContent = list.length ? 'Пропустить' : 'Далее';
    const res = importState.res[onbPick];
    // В настройках профиль подписан у браузера, здесь — в самом результате.
    $('#onbImport').textContent = res ? res.text + (res.profile ? ` (профиль «${res.profile}»)` : '') : '';
    $('#onbImport').classList.toggle('on', !!res);
    $('#onbImport').classList.toggle('bad', !!res && !res.ok);
  }

  // ---------- ответы из Rust ----------
  function onImported(m) {
    const from = browsers().find((b) => b.id === m.browser)?.from || m.name;
    importState.busy = '';
    importState.res[m.browser] = m.ok
      ? { ok: true, done: true, profile: m.profile, text: `Перенесено: ${m.bookmarks} ${plural(m.bookmarks, 'закладка', 'закладки', 'закладок')} и ${m.history} ${plural(m.history, 'страница', 'страницы', 'страниц')} истории` }
      : { ok: false, text: `Не получилось: ${m.error}` };
    ax.toast(m.ok ? `Данные из ${from} перенесены` : 'Импорт не удался');
    sync();
    syncOnbImport();
  }
  function onInstalled(m) {
    $$('[data-act=install]').forEach((b) => b.classList.remove('busy'));
    ax.toast(m.ok ? 'Axiom установлен' : `Не удалось установить: ${m.error}`);
  }
  // Только для разработки (#import:браузер): начать перенос, когда придёт список браузеров.
  let pendingImport = '';
  const importSoon = (id) => { pendingImport = id || ''; };
  function onSystem() {
    sync();
    syncOnbSystem();
    syncOnbImport();
    if (pendingImport && browsers().some((b) => b.id === pendingImport)) {
      startImport(pendingImport);
      pendingImport = '';
    }
  }

  function plural(n, one, few, many) {
    const a = n % 10, b = n % 100;
    if (a === 1 && b !== 11) return one;
    if (a >= 2 && a <= 4 && (b < 12 || b > 14)) return few;
    return many;
  }

  return { open, hide, leave, close, isOpen, sync, openOnboarding, onImported, onInstalled, onSystem, importSoon, onReportInfo, onReported, onReportFile, bugSoon };
}
