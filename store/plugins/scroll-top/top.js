// Кнопки «наверх» и «вниз»: в своём Shadow DOM, стили сайта их не трогают.
let settings = axiom.settings;
const reduced = matchMedia('(prefers-reduced-motion: reduce)');

const host = document.createElement('axiom-scroll-top');
const root = host.attachShadow({ mode: 'closed' });
const arrow = (d) => `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="${d}"/></svg>`;
// Стили — листом (adoptedStyleSheets): <style> в Shadow DOM строгая CSP сайта не пустит.
const sheet = new CSSStyleSheet();
sheet.replaceSync(`
  :host { all: initial; position: fixed; bottom: 24px; z-index: 2147483646; display: flex; flex-direction: column; gap: 10px; }
  :host([data-side=right]) { right: 24px; }
  :host([data-side=left]) { left: 24px; }
  button { all: unset; box-sizing: border-box; width: 44px; height: 44px; border-radius: 50%; display: grid; place-items: center;
    cursor: pointer; color: #fff; background: rgba(28, 28, 34, .72); backdrop-filter: blur(8px);
    box-shadow: 0 6px 20px rgba(0, 0, 0, .28), inset 0 0 0 1px rgba(255, 255, 255, .12);
    opacity: 0; transform: translateY(10px) scale(.9); pointer-events: none;
    transition: opacity .2s, transform .2s, background-color .15s; }
  button.on { opacity: 1; transform: none; pointer-events: auto; }
  :host(:not([data-down])) .down { display: none; }
  button:hover { background: rgba(28, 28, 34, .9); }
  button:active { transform: scale(.94); }
  button:focus-visible { outline: 2px solid #8b9cff; outline-offset: 2px; }
  svg { width: 20px; height: 20px; fill: none; stroke: currentColor; stroke-width: 2.2; stroke-linecap: round; stroke-linejoin: round; }
  @media (prefers-reduced-motion: reduce) { button { transition: none; } }
  @media print { :host { display: none !important; } }
`);
root.adoptedStyleSheets = [sheet];
root.innerHTML = `<button class="up" title="Наверх" aria-label="Наверх">${arrow('M12 19V5M6 11l6-6 6 6')}</button>
<button class="down" title="Вниз" aria-label="В конец страницы">${arrow('M12 5v14M6 13l6 6 6-6')}</button>`;
const up = root.querySelector('.up');
const down = root.querySelector('.down');

const scrollTo = (top) => window.scrollTo({ top, behavior: reduced.matches ? 'auto' : 'smooth' });
up.addEventListener('click', () => scrollTo(0));
down.addEventListener('click', () => scrollTo(document.documentElement.scrollHeight));

let queued = false;
const update = () => {
  queued = false;
  const doc = document.documentElement;
  const max = doc.scrollHeight - innerHeight;
  // Страница почти не прокручивается — кнопки не нужны.
  const long = max > innerHeight * 0.5;
  const showUp = long && scrollY > innerHeight;
  const showDown = long && settings.down && scrollY < max - innerHeight * 0.5;
  up.classList.toggle('on', showUp);
  down.classList.toggle('on', showDown);
  host.dataset.side = settings.side === 'left' ? 'left' : 'right';
  host.toggleAttribute('data-down', !!settings.down);
  if (!host.isConnected && (showUp || showDown)) doc.appendChild(host);
};
const schedule = () => {
  if (!queued) {
    queued = true;
    requestAnimationFrame(update);
  }
};

addEventListener('scroll', schedule, { passive: true });
addEventListener('resize', schedule, { passive: true });
update();

axiom.on('settings', (next) => {
  settings = next;
  update();
});
