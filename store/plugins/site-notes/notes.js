// Заметки к сайтам: окошко с заметкой этого сайта и значок, если она есть.
// Всё — в своём Shadow DOM: стили сайта его не трогают.
const site = location.hostname.replace(/^www\./, '');
const KEY = `note:${site}`;
const MAX = 20000;
let settings = axiom.settings;
let text = '';
let saveTimer = 0;

const host = document.createElement('axiom-site-notes');
const root = host.attachShadow({ mode: 'closed' });
// Стили — листом (adoptedStyleSheets): <style> в Shadow DOM строгая CSP сайта не пустит.
const sheet = new CSSStyleSheet();
sheet.replaceSync(`
  :host { all: initial; position: fixed; left: 18px; bottom: 18px; z-index: 2147483646;
    font: 14px/1.4 system-ui, 'Segoe UI', Roboto, sans-serif; color: #1d1d1f; }
  .badge { all: unset; box-sizing: border-box; width: 34px; height: 34px; border-radius: 50%; display: none; place-items: center;
    cursor: pointer; background: #ffd66b; color: #5a4300; box-shadow: 0 4px 14px rgba(0, 0, 0, .22); opacity: .82; transition: opacity .15s, transform .15s; }
  .badge:hover { opacity: 1; transform: scale(1.06); }
  .badge:focus-visible, button:focus-visible { outline: 2px solid #3d6bff; outline-offset: 2px; }
  :host([data-badge]) .badge { display: grid; }
  :host([data-open]) .badge { display: none; }
  .panel { display: none; box-sizing: border-box; width: calc(var(--w) * 1px); max-width: calc(100vw - 36px); padding: 12px 12px 10px;
    border-radius: 14px; background: #fffdf5; box-shadow: 0 12px 40px rgba(0, 0, 0, .28), 0 0 0 1px rgba(0, 0, 0, .06);
    animation: in .16s ease-out; }
  :host([data-open]) .panel { display: block; }
  @keyframes in { from { opacity: 0; transform: translateY(8px); } }
  @media (prefers-reduced-motion: reduce) { .panel { animation: none; } .badge { transition: none; } }
  .head { display: flex; align-items: center; gap: 8px; margin-bottom: 8px; }
  .head b { font-weight: 600; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .head button { all: unset; cursor: pointer; width: 28px; height: 28px; border-radius: 7px; display: grid; place-items: center; color: #6e6e73; }
  .head button:hover { background: rgba(0, 0, 0, .06); color: #1d1d1f; }
  textarea { box-sizing: border-box; width: 100%; height: 180px; resize: vertical; padding: 10px 11px; border: 0; border-radius: 9px;
    background: #fff; color: #1d1d1f; font: inherit; line-height: 1.5; outline: none; box-shadow: inset 0 0 0 1px rgba(0, 0, 0, .12); }
  textarea:focus { box-shadow: inset 0 0 0 2px #e0a800; }
  .foot { margin-top: 6px; font-size: 12px; color: #8a8a8f; min-height: 16px; }
  svg { width: 17px; height: 17px; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
`);
root.adoptedStyleSheets = [sheet];
root.innerHTML = `<button class="badge" title="Заметка к сайту" aria-label="Открыть заметку к сайту"><svg viewBox="0 0 24 24"><path d="M6 4h9l4 4v12H6z"/><path d="M14 4v5h5M9 13h7M9 16.5h5"/></svg></button>
<div class="panel" role="dialog" aria-label="Заметка к сайту">
  <div class="head"><b></b><button class="close" title="Закрыть (Esc)" aria-label="Закрыть"><svg viewBox="0 0 24 24"><path d="M7 7l10 10M17 7L7 17"/></svg></button></div>
  <textarea placeholder="Что запомнить об этом сайте?" maxlength="${MAX}" spellcheck="true"></textarea>
  <div class="foot" aria-live="polite"></div>
</div>`;
const badge = root.querySelector('.badge');
const area = root.querySelector('textarea');
const foot = root.querySelector('.foot');
root.querySelector('.head b').textContent = site;

const sync = () => {
  host.style.setProperty('--w', settings.width);
  host.toggleAttribute('data-badge', settings.badge && text.trim().length > 0);
  if (!host.isConnected && (host.hasAttribute('data-badge') || host.hasAttribute('data-open'))) document.documentElement.appendChild(host);
};

const save = async () => {
  clearTimeout(saveTimer);
  saveTimer = 0;
  const value = area.value.slice(0, MAX);
  if (value === text) return;
  text = value;
  try {
    if (value.trim()) await axiom.storage.set(KEY, value);
    else await axiom.storage.remove(KEY);
    foot.textContent = 'Сохранено';
  } catch (err) {
    foot.textContent = `Не сохранилось: ${err.message}`;
  }
  sync();
};

const setOpen = (open) => {
  if (!open && saveTimer) save();
  host.toggleAttribute('data-open', open);
  sync();
  if (open) {
    area.value = text;
    foot.textContent = text ? '' : 'Заметка сохраняется сама';
    area.focus();
    area.setSelectionRange(area.value.length, area.value.length);
  }
};

area.addEventListener('input', () => {
  foot.textContent = 'Сохраняется…';
  clearTimeout(saveTimer);
  saveTimer = setTimeout(save, 500);
});
area.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') setOpen(false);
});
// Набор текста — не горячие клавиши сайта (YouTube поставил бы видео на паузу по «k»).
for (const type of ['keydown', 'keypress', 'keyup']) area.addEventListener(type, (e) => e.stopPropagation());
root.querySelector('.close').addEventListener('click', () => setOpen(false));
badge.addEventListener('click', () => setOpen(true));
// Уходят со страницы, не дождавшись сохранения, — сохраняем сразу.
addEventListener('pagehide', () => {
  if (saveTimer) save();
});

axiom.storage.get(KEY).then((saved) => {
  text = typeof saved === 'string' ? saved : '';
  sync();
}).catch(() => {});

axiom.on('action', () => setOpen(!host.hasAttribute('data-open')));
axiom.on('settings', (next) => {
  settings = next;
  sync();
});
