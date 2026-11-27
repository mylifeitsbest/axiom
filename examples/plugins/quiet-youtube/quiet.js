// run_at: "start" — код выполняется раньше скриптов YouTube, когда страницы ещё нет.
const apply = (settings) => {
  const html = document.documentElement;
  html.toggleAttribute('data-quiet-shorts', settings.shorts);
  html.toggleAttribute('data-quiet-comments', settings.comments === 'hide');
};

if (document.documentElement) {
  apply(axiom.settings);
} else {
  // Корневой элемент появится, как только браузер начнёт разбирать страницу.
  new MutationObserver((_, observer) => {
    if (!document.documentElement) return;
    observer.disconnect();
    apply(axiom.settings);
  }).observe(document, { childList: true });
}

// Настройки поменяли в «Настройки → Плагины» — страница меняется сразу, без перезагрузки.
axiom.on('settings', apply);
