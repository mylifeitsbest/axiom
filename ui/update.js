'use strict';
// Обновления: кнопка «Обновить» в панели инструментов и окошко под ней.
// Что и откуда скачать, решает Rust; отсюда уходят только «проверь» и «обнови».
// В том же окошке — «Что нового»: список изменений, который Rust берёт из CHANGELOG.md
// и списка бета-версии. Сразу после обновления кнопка в панели называется «Что нового».
function createUpdate(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const btn = $('#updBtn');
  const label = btn.querySelector('.upd-t');
  const panel = $('#upp');

  let U = null;
  // «Позже» прячет кнопку до перезапуска (или до следующей версии).
  let later = null;
  let announced = null;
  // Только для разработки: что открыть, когда придёт состояние (см. openSoon).
  let soon = null;
  let shape = '';
  // Окошко показывает обновление ('main') или список изменений ('notes').
  let view = 'main';
  // Чей список: доступного обновления ('next') или установленной версии ('current').
  let which = 'next';
  // Ответы Rust по ключу `next|версия` или `current|версия`: за сеанс список не меняется.
  const notes = new Map();
  // «Что нового» после обновления уже открывали — кнопка в панели больше не нужна.
  let freshSeen = false;

  const mb = (n) => `${(n / 1048576).toLocaleString('ru-RU', { maximumFractionDigits: 1 })} МБ`;
  const busy = () => U && (U.phase === 'downloading' || U.phase === 'restarting');
  const isOpen = () => panel.classList.contains('on');
  // Axiom только что обновился, а «Что нового» ещё не открывали.
  const fresh = () => !!U?.updated_from && !freshSeen;
  // Кнопка в панели сейчас про обновление, а не про «Что нового».
  const offersUpdate = () => !!U?.latest && (later !== U.latest || busy() || (isOpen() && view === 'main'));

  // ---------- кнопка ----------
  let btnIcon = 'up';
  function renderBtn() {
    const v = U?.latest;
    const update = offersUpdate();
    // Пока открыт список изменений после обновления, кнопка остаётся под ним.
    const show = update || fresh() || (isOpen() && view === 'notes' && which === 'current' && !!U?.updated_from);
    const appear = show && btn.hidden;
    btn.hidden = !show;
    btn.classList.toggle('busy', !!busy());
    const ic = update ? 'up' : 'sparkles';
    if (ic !== btnIcon) { btnIcon = ic; btn.querySelector('[data-i]').innerHTML = icon(ic); }
    label.textContent = busy() ? 'Обновляю…' : update ? 'Обновить' : 'Что нового';
    btn.title = update ? `Доступна Axiom ${v}, у вас ${U.current}`
      : U?.updated_from ? `Axiom обновлён: ${U.updated_from} → ${U.current}` : '';
    if (appear && ax.speed()) {
      btn.classList.remove('in');
      void btn.offsetWidth;
      btn.classList.add('in');
      setTimeout(() => btn.classList.remove('in'), ax.ms(2200) + 50);
    }
  }

  // ---------- окошко ----------
  const button = (act, text, primary) => `<button class="btn${primary ? ' primary' : ' ghost'}" data-u="${act}">${text}</button>`;

  function message() {
    switch (U.phase) {
      case 'downloading': return U.received ? `Скачиваю новую версию… ${mb(U.received)}` : 'Скачиваю новую версию…';
      case 'restarting': return 'Перезапускаю Axiom…';
      default: return '';
    }
  }

  // Бета-версия (`0.5.4-beta.312`) — каждый коммит в main до выхода версии.
  const beta = () => /-beta\.\d+$/.test(U.latest || '');

  function body() {
    const head = `<div class="upp-head"><span class="upp-logo">${logoSvg(36, 'upd')}</span>
      <span class="upp-t"><b>${beta() ? 'Бета-версия' : 'Axiom'} ${ax.esc(U.latest)}</b><small>Сейчас установлена ${ax.esc(U.current)}</small></span>
      <button class="lnk" data-u="notes" title="Что изменилось в новой версии">Что нового${icon('arrowR')}</button></div>`;
    let text = '';
    let extra = '';
    let acts = '';
    if (U.how === 'in_place') {
      if (busy()) {
        text = ax.esc(message());
        extra = '<span class="dl-bar ind upp-bar"><i></i></span>';
      } else if (U.phase === 'failed') {
        text = `<span class="upp-err">Не получилось: ${ax.esc(U.error)}</span>`;
        acts = button('manual', 'Скачать вручную') + button('install', 'Повторить', true);
      } else {
        text = 'Axiom скачает новую версию, сверит контрольную сумму и перезапустится. Вкладки откроются снова.';
        if (U.busy) extra = `<p class="upp-warn">${icon('download')}<span>Идут загрузки — перезапуск их прервёт.</span></p>`;
        acts = button('later', 'Позже') + button('install', `${icon('up')}Обновить и перезапустить`, true);
      }
    } else if (U.how === 'apt' || U.how === 'pacman' || U.how === 'aur') {
      text = U.how === 'apt'
        ? 'Axiom стоит из репозитория apt и обновится вместе с системой. Чтобы обновить сейчас, выполните в терминале:'
        : beta()
          ? 'Это бета-версия: её ставит установщик Axiom, вся система при этом не обновляется. Выполните в терминале:'
          : U.how === 'aur'
          ? 'Axiom стоит из AUR (axiom-browser-bin). Чтобы обновить только его, без обновления всей системы, выполните в терминале:'
          : 'Axiom обновится вместе с системой. Чтобы обновить сейчас, выполните в терминале:';
      // Переносы только между словами: `--only-upgrade` не разорвётся на дефисе.
      const words = U.command.split(' ').map((w) => `<span>${ax.esc(w)}</span>`).join(' ');
      extra = `<div class="upp-cmd"><code>${words}</code><button class="ib sm" data-u="copy" title="Скопировать">${icon('copy')}</button></div>`;
      acts = button('later', 'Позже') + button('copy', 'Скопировать команду', true);
    } else {
      text = 'Новую версию можно скачать со страницы релиза на GitHub.';
      acts = button('later', 'Позже') + button('manual', 'Открыть страницу', true);
    }
    return `${head}<p class="upp-msg">${text}</p>${extra}${acts ? `<div class="upp-acts">${acts}</div>` : ''}`;
  }

  // ---------- «Что нового» ----------
  // Ключ ответа Rust: чей список и до какой версии.
  const notesKey = () => (which === 'next' && U?.latest ? `next|${U.latest}` : `current|${U?.current || ''}`);

  function requestNotes() {
    const key = notesKey();
    // Уже загружен или загружается; после ошибки пробуем снова при следующем открытии.
    const n = notes.get(key);
    if (n === null || (n && !n.error)) return;
    notes.set(key, null);
    ax.send({ cmd: 'update_notes', which: key.split('|')[0] });
  }

  // Ответ Rust: разделы с текстом в простом markdown (абзацы, списки, **жирный**, `код`).
  function onNotes(m) {
    if (!m || typeof m.to !== 'string') return;
    notes.set(`${m.which}|${m.to}`, {
      from: String(m.from || ''),
      page: String(m.page || ''),
      error: String(m.error || ''),
      sections: Array.isArray(m.sections) ? m.sections : [],
    });
    if (isOpen() && view === 'notes') { renderPanel(); position(); }
  }

  // Строка текста: экранируем, потом `код` и **жирный**; ссылки [текст](адрес) — просто текст.
  function inline(s) {
    return String(s).split('`').map((part, i) => {
      if (i % 2) return `<code>${ax.esc(part)}</code>`;
      return ax.esc(part.replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')).replace(/\*\*(.+?)\*\*/g, '<b>$1</b>');
    }).join('');
  }

  function markdown(text) {
    let html = '';
    let para = [];
    let list = false;
    const endPara = () => { if (para.length) html += `<p>${inline(para.join(' '))}</p>`; para = []; };
    const endList = () => { if (list) html += '</ul>'; list = false; };
    for (const line of String(text).split('\n')) {
      const item = /^(\s*)[-*] (.*)$/.exec(line);
      if (item) {
        endPara();
        if (!list) { html += '<ul>'; list = true; }
        html += `<li${item[1].length >= 2 ? ' class="sub"' : ''}>${inline(item[2])}</li>`;
      } else if (!line.trim()) {
        endPara();
        endList();
      } else {
        endList();
        para.push(line.trim());
      }
    }
    endPara();
    endList();
    return html;
  }

  // «0.6.1 — 26 ноября 2026»: номер крупно, дата мельче.
  function section(s) {
    const title = String(s.title || '');
    const [ver, date] = title.split(' — ');
    const head = date ? `<b>${ax.esc(ver)}</b><small>${ax.esc(date)}</small>` : `<b>${ax.esc(title)}</b>`;
    return `<section class="upn-sec"><h4>${head}</h4>${markdown(s.text || '')}</section>`;
  }

  function notesBody() {
    const n = notes.get(notesKey());
    const next = which === 'next' && !!U.latest;
    const from = next ? U.current : n?.from || U.updated_from || '';
    const sub = next ? `${beta() ? 'Бета-версия' : 'Axiom'} ${ax.esc(U.latest)} · у вас ${ax.esc(U.current)}`
      : from ? `Axiom ${ax.esc(U.current)} · было ${ax.esc(from)}` : `Установлена Axiom ${ax.esc(U.current)}`;
    const head = `<div class="upp-head"><span class="upp-logo">${logoSvg(36, 'upd')}</span>
      <span class="upp-t"><b>Что нового</b><small>${sub}</small></span></div>`;
    let list;
    if (!n) {
      list = '<div class="upn-wait"><span class="dl-bar ind upp-bar"><i></i></span><small>Загружаю список изменений…</small></div>';
    } else if (n.error) {
      list = `<p class="upp-msg"><span class="upp-err">Не удалось загрузить: ${ax.esc(n.error)}</span></p>`;
    } else if (!n.sections.length) {
      list = '<p class="upp-msg">Подробного списка для этой версии нет — он есть на странице релиза.</p>';
    } else {
      list = `<div class="upn">${n.sections.map(section).join('')}</div>`;
    }
    const github = n ? `<button class="lnk upn-gh" data-u="page" title="Страница релиза на GitHub">На GitHub${icon('popout')}</button>` : '';
    let acts;
    if (next) {
      const install = U.how === 'in_place' && !busy();
      acts = button('back', 'Назад') + (install ? button('install', `${icon('up')}Обновить и перезапустить`, true) : '');
    } else {
      acts = button('done', 'Понятно', true);
    }
    return `${head}${list}<div class="upp-acts">${github}${acts}</div>`;
  }

  // Прогресс приходит несколько раз в секунду: меняем только текст, чтобы полоска не дёргалась.
  function renderPanel() {
    const next = view === 'notes'
      ? ['notes', notesKey(), notes.get(notesKey()) ? 1 : 0, U.how, busy() ? 1 : 0].join('|')
      : [U.latest, U.how, U.phase, U.busy, U.error].join('|');
    if (next === shape && (busy() || view === 'notes')) {
      const msg = view === 'main' && panel.querySelector('.upp-msg');
      if (msg) msg.textContent = message();
      return;
    }
    shape = next;
    if (view === 'notes') requestNotes();
    panel.classList.toggle('notes', view === 'notes');
    panel.innerHTML = view === 'notes' ? notesBody() : body();
  }

  // Открыть в окошке обновление ('main') или список изменений ('notes', `w`: 'next' или 'current').
  function setView(v, w) {
    view = v;
    if (w) which = w;
    if (view !== 'notes') return;
    requestNotes();
    if (which === 'current') freshSeen = true;
  }
  function show(v, w) {
    setView(v, w);
    shape = '';
    renderPanel();
    position();
    renderBtn();
  }

  function position() {
    const anchor = btn.hidden ? $('#setBtn') : btn;
    const r = anchor.getBoundingClientRect();
    const w = Math.min(view === 'notes' ? 430 : 360, innerWidth - 16);
    panel.style.width = w + 'px';
    const h = panel.offsetHeight;
    const left = Math.min(Math.max(8, r.right - w), innerWidth - 8 - w);
    const up = r.bottom + 6 + h > innerHeight - 8 && r.top - 6 - h >= 8;
    panel.classList.toggle('up', up);
    Object.assign(panel.style, { left: Math.round(left) + 'px', top: Math.round(up ? r.top - 6 - h : r.bottom + 6) + 'px' });
  }
  function open(v = 'main', w = 'next') {
    if (!U || (v === 'main' && !U.latest)) return;
    if (isOpen()) { if (v !== view || w !== which) show(v, w); return; }
    setView(v, w);
    shape = '';
    renderPanel();
    panel.classList.add('on');
    // После «on»: кнопка «Что нового» остаётся в панели, пока открыт её список.
    renderBtn();
    position();
    btn.classList.add('on');
    ax.setOverlay('upd', true);
  }
  function close() {
    if (!isOpen()) return;
    panel.classList.remove('on');
    btn.classList.remove('on');
    ax.setOverlay('upd', false);
    renderBtn();
  }
  // Кнопка в панели: обновление, а сразу после обновления — «Что нового».
  const toggle = () => (isOpen() ? close() : offersUpdate() ? open() : open('notes', 'current'));

  btn.addEventListener('click', toggle);
  document.addEventListener('mousedown', (e) => {
    if (isOpen() && !e.target.closest('#upp,#updBtn')) close();
  }, true);
  addEventListener('resize', () => { if (isOpen()) position(); });

  panel.addEventListener('click', (e) => {
    const act = e.target.closest('[data-u]')?.dataset.u;
    switch (act) {
      // Прогресс скачивания видно в основном виде окошка.
      case 'install': if (view !== 'main') show('main'); ax.send({ cmd: 'update_install' }); break;
      case 'notes': show('notes', 'next'); break;
      case 'back': show('main'); break;
      case 'done': close(); break;
      case 'page': ax.send({ cmd: 'new_tab', input: notes.get(notesKey())?.page || U.page }); close(); break;
      case 'manual': ax.send({ cmd: 'new_tab', input: U.page }); close(); break;
      case 'copy': ax.copyText(U.command); ax.toast('Команда скопирована'); break;
      case 'later':
        later = U.latest;
        close();
        ax.toast('Обновиться можно в настройках, раздел «Система»');
        break;
      default: break;
    }
  });

  // ---------- сообщения из Rust ----------
  function onState(m) {
    U = m;
    if (m.manual) {
      if (m.latest) ax.toast(`Доступна Axiom ${m.latest}`);
      else if (m.checked === 'failed') ax.toast(`Не удалось проверить: ${m.check_error}`);
      else ax.toast(`У вас последняя версия — ${m.current}`);
    } else if (m.latest && m.latest !== announced && later !== m.latest) {
      ax.toast(`Вышла Axiom ${m.latest}`);
    }
    if (m.latest) announced = m.latest;
    renderBtn();
    if (isOpen()) {
      // Список изменений установленной версии от доступного обновления не зависит.
      if (view === 'notes' && !m.latest) which = 'current';
      if (m.latest || view === 'notes') { renderPanel(); position(); } else close();
    } else {
      trySoon();
    }
    ax.onChange();
  }

  const reposition = () => { if (isOpen()) requestAnimationFrame(position); };
  const state = () => U;
  // Из настроек: окошко открывается и после «Позже».
  function openFromSettings() { later = null; renderBtn(); open(); }
  // «Что нового» в настройках: что принесёт доступное обновление, а без него — что в этой версии.
  const openNotes = () => open('notes', U?.latest ? 'next' : 'current');
  // Только для разработки (#update, #update:install, #update:notes, #update:current):
  // открыть окошко, когда придёт состояние.
  function openSoon(arg) {
    soon = arg || 'main';
    trySoon();
  }
  function trySoon() {
    if (!soon || !U) return;
    const what = soon;
    if (what === 'current') { soon = null; open('notes', 'current'); return; }
    if (!U.latest) return;
    soon = null;
    open(what === 'notes' ? 'notes' : 'main');
    if (what === 'install') ax.send({ cmd: 'update_install' });
  }

  return { open: openFromSettings, openNotes, openSoon, close, isOpen, reposition, onState, onNotes, state };
}
