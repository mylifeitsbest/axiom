'use strict';
// Меню вкладки по правому клику: перезагрузить, дублировать, звук и громкость,
// «не усыплять», закрыть эту, другие или те, что справа, и вернуть закрытую.
// Отсюда уходят только команда и номер вкладки (и громкость): что можно сделать, решает Rust.

// Значок громкости: 0 % — без волн, ниже 100 % — одна волна.
const volIcon = (v) => (v <= 0 ? 'vol0' : v < 100 ? 'vol1' : 'vol');
// Шаг колеса и стрелок — 5 %, с привязкой к кратным 5: 37 → 40 или 35.
const volStep = (v, up) => (up ? Math.min(100, Math.floor(v / 5) * 5 + 5) : Math.max(0, Math.ceil(v / 5) * 5 - 5));
// Служебная вкладка (axiom://newtab, axiom://settings): громкость ей Rust не меняет.
const isInternal = (t) => String(t.url).startsWith('axiom://');

function createTabMenu(ax) {
  const menu = document.createElement('div');
  menu.className = 'dlp tabm';
  menu.id = 'tabm';
  menu.setAttribute('role', 'menu');
  menu.setAttribute('aria-label', 'Вкладка');
  document.getElementById('app').append(menu);

  const COMMANDS = {
    reload: 'reload_tab', duplicate: 'duplicate_tab', mute: 'toggle_mute', nosleep: 'toggle_no_sleep',
    close: 'close_tab', others: 'close_other_tabs', right: 'close_tabs_right',
  };
  let tabId = null;
  let at = { x: 0, y: 0 };
  let sel = -1;
  let shown = '';

  const isOpen = () => menu.classList.contains('on');
  const tab = () => ax.tabs().find((t) => t.id === tabId);

  function items(t) {
    const tabs = ax.tabs();
    const i = tabs.findIndex((x) => x.id === t.id);
    const page = t.url !== ax.NEWTAB;
    return [
      { a: 'reload', ic: 'reload', t: 'Перезагрузить', k: 'Ctrl+R', off: !page },
      { a: 'duplicate', ic: 'copy', t: 'Дублировать' },
      { a: 'mute', ic: t.muted ? 'vol' : 'mute', t: t.muted ? 'Включить звук' : 'Выключить звук', k: 'Ctrl+M', off: !page },
      // Значение ползунка в ключ не входит: меню не перестраивается, пока его тянут.
      // У служебных вкладок (новая, настройки) страницы нет — и громкости тоже.
      { vol: true, off: isInternal(t) },
      { a: 'nosleep', ic: 'cup', t: 'Не усыплять', check: !!t.no_sleep, off: !page },
      null,
      { a: 'close', ic: 'x', t: 'Закрыть', k: 'Ctrl+W' },
      { a: 'others', t: 'Закрыть другие вкладки', off: tabs.length < 2 },
      { a: 'right', t: 'Закрыть вкладки справа', off: i < 0 || i === tabs.length - 1 },
      null,
      { a: 'restore', ic: 'undo', t: 'Открыть закрытую вкладку', k: 'Ctrl+Shift+T', off: !ax.canRestore() },
    ];
  }

  function render(t) {
    const list = items(t);
    const key = JSON.stringify(list);
    if (key === shown) { syncVolume(t, false); return; }
    shown = key;
    menu.innerHTML = list.map((it) => {
      if (!it) return '<i class="tm-sep" role="separator"></i>';
      if (it.vol) {
        return `<div class="tm-vol${it.off ? ' off' : ''}" role="group" aria-label="Громкость">`
          + '<span class="tm-ic"></span><span class="tm-t">Громкость</span><span class="tm-k tm-pct"></span>'
          + `<span class="rng tm-rng"><input type="range" min="0" max="100" step="1" aria-label="Громкость вкладки"${it.off ? ' disabled' : ''}>`
          + '<output class="bub" aria-hidden="true"></output></span></div>';
      }
      const checkable = it.check !== undefined;
      const tail = checkable ? `<span class="tm-ok"${it.check ? '' : ' hidden'}>${icon('check')}</span>` : it.k ? `<span class="tm-k">${it.k}</span>` : '';
      return `<button class="tm-i" data-a="${it.a}" role="${checkable ? 'menuitemcheckbox' : 'menuitem'}"${checkable ? ` aria-checked="${it.check}"` : ''}${it.off ? ' disabled' : ''}>`
        + `<span class="tm-ic">${it.ic ? icon(it.ic) : ''}</span><span class="tm-t">${it.t}</span>${tail}</button>`;
    }).join('');
    highlight();
    syncVolume(t, true);
  }

  // ---------- громкость ----------
  const slider = () => menu.querySelector('.tm-rng input');
  // Своё изменение только что ушло в Rust: его ответ со старым значением бегунок не дёргает.
  let touched = 0;

  // Ползунок, проценты, значок и пузырёк над бегунком — по громкости `v`.
  function showVolume(v) {
    const r = slider();
    if (!r) return;
    if (+r.value !== v) r.value = v;
    r.style.setProperty('--fill', `${v}%`);
    const wrap = r.parentElement;
    wrap.style.setProperty('--p', v / 100);
    wrap.querySelector('.bub').textContent = `${v}%`;
    const row = wrap.parentElement;
    row.querySelector('.tm-pct').textContent = `${v}%`;
    const ic = row.querySelector('.tm-ic');
    if (ic.dataset.i !== volIcon(v)) { ic.dataset.i = volIcon(v); ic.innerHTML = icon(volIcon(v)); }
  }

  // `fresh` — меню только что перестроено, ползунок новый.
  function syncVolume(t, fresh) {
    const r = slider();
    if (!r) return;
    const busy = r.parentElement.classList.contains('drag') || performance.now() - touched < 500;
    if (fresh || !busy) showVolume(t.volume ?? 100);
  }

  function setVolume(v) {
    const t = tab();
    if (!t || isInternal(t) || v === (t.volume ?? 100)) { showVolume(v); return; }
    touched = performance.now();
    t.volume = v;
    showVolume(v);
    ax.send({ cmd: 'set_volume', id: t.id, volume: v });
  }

  // Меню целиком в окне: у правого края сдвигается влево, у нижнего — открывается вверх.
  function place() {
    const w = menu.offsetWidth;
    const h = menu.offsetHeight;
    const left = Math.max(8, Math.min(at.x, innerWidth - 8 - w));
    const down = at.y + 2 + h <= innerHeight - 8;
    menu.classList.toggle('up', !down);
    Object.assign(menu.style, { left: left + 'px', top: (down ? at.y + 2 : Math.max(8, at.y - 2 - h)) + 'px' });
  }

  // Вкладка, чьё меню открыто, подсвечена, как под курсором.
  function mark() {
    document.querySelectorAll('.tab.ctx').forEach((el) => { if (+el.dataset.id !== tabId) el.classList.remove('ctx'); });
    document.querySelector(`.tab[data-id="${tabId}"]`)?.classList.add('ctx');
  }

  function open(id, x, y) {
    tabId = id;
    const t = tab();
    if (!t) { close(); return; }
    at = { x, y };
    sel = -1;
    shown = '';
    render(t);
    const was = isOpen();
    menu.classList.add('on');
    if (!was) {
      menu.classList.add('anim');
      setTimeout(() => menu.classList.remove('anim'), ax.ms(400) + 50);
    }
    place();
    mark();
    if (!was) ax.setOverlay('tabm', true);
  }

  function close() {
    if (!isOpen()) return;
    if (menu.contains(document.activeElement)) document.activeElement.blur();
    menu.classList.remove('on');
    tabId = null;
    mark();
    ax.setOverlay('tabm', false);
  }

  function run(a) {
    const id = tabId;
    close();
    if (id == null) return;
    if (a === 'restore') ax.send({ cmd: 'restore_tab' });
    else if (COMMANDS[a]) ax.send({ cmd: COMMANDS[a], id });
  }

  const enabled = () => Array.from(menu.querySelectorAll('.tm-i:not([disabled])'));
  function highlight() {
    enabled().forEach((b, i) => b.classList.toggle('on', i === sel));
  }

  // Вкладку закрыли или она поменялась (звук, «не усыплять») — меню вслед за ней.
  function sync() {
    if (!isOpen()) return;
    const t = tab();
    if (!t) { close(); return; }
    render(t);
    place();
    mark();
  }

  // Фокус остаётся, где был; ползунок громкости тянется как обычно.
  menu.addEventListener('mousedown', (e) => { if (!e.target.closest('.tm-rng')) e.preventDefault(); });
  // Громкость применяется сразу, меню не закрывается.
  menu.addEventListener('input', (e) => {
    if (e.target.matches('.tm-rng input')) setVolume(+e.target.value);
  });
  // Пузырёк над бегунком держится, пока его тянут, даже если мышь ушла с ползунка.
  menu.addEventListener('pointerdown', (e) => {
    const wrap = e.target.closest('.tm-rng');
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
  menu.addEventListener('click', (e) => {
    const b = e.target.closest('.tm-i');
    if (b && !b.disabled) run(b.dataset.a);
  });
  menu.addEventListener('mousemove', (e) => {
    const b = e.target.closest('.tm-i');
    const i = b ? enabled().indexOf(b) : -1;
    if (i !== sel) { sel = i; highlight(); }
  });
  menu.addEventListener('mouseleave', () => { sel = -1; highlight(); });
  // Клик мимо закрывает меню. Правый клик по другой вкладке сразу откроет её меню.
  document.addEventListener('mousedown', (e) => {
    if (!isOpen() || e.target.closest('#tabm')) return;
    if (e.button === 2 && e.target.closest('.tab')) return;
    close();
  }, true);
  addEventListener('resize', close);
  addEventListener('blur', close);
  // Клавиатура: стрелки вверх-вниз и Enter — по пунктам, влево-вправо — громкость,
  // Esc закрывает, остальные клавиши (в том числе сочетания) закрывают меню и делают своё дело.
  document.addEventListener('keydown', (e) => {
    if (!isOpen()) return;
    const stop = () => { e.preventDefault(); e.stopImmediatePropagation(); };
    const list = enabled();
    if (e.key === 'Escape') { stop(); close(); }
    else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      stop();
      if (!list.length) return;
      const n = list.length;
      sel = e.key === 'ArrowDown' ? (sel + 1) % n : sel < 0 ? n - 1 : (sel - 1 + n) % n;
      highlight();
    } else if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      stop();
      const r = slider();
      if (r && !r.disabled) setVolume(volStep(+r.value, e.key === 'ArrowRight'));
    } else if ((e.key === 'Enter' || e.key === ' ') && list[sel]) { stop(); run(list[sel].dataset.a); }
    else if (e.key === 'Tab') stop();
    else if (!['Shift', 'Control', 'Alt', 'Meta'].includes(e.key)) close();
  }, true);

  return { open, close, isOpen, sync };
}
