// Тёплый свет: атрибут на <html> включает слой из night.css, его сила — переменная.
let settings = axiom.settings;
// Включён ли фильтр кнопкой; пока не прочитали из хранилища — да (так по умолчанию).
let on = true;

const evening = () => {
  if (!settings.evening) return true;
  const h = new Date().getHours();
  const { from, to } = settings;
  if (from === to) return true;
  // 20 → 7: через полночь.
  return from < to ? h >= from && h < to : h >= from || h < to;
};

const apply = () => {
  const html = document.documentElement;
  if (!html) return;
  const show = on && evening();
  html.toggleAttribute('data-axiom-night', show);
  if (show) html.style.setProperty('--axiom-night', String(settings.intensity / 100));
  else html.style.removeProperty('--axiom-night');
};

if (document.documentElement) apply();
else {
  new MutationObserver((_, o) => {
    if (!document.documentElement) return;
    o.disconnect();
    apply();
  }).observe(document, { childList: true });
}

axiom.storage.get('on').then((saved) => {
  on = saved !== false;
  apply();
}).catch(() => {});

// Расписание: проверяем раз в минуту.
setInterval(apply, 60000);
// Вернулись на вкладку — возможно, фильтр включили или выключили на другой.
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState !== 'visible') return;
  axiom.storage.get('on').then((saved) => {
    on = saved !== false;
    apply();
  }).catch(() => {});
});

axiom.on('settings', (next) => {
  settings = next;
  apply();
});

axiom.on('action', async () => {
  on = !on;
  apply();
  await axiom.storage.set('on', on);
});
