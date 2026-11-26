'use strict';
// Ссылки на приложения (mailto:, tg:, steam:…): окошко «Сайт хочет открыть …» под адресной строкой.
// Что открывать, знает только Rust: сюда приходят сайт, название программы и ссылка для показа,
// а обратно уходят номер запроса и ответ.
function createAppLink(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const panel = $('#xap');
  // Кнопки оживают не сразу: второй клик по ссылке не должен попасть в «Открыть».
  const ARM_MS = 400;

  let req = null;
  let armedAt = 0;
  const isOpen = () => panel.classList.contains('on');

  function render() {
    const what = req.app ? `«${req.app}»` : `приложение для ссылок ${req.scheme}:`;
    const title = req.site ? `Сайт ${req.site} хочет открыть ${what}` : `Открыть ${what}?`;
    panel.innerHTML = `<div class="trp-head"><span class="trp-ic">${icon('popout')}</span>
        <span class="trp-t"><b>${ax.esc(title)}</b><small class="xap-link" title="${ax.esc(req.link)}">${ax.esc(req.link)}</small></span></div>
      ${req.remember ? `<div class="trp-list"><label class="trp-row"><span>Всегда разрешать для этого сайта</span><input type="checkbox" class="tg" data-x="always"></label></div>` : ''}
      <div class="xap-acts"><button class="btn ghost" data-x="cancel">Отмена</button><button class="btn primary" data-x="open">Открыть</button></div>`;
  }

  function position() {
    const anchor = $('#omni');
    const r = anchor?.getBoundingClientRect();
    const w = Math.min(360, innerWidth - 16);
    panel.style.width = w + 'px';
    const visible = r && r.width > 0 && r.bottom > 0;
    const left = visible ? Math.min(Math.max(8, r.left), innerWidth - 8 - w) : (innerWidth - w) / 2;
    // Панель у нижнего края окна — окошко над адресной строкой.
    const up = visible && r.top > innerHeight / 2;
    panel.classList.toggle('up', up);
    const top = !visible ? 56 : up ? r.top - 8 - panel.offsetHeight : r.bottom + 8;
    Object.assign(panel.style, { left: left + 'px', top: Math.round(top) + 'px' });
  }

  function open(m) {
    req = m;
    render();
    if (!isOpen()) {
      panel.classList.add('on');
      ax.setOverlay('xap', true);
    }
    position();
    armedAt = performance.now() + ARM_MS;
    // Enter на «Отмена» ничего не откроет; Esc тоже закрывает.
    $('[data-x=cancel]', panel)?.focus({ preventScroll: true });
  }

  function close() {
    if (!isOpen()) return;
    panel.classList.remove('on');
    ax.setOverlay('xap', false);
    req = null;
  }

  function answer(openIt) {
    if (!req) return;
    const always = !!$('[data-x=always]', panel)?.checked;
    ax.send({ cmd: 'app_link', id: req.id, open: !!openIt, always: openIt && always });
    close();
  }

  panel.addEventListener('click', (e) => {
    const b = e.target.closest('button[data-x]');
    if (!b) return;
    if (performance.now() < armedAt) return;
    answer(b.dataset.x === 'open');
  });
  document.addEventListener('mousedown', (e) => {
    if (isOpen() && !e.target.closest('#xap')) answer(false);
  }, true);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && isOpen()) { e.preventDefault(); e.stopImmediatePropagation(); answer(false); }
  }, true);
  // Строка адреса меняется вместе с окном, но уже после resize (и с анимацией).
  const follow = () => { if (isOpen()) requestAnimationFrame(position); };
  addEventListener('resize', follow);
  if (window.ResizeObserver && $('#omni')) new ResizeObserver(follow).observe($('#omni'));

  // Из Rust: новый запрос или «закрыть» (id = 0: открыли другую вкладку).
  function onRequest(m) {
    if (!m || typeof m.id !== 'number') return;
    if (m.id === 0) { close(); return; }
    open({
      id: m.id,
      site: String(m.site || ''),
      app: String(m.app || ''),
      scheme: String(m.scheme || ''),
      link: String(m.link || ''),
      remember: !!m.remember,
    });
  }

  return { onRequest, isOpen, close: () => answer(false) };
}
