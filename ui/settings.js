'use strict';
// Меню настроек (полноэкранный лист) и экран первого запуска.
// Вся логика темы живёт в app.js: сюда передаётся объект `ax` с доступом к ней.

function createPanels(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const { esc } = ax;

  const SECTIONS = [
    { id: 'look', icon: 'palette', title: 'Внешний вид', desc: 'Тема, цвета и шрифт. Всё меняется сразу.' },
    { id: 'layout', icon: 'layout', title: 'Раскладка', desc: 'Где живут вкладки и сколько воздуха вокруг.' },
    { id: 'search', icon: 'sparkles', title: 'Поиск и ИИ', desc: 'DuckDuckGo, Google AI Mode и прокси только для ИИ.' },
    { id: 'memory', icon: 'cpu', title: 'Память', desc: 'Спящие вкладки не занимают ни мегабайта.' },
    { id: 'data', icon: 'database', title: 'Данные', desc: 'Закладки и история из Chrome.' },
    { id: 'system', icon: 'monitor', title: 'Система', desc: 'Установка, обновления и браузер по умолчанию.' },
    { id: 'keys', icon: 'keyboard', title: 'Клавиши', desc: 'Всё как в Chrome, плюс пара своих.' },
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
    ['Ctrl D', 'Закладка'], ['Ctrl J', 'Загрузки'], ['Ctrl F', 'Найти на странице'], ['F5', 'Обновить'], ['Alt ← →', 'Назад, вперёд'], ['Ctrl ,', 'Настройки'],
  ];

  const sheet = $('#sheet');
  const onb = $('#onb');
  let built = false;
  let section = 'look';
  let importState = { busy: false, text: '' };

  // ---------- общие кусочки разметки ----------
  const kbd = (combo) => combo.split(' ').map((k) => `<kbd>${esc(k)}</kbd>`).join('');
  const toggle = (attr, key) => `<input type="checkbox" class="tg" ${attr}="${key}">`;
  const row = (title, note, control, extra = '') =>
    `<div class="srow" ${extra}><span class="sl"><b>${title}</b>${note ? `<small>${note}</small>` : ''}</span><span class="sc">${control}</span></div>`;
  const presetCards = (big) => Object.entries(ax.PRESETS).map(([k, p]) =>
    `<button class="preset${big ? ' big' : ''}" data-preset="${k}" data-mode="${p.mode}" data-oled="${p.oled}" style="--h:${p.hue};--c:${p.tint};--acc:${p.accent}">
      <span class="pv"><i class="a"><b></b><b></b><b></b></i><i class="b"><b></b><b></b></i><i class="c"></i></span>
      <span class="pn">${p.name}<i class="ok">${icon('check')}</i></span></button>`).join('');
  const layoutCards = () => LAYOUTS.map((l) =>
    `<button class="lcard" data-layout-pick="${l.v}"><span class="lmini" data-l="${l.v}"><i class="t"></i><i class="s"></i><i class="p"></i></span>
      <span class="ln"><b>${l.name}</b><small>${l.note}</small></span></button>`).join('');
  const swatches = () => ax.ACCENTS.map((c) => `<button class="sw" data-accent="${c}" style="--sw:${c}" title="${c}"></button>`).join('') +
    '<label class="sw pick" title="Свой цвет"><input type="color" data-key="accent"></label>';
  const preview = () => `<div class="pvw" id="pvw"><div class="pvw-bar"><i class="pvw-nav"></i><span class="pvw-tabs"><i class="on"></i><i></i><i></i></span><i class="pvw-omni"></i><i class="pvw-dot"></i></div>
    <div class="pvw-main"><i class="pvw-side"><b></b><b></b><b></b></i><i class="pvw-page"><b></b><b></b><b></b><b></b></i><i class="pvw-ai"><b></b><b></b></i></div></div>`;

  // ---------- лист настроек ----------
  function build() {
    const S = {
      look: `
        ${preview()}
        <h4>Темы</h4><div class="presets">${presetCards(false)}</div>
        <h4>Цвет</h4><div class="card">
          ${row('Режим', 'Авто — как в Windows', '<span class="seg" data-key="mode"><button data-v="dark">Тёмный</button><button data-v="light">Светлый</button><button data-v="system">Авто</button></span>')}
          ${row('Акцент', 'Кнопки, ссылки, выделение', `<span class="swatches">${swatches()}</span>`)}
          ${row('Оттенок фона', '', '<input type="range" class="hue" min="0" max="360" step="1" data-key="hue">')}
          ${row('Насыщенность фона', '0 — чистый серый', '<input type="range" min="0" max="0.06" step="0.002" data-key="tint">')}
          ${row('Чистый чёрный', 'Для OLED-экранов', toggle('data-key', 'oled'))}
        </div>
        <h4>Сайты</h4><div class="card">
          ${row('Тема сайтов', 'Сайты получают тему Axiom: в тёмном режиме DuckDuckGo, GitHub, YouTube и другие сами становятся тёмными', '<span class="pill-v" id="siteScheme"></span>')}
          ${row('Тёмные сайты принудительно', 'Затемняет и сайты без своей тёмной темы, как флаг в Chrome. Применится после перезапуска Axiom.', toggle('data-bkey', 'force_dark'), 'id="forceDarkRow"')}
        </div>
        <h4>Текст</h4><div class="card">
          ${row('Шрифт', '', '<select data-key="font"><option value="system">Системный</option><option value="rounded">Округлый</option><option value="mono">Моноширинный</option><option value="serif">С засечками</option><option value="custom">Свой…</option></select>')}
          ${row('Название шрифта', 'Любой установленный', '<input type="text" data-key="customFont" placeholder="Inter">', 'data-show="font=custom"')}
          ${row('Размер текста', '', '<input type="range" min="11" max="15" step="0.5" data-key="fontSize"><output data-out="fontSize"></output>')}
        </div>
        <h4>Свой CSS</h4>
        <div class="card pad"><textarea data-key="customCss" spellcheck="false" placeholder=".tab.on { box-shadow: inset 0 -2px var(--acc) }"></textarea>
          <div class="btns"><button class="btn" data-act="copy">Скопировать тему</button><button class="btn" data-act="import-toggle">Вставить тему</button><button class="btn ghost" data-act="reset">Сбросить всё</button></div>
          <div class="import-box" hidden><textarea id="themeJson" spellcheck="false" placeholder='{"accent":"#ff7aa2","radius":14}'></textarea><div class="btns"><button class="btn primary" data-act="import-apply">Применить</button></div></div>
        </div>`,
      layout: `
        <div class="lcards">${layoutCards()}</div>
        <h4>Форма</h4><div class="card">
          ${row('Скругления', '', '<input type="range" min="0" max="18" step="1" data-key="radius"><output data-out="radius"></output>')}
          ${row('Отступы', '0 — страница во всё окно', '<input type="range" min="0" max="14" step="1" data-key="gap"><output data-out="gap"></output>')}
        </div>
        <h4>Элементы</h4><div class="card">
          ${row('Кнопки навигации', 'Назад, вперёд, обновить', toggle('data-key', 'showNav'))}
          ${row('Кнопка ИИ', 'Панель открывается и по Ctrl+K', toggle('data-key', 'showAiBtn'))}
          ${row('Полный адрес', 'Иначе — только домен', toggle('data-key', 'fullUrl'))}
        </div>
        <h4>Движение</h4><div class="card">
          ${row('Скорость анимаций', '0 — выключены', '<input type="range" min="0" max="2" step="0.25" data-key="speed"><output data-out="speed"></output>')}
        </div>`,
      search: `
        <div class="card">
          ${row('Поисковая система', 'Без слежки и персональной выдачи', '<span class="pill-v">DuckDuckGo</span>')}
          ${row('Двойной поиск', 'Запрос уходит в DuckDuckGo и сразу в AI Mode — и из адресной строки, и из поиска на странице', toggle('data-bkey', 'double_search'))}
        </div>
        <h4>Панель ИИ</h4><div class="card">
          ${row('Прокси только для AI Mode', 'Вкладки ходят напрямую. Пусто — без прокси.', '')}
          <div class="srow"><span class="inline"><input type="text" id="proxyIn" placeholder="socks5://127.0.0.1:1080" spellcheck="false"><button class="btn primary" data-act="proxy">Применить</button></span></div>
          ${row('Ширина панели', 'Или потяните за край панели', '<input type="range" min="280" max="900" step="10" data-key="aiWidth"><output data-out="aiWidth"></output>')}
        </div>`,
      memory: `
        <div class="card">
          ${row('Фоновых вкладок в памяти', '<span id="liveOut"></span>', '<input type="range" min="0" max="10" step="1" data-bkey="live_tabs">')}
          ${row('Усыплять вкладки', '<span id="sleepOut"></span>', '<input type="range" min="0" max="120" step="5" data-bkey="sleep_minutes">')}
        </div>
        <div class="note">${icon('moon')}<span>Спящая вкладка полностью выгружается из памяти — остаются адрес, заголовок и место прокрутки. Не засыпают вкладки, где играет звук или остался недописанный текст. Фоновые вкладки сразу переходят в экономный режим.</span></div>`,
      data: `
        <div class="card hero-card"><div class="hero-ic">${icon('download')}</div>
          <div class="hero-t"><b>Перенести из Google Chrome</b><small>Закладки и история последнего профиля. Chrome можно не закрывать.</small><small class="imp-res" id="impRes"></small></div>
          <button class="btn primary" data-act="import">Перенести</button></div>
        <div class="note">${icon('lock')}<span>Пароли и входы на сайты не переносятся: Chrome шифрует их так, что прочитать их может только он сам. Войдите на нужные сайты один раз — Axiom предложит сохранить пароль.</span></div>`,
      system: `
        <div class="card">
          <div class="srow" id="instRow"><span class="sl"><b>Установка</b><small id="instNote"></small></span><span class="sc"><button class="btn primary" data-act="install" id="instBtn">Установить</button></span></div>
          <div class="srow"><span class="sl"><b>Браузер по умолчанию</b><small id="defNote"></small></span><span class="sc"><button class="btn" data-act="default" id="defBtn">Открыть параметры</button></span></div>
        </div>
        <div class="card">
          <div class="srow" id="updRow"><span class="sl"><b>Обновления</b><small id="updNote"></small></span><span class="sc"><button class="btn" data-act="update" id="updSet">Проверить</button></span></div>
          ${row('Проверять автоматически', 'Раз в несколько часов Axiom узнаёт у GitHub номер последней версии', toggle('data-bkey', 'check_updates'))}
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
    $$('.ssec', sheet).forEach((sec) => {
      let hit = false;
      $$('.srow,.krow', sec).forEach((r) => { const m = r.textContent.toLowerCase().includes(q); r.classList.toggle('miss', !m); hit ||= m; });
      $$('.card', sec).forEach((c) => c.classList.toggle('miss', !$$('.srow:not(.miss),.krow:not(.miss)', c).length && !c.textContent.toLowerCase().includes(q)));
      $$('h4,.lcards,.presets,.pvw,.note', sec).forEach((e) => e.classList.add('miss'));
      if (!hit && sec.querySelector('.hero-card')?.textContent.toLowerCase().includes(q)) { hit = true; sec.querySelector('.hero-card').classList.remove('miss'); }
      sec.hidden = !hit;
      any ||= hit;
    });
    $('#nothing').hidden = any;
  }

  function sync() {
    if (built) syncScope(sheet);
    if (onb.classList.contains('on')) syncScope(onb);
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
      $('#impRes').textContent = importState.text;
      const pv = $('#pvw');
      if (pv) pv.dataset.l = ax.A.layout;
      $('#siteScheme').textContent = ax.A.mode === 'system' ? 'Как в системе' : document.documentElement.dataset.mode === 'light' ? 'Светлая' : 'Тёмная';
      $('#forceDarkRow').hidden = sys.platform !== 'windows';
    }
  }

  function syncUpdate(sys) {
    const u = ax.update();
    const b = $('#updSet');
    const checking = u?.phase === 'checking';
    b.disabled = checking;
    b.classList.toggle('primary', !!u?.latest);
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
    $$('[data-bkey]', box).forEach((el) => { const v = B[el.dataset.bkey]; if (el.type === 'checkbox') el.checked = !!v; else el.value = v; });
    $$('input[type=range]', box).forEach((r) => r.style.setProperty('--fill', `${((r.value - r.min) / (r.max - r.min)) * 100}%`));
    $$('[data-out]', box).forEach((el) => {
      const k = el.dataset.out;
      el.textContent = k === 'speed' ? (ax.speed() ? `×${ax.speed()}` : 'выкл') : k === 'fontSize' ? `${A[k]}px` : String(A[k]);
    });
    const proxy = $('#proxyIn', box);
    if (proxy && document.activeElement !== proxy) proxy.value = B.ai_proxy || '';
    $$('[data-preset]', box).forEach((b) => b.classList.toggle('on', b.dataset.preset === A.preset));
    $$('[data-accent]', box).forEach((b) => b.classList.toggle('on', b.dataset.accent.toLowerCase() === String(A.accent).toLowerCase()));
    $$('[data-layout-pick]', box).forEach((b) => b.classList.toggle('on', b.dataset.layoutPick === A.layout));
    $$('[data-show]', box).forEach((el) => { const [k, v] = el.dataset.show.split('='); el.hidden = String(A[k]) !== v; });
    const so = $('#sleepOut', box);
    if (so) { const m = +B.sleep_minutes; so.textContent = m ? `Через ${m} мин без активности` : 'Никогда'; }
    const lo = $('#liveOut', box);
    if (lo) { const n = +B.live_tabs; lo.textContent = n ? `Не больше ${n}, лишние засыпают сразу` : 'Без ограничения'; }
  }

  // ---------- общие обработчики для листа и приветствия ----------
  const readVal = (el) => (el.type === 'checkbox' ? el.checked : el.type === 'range' ? parseFloat(el.value) : el.value);

  function onInput(e) {
    const el = e.target;
    if (el.id === 'setFind') { filter(el.value); return; }
    if (el.dataset.key) ax.setA(el.dataset.key, readVal(el));
    else if (el.dataset.bkey) {
      ax.B[el.dataset.bkey] = readVal(el);
      ax.saveSettings();
      ax.refresh();
      if (el.dataset.bkey === 'force_dark') ax.toast('Перезапустите Axiom, чтобы применить');
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
    switch (b.dataset.act) {
      case 'close': close(); break;
      case 'proxy': applyProxy(b.closest('.card, .step') || sheet); break;
      case 'copy': ax.copyText(JSON.stringify(ax.A, null, 2)); ax.toast('Тема скопирована'); break;
      case 'import-toggle': { const box = $('.import-box', sheet); box.hidden = !box.hidden; break; }
      case 'import-apply': importTheme(); break;
      case 'reset': ax.swap(() => { ax.resetAppearance(); }, e); ax.saveSettings(); ax.toast('Настройки вида сброшены'); break;
      case 'import': startImport(); break;
      case 'install': ax.send({ cmd: 'install' }); b.classList.add('busy'); break;
      case 'default': ax.send({ cmd: 'open_default_apps' }); break;
      case 'update':
        if (ax.update()?.latest) ax.openUpdate();
        else ax.send({ cmd: 'update_check' });
        break;
      default: break;
    }
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
    ax.swap(() => { ax.mergeAppearance(obj); });
    ax.saveSettings();
    $('.import-box', sheet).hidden = true;
    ax.toast('Тема применена');
  }

  function startImport() {
    if (importState.busy) return;
    importState = { busy: true, text: 'Переносим…' };
    $$('[data-act=import]').forEach((b) => b.classList.add('busy'));
    ax.send({ cmd: 'import_chrome' });
    sync();
    syncOnbImport();
  }

  function bindSheet() {
    sheet.addEventListener('input', onInput);
    sheet.addEventListener('click', onClick);
    sheet.addEventListener('keydown', (e) => {
      if (e.target.id === 'proxyIn' && e.key === 'Enter') applyProxy(sheet);
    });
  }

  function open(id) {
    if (!built) build();
    if (id) show(id);
    if (sheet.classList.contains('on')) return;
    sheet.classList.add('on');
    ax.setOverlay('sheet', true);
    $('#setBtn').classList.add('on');
    requestAnimationFrame(() => $('#setFind')?.focus({ preventScroll: true }));
  }
  function close() {
    if (!sheet.classList.contains('on')) return;
    sheet.classList.remove('on');
    ax.setOverlay('sheet', false);
    $('#setBtn').classList.remove('on');
  }
  const isOpen = () => sheet.classList.contains('on');

  // ---------- первый запуск ----------
  const STEPS = ['hello', 'look', 'data', 'ai', 'system', 'done'];
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
          <h2>Перенести из Chrome?</h2>
          <p class="lead">Закладки и история переедут за пару секунд. Chrome можно не закрывать.</p>
          <div class="onb-list"><span>${icon('check')}Закладки со всеми папками</span><span>${icon('check')}История — для подсказок и плиток</span><span class="muted">${icon('lock')}Пароли и входы — нет: Chrome не отдаёт их другим программам</span></div>
          <div class="onb-result" id="onbImport"></div>
          <div class="onb-actions"><button class="btn primary big" data-act="import">Перенести</button><button class="btn ghost big" data-onb="next">Пропустить</button></div>
        </section>
        <section class="step" data-step="ai">
          <div class="step-ic">${icon('sparkles')}</div>
          <h2>Поиск и ИИ</h2>
          <p class="lead">Запрос из адресной строки уходит в DuckDuckGo, а рядом сразу отвечает Google AI Mode.</p>
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
    $('#onbImport').textContent = importState.text;
    $('#onbImport').classList.toggle('on', !!importState.text);
  }

  // ---------- ответы из Rust ----------
  function onImported(m) {
    $$('[data-act=import]').forEach((b) => b.classList.remove('busy'));
    importState = {
      busy: false,
      text: m.ok ? `Перенесено: ${m.bookmarks} ${plural(m.bookmarks, 'закладка', 'закладки', 'закладок')} и ${m.history} ${plural(m.history, 'страница', 'страницы', 'страниц')} истории (профиль «${m.profile}»)` : `Не получилось: ${m.error}`,
    };
    ax.toast(m.ok ? 'Данные из Chrome перенесены' : 'Импорт не удался');
    sync();
    syncOnbImport();
  }
  function onInstalled(m) {
    $$('[data-act=install]').forEach((b) => b.classList.remove('busy'));
    ax.toast(m.ok ? 'Axiom установлен' : `Не удалось установить: ${m.error}`);
  }
  function onSystem() { sync(); syncOnbSystem(); }

  function plural(n, one, few, many) {
    const a = n % 10, b = n % 100;
    if (a === 1 && b !== 11) return one;
    if (a >= 2 && a <= 4 && (b < 12 || b > 14)) return few;
    return many;
  }

  return { open, close, isOpen, toggle: () => (isOpen() ? close() : open()), sync, openOnboarding, onImported, onInstalled, onSystem };
}
