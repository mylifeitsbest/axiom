'use strict';
// Перевод страниц: кнопка 文A в адресной строке, окошко под ней и раздел настроек.
// Что, когда и сколько переводить, решает Rust; отсюда уходят только «переведи»,
// «покажи оригинал» и настройки перевода.
function createTranslate(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const btn = $('#trBtn');
  const panel = $('#trp');
  const sheet = $('#sheet');
  const TARGETS = ['ru', 'en', 'uk', 'de', 'fr', 'es', 'zh', 'ja', 'ko', 'tr'];
  const CODE = /^[a-z]{2,3}$/;

  let names = null;
  try { names = new Intl.DisplayNames(['ru'], { type: 'language' }); } catch { names = null; }
  const known = (code) => typeof code === 'string' && CODE.test(code) && code !== 'und';
  // «английский», «японский»: для «на …» и «всегда переводить …» склонять не нужно.
  function langName(code) {
    if (!known(code)) return '';
    let name = '';
    try { name = names?.of(code) || ''; } catch { name = ''; }
    return name && name !== code ? name : code.toUpperCase();
  }
  const cap = (s) => (s ? s[0].toUpperCase() + s.slice(1) : s);

  let P = { to: 'ru', always: [], offer: true };
  // Состояние кнопки по вкладкам: присылает Rust.
  const views = new Map();
  let tab = null;
  // Только для разработки (#translate, #translate:menu): нажать кнопку самим.
  let debug = '';
  let debugSent = false;

  const view = () => (tab == null ? null : views.get(tab));
  const phase = () => view()?.phase || 'none';
  const isOpen = () => panel.classList.contains('on');

  // ---------- кнопка ----------
  function renderBtn() {
    const v = view();
    const p = phase();
    const appear = p !== 'none' && btn.hidden;
    btn.hidden = p === 'none';
    btn.classList.toggle('busy', p === 'busy');
    btn.classList.toggle('on', p === 'done' || isOpen());
    const to = langName(v?.to || P.to);
    btn.title = p === 'busy' ? `Перевожу на ${to}…`
      : p === 'done' ? `Страница переведена на ${to}`
      : `Перевести страницу на ${to}`;
    if (appear && ax.speed()) {
      btn.classList.remove('in');
      void btn.offsetWidth;
      btn.classList.add('in');
    }
  }
  btn.addEventListener('animationend', (e) => { if (e.target === btn) btn.classList.remove('in'); });

  btn.addEventListener('click', () => {
    const p = phase();
    if (p === 'offer') ax.send({ cmd: 'translate' });
    else if (p === 'busy' || p === 'done') toggle();
  });

  // ---------- окошко ----------
  const options = (sel) => TARGETS.map((c) => `<option value="${c}"${c === sel ? ' selected' : ''}>${ax.esc(cap(langName(c)))}</option>`).join('');

  function renderPanel() {
    const v = view();
    if (!v) return;
    const from = langName(v.lang);
    const to = langName(v.to || P.to);
    const busy = v.phase === 'busy';
    const always = known(v.lang) && P.always.includes(v.lang);
    panel.innerHTML = `<div class="trp-head"><span class="trp-ic">${icon('translate')}</span>
        <span class="trp-t"><b>${busy ? 'Перевожу страницу…' : 'Страница переведена'}</b>
        <small>${from ? `${ax.esc(cap(from))} → ` : ''}${ax.esc(cap(to))}</small></span></div>
      ${busy ? '<span class="dl-bar ind trp-bar"><i></i></span>' : ''}
      <div class="trp-list">
        <button class="trp-row" data-t="original">${icon('reload')}<span>Показать оригинал</span></button>
        ${known(v.lang) ? `<label class="trp-row"><span>Всегда переводить ${ax.esc(from)}</span><input type="checkbox" class="tg" data-t="always"${always ? ' checked' : ''}></label>` : ''}
        <label class="trp-row"><span>Переводить на</span><select data-t="to">${options(v.to || P.to)}</select></label>
      </div>
      <p class="trp-foot">Переводит Google Переводчик</p>`;
  }

  function position() {
    const r = btn.getBoundingClientRect();
    const w = Math.min(300, innerWidth - 16);
    panel.style.width = w + 'px';
    const left = Math.min(Math.max(8, r.left + r.width / 2 - w + 30), innerWidth - 8 - w);
    Object.assign(panel.style, { left: Math.round(left) + 'px', top: Math.round(r.bottom + 8) + 'px' });
  }
  function open() {
    if (isOpen() || btn.hidden) return;
    renderPanel();
    panel.classList.add('on');
    position();
    ax.setOverlay('trp', true);
    renderBtn();
  }
  function close() {
    if (!isOpen()) return;
    panel.classList.remove('on');
    ax.setOverlay('trp', false);
    renderBtn();
  }
  const toggle = () => (isOpen() ? close() : open());

  document.addEventListener('mousedown', (e) => {
    if (isOpen() && !e.target.closest('#trp,#trBtn')) close();
  }, true);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && isOpen()) { e.preventDefault(); e.stopImmediatePropagation(); close(); }
  }, true);
  addEventListener('resize', () => { if (isOpen()) position(); });

  panel.addEventListener('click', (e) => {
    if (e.target.closest('[data-t=original]')) {
      ax.send({ cmd: 'translate_original' });
      close();
    }
  });
  panel.addEventListener('change', (e) => {
    const el = e.target;
    const v = view();
    if (el.dataset.t === 'always' && known(v?.lang)) {
      setAlways(v.lang, el.checked);
      ax.toast(el.checked ? `${cap(langName(v.lang))} будет переводиться сам` : 'Автоперевод выключен');
    } else if (el.dataset.t === 'to' && TARGETS.includes(el.value)) {
      setTarget(el.value);
      close();
    }
  });

  // ---------- настройки ----------
  function sendPrefs() {
    ax.send({ cmd: 'translate_prefs', prefs: { to: P.to, always: P.always, offer: P.offer } });
    syncSheet();
  }
  function setAlways(code, on) {
    P.always = P.always.filter((c) => c !== code);
    if (on && code !== P.to) P.always.push(code);
    sendPrefs();
  }
  function setTarget(code) {
    P.to = code;
    P.always = P.always.filter((c) => c !== code);
    sendPrefs();
  }

  function onSettings(s) {
    const t = s && typeof s === 'object' ? s : {};
    P = {
      to: TARGETS.includes(t.to) ? t.to : 'ru',
      always: Array.isArray(t.always) ? t.always.filter(known) : [],
      offer: t.offer !== false,
    };
    syncSheet();
    if (isOpen()) renderPanel();
  }

  const srow = (title, note, control) =>
    `<div class="srow"><span class="sl"><b>${title}</b>${note ? `<small>${note}</small>` : ''}</span><span class="sc">${control}</span></div>`;
  function sectionHtml() {
    return `<div class="card">
        ${srow('Предлагать перевод', 'Кнопка 文A появляется в адресной строке на страницах не на языке перевода', '<input type="checkbox" class="tg" data-tr="offer">')}
        ${srow('Переводить на', '', `<select data-tr="to">${options(P.to)}</select>`)}
      </div>
      <h4>Всегда переводить</h4>
      <div class="card" id="trAlways"></div>
      <div class="note">${icon('lock')}<span>Текст страницы уходит в Google Переводчик, только когда вы нажали 文A или включили «Всегда переводить» для её языка. Сам по себе Axiom ничего не отправляет.</span></div>`;
  }
  function syncSheet() {
    const offer = $('[data-tr=offer]', sheet);
    if (!offer) return;
    offer.checked = !!P.offer;
    const to = $('[data-tr=to]', sheet);
    if (document.activeElement !== to) to.value = P.to;
    $('#trAlways', sheet).innerHTML = P.always.length
      ? P.always.map((c) => srow(ax.esc(cap(langName(c))), '', `<button class="ib sm" data-tr-rm="${c}" title="Больше не переводить сам">${icon('x')}</button>`)).join('')
      : srow('Пока ни одного языка', 'Переведите страницу и включите «Всегда переводить» в окошке перевода', '');
  }
  sheet.addEventListener('change', (e) => {
    const el = e.target;
    if (el.dataset.tr === 'offer') { P.offer = el.checked; sendPrefs(); }
    else if (el.dataset.tr === 'to' && TARGETS.includes(el.value)) setTarget(el.value);
  });
  sheet.addEventListener('click', (e) => {
    const b = e.target.closest('[data-tr-rm]');
    if (b) setAlways(b.dataset.trRm, false);
  });

  // ---------- сообщения из Rust ----------
  function onState(m) {
    if (typeof m.id !== 'number') return;
    views.set(m.id, m);
    if (m.id !== tab) return;
    if (isOpen()) { if (m.phase === 'busy' || m.phase === 'done') renderPanel(); else close(); }
    renderBtn();
    if (debug && !debugSent && m.phase === 'offer') { debugSent = true; ax.send({ cmd: 'translate' }); }
    if (debug && m.phase === 'done') {
      if (debug === 'menu') setTimeout(open, 300);
      if (debug === 'revert') setTimeout(() => ax.send({ cmd: 'translate_original' }), 8000);
      debug = '';
    }
  }

  // Из app.js при каждой отрисовке адресной строки.
  function render(t, tabs) {
    if (Array.isArray(tabs)) for (const id of views.keys()) if (!tabs.some((x) => x.id === id)) views.delete(id);
    const id = t && !t.sleeping && !/^axiom:/.test(t.url) ? t.id : null;
    if (id !== tab) { tab = id; close(); }
    renderBtn();
  }

  return { render, onState, onSettings, sectionHtml, syncSheet, isOpen, close, debug: (arg) => { debug = arg || 'go'; } };
}
