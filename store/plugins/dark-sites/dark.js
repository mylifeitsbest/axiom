// Тёмные сайты: решает, переворачивать ли эту страницу, и ставит атрибут на <html>.
// run_at: "start" — до скриптов сайта, чтобы светлая страница не мелькнула.
//
// В хранилище: «choice» — что человек выбрал кнопкой для сайта ({ сайт: true/false }),
// «native» — сайты, которые оказались тёмными сами (в режиме «на всех» их не трогаем).
const host = location.hostname.replace(/^www\./, '');
let settings = axiom.settings;
let choice = {};
let native = new Set();
// Хранилище ещё не прочитано: решаем по одному режиму.
let loaded = false;

const wanted = () => {
  if (loaded && typeof choice[host] === 'boolean') return choice[host];
  return settings.mode === 'all' && !native.has(host);
};

const apply = () => {
  const html = document.documentElement;
  if (!html) return;
  if (wanted()) {
    html.style.setProperty('--axiom-dark-brightness', String(settings.brightness / 100));
    if (!html.hasAttribute('data-axiom-dark')) html.setAttribute('data-axiom-dark', '');
  } else if (html.hasAttribute('data-axiom-dark')) {
    html.removeAttribute('data-axiom-dark');
    html.style.removeProperty('--axiom-dark-brightness');
  }
};

// Сайт мог переписать атрибуты <html> (так делают некоторые фреймворки) — возвращаем.
const watch = () => {
  apply();
  new MutationObserver(() => {
    if (wanted() !== document.documentElement.hasAttribute('data-axiom-dark')) apply();
  }).observe(document.documentElement, { attributes: true, attributeFilter: ['data-axiom-dark'] });
};
if (document.documentElement) watch();
else {
  new MutationObserver((_, o) => {
    if (!document.documentElement) return;
    o.disconnect();
    watch();
  }).observe(document, { childList: true });
}

// Тёмный ли фон сайта сам по себе (computed style наш фильтр не меняет).
const darkByItself = () => {
  for (const el of [document.body, document.documentElement]) {
    if (!el) continue;
    const m = getComputedStyle(el).backgroundColor.match(/[\d.]+/g);
    if (!m || (m.length > 3 && +m[3] < 0.1)) continue;
    const [r, g, b] = m.map(Number);
    return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255 < 0.4;
  }
  return false;
};

const loading = axiom.storage.get(['choice', 'native']).then((saved) => {
  choice = saved.choice && typeof saved.choice === 'object' ? saved.choice : {};
  native = new Set(saved.native || []);
}).catch(() => {}).finally(() => {
  loaded = true;
  apply();
});

// Режим «на всех»: сайт уже тёмный и вы его не выбирали — не переворачиваем и запоминаем.
const checkNative = async () => {
  await loading;
  if (settings.mode !== 'all' || typeof choice[host] === 'boolean' || native.has(host) || !darkByItself()) return;
  native.add(host);
  apply();
  await axiom.storage.set('native', [...native].slice(-500)).catch(() => {});
};
if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', checkNative, { once: true });
else checkNative();

axiom.on('settings', (next) => {
  settings = next;
  apply();
  checkNative();
});

// Кнопка на панели: тёмный — станет обычным, обычный — тёмным. Выбор запоминается для сайта.
axiom.on('action', async () => {
  await loading;
  const next = !wanted();
  // Совпадает с тем, что дал бы сам режим, — выбор не нужен.
  const byMode = settings.mode === 'all' && !native.has(host);
  if (next === byMode && !(next && native.has(host))) delete choice[host];
  else choice[host] = next;
  apply();
  await axiom.storage.set('choice', choice);
});
