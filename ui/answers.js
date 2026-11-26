'use strict';
// Ответы разработчика на «Сообщить о проблеме»: кнопка «Вам ответили» в панели
// и окошко с ответом под ней. Сервер спрашивает и сообщения хранит Rust
// (src/reports/answers.rs); сюда приходит готовый список, отсюда уходит только
// «прочитано».
function createAnswers(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const btn = $('#ansBtn');
  const panel = $('#ansp');

  let items = [];
  // Ответы в открытом окошке: закрыли окошко — их прочитали.
  let shown = [];

  const unseen = () => items.filter((m) => m.answer && !m.seen);
  const isOpen = () => panel.classList.contains('on');
  const date = (sec) => new Date(sec * 1000).toLocaleDateString('ru-RU', { day: 'numeric', month: 'long' });

  // ---------- кнопка ----------
  function renderBtn() {
    const n = unseen().length;
    const show = n > 0 || isOpen();
    const appear = show && btn.hidden;
    btn.hidden = !show;
    btn.title = n > 1 ? `Разработчик ответил на ваши сообщения: ${n}` : 'Разработчик ответил на ваше сообщение';
    if (appear && ax.speed()) {
      btn.classList.remove('in');
      void btn.offsetWidth;
      btn.classList.add('in');
      setTimeout(() => btn.classList.remove('in'), ax.ms(2200) + 50);
    }
  }

  // ---------- окошко ----------
  // Статус ответа: «Исправлено» и где исправление, или просто «Ответ».
  function status(m) {
    const a = m.answer;
    if (a.status !== 'fixed') return { chip: `${icon('chat')}Ответ`, cls: '', note: '' };
    let note = '';
    if (a.version) {
      note = m.has_fix ? `Исправление уже в вашей версии (${ax.esc(a.version)})`
        : `Исправление придёт с обновлением до ${ax.esc(a.version)}`;
    }
    return { chip: `${icon('check')}Исправлено`, cls: ' ok', note };
  }

  // Ответ одним блоком: что человек писал, статус и текст разработчика.
  // Им же пользуются Настройки → Система («Ваши сообщения»).
  function card(m) {
    const s = status(m);
    return `<div class="ans-quote"><small>Вы писали ${date(m.at)}</small><p>${ax.esc(m.text)}</p></div>
      <div class="ans-st"><span class="ans-chip${s.cls}">${s.chip}</span>${s.note ? `<small>${s.note}</small>` : ''}</div>
      ${m.answer.text ? `<p class="ans-text">${ax.esc(m.answer.text)}</p>` : ''}`;
  }

  // Исправление ещё не у человека, а обновление уже вышло: кнопка «Обновить».
  const needsUpdate = (list) => !!ax.update()?.latest && list.some((m) => m.answer.status === 'fixed' && m.has_fix === false);

  function body() {
    const list = shown.map((id) => items.find((m) => m.id === id)).filter((m) => m?.answer);
    const many = list.length > 1;
    const head = `<div class="upp-head"><span class="upp-logo">${logoSvg(36, 'ans')}</span>
      <span class="upp-t"><b>Ответ разработчика</b><small>${many ? `На ваши сообщения: ${list.length}` : 'На ваше сообщение из «Сообщить о проблеме»'}</small></span></div>`;
    const cards = list.map((m) => `<div class="ans-card">${card(m)}</div>`).join('');
    const update = needsUpdate(list) ? `<button class="btn ghost" data-a="update">${icon('up')}Обновить</button>` : '';
    return `${head}<div class="ans-list">${cards}</div>
      <div class="upp-acts"><button class="btn ghost" data-a="write">Написать ещё</button>${update}<button class="btn primary" data-a="ok">Понятно</button></div>`;
  }

  function position() {
    const anchor = btn.hidden ? $('#setBtn') : btn;
    const r = anchor.getBoundingClientRect();
    const w = Math.min(400, innerWidth - 16);
    panel.style.width = w + 'px';
    const h = panel.offsetHeight;
    const left = Math.min(Math.max(8, r.right - w), innerWidth - 8 - w);
    const up = r.bottom + 6 + h > innerHeight - 8 && r.top - 6 - h >= 8;
    panel.classList.toggle('up', up);
    Object.assign(panel.style, { left: Math.round(left) + 'px', top: Math.round(up ? r.top - 6 - h : r.bottom + 6) + 'px' });
  }

  function open() {
    const fresh = unseen();
    if (isOpen() || !fresh.length) return;
    shown = fresh.map((m) => m.id);
    panel.innerHTML = body();
    panel.classList.add('on');
    btn.classList.add('on');
    renderBtn();
    position();
    ax.setOverlay('ans', true);
  }
  // Закрыли окошко — ответы прочитаны: кнопка в панели больше не нужна,
  // а сами ответы остаются в Настройках → Система.
  function close() {
    if (!isOpen()) return;
    panel.classList.remove('on');
    btn.classList.remove('on');
    ax.setOverlay('ans', false);
    if (shown.length) ax.send({ cmd: 'answers_seen', ids: shown });
    for (const m of items) if (shown.includes(m.id)) m.seen = true;
    shown = [];
    renderBtn();
  }

  btn.addEventListener('click', () => (isOpen() ? close() : open()));
  document.addEventListener('mousedown', (e) => {
    if (isOpen() && !e.target.closest('#ansp,#ansBtn')) close();
  }, true);
  addEventListener('resize', () => { if (isOpen()) position(); });
  panel.addEventListener('click', (e) => {
    switch (e.target.closest('[data-a]')?.dataset.a) {
      case 'ok': close(); break;
      case 'write': close(); ax.writeAgain(); break;
      case 'update': close(); ax.openUpdate(); break;
      default: break;
    }
  });

  // ---------- сообщения из Rust ----------
  function onList(m) {
    const before = new Set(unseen().map((x) => `${x.id}|${x.answer.at}`));
    items = Array.isArray(m.items) ? m.items : [];
    const news = unseen().filter((x) => !before.has(`${x.id}|${x.answer.at}`));
    renderBtn();
    if (m.fresh && news.length) ax.toast('Разработчик ответил на ваше сообщение');
    ax.onChange();
  }

  const reposition = () => { if (isOpen()) requestAnimationFrame(position); };
  // Только для разработки (#answers): открыть окошко, когда придут ответы.
  let soon = false;
  function openSoon() { soon = true; }
  const list = () => items;
  return {
    open, close, isOpen, reposition, card, list, openSoon,
    onList(m) { onList(m); if (soon && unseen().length) { soon = false; open(); } },
  };
}
