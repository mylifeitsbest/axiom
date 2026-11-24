'use strict';
// Снимок страницы под окнами интерфейса (см. src/tabs/snapshot.rs).
// Страница и панель ИИ — нативные webview поверх интерфейса: пока открыто окно,
// их приходится прятать. Rust сначала снимает их, мы кладём картинку пиксель
// в пиксель на их место и только потом отвечаем «можно прятать». Пока страница
// ещё сверху, анимация появления окна придержана: иначе её начало прошло бы под ней.
// При закрытии Rust сначала показывает webview и лишь потом присылает clear.
function createSnapshot(ax) {
  const HOSTS = { page: '#content', ai: '#aiBody' };
  // Настройкам снимок не нужен: это отдельная вкладка, под ней страницы нет.
  const POPUPS = { sug: '#sug', dl: '#dlp', upd: '#upp', trp: '#trp', onb: '#onb' };
  // Rust ждёт снимок 150 мс и наш ответ 250 мс — дольше окно не придерживаем.
  const WAIT_LIMIT = 450;
  const shots = {};
  const held = new Set();
  let seq = 0;
  let open = false;
  let waiting = false;
  let waitTimer;
  let leftTimer;

  const frames = (n) => new Promise((done) => {
    const step = (i) => (i ? requestAnimationFrame(() => step(i - 1)) : done());
    step(n);
  });

  // `first` — до этого поверх контента ничего не было.
  function opening(reason, first) {
    if (first) {
      open = true;
      clearTimeout(leftTimer);
      const n = ax.native();
      waiting = n.page != null || n.ai;
      if (waiting) waitTimer = setTimeout(reveal, WAIT_LIMIT);
    }
    if (waiting) hold(reason);
  }
  // `last` — поверх контента больше ничего нет.
  function closing(reason, last) {
    if (!last) return;
    open = false;
    reveal();
    // clear от Rust должен прийти сразу; это на случай, если он потерялся.
    leftTimer = setTimeout(() => { if (!open) for (const k of Object.keys(shots)) drop(k); }, 1000);
  }

  function hold(reason) {
    const el = POPUPS[reason] && document.querySelector(POPUPS[reason]);
    if (!el) return;
    for (const a of el.getAnimations({ subtree: true })) { a.pause(); held.add(a); }
  }
  function reveal() {
    waiting = false;
    clearTimeout(waitTimer);
    // Окно могли закрыть, пока анимация стояла: отменённые не запускаем заново.
    for (const a of held) if (a.playState === 'paused') a.play();
    held.clear();
  }

  function onMessage(m) {
    if (!(m.seq >= seq)) return;
    seq = m.seq;
    if (m.clear) {
      // Webview уже снова на экране: снимок под ним можно убирать.
      frames(2).then(() => { for (const k of Object.keys(shots)) if (shots[k].seq <= m.seq) drop(k); });
      return;
    }
    if (!open) return;
    const parts = Object.keys(HOSTS).filter((k) => m[k]);
    // Снимка не будет (или это пересъёмка уже под окном) — ждать нечего.
    if (!parts.length) { reveal(); return; }
    Promise.all(parts.map((k) => put(k, m[k], m.seq))).then(() => frames(2)).then(() => {
      if (seq !== m.seq || !open) return;
      ax.send({ cmd: 'snapshot_shown', seq: m.seq });
      reveal();
    });
  }

  function put(k, s, g) {
    const img = new Image();
    img.alt = '';
    img.draggable = false;
    img.src = s.src;
    return img.decode().then(() => {
      if (seq !== g || !open) return;
      const box = shots[k]?.box || document.body.appendChild(Object.assign(document.createElement('div'), { className: 'snap' }));
      box.replaceChildren(img);
      shots[k] = { box, img, seq: g, tab: s.tab, r: s };
      place(k);
    }, () => { if (!(shots[k]?.seq > g)) drop(k); });
  }

  function drop(k) {
    const s = shots[k];
    if (!s) return;
    delete shots[k];
    s.img.removeAttribute('src');
    s.box.remove();
  }

  // Снимок лежит ровно там, где был webview (r — его место в физических пикселях).
  // Если раскладка с тех пор сменилась (окно растянули), рамка идёт за областью
  // контента, а картинка остаётся у её угла в своём размере — без растяжения.
  function place(k) {
    const s = shots[k];
    const host = document.querySelector(HOSTS[k]);
    if (!s || !host) return;
    const d = devicePixelRatio || 1;
    const r = s.r;
    const b = host.getBoundingClientRect();
    const near = (css, px) => Math.abs(Math.round(css) - px / d) < 1;
    const same = near(b.left, r.x) && near(b.top, r.y) && near(b.width, r.w) && near(b.height, r.h);
    const [x, y, w, h] = same ? [r.x / d, r.y / d, r.w / d, r.h / d] : [b.left, b.top, b.width, b.height].map(Math.round);
    Object.assign(s.box.style, { left: x + 'px', top: y + 'px', width: w + 'px', height: h + 'px' });
    Object.assign(s.img.style, { width: r.w / d + 'px', height: r.h / d + 'px' });
  }

  // Вкладку переключили, панель ИИ закрыли — прежний снимок там уже неправда.
  function sync() {
    const n = ax.native();
    if (shots.page && shots.page.tab !== n.page) drop('page');
    if (shots.ai && !n.ai) drop('ai');
    for (const k of Object.keys(shots)) place(k);
  }
  const ro = new ResizeObserver(() => { for (const k of Object.keys(shots)) place(k); });
  for (const sel of Object.values(HOSTS)) ro.observe(document.querySelector(sel));

  return { opening, closing, onMessage, sync };
}
