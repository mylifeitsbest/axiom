// Скорость видео с клавиатуры: Shift+. быстрее, Shift+, медленнее, Shift+0 — обычная.
// По коду клавиши (Period, Comma, Digit0) — работает и в русской раскладке.
const MIN = 0.25;
const MAX = 4;
const host = location.hostname.replace(/^www\./, '');
const isTop = window === window.top;
let settings = axiom.settings;
// Скорость, которую выбрал человек на этой странице; null — не выбирал.
let chosen = null;
// Скорость, запомненная для сайта.
let saved = null;

const clamp = (v) => Math.min(MAX, Math.max(MIN, Math.round(v * 100) / 100));
const current = () => chosen ?? (settings.remember ? saved : null) ?? (settings.speed !== 1 ? settings.speed : null);

const typing = (el) => {
  for (let e = el; e; e = e.parentElement) {
    if (e.isContentEditable) return true;
  }
  return !!el && /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName);
};

const visibleArea = (v) => {
  const r = v.getBoundingClientRect();
  const w = Math.max(0, Math.min(r.right, innerWidth) - Math.max(r.left, 0));
  const h = Math.max(0, Math.min(r.bottom, innerHeight) - Math.max(r.top, 0));
  return w * h;
};

// Видео, которое сейчас играет, а если такого нет — самое большое на экране.
const target = () => {
  const videos = [...document.querySelectorAll('video')];
  const playing = videos.filter((v) => !v.paused && !v.ended);
  const pool = playing.length ? playing : videos;
  let best = null;
  let area = 0;
  for (const v of pool) {
    const a = visibleArea(v);
    if (a > area) {
      area = a;
      best = v;
    }
  }
  return best || pool[0] || null;
};

// ---------- подпись со скоростью ----------
let label = null;
let hideTimer = 0;
const reduced = matchMedia('(prefers-reduced-motion: reduce)');
const show = (video, rate) => {
  if (!label) {
    const el = document.createElement('axiom-video-speed');
    const root = el.attachShadow({ mode: 'closed' });
    // Стили — листом (adoptedStyleSheets): <style> в Shadow DOM строгая CSP сайта не пустит.
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(`
      :host { all: initial; position: fixed; z-index: 2147483647; pointer-events: none; }
      div { font: 600 15px/1 system-ui, 'Segoe UI', sans-serif; color: #fff; background: rgba(20, 20, 24, .78);
        padding: 8px 11px; border-radius: 9px; box-shadow: 0 4px 16px rgba(0, 0, 0, .3);
        opacity: 0; transform: scale(.92); transition: opacity .15s, transform .15s; }
      div.on { opacity: 1; transform: none; }
      @media (prefers-reduced-motion: reduce) { div { transition: none; } }
    `);
    root.adoptedStyleSheets = [sheet];
    root.innerHTML = `<div></div>`;
    label = { host: el, box: root.querySelector('div') };
  }
  const parent = document.fullscreenElement && document.fullscreenElement.tagName !== 'VIDEO' ? document.fullscreenElement : document.documentElement;
  if (label.host.parentNode !== parent) parent.appendChild(label.host);
  const r = video.getBoundingClientRect();
  label.host.style.left = `${Math.max(8, r.left + 12)}px`;
  label.host.style.top = `${Math.max(8, r.top + 12)}px`;
  label.box.textContent = `${String(rate).replace('.', ',')}×`;
  label.box.classList.add('on');
  clearTimeout(hideTimer);
  hideTimer = setTimeout(() => label.box.classList.remove('on'), reduced.matches ? 900 : 1000);
};

const applyTo = (video) => {
  const rate = current();
  if (rate != null && Math.abs(video.playbackRate - rate) > 0.001) video.playbackRate = rate;
};
const applyAll = () => document.querySelectorAll('video').forEach(applyTo);

const remember = (rate) => {
  if (!isTop || !settings.remember) return;
  const write = Math.abs(rate - 1) < 0.001 ? axiom.storage.remove(`rate:${host}`) : axiom.storage.set(`rate:${host}`, rate);
  write.catch(() => {});
};

const change = (rate) => {
  const video = target();
  if (!video) return false;
  chosen = clamp(rate);
  saved = chosen;
  applyAll();
  show(video, chosen);
  remember(chosen);
  return true;
};

addEventListener('keydown', (e) => {
  if (!e.shiftKey || e.ctrlKey || e.altKey || e.metaKey || typing(e.target)) return;
  const step = Number(settings.step) || 0.25;
  const video = target();
  if (!video) return;
  const now = video.playbackRate || 1;
  let next = null;
  if (e.code === 'Period') next = now + step;
  else if (e.code === 'Comma') next = now - step;
  else if (e.code === 'Digit0') next = 1;
  if (next == null) return;
  // Свои клавиши скорости есть и у сайтов (YouTube): работает одно правило — наше.
  e.preventDefault();
  e.stopImmediatePropagation();
  change(next);
}, true);

// Новое видео и следующее в плейлисте — с выбранной скоростью: сайты сбрасывают её на 1.
for (const type of ['play', 'loadedmetadata']) {
  document.addEventListener(type, (e) => {
    if (e.target instanceof HTMLVideoElement) applyTo(e.target);
  }, true);
}

if (isTop) {
  axiom.storage.get(`rate:${host}`).then((rate) => {
    if (typeof rate === 'number') saved = clamp(rate);
    applyAll();
  }).catch(() => {});
}

axiom.on('settings', (next) => {
  settings = next;
  applyAll();
});
