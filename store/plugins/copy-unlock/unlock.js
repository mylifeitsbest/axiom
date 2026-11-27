// Копирование без запретов. Обработчики ставим сразу (run_at: "start"), раньше
// обработчиков сайта: они ловят событие первыми и не дают ему дойти до сайта.
// Действие по умолчанию (само копирование, меню) не трогаем — только запреты сайта.
const host = location.hostname.replace(/^www\./, '');
let settings = axiom.settings;
// Список сайтов приходит из хранилища не сразу: до этого плагин ничего не делает.
let active = false;
let sites = new Set();

const EVENTS = ['copy', 'cut', 'paste', 'selectstart', 'dragstart', 'contextmenu'];
const guard = (e) => {
  if (!active) return;
  if (e.type === 'contextmenu' && !settings.context) return;
  e.stopImmediatePropagation();
};
for (const type of EVENTS) addEventListener(type, guard, true);

// Ctrl+C, Ctrl+X, Ctrl+A, Ctrl+V сайты тоже любят перехватывать.
addEventListener('keydown', (e) => {
  if (!active || !(e.ctrlKey || e.metaKey) || e.altKey) return;
  if (['KeyC', 'KeyX', 'KeyA', 'KeyV', 'Insert'].includes(e.code)) e.stopImmediatePropagation();
}, true);

const mark = () => {
  const html = document.documentElement;
  if (html) html.toggleAttribute('data-axiom-copy', active);
};
const whenRoot = () => {
  if (document.documentElement) mark();
  else {
    new MutationObserver((_, o) => {
      if (!document.documentElement) return;
      o.disconnect();
      mark();
    }).observe(document, { childList: true });
  }
};

axiom.storage.get('sites').then((list) => {
  sites = new Set(list || []);
  active = sites.has(host);
  whenRoot();
}).catch(() => {});

axiom.on('settings', (next) => {
  settings = next;
});

axiom.on('action', async () => {
  active = !active;
  if (active) sites.add(host);
  else sites.delete(host);
  mark();
  await axiom.storage.set('sites', [...sites]);
  if (settings.notify) {
    await axiom.notify(active
      ? `копирование и правый клик на ${host} разрешены. Если что-то не заработало — обновите страницу`
      : `на ${host} снова как задумал сайт`);
  }
});
