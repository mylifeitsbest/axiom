'use strict';
// Обновления: кнопка «Обновить» в панели инструментов и окошко под ней.
// Что и откуда скачать, решает Rust; отсюда уходят только «проверь» и «обнови».
function createUpdate(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const btn = $('#updBtn');
  const label = btn.querySelector('.upd-t');
  const panel = $('#upp');

  let U = null;
  // «Позже» прячет кнопку до перезапуска (или до следующей версии).
  let later = null;
  let announced = null;
  let openWhenReady = false;
  let installWhenReady = false;
  let shape = '';

  const mb = (n) => `${(n / 1048576).toLocaleString('ru-RU', { maximumFractionDigits: 1 })} МБ`;
  const busy = () => U && (U.phase === 'downloading' || U.phase === 'restarting');
  const isOpen = () => panel.classList.contains('on');

  // ---------- кнопка ----------
  function renderBtn() {
    const v = U?.latest;
    const show = !!v && (later !== v || busy() || isOpen());
    const appear = show && btn.hidden;
    btn.hidden = !show;
    btn.classList.toggle('busy', !!busy());
    label.textContent = busy() ? 'Обновляю…' : 'Обновить';
    btn.title = v ? `Доступна Axiom ${v}, у вас ${U.current}` : '';
    if (appear && ax.speed()) {
      btn.classList.remove('in');
      void btn.offsetWidth;
      btn.classList.add('in');
      setTimeout(() => btn.classList.remove('in'), ax.ms(2200) + 50);
    }
  }

  // ---------- окошко ----------
  const button = (act, text, primary) => `<button class="btn${primary ? ' primary' : ' ghost'}" data-u="${act}">${text}</button>`;

  function message() {
    switch (U.phase) {
      case 'downloading': return U.received ? `Скачиваю новую версию… ${mb(U.received)}` : 'Скачиваю новую версию…';
      case 'restarting': return 'Перезапускаю Axiom…';
      default: return '';
    }
  }

  function body() {
    const head = `<div class="upp-head"><span class="upp-logo">${logoSvg(36, 'upd')}</span>
      <span class="upp-t"><b>Axiom ${ax.esc(U.latest)}</b><small>Сейчас установлена ${ax.esc(U.current)}</small></span>
      <button class="lnk" data-u="notes" title="Список изменений на GitHub">Что нового${icon('popout')}</button></div>`;
    let text = '';
    let extra = '';
    let acts = '';
    if (U.how === 'in_place') {
      if (busy()) {
        text = ax.esc(message());
        extra = '<span class="dl-bar ind upp-bar"><i></i></span>';
      } else if (U.phase === 'failed') {
        text = `<span class="upp-err">Не получилось: ${ax.esc(U.error)}</span>`;
        acts = button('manual', 'Скачать вручную') + button('install', 'Повторить', true);
      } else {
        text = 'Axiom скачает новую версию, сверит контрольную сумму и перезапустится. Вкладки откроются снова.';
        if (U.busy) extra = `<p class="upp-warn">${icon('download')}<span>Идут загрузки — перезапуск их прервёт.</span></p>`;
        acts = button('later', 'Позже') + button('install', `${icon('up')}Обновить и перезапустить`, true);
      }
    } else if (U.how === 'apt' || U.how === 'pacman') {
      text = U.how === 'apt'
        ? 'Axiom стоит из репозитория apt и обновится вместе с системой. Чтобы обновить сейчас, выполните в терминале:'
        : 'Axiom обновится вместе с системой. Чтобы обновить сейчас, выполните в терминале:';
      // Переносы только между словами: `--only-upgrade` не разорвётся на дефисе.
      const words = U.command.split(' ').map((w) => `<span>${ax.esc(w)}</span>`).join(' ');
      extra = `<div class="upp-cmd"><code>${words}</code><button class="ib sm" data-u="copy" title="Скопировать">${icon('copy')}</button></div>`;
      acts = button('later', 'Позже') + button('copy', 'Скопировать команду', true);
    } else {
      text = 'Новую версию можно скачать со страницы релиза на GitHub.';
      acts = button('later', 'Позже') + button('manual', 'Открыть страницу', true);
    }
    return `${head}<p class="upp-msg">${text}</p>${extra}${acts ? `<div class="upp-acts">${acts}</div>` : ''}`;
  }

  // Прогресс приходит несколько раз в секунду: меняем только текст, чтобы полоска не дёргалась.
  function renderPanel() {
    const next = [U.latest, U.how, U.phase, U.busy, U.error].join('|');
    if (next === shape && busy()) {
      const msg = panel.querySelector('.upp-msg');
      if (msg) msg.textContent = message();
      return;
    }
    shape = next;
    panel.innerHTML = body();
  }

  function position() {
    const anchor = btn.hidden ? $('#setBtn') : btn;
    const r = anchor.getBoundingClientRect();
    const w = Math.min(360, innerWidth - 16);
    panel.style.width = w + 'px';
    const h = panel.offsetHeight;
    const left = Math.min(Math.max(8, r.right - w), innerWidth - 8 - w);
    const up = r.bottom + 6 + h > innerHeight - 8 && r.top - 6 - h >= 8;
    panel.classList.toggle('up', up);
    Object.assign(panel.style, { left: Math.round(left) + 'px', top: Math.round(up ? r.top - 6 - h : r.bottom + 6) + 'px' });
  }
  function open() {
    if (!U?.latest) return;
    if (isOpen()) return;
    shape = '';
    renderPanel();
    renderBtn();
    panel.classList.add('on');
    position();
    btn.classList.add('on');
    ax.setOverlay('upd', true);
  }
  function close() {
    if (!isOpen()) return;
    panel.classList.remove('on');
    btn.classList.remove('on');
    ax.setOverlay('upd', false);
    renderBtn();
  }
  const toggle = () => (isOpen() ? close() : open());

  btn.addEventListener('click', toggle);
  document.addEventListener('mousedown', (e) => {
    if (isOpen() && !e.target.closest('#upp,#updBtn')) close();
  }, true);
  addEventListener('resize', () => { if (isOpen()) position(); });

  panel.addEventListener('click', (e) => {
    const act = e.target.closest('[data-u]')?.dataset.u;
    switch (act) {
      case 'install': ax.send({ cmd: 'update_install' }); break;
      case 'notes':
      case 'manual': ax.send({ cmd: 'new_tab', input: U.page }); close(); break;
      case 'copy': ax.copyText(U.command); ax.toast('Команда скопирована'); break;
      case 'later':
        later = U.latest;
        close();
        ax.toast('Обновиться можно в настройках, раздел «Система»');
        break;
      default: break;
    }
  });

  // ---------- сообщения из Rust ----------
  function onState(m) {
    U = m;
    if (m.manual) {
      if (m.latest) ax.toast(`Доступна Axiom ${m.latest}`);
      else if (m.checked === 'failed') ax.toast(`Не удалось проверить: ${m.check_error}`);
      else ax.toast(`У вас последняя версия — ${m.current}`);
    } else if (m.latest && m.latest !== announced && later !== m.latest) {
      ax.toast(`Вышла Axiom ${m.latest}`);
    }
    if (m.latest) announced = m.latest;
    renderBtn();
    if (isOpen()) {
      if (m.latest) { renderPanel(); position(); } else close();
    } else if (openWhenReady && m.latest) {
      openWhenReady = false;
      open();
      if (installWhenReady) ax.send({ cmd: 'update_install' });
    }
    ax.onChange();
  }

  const reposition = () => { if (isOpen()) requestAnimationFrame(position); };
  const state = () => U;
  // Из настроек: окошко открывается и после «Позже».
  function openFromSettings() { later = null; renderBtn(); open(); }
  // Только для разработки (#update, #update:install): открыть окошко, когда придёт состояние.
  function openSoon(install) {
    installWhenReady = install;
    if (U?.latest) { open(); if (install) ax.send({ cmd: 'update_install' }); } else openWhenReady = true;
  }

  return { open: openFromSettings, openSoon, close, isOpen, reposition, onState, state };
}
