'use strict';
// Собственный минимальный набор иконок (stroke, 24×24).
const ICONS = {
  back: '<path d="M15 6l-6 6 6 6"/>',
  fwd: '<path d="M9 6l6 6-6 6"/>',
  reload: '<path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3"/><path d="M19.5 4.5V9H15"/>',
  plus: '<path d="M12 5.5v13M5.5 12h13"/>',
  x: '<path d="M7 7l10 10M17 7L7 17"/>',
  lock: '<rect x="5.5" y="10.5" width="13" height="9" rx="2"/><path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5"/>',
  globe: '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.4 2.5 3.4 5.3 3.4 8.5s-1 6-3.4 8.5c-2.4-2.5-3.4-5.3-3.4-8.5s1-6 3.4-8.5z"/>',
  search: '<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.2-4.2"/>',
  sparkles: '<path d="M12 3.5l1.7 4.8 4.8 1.7-4.8 1.7L12 16.5l-1.7-4.8L5.5 10l4.8-1.7L12 3.5z"/><path d="M18.5 15l.7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7.7-1.8z"/>',
  spark: '<path d="M12 5l1.4 4.1 4.1 1.4-4.1 1.4L12 16l-1.4-4.1-4.1-1.4 4.1-1.4L12 5z"/>',
  sliders: '<path d="M4 8h9M17 8h3M4 16h3M11 16h9"/><circle cx="15" cy="8" r="2"/><circle cx="9" cy="16" r="2"/>',
  min: '<path d="M6.5 12h11"/>',
  max: '<rect x="6.5" y="6.5" width="11" height="11" rx="1.5"/>',
  restore: '<rect x="6" y="8.5" width="9.5" height="9.5" rx="1.5"/><path d="M9 8.5V7.5A1.5 1.5 0 0 1 10.5 6h6A1.5 1.5 0 0 1 18 7.5v6a1.5 1.5 0 0 1-1.5 1.5h-1"/>',
  popout: '<path d="M14 5h5v5"/><path d="M19 5l-7.5 7.5"/><path d="M17 13.5V17a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2h3.5"/>',
  clock: '<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>',
  star: '<path d="M12 4l2.5 5.1 5.6.8-4.1 4 1 5.6L12 16.8l-5 2.7 1-5.6-4.1-4 5.6-.8L12 4z"/>',
  moon: '<path d="M19.5 14.5A7.5 7.5 0 0 1 9.5 4.5a7.5 7.5 0 1 0 10 10z"/>',
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
