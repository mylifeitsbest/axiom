'use strict';
// Плагины: раздел «Плагины» в настройках (установленные и магазин) и кнопки плагинов
// на панели. Список, включение, настройки, удаление, магазин и публикацию решает Rust
// (src/app/plugins.rs, src/plugins/market.rs): сюда приходят списки, обратно — команды.
// Код плагинов в интерфейс не попадает.

// Как писать плагины: документация в репозитории.
const PLUGINS_DOCS = 'https://github.com/mylifeitsbest/axiom/blob/main/docs/plugins.md';

function createPlugins(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const { esc } = ax;
  const sheet = $('#sheet');
  const bar = $('#plActs');

  let L = { dir: '', items: [], broken: [] };
  // Магазин: каталог из Rust и что сейчас ставится или публикуется.
  let M = { loaded: false, loading: false, error: '', items: [], busy: [] };
  let tab = 'mine';
  let openedAt = 0;
  let findQ = '';
  // У какого плагина открыта форма «Опубликовать».
  let pubOpen = null;
  // «Удалить» нажали один раз: второй клик в течение нескольких секунд удаляет.
  let armed = null;
  let armTimer = 0;
  // Пока человек печатает в поле, список не перерисовываем (уйдёт фокус).
  let stale = false;

  const str = (v) => (typeof v === 'string' ? v : '');
  const list = (v) => (Array.isArray(v) ? v : []);
  const plural = (n, one, few, many) => {
    const a = Math.abs(n) % 100, b = a % 10;
    return a > 10 && a < 20 ? many : b === 1 ? one : b >= 2 && b <= 4 ? few : many;
  };
  // Значок плагина — только data: (CSP интерфейса другие не пустит), иначе буква.
  const imgOk = (src) => typeof src === 'string' && src.startsWith('data:image/');
  const badge = (p, src, cls) => imgOk(src)
    ? `<span class="${cls}"><img src="${esc(src)}" alt=""></span>`
    : `<span class="${cls} letter">${esc((p.name || '?').trim().charAt(0).toUpperCase())}</span>`;
  const site = (m) => (m === '<all_urls>' || m === '*' ? 'все сайты' : m.replace(/^(\*|https?):\/\//, '').replace(/\/\*$/, ''));
  const sitesTag = (sites) => {
    const s = list(sites).map(site);
    return s.length ? `<span class="pl-tag">${icon('globe')}${esc(s.slice(0, 4).join(', '))}${s.length > 4 ? ` и ещё ${s.length - 4}` : ''}</span>` : '';
  };
  const busy = (id) => M.busy.includes(id);
  const storeItem = (id) => M.items.find((x) => x.id === id);
  const PERM_NAMES = { notify: 'Показывать сообщения', tabs: 'Открывать вкладки' };

  function sectionHtml() {
    return `
      <span class="seg pl-tabs" role="tablist" aria-label="Плагины"><button data-pl-tab="mine" class="on">Установленные</button><button data-pl-tab="store">${icon('puzzle')}Магазин</button></span>
      <div id="plMine">
        <div class="card">
          <div class="srow" data-kw="плагины расширения папка установить добавить свои скрипты"><span class="sl"><b>Папка плагинов</b><small id="plDir"></small></span>
            <span class="sc"><button class="btn" data-pl-act="folder">${icon('folder')}Открыть</button><button class="btn" data-pl-act="reload" title="Перечитать папку: новые, изменённые и удалённые плагины">${icon('reload')}Обновить</button></span></div>
        </div>
        <div id="plList"></div>
        <div class="note">${icon('shield')}<span>Плагин видит и меняет страницы сайтов, на которых работает, — как расширение в Chrome. Включайте только плагины, которым доверяете. Новый плагин в папке сам не включается; что ему можно сверх страницы, видно в его карточке.</span></div>
        <div class="note">${icon('file')}<span>Свой плагин — это папка с файлом plugin.json и скриптами. Положите её в папку плагинов и нажмите «Обновить», а когда он готов — «Опубликовать» в его карточке, и он попадёт в магазин. <button class="lnk" data-pl-act="docs">Как написать плагин</button></span></div>
      </div>
      <div id="plStore" hidden>
        <div class="pl-sbar"><label class="sfind">${icon('search')}<input id="plFind" type="text" placeholder="Найти плагин" spellcheck="false"></label>
          <button class="btn" data-pl-act="store-reload" title="Загрузить каталог заново">${icon('reload')}Обновить</button></div>
        <div id="plStoreList"></div>
        <div class="note">${icon('shield')}<span>Каждый плагин в магазине проверил разработчик Axiom, прежде чем опубликовать. Плагин ставится в папку плагинов и сразу включается; выключить и удалить его можно на вкладке «Установленные». Каталог загружается, только когда вы открываете магазин.</span></div>
      </div>`;
  }

  function field(p, f) {
    const v = p.values ? p.values[f.key] : undefined;
    const attrs = `data-pl-id="${esc(p.id)}" data-pl-key="${esc(f.key)}"`;
    let control = '';
    if (f.type === 'toggle') {
      control = `<input type="checkbox" class="tg" ${attrs}${v === true ? ' checked' : ''}>`;
    } else if (f.type === 'choice') {
      const opts = list(f.options);
      control = opts.length <= 3
        ? `<span class="seg" ${attrs}>${opts.map((o) => `<button data-v="${esc(o.value)}" class="${o.value === v ? 'on' : ''}">${esc(o.label)}</button>`).join('')}</span>`
        : `<select ${attrs}>${opts.map((o) => `<option value="${esc(o.value)}"${o.value === v ? ' selected' : ''}>${esc(o.label)}</option>`).join('')}</select>`;
    } else if (f.type === 'number') {
      const lim = ['min', 'max', 'step'].filter((k) => Number.isFinite(f[k])).map((k) => `${k}="${f[k]}"`).join(' ');
      control = `<input type="number" class="pl-num" ${attrs} ${lim} value="${Number.isFinite(v) ? v : ''}">`;
    } else {
      const max = Number.isFinite(f.max_length) ? f.max_length : 1000;
      control = `<input type="text" class="pl-text" ${attrs} maxlength="${max}" spellcheck="false" placeholder="${esc(str(f.placeholder))}" value="${esc(str(v))}">`;
    }
    return `<div class="srow" data-kw="${esc(p.name.toLowerCase())}"><span class="sl"><b>${esc(f.label)}</b>${f.note ? `<small>${esc(f.note)}</small>` : ''}</span><span class="sc">${control}</span></div>`;
  }

  // Что с плагином в магазине, если его публиковали отсюда.
  function pubLine(p) {
    const s = p.published;
    if (!s || typeof s !== 'object') return '';
    const live = s.published ? ` В магазине сейчас версия ${esc(s.published)}.` : '';
    if (s.status === 'pending') return `<div class="pl-pubst wait">${icon('clock')}<span>Версия ${esc(s.version)} на проверке — после неё появится в магазине.${live}</span></div>`;
    if (s.status === 'approved') return `<div class="pl-pubst ok">${icon('check')}<span>В магазине: версия ${esc(s.version)}.</span></div>`;
    if (s.status === 'rejected') return `<div class="pl-pubst bad">${icon('x')}<span>Версию ${esc(s.version)} не опубликовали${s.reason ? `: ${esc(s.reason)}` : ''}.${live}</span></div>`;
    return '';
  }

  function pubForm(p) {
    return `<div class="pl-pub">
      <div class="pl-pub-row">
        <label><span>Автор</span><input type="text" class="pl-pf" data-f="author" maxlength="100" value="${esc(str(p.author))}" placeholder="Как подписать плагин"></label>
        <label><span>Как с вами связаться</span><input type="text" class="pl-pf" data-f="contact" maxlength="200" placeholder="Telegram или почта — увидит только проверяющий"></label>
      </div>
      <label><span>Что в этой версии и на что посмотреть</span><textarea class="pl-pf" data-f="note" maxlength="1000" placeholder="Необязательно"></textarea></label>
      <small>Плагин (все файлы его папки, кроме скрытых) уйдёт на проверку разработчику Axiom и появится в магазине после неё. Новые версии принимаются только из этого профиля Axiom: ключ автора хранится в нём.</small>
      <div class="btns"><button class="btn primary" data-pl-send="${esc(p.id)}">Отправить на проверку</button><button class="btn ghost" data-pl-act="pub-cancel">Отмена</button></div>
    </div>`;
  }

  function card(p) {
    const meta = [p.version && `версия ${esc(p.version)}`, p.author && esc(p.author), p.from_store && 'из магазина'].filter(Boolean).join(' · ');
    const perms = list(p.permissions);
    const fields = list(p.settings);
    const sure = armed === p.folder;
    const it = p.from_store ? storeItem(p.id) : null;
    const update = it && it.version !== p.version
      ? `<button class="btn primary${busy(p.id) ? ' busy' : ''}" data-pl-install="${esc(p.id)}">${icon('download')}Обновить до ${esc(it.version)}</button>` : '';
    const publish = p.from_store ? '' : busy(p.id)
      ? '<button class="btn busy" disabled>Отправляется</button>'
      : `<button class="btn ghost" data-pl-pub="${esc(p.id)}">${icon('up')}${p.published ? 'Отправить новую версию' : 'Опубликовать'}</button>`;
    return `<div class="card pl${p.enabled ? ' on' : ''}" data-kw="${esc(`${p.name} ${p.id} ${p.description}`.toLowerCase())}">
      <div class="srow pl-top">${badge(p, p.icon, 'pl-ic')}
        <span class="sl"><b>${esc(p.name)}</b>${p.description ? `<small>${esc(p.description)}</small>` : ''}${meta ? `<small class="pl-meta">${meta}</small>` : ''}</span>
        <span class="sc"><input type="checkbox" class="tg" data-pl-on="${esc(p.id)}"${p.enabled ? ' checked' : ''} aria-label="Включить «${esc(p.name)}»"></span></div>
      <div class="pl-info">
        ${sitesTag(p.sites)}
        ${perms.map((x) => `<span class="pl-tag">${icon('check')}${esc(x)}</span>`).join('')}
        ${p.action ? `<span class="pl-tag">${icon('plus')}Кнопка на панели</span>` : ''}
      </div>
      ${fields.length ? `<div class="pl-set"${p.enabled ? '' : ' data-off'}>${fields.map((f) => field(p, f)).join('')}</div>` : ''}
      ${pubLine(p)}
      ${pubOpen === p.id ? pubForm(p) : ''}
      <div class="pl-foot">
        ${/^https?:\/\//.test(str(p.homepage)) ? `<button class="btn ghost" data-pl-home="${esc(p.homepage)}">${icon('popout')}Страница плагина</button>` : ''}
        ${pubOpen === p.id ? '' : publish}
        <span class="grow"></span>
        ${update}
        <button class="btn ghost${sure ? ' danger' : ''}" data-pl-remove="${esc(p.folder)}">${icon('trash')}${sure ? 'Точно удалить?' : 'Удалить'}</button>
      </div>
    </div>`;
  }

  // Карточка плагина в магазине.
  function storeCard(it) {
    let btn;
    if (busy(it.id)) btn = '<button class="btn primary busy" disabled>Ставится</button>';
    else if (it.installed && !it.from_store) btn = `<button class="btn" disabled title="В папке плагинов уже есть свой плагин с id «${esc(it.id)}»">Есть свой</button>`;
    else if (it.installed && it.installed !== it.version) btn = `<button class="btn primary" data-pl-install="${esc(it.id)}">${icon('download')}Обновить до ${esc(it.version)}</button>`;
    else if (it.installed) btn = `<button class="btn ghost" disabled>${icon('check')}Установлен</button>`;
    else btn = `<button class="btn primary" data-pl-install="${esc(it.id)}">${icon('download')}Установить</button>`;
    const n = Number(it.installs) || 0;
    const meta = [it.author && esc(it.author), `версия ${esc(it.version)}`, `${n} ${plural(n, 'установка', 'установки', 'установок')}`].filter(Boolean).join(' · ');
    const perms = list(it.permissions).map((x) => PERM_NAMES[x]).filter(Boolean);
    const settings = Number(it.settings) || 0;
    return `<div class="card pl pl-sc">
      <div class="srow pl-top">${badge(it, it.icon, 'pl-ic')}
        <span class="sl"><b>${esc(it.name)}${it.official ? '<i class="pl-off">Axiom</i>' : ''}</b>${it.description ? `<small>${esc(it.description)}</small>` : ''}<small class="pl-meta">${meta}</small></span>
        <span class="sc">${btn}</span></div>
      <div class="pl-info">
        ${sitesTag(it.sites)}
        ${perms.map((x) => `<span class="pl-tag">${icon('check')}${esc(x)}</span>`).join('')}
        ${it.action ? `<span class="pl-tag">${icon('plus')}Кнопка на панели</span>` : ''}
        ${settings ? `<span class="pl-tag">${icon('sliders')}${settings} ${plural(settings, 'настройка', 'настройки', 'настроек')}</span>` : ''}
      </div>
    </div>`;
  }

  function typing(box) {
    const a = document.activeElement;
    return box.contains(a) && a.matches('input[type=text],input[type=number],textarea');
  }

  function syncSheet() {
    const box = $('#plList', sheet);
    if (!box) return;
    $$('[data-pl-tab]', sheet).forEach((b) => b.classList.toggle('on', b.dataset.plTab === tab));
    $('#plMine', sheet).hidden = tab !== 'mine';
    $('#plStore', sheet).hidden = tab !== 'store';
    syncStore();
    if (typing(box)) { stale = true; return; }
    stale = false;
    $('#plDir', sheet).textContent = L.dir || 'В папке профиля Axiom';
    const broken = L.broken.map((b) => `<div class="card pl bad"><div class="srow"><span class="pl-ic letter">!</span>
      <span class="sl"><b>Папка «${esc(b.folder)}»</b><small>${esc(b.error)}</small></span>
      <span class="sc"><button class="btn ghost" data-pl-remove="${esc(b.folder)}">${icon('trash')}${armed === b.folder ? 'Точно удалить?' : 'Удалить'}</button></span></div></div>`).join('');
    box.innerHTML = !L.items.length && !L.broken.length
      ? `<div class="card"><div class="srow pl-empty"><span class="sl"><b>Плагинов пока нет</b><small>Загляните в магазин — там готовые плагины ставятся в один клик. Или положите папку своего плагина в папку плагинов (кнопка «Открыть» выше) и нажмите «Обновить».</small></span><span class="sc"><button class="btn primary" data-pl-tab="store">${icon('puzzle')}Магазин</button></span></div></div>`
      : L.items.map(card).join('') + broken;
  }

  function syncStore() {
    const box = $('#plStoreList', sheet);
    if (!box || tab !== 'store') return;
    const q = findQ.trim().toLowerCase();
    const shown = q
      ? M.items.filter((it) => `${it.name} ${it.description} ${it.author} ${it.id} ${list(it.sites).join(' ')}`.toLowerCase().includes(q))
      : M.items;
    const msg = (title, note, retry) => `<div class="card"><div class="srow pl-empty"><span class="sl"><b>${title}</b>${note ? `<small>${note}</small>` : ''}</span>${retry ? `<span class="sc"><button class="btn" data-pl-act="store-reload">${icon('reload')}Повторить</button></span>` : ''}</div></div>`;
    if (!M.loaded) {
      box.innerHTML = M.error
        ? msg('Магазин недоступен', esc(M.error), true)
        : '<div class="pl-skel"><i></i><i></i><i></i></div>';
      return;
    }
    const fail = M.error ? msg('Каталог не обновился', esc(M.error), true) : '';
    box.innerHTML = fail + (!M.items.length ? msg('В магазине пока пусто', 'Опубликуйте свой плагин — кнопка «Опубликовать» в его карточке на вкладке «Установленные».', false)
      : !shown.length ? msg('Ничего не нашлось', '', false)
      : shown.map(storeCard).join(''));
  }

  function setTab(t) {
    tab = t === 'store' ? 'store' : 'mine';
    // Каталог — заново, если его не было или он старше минуты.
    if (tab === 'store' && (!M.loaded || Date.now() - openedAt > 60000)) openStore();
    syncSheet();
  }

  function openStore() {
    openedAt = Date.now();
    ax.send({ cmd: 'market_open' });
  }

  // Кнопки включённых плагинов на панели, рядом с кнопкой ИИ.
  function renderBar() {
    if (!bar) return;
    const acts = L.items.filter((p) => p.enabled && p.action);
    bar.hidden = !acts.length;
    bar.innerHTML = acts.map((p) => `<button class="ib pl-act" data-pl-action="${esc(p.id)}" title="${esc(p.action.title)} — ${esc(p.name)}">${badge(p, p.action.icon, 'pl-bi')}</button>`).join('');
  }

  function setValue(el, value) {
    ax.send({ cmd: 'plugin_settings', id: el.dataset.plId, values: { [el.dataset.plKey]: value } });
  }

  // Форма уходит: фокус из её поля — иначе список считал бы, что человек ещё печатает.
  function closeForm() {
    pubOpen = null;
    if (document.activeElement?.matches('.pl-pf')) document.activeElement.blur();
    syncSheet();
  }

  function disarm() {
    armed = null;
    clearTimeout(armTimer);
  }

  sheet.addEventListener('change', (e) => {
    const t = e.target;
    if (t.dataset.plOn) { ax.send({ cmd: 'plugin_toggle', id: t.dataset.plOn, on: t.checked }); return; }
    if (!t.dataset.plKey) return;
    if (t.type === 'checkbox') setValue(t, t.checked);
    else if (t.tagName === 'SELECT') setValue(t, t.value);
    else if (t.type === 'number') {
      const n = Number(t.value);
      if (t.value !== '' && Number.isFinite(n) && t.checkValidity()) setValue(t, n);
      else { t.classList.add('bad'); ax.toast('Число вне допустимых границ'); }
    } else setValue(t, t.value);
  });
  sheet.addEventListener('input', (e) => {
    if (e.target.classList.contains('pl-num')) e.target.classList.remove('bad');
    if (e.target.id === 'plFind') { findQ = e.target.value; syncStore(); }
  });
  sheet.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && e.target.matches('.pl-text,.pl-num')) e.target.blur();
  });
  // Ушли из поля — список можно перерисовать, если он менялся, пока печатали.
  sheet.addEventListener('focusout', (e) => {
    if (stale && e.target.matches('.pl-text,.pl-num,.pl-pf')) setTimeout(() => { if (!typing($('#plList', sheet))) syncSheet(); }, 0);
  });
  sheet.addEventListener('click', (e) => {
    const b = e.target.closest('button');
    if (!b) return;
    if (b.dataset.plTab) { setTab(b.dataset.plTab); return; }
    const act = b.dataset.plAct;
    if (act === 'folder') { ax.send({ cmd: 'plugins_folder' }); return; }
    if (act === 'reload') { ax.send({ cmd: 'plugins_reload' }); return; }
    if (act === 'store-reload') { M.error = ''; openStore(); return; }
    if (act === 'docs') { ax.send({ cmd: 'new_tab', input: PLUGINS_DOCS }); ax.closeSettings(); return; }
    if (act === 'pub-cancel') { closeForm(); return; }
    if (b.dataset.plHome) { ax.send({ cmd: 'new_tab', input: b.dataset.plHome }); ax.closeSettings(); return; }
    if (b.dataset.plInstall) { ax.send({ cmd: 'market_install', id: b.dataset.plInstall }); return; }
    if (b.dataset.plPub) {
      pubOpen = b.dataset.plPub;
      syncSheet();
      const first = $('.pl-pub .pl-pf', sheet);
      if (first) first.focus();
      return;
    }
    if (b.dataset.plSend) {
      const form = b.closest('.pl-pub');
      const v = (f) => str($(`.pl-pf[data-f="${f}"]`, form)?.value).trim();
      ax.send({ cmd: 'plugin_publish', id: b.dataset.plSend, author: v('author'), contact: v('contact'), note: v('note') });
      closeForm();
      return;
    }
    const seg = b.closest('.seg[data-pl-key]');
    if (seg && b.dataset.v != null) {
      seg.querySelectorAll('button').forEach((x) => x.classList.toggle('on', x === b));
      setValue(seg, b.dataset.v);
      return;
    }
    const folder = b.dataset.plRemove;
    if (folder != null) {
      if (armed === folder) { disarm(); ax.send({ cmd: 'plugin_remove', folder }); return; }
      disarm();
      armed = folder;
      armTimer = setTimeout(() => { disarm(); syncSheet(); }, 4000);
      syncSheet();
    }
  });
  if (bar) {
    bar.addEventListener('click', (e) => {
      const b = e.target.closest('[data-pl-action]');
      if (b) ax.send({ cmd: 'plugin_action', id: b.dataset.plAction });
    });
  }

  // Из Rust: список плагинов (при запуске и после каждой перемены).
  function onList(m) {
    const items = list(m.items).filter((p) => p && typeof p.id === 'string' && typeof p.name === 'string' && typeof p.folder === 'string');
    L = {
      dir: str(m.dir),
      items,
      broken: list(m.broken).filter((b) => b && typeof b.folder === 'string' && typeof b.error === 'string'),
    };
    if (armed && !items.some((p) => p.folder === armed) && !L.broken.some((b) => b.folder === armed)) disarm();
    if (pubOpen && !items.some((p) => p.id === pubOpen)) pubOpen = null;
    syncSheet();
    renderBar();
  }

  // Из Rust: магазин (каталог, загрузка, ошибка, что ставится).
  function onMarket(m) {
    M = {
      loaded: !!m.loaded,
      loading: !!m.loading,
      error: str(m.error),
      items: list(m.items).filter((it) => it && typeof it.id === 'string' && typeof it.name === 'string' && typeof it.version === 'string'),
      busy: list(m.busy).filter((x) => typeof x === 'string'),
    };
    syncSheet();
  }

  return { sectionHtml, syncSheet, onList, onMarket };
}
