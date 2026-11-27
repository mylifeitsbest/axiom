'use strict';
// Плагины: раздел «Плагины» в настройках и кнопки плагинов на панели.
// Список, включение, настройки и удаление решает Rust (src/app/plugins.rs):
// сюда приходят списки, обратно — команды. Код плагинов в интерфейс не попадает.

// Как писать плагины: документация в репозитории.
const PLUGINS_DOCS = 'https://github.com/mylifeitsbest/axiom/blob/main/docs/plugins.md';

function createPlugins(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const { esc } = ax;
  const sheet = $('#sheet');
  const bar = $('#plActs');

  let L = { dir: '', items: [], broken: [] };
  // «Удалить» нажали один раз: второй клик в течение нескольких секунд удаляет.
  let armed = null;
  let armTimer = 0;
  // Пока человек печатает в поле настройки, список не перерисовываем (уйдёт фокус).
  let stale = false;

  const str = (v) => (typeof v === 'string' ? v : '');
  const list = (v) => (Array.isArray(v) ? v : []);
  // Значок плагина — только data: из Rust (CSP интерфейса другие не пустит), иначе буква.
  const imgOk = (src) => typeof src === 'string' && src.startsWith('data:image/');
  const badge = (p, src, cls) => imgOk(src)
    ? `<span class="${cls}"><img src="${esc(src)}" alt=""></span>`
    : `<span class="${cls} letter">${esc((p.name || '?').trim().charAt(0).toUpperCase())}</span>`;
  const site = (m) => (m === '<all_urls>' || m === '*' ? 'все сайты' : m.replace(/^(\*|https?):\/\//, '').replace(/\/\*$/, ''));

  function sectionHtml() {
    return `
      <div class="card">
        <div class="srow" data-kw="плагины расширения папка установить добавить свои скрипты"><span class="sl"><b>Папка плагинов</b><small id="plDir"></small></span>
          <span class="sc"><button class="btn" data-pl-act="folder">${icon('folder')}Открыть</button><button class="btn" data-pl-act="reload" title="Перечитать папку: новые, изменённые и удалённые плагины">${icon('reload')}Обновить</button></span></div>
      </div>
      <div id="plList"></div>
      <div class="note">${icon('shield')}<span>Плагин видит и меняет страницы сайтов, на которых работает, — как расширение в Chrome. Включайте только плагины, которым доверяете. Новый плагин в папке сам не включается; что ему можно сверх страницы, видно в его карточке.</span></div>
      <div class="note">${icon('file')}<span>Свой плагин — это папка с файлом plugin.json и скриптами. Положите её в папку плагинов и нажмите «Обновить». <button class="lnk" data-pl-act="docs">Как написать плагин</button></span></div>`;
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

  function card(p) {
    const sites = list(p.sites).map(site);
    const meta = [p.version && `версия ${esc(p.version)}`, p.author && esc(p.author)].filter(Boolean).join(' · ');
    const perms = list(p.permissions);
    const fields = list(p.settings);
    const sure = armed === p.folder;
    return `<div class="card pl${p.enabled ? ' on' : ''}" data-kw="${esc(`${p.name} ${p.id} ${p.description}`.toLowerCase())}">
      <div class="srow pl-top">${badge(p, p.icon, 'pl-ic')}
        <span class="sl"><b>${esc(p.name)}</b>${p.description ? `<small>${esc(p.description)}</small>` : ''}${meta ? `<small class="pl-meta">${meta}</small>` : ''}</span>
        <span class="sc"><input type="checkbox" class="tg" data-pl-on="${esc(p.id)}"${p.enabled ? ' checked' : ''} aria-label="Включить «${esc(p.name)}»"></span></div>
      <div class="pl-info">
        <span class="pl-tag">${icon('globe')}${esc(sites.slice(0, 4).join(', '))}${sites.length > 4 ? ` и ещё ${sites.length - 4}` : ''}</span>
        ${perms.map((x) => `<span class="pl-tag">${icon('check')}${esc(x)}</span>`).join('')}
        ${p.action ? `<span class="pl-tag">${icon('plus')}Кнопка на панели</span>` : ''}
      </div>
      ${fields.length ? `<div class="pl-set"${p.enabled ? '' : ' data-off'}>${fields.map((f) => field(p, f)).join('')}</div>` : ''}
      <div class="pl-foot">
        ${/^https?:\/\//.test(str(p.homepage)) ? `<button class="btn ghost" data-pl-home="${esc(p.homepage)}">${icon('popout')}Страница плагина</button>` : ''}
        <span class="grow"></span>
        <button class="btn ghost${sure ? ' danger' : ''}" data-pl-remove="${esc(p.folder)}">${icon('trash')}${sure ? 'Точно удалить?' : 'Удалить'}</button>
      </div>
    </div>`;
  }

  function syncSheet() {
    const box = $('#plList', sheet);
    if (!box) return;
    if (box.contains(document.activeElement) && document.activeElement.matches('input[type=text],input[type=number]')) {
      stale = true;
      return;
    }
    stale = false;
    $('#plDir', sheet).textContent = L.dir || 'В папке профиля Axiom';
    const broken = L.broken.map((b) => `<div class="card pl bad"><div class="srow"><span class="pl-ic letter">!</span>
      <span class="sl"><b>Папка «${esc(b.folder)}»</b><small>${esc(b.error)}</small></span>
      <span class="sc"><button class="btn ghost" data-pl-remove="${esc(b.folder)}">${icon('trash')}${armed === b.folder ? 'Точно удалить?' : 'Удалить'}</button></span></div></div>`).join('');
    box.innerHTML = !L.items.length && !L.broken.length
      ? `<div class="card"><div class="srow pl-empty"><span class="sl"><b>Плагинов пока нет</b><small>Скопируйте папку плагина в папку плагинов (кнопка «Открыть» выше) и нажмите «Обновить». Примеры лежат в репозитории Axiom, в examples/plugins</small></span></div></div>`
      : L.items.map(card).join('') + broken;
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
  sheet.addEventListener('input', (e) => { if (e.target.classList.contains('pl-num')) e.target.classList.remove('bad'); });
  sheet.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && e.target.matches('.pl-text,.pl-num')) e.target.blur();
  });
  // Ушли из поля — список можно перерисовать, если он менялся, пока печатали.
  sheet.addEventListener('focusout', (e) => {
    if (stale && e.target.matches('.pl-text,.pl-num')) setTimeout(syncSheet, 0);
  });
  sheet.addEventListener('click', (e) => {
    const b = e.target.closest('button');
    if (!b) return;
    const act = b.dataset.plAct;
    if (act === 'folder') { ax.send({ cmd: 'plugins_folder' }); return; }
    if (act === 'reload') { ax.send({ cmd: 'plugins_reload' }); return; }
    if (act === 'docs') { ax.send({ cmd: 'new_tab', input: PLUGINS_DOCS }); ax.closeSettings(); return; }
    if (b.dataset.plHome) { ax.send({ cmd: 'new_tab', input: b.dataset.plHome }); ax.closeSettings(); return; }
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
    syncSheet();
    renderBar();
  }

  return { sectionHtml, syncSheet, onList };
}
