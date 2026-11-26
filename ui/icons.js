'use strict';
// Собственный минимальный набор иконок: контур, сетка 24×24, без дробных сдвигов.
// Толщину линии задаёт CSS (.i) в пикселях экрана, она одна при любом размере значка.
// Деталь с классом f закрашивается во «включённом» состоянии кнопки.
const ICONS = {
  back: '<path d="M15 6l-6 6 6 6"/>',
  fwd: '<path d="M9 6l6 6-6 6"/>',
  reload: '<path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3"/><path d="M19.5 4.5V9H15"/>',
  plus: '<path d="M12 5.5v13M5.5 12h13"/>',
  minus: '<path d="M5.5 12h13"/>',
  x: '<path d="M7 7l10 10M17 7L7 17"/>',
  lock: '<rect x="5.5" y="10.5" width="13" height="9" rx="2"/><path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5"/>',
  globe: '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.4 2.5 3.4 5.3 3.4 8.5s-1 6-3.4 8.5c-2.4-2.5-3.4-5.3-3.4-8.5s1-6 3.4-8.5z"/>',
  search: '<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.2-4.2"/>',
  // Лупа масштаба в адресной строке: страница крупнее или мельче 100 %.
  zoomIn: '<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.2-4.2M8.5 11h5M11 8.5v5"/>',
  zoomOut: '<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.2-4.2M8.5 11h5"/>',
  sparkles: '<path d="M12 3.5l1.7 4.8 4.8 1.7-4.8 1.7L12 16.5l-1.7-4.8L5.5 10l4.8-1.7L12 3.5z"/><path d="M18.5 15l.7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7.7-1.8z"/>',
  spark: '<path d="M12 4l2 6 6 2-6 2-2 6-2-6-6-2 6-2 2-6z"/>',
  sliders: '<path d="M4 8h9M17 8h3M4 16h3M11 16h9"/><circle cx="15" cy="8" r="2"/><circle cx="9" cy="16" r="2"/>',
  min: '<path d="M6.5 12h11"/>',
  max: '<rect x="6.5" y="6.5" width="11" height="11" rx="1.5"/>',
  restore: '<rect x="6" y="8.5" width="9.5" height="9.5" rx="1.5"/><path d="M9 8.5V7.5A1.5 1.5 0 0 1 10.5 6h6A1.5 1.5 0 0 1 18 7.5v6a1.5 1.5 0 0 1-1.5 1.5h-1"/>',
  popout: '<path d="M14 5h5v5"/><path d="M19 5l-7.5 7.5"/><path d="M17 13.5V17a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2h3.5"/>',
  clock: '<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>',
  star: '<path d="M12 4l2.5 5.1 5.6.8-4.1 4 1 5.6L12 16.8l-5 2.7 1-5.6-4.1-4 5.6-.8L12 4z"/>',
  translate: '<path d="M3 5.5h10.5M8.25 3v2.5"/><path d="M11.5 5.5c-.9 4-3.4 7.3-7.5 9.3"/><path d="M6 8.8c1.2 2.4 3 4.2 5.3 5.4"/><path d="M12.5 21l4-10 4 10M13.9 17.6h5.2"/>',
  moon: '<path d="M19.5 14.5A7.5 7.5 0 0 1 9.5 4.5a7.5 7.5 0 1 0 10 10z"/>',
  // Кружка кофе — «не усыплять»: когда включено, кружка закрашена.
  cup: '<path class="f" d="M4 9.5h12v6.5a4 4 0 0 1-4 4H8a4 4 0 0 1-4-4z"/><path d="M16 11h1.5a2.75 2.75 0 0 1 0 5.5H16"/><path d="M8 3v3.5M12 3v3.5"/>',
  palette: '<path d="M12 3.5a8.5 8.5 0 1 0 0 17c1.2 0 1.8-.8 1.8-1.7 0-.5-.2-.9-.5-1.2-.3-.3-.5-.7-.5-1.2 0-.9.8-1.6 1.7-1.6h2A4.5 4.5 0 0 0 20.5 10C20.5 6.4 16.7 3.5 12 3.5z"/><circle cx="7.5" cy="11" r="1"/><circle cx="10" cy="7.5" r="1"/><circle cx="14.5" cy="7.5" r="1"/>',
  layout: '<rect x="3.5" y="4.5" width="17" height="15" rx="2"/><path d="M3.5 9h17M9 9v10.5"/>',
  cpu: '<rect x="7" y="7" width="10" height="10" rx="1.5"/><path d="M10 3.5V7M14 3.5V7M10 17v3.5M14 17v3.5M3.5 10H7M3.5 14H7M17 10h3.5M17 14h3.5"/>',
  database: '<ellipse cx="12" cy="6" rx="7" ry="2.5"/><path d="M5 6v12c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5V6"/><path d="M5 12c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5"/>',
  monitor: '<rect x="3.5" y="4.5" width="17" height="11.5" rx="2"/><path d="M9 20h6M12 16v4"/>',
  keyboard: '<rect x="3" y="6.5" width="18" height="11" rx="2"/><path d="M7 10h.01M11 10h.01M15 10h.01M7 13.5h.01M17 13.5h.01M10 13.5h4"/>',
  check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
  arrowR: '<path d="M5 12h14M13 6l6 6-6 6"/>',
  arrowL: '<path d="M19 12H5M11 6l-6 6 6 6"/>',
  download: '<path d="M12 4v11M7 10l5 5 5-5M5 20h14"/>',
  folder: '<path d="M3.5 7.5a2 2 0 0 1 2-2h3.6l2 2h7.4a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"/>',
  up: '<path d="M12 19V5.5M6.5 11L12 5.5 17.5 11"/>',
  down: '<path d="M12 5v13.5M6.5 13L12 18.5 17.5 13"/>',
  copy: '<rect x="8.5" y="8.5" width="11" height="11" rx="2"/><path d="M15.5 8.5v-2a2 2 0 0 0-2-2h-7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2"/>',
  file: '<path d="M13.5 3.5H8a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V8z"/><path d="M13.5 3.5V8H18"/>',
  // Динамик на вкладке со звуком; перечёркнутый — звук выключен.
  // Громкость вкладки ниже 100 % — одна волна, 0 % — без волн.
  vol: '<path d="M4 9.5h3.2L12 5.5v13l-4.8-4H4z"/><path d="M15.5 9.2a4 4 0 0 1 0 5.6M18.2 6.6a7.6 7.6 0 0 1 0 10.8"/>',
  vol1: '<path d="M4 9.5h3.2L12 5.5v13l-4.8-4H4z"/><path d="M15.5 9.2a4 4 0 0 1 0 5.6"/>',
  vol0: '<path d="M4 9.5h3.2L12 5.5v13l-4.8-4H4z"/>',
  mute: '<path d="M4 9.5h3.2L12 5.5v13l-4.8-4H4z"/><path d="M16 9.5l5 5M21 9.5l-5 5"/>',
  undo: '<path d="M9 14.5L4.5 10 9 5.5"/><path d="M4.5 10h10a5 5 0 0 1 0 10H11"/>',
  // Островок: кнопки плеера и «картинка в картинке».
  play: '<path d="M8 5.8v12.4a.8.8 0 0 0 1.2.7l9.6-6.2a.8.8 0 0 0 0-1.4L9.2 5.1a.8.8 0 0 0-1.2.7z" fill="currentColor"/>',
  pause: '<rect x="6.5" y="5.5" width="3.8" height="13" rx="1.2" fill="currentColor"/><rect x="13.7" y="5.5" width="3.8" height="13" rx="1.2" fill="currentColor"/>',
  next: '<path d="M6 6.8v10.4a.7.7 0 0 0 1.1.6l7.4-5.2a.7.7 0 0 0 0-1.2L7.1 6.2a.7.7 0 0 0-1.1.6z" fill="currentColor"/><path d="M18 6v12"/>',
  prev: '<path d="M18 6.8v10.4a.7.7 0 0 1-1.1.6l-7.4-5.2a.7.7 0 0 1 0-1.2l7.4-5.2a.7.7 0 0 1 1.1.6z" fill="currentColor"/><path d="M6 6v12"/>',
  pip: '<rect x="3.5" y="5.5" width="17" height="13" rx="2"/><rect x="12" y="11.5" width="6" height="4.5" rx="1" fill="currentColor"/>',
  // Свернуть островок в полоску.
  fold: '<path d="M7 14.5l5-5 5 5"/>',
  // Разрешения сайтов: окошко «Сайт хочет…», карточка сайта и «Безопасность».
  shield: '<path d="M12 3.5l7 2.6v5.6c0 4.2-2.9 7.4-7 8.8-4.1-1.4-7-4.6-7-8.8V6.1z"/><path d="M9.2 12.2l2 2 3.8-4"/>',
  mic: '<rect x="9" y="3.5" width="6" height="10.5" rx="3"/><path d="M5.8 11a6.2 6.2 0 0 0 12.4 0M12 17.2v3.3"/>',
  camera: '<rect x="3.5" y="6.5" width="12" height="11" rx="2.2"/><path d="M15.5 10.5l5-2.8v8.6l-5-2.8"/>',
  pin: '<path d="M12 20.5s-6.3-5.4-6.3-10.3a6.3 6.3 0 0 1 12.6 0c0 4.9-6.3 10.3-6.3 10.3z"/><circle cx="12" cy="10.2" r="2.3"/>',
  chat: '<path d="M6.5 5.5h11a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H11.5l-4 3v-3h-1a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2z"/>',
  bell: '<path d="M18.5 16.5h-13l1.3-1.8V10.5a5.2 5.2 0 0 1 10.4 0v4.2z"/><path d="M10 19.5a2.2 2.2 0 0 0 4 0"/>',
  clipboard: '<rect x="5.5" y="5" width="13" height="15.5" rx="2"/><path d="M9.5 5V4.5a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1V5M9 11h6M9 15h4"/>',
  midi: '<path d="M9.5 17V5.8l9-2.3v11.3"/><circle cx="7" cy="17" r="2.5"/><circle cx="16" cy="14.8" r="2.5"/>',
  font: '<path d="M4.5 19l5.6-14h1.8L17.5 19M7 13.5h8"/>',
  windows: '<rect x="3.5" y="4.5" width="12" height="9" rx="1.6"/><path d="M8.5 16.5v1.4a1.6 1.6 0 0 0 1.6 1.6h8.8a1.6 1.6 0 0 0 1.6-1.6v-5.8a1.6 1.6 0 0 0-1.6-1.6h-1.4"/>',
  // «Сообщить о проблеме»: «Приложить файлы».
  clip: '<path d="M19 11.5l-6.6 6.6a4.4 4.4 0 0 1-6.2-6.2l7.1-7.1a2.9 2.9 0 0 1 4.1 4.1l-7 7a1.45 1.45 0 0 1-2.05-2.05L15 7.2"/>',
  // История: «Очистить историю».
  trash: '<path d="M5 7h14M10 4.5h4"/><path d="M7 7l.8 11.2a2 2 0 0 0 2 1.8h4.4a2 2 0 0 0 2-1.8L17 7"/><path d="M10.5 10.5v6M13.5 10.5v6"/>',
};

/// Логотип: плоская тёмная плитка, белая «Λ» без перекладины и акцентная точка.
function logoSvg(size, _id = 'l', animated = false) {
  const cls = animated ? ' class="logo-anim"' : '';
  return `<svg${cls} width="${size}" height="${size}" viewBox="0 0 256 256" aria-hidden="true">
    <rect class="lg-bg" width="256" height="256" rx="58" fill="#0f0f13"/>
    <path class="lg-mk" pathLength="1" d="M78.1 183 L128 73 L177.9 183" fill="none" stroke="#f4f5fa" stroke-width="19" stroke-linecap="round" stroke-linejoin="round"/>
    <circle class="lg-dot" cx="128" cy="154.9" r="12" fill="#8b9cff"/>
  </svg>`;
}

function icon(name) {
  return `<svg class="i" viewBox="0 0 24 24" aria-hidden="true">${ICONS[name] || ''}</svg>`;
}

/// Значки кнопок окна («свернуть», «развернуть», «восстановить», «закрыть») в пикселях экрана:
/// viewBox — это 16 CSS px × масштаб Windows, координаты линий — центры пикселей. Тонкие
/// линии остаются в один чёткий пиксель на 100%, 125% и 150%, как у системных кнопок,
/// а не расплываются в серую полосу в два пикселя. При смене масштаба app.js рисует заново.
function capIcon(kind, dpr = window.devicePixelRatio || 1) {
  const n = 16 * dpr;
  const g = Math.round(10 * dpr);
  const w = Math.max(1, Math.round(dpr - 0.25));
  const o = Math.round((n - g) / 2);
  const a = o + w / 2;
  const b = o + g - w / 2;
  const s = Math.round(2 * dpr);
  const r = Math.max(1, Math.round(dpr));
  const body = {
    min: `<path d="M${o} ${Math.floor(n / 2) + w / 2}H${o + g}"/>`,
    max: `<rect x="${a}" y="${a}" width="${g - w}" height="${g - w}" rx="${r}"/>`,
    restore: `<rect x="${a}" y="${a + s}" width="${g - s - w}" height="${g - s - w}" rx="${r}"/><path d="M${a + s} ${a + s}V${a}H${b}V${b - s}H${b - s}"/>`,
    x: `<path d="M${o} ${o}L${o + g} ${o + g}M${o + g} ${o}L${o} ${o + g}"/>`,
  }[kind] || '';
  return `<svg class="i cap" viewBox="0 0 ${n} ${n}" style="stroke-width:${w}" aria-hidden="true">${body}</svg>`;
}
