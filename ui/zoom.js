'use strict';
// Масштаб страницы, как в Chrome: лупа в адресной строке, пока масштаб не 100 %,
// и окошко под ней с процентами, −, + и «Сбросить». После каждой смены масштаба
// окошко показывается само и прячется через пару секунд, если мышь не над ним.
//
// Страница — нативный webview поверх интерфейса, поэтому, пока окошко на экране,
// Rust прорезает в ней дыру ровно по окошку (zoom_bubble): страница остаётся живой
// и видна, а не прячется под снимок, как под другими окнами. Окошко ничего не
// фокусирует; после щелчка по его кнопкам Rust возвращает фокус странице.
function createZoom(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const btn = $('#zoomBtn');
  const panel = $('#zmp');
  // Столько окошко держится после смены масштаба (пока мышь над ним — дольше).
  const HIDE_AFTER = 2000;
  // Сколько ждать, пока страница снова закроет место окошка, прежде чем его прятать.
  const UNCOVER = 60;

  let tab = null;
  let pct = 100;
  let shown = false;
  let hover = false;
  let timer;
  let hideTimer;

  const isOpen = () => shown;
  const label = (p) => `${p} %`;

  // ---------- лупа ----------
  function renderBtn() {
    const show = ax.isPage() && (pct !== 100 || shown);
    const appear = show && btn.hidden;
    btn.hidden = !show;
    const name = pct < 100 ? 'zoomOut' : 'zoomIn';
    if (btn.dataset.i !== name) { btn.dataset.i = name; btn.innerHTML = icon(name); }
    btn.classList.toggle('on', shown);
    btn.title = `Масштаб: ${label(pct)}`;
    if (appear && ax.speed()) {
      btn.classList.remove('in');
      void btn.offsetWidth;
      btn.classList.add('in');
    }
  }
  btn.addEventListener('animationend', (e) => { if (e.target === btn) btn.classList.remove('in'); });
  btn.addEventListener('click', () => (shown ? close() : open()));

  // ---------- окошко ----------
  panel.innerHTML = `<b class="zmp-v" aria-live="polite"></b>
    <span class="zmp-pm">
      <button class="ib sm" data-z="out" title="Уменьшить (Ctrl+−)">${icon('minus')}</button>
      <button class="ib sm" data-z="in" title="Увеличить (Ctrl+=)">${icon('plus')}</button>
    </span>
    <button class="btn ghost" data-z="reset" title="Вернуть 100 % (Ctrl+0)">Сбросить</button>`;
  const value = $('.zmp-v', panel);

  function renderPanel() {
    value.textContent = label(pct);
    $('[data-z=out]', panel).disabled = pct <= 25;
    $('[data-z=in]', panel).disabled = pct >= 500;
  }

  // Под лупой, правым краем к ней, как окошко перевода. Пока в строке набирают
  // адрес, лупы не видно — тогда под правым краем строки. Окошко целиком над
  // страницей, с отступом от её края: прижатое к краю, оно оставляло бы над собой
  // полоску страницы в пиксель. На панель ИИ не заходит: она тоже нативный
  // webview и закрыла бы окошко.
  function position() {
    const anchor = btn.offsetWidth ? btn : $('#omni');
    const r = anchor.getBoundingClientRect();
    const page = ax.page();
    const w = panel.offsetWidth;
    const h = panel.offsetHeight;
    const right = Math.min(innerWidth, page.right) - 8;
    const left = Math.round(Math.max(8, Math.min(r.right + 6 - w, right - w)));
    // Панель у нижнего края окна (лупа ниже страницы) — окошко над лупой.
    const top = Math.round(r.top >= page.bottom - 1 ? Math.min(r.top - 8 - h, page.bottom - 6 - h) : Math.max(r.bottom + 8, page.top + 6));
    Object.assign(panel.style, { left: left + 'px', top: top + 'px' });
    return { x: left, y: top, w, h };
  }
  let lastHole = '';
  function sendHole(rect) {
    const radius = rect ? parseFloat(getComputedStyle(panel).borderTopLeftRadius) || 0 : 0;
    const msg = { cmd: 'zoom_bubble', rect, radius: Math.min(64, Math.max(0, radius)) };
    const key = JSON.stringify(msg);
    if (key !== lastHole) { lastHole = key; ax.send(msg); }
  }

  function open() {
    if (!ax.isPage()) return;
    clearTimeout(hideTimer);
    renderPanel();
    renderBtn();
    if (shown) {
      sendHole(position());
    } else {
      shown = true;
      panel.classList.remove('anim');
      panel.classList.add('on');
      const rect = position();
      if (ax.speed()) panel.classList.add('anim');
      // Вырез — когда окошко уже нарисовано: иначе на кадр в нём был бы виден
      // пустой фон интерфейса.
      requestAnimationFrame(() => requestAnimationFrame(() => { if (shown) sendHole(rect); }));
    }
    arm();
  }
  function arm() {
    clearTimeout(timer);
    if (!hover) timer = setTimeout(close, HIDE_AFTER);
  }
  function close() {
    clearTimeout(timer);
    if (!shown) return;
    shown = false;
    hover = false;
    // Сначала страница снова закрывает это место, потом прячем само окошко —
    // иначе на миг мелькнул бы фон интерфейса под ним.
    sendHole(null);
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => { panel.classList.remove('on', 'anim'); renderBtn(); }, UNCOVER);
  }

  panel.addEventListener('mouseenter', () => { hover = true; clearTimeout(timer); });
  panel.addEventListener('mouseleave', () => { hover = false; if (shown) arm(); });
  // Кнопки не забирают фокус и не выделяют текст.
  panel.addEventListener('mousedown', (e) => e.preventDefault());
  panel.addEventListener('click', (e) => {
    const b = e.target.closest('[data-z]');
    if (b && !b.disabled && shown) ax.send({ cmd: 'zoom', step: b.dataset.z });
  });
  // Ctrl+колёсико над окошком — тоже масштаб страницы: окошко лежит над ней.
  let wheel = 0;
  panel.addEventListener('wheel', (e) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    const dy = e.deltaMode === 1 ? e.deltaY * 40 : e.deltaY;
    if (wheel * dy < 0) wheel = 0;
    wheel += dy;
    if (Math.abs(wheel) < 40) return;
    ax.send({ cmd: 'zoom', step: wheel < 0 ? 'in' : 'out' });
    wheel = 0;
  }, { passive: false });

  document.addEventListener('mousedown', (e) => {
    if (shown && !e.target.closest('#zmp,#zoomBtn')) close();
  }, true);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && shown) { e.preventDefault(); e.stopImmediatePropagation(); close(); }
  }, true);
  addEventListener('resize', () => { if (shown) sendHole(position()); });

  // ---------- из Rust ----------
  // Состояние вкладок: масштаб активной. Сменили вкладку или ушли со страницы —
  // окошко прежней ни к чему.
  function render(t) {
    const id = t ? t.id : null;
    if (shown && (id !== tab || !ax.isPage())) close();
    tab = id;
    pct = Number.isFinite(t?.zoom) ? t.zoom : 100;
    renderBtn();
    if (shown) { renderPanel(); sendHole(position()); }
  }
  // Масштаб поменяли клавишами, колёсиком или кнопками окошка.
  function onZoom(m) {
    if (m.id !== tab || !Number.isFinite(m.zoom)) return;
    pct = m.zoom;
    open();
  }

  return { render, onZoom, close, isOpen };
}
