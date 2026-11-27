// Режим чтения: по кнопке на панели — статья страницы поверх неё, без лишнего.
// Текст собирается заново из разрешённых тегов: ни скриптов, ни стилей, ни
// обработчиков сайта в него не попадает.
let settings = axiom.settings;
let open = null;

const SKIP = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'IFRAME', 'FORM', 'BUTTON', 'INPUT', 'SELECT',
  'TEXTAREA', 'NAV', 'ASIDE', 'FOOTER', 'SVG', 'CANVAS', 'VIDEO', 'AUDIO', 'OBJECT', 'EMBED', 'DIALOG', 'LABEL', 'MENU']);
// Блоки «поделиться», «комментарии», реклама и прочее вокруг статьи — по классу или id.
const NOISE = /(share|social|comment|related|recommend|promo|advert|(^|[\s_-])ads?([\s_-]|$)|banner|subscribe|newsletter|sidebar|footer|cookie|popup|modal|breadcrumb|toolbar|menu)/i;
// Что остаётся тегом и каким.
const KEEP = {
  P: 'p', H1: 'h2', H2: 'h2', H3: 'h3', H4: 'h4', H5: 'h4', H6: 'h4', UL: 'ul', OL: 'ol', LI: 'li',
  BLOCKQUOTE: 'blockquote', PRE: 'pre', CODE: 'code', FIGURE: 'figure', FIGCAPTION: 'figcaption',
  B: 'strong', STRONG: 'strong', I: 'em', EM: 'em', BR: 'br', HR: 'hr', SUB: 'sub', SUP: 'sup', MARK: 'mark',
  TABLE: 'table', THEAD: 'thead', TBODY: 'tbody', TR: 'tr', TD: 'td', TH: 'th', DL: 'dl', DT: 'dt', DD: 'dd',
};
const BLOCKS = 'p, div, section, article, ul, ol, h1, h2, h3, h4, h5, h6, blockquote, pre, table, figure';

const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
const textLen = (el) => clean(el.textContent).length;
const hidden = (el) => el.hidden || el.getAttribute('aria-hidden') === 'true';

// ---------- где статья ----------
const findArticle = () => {
  const scores = new Map();
  for (const p of document.querySelectorAll('p, pre')) {
    if (p.closest('nav, aside, footer, form, [role=navigation], [role=complementary]')) continue;
    const len = textLen(p);
    if (len < 40) continue;
    // Вес абзаца — его родителю, половина — деду, четверть — прадеду.
    let el = p.parentElement;
    for (let w = 1; el && el !== document.documentElement && w >= 0.25; el = el.parentElement, w /= 2) {
      scores.set(el, (scores.get(el) || 0) + Math.min(len, 1500) * w);
    }
  }
  let best = null;
  let top = 0;
  for (const [el, score] of scores) {
    // Ссылки вместо текста — это меню или список статей, а не статья.
    let links = 0;
    for (const a of el.querySelectorAll('a')) links += textLen(a);
    const s = score * (1 - Math.min(links / Math.max(1, textLen(el)), 0.9));
    if (s > top) {
      top = s;
      best = el;
    }
  }
  const semantic = document.querySelector('article, [itemprop=articleBody], main, [role=main]');
  if (!best) return semantic || document.body;
  // Статья целиком (с заголовками и картинками), если в ней нет много лишнего.
  if (semantic && semantic.contains(best) && textLen(semantic) <= textLen(best) * 2.5) return semantic;
  return best;
};

const pageTitle = (article) => {
  const og = document.querySelector('meta[property="og:title"]');
  const h1 = article.querySelector('h1') || document.querySelector('h1');
  return clean((h1 && h1.textContent) || (og && og.content) || document.title);
};

// ---------- сборка текста ----------
const absolute = (url) => {
  try {
    const u = new URL(url, location.href);
    return /^(https?|mailto):$/.test(u.protocol) ? u.href : '';
  } catch {
    return '';
  }
};

const addImage = (img, out) => {
  const lazy = img.getAttribute('data-src') || img.getAttribute('data-lazy-src') || img.getAttribute('data-original');
  let src = img.currentSrc || img.src;
  if ((!src || src.startsWith('data:')) && lazy) src = lazy;
  // Встроенная картинка (data:image) — как есть, остальное — полным адресом http(s).
  src = /^data:image\//.test(src) ? src : absolute(src);
  // Значки и счётчики не нужны.
  const small = (img.naturalWidth && img.naturalWidth < 60) || (+img.getAttribute('width') && +img.getAttribute('width') < 60);
  if (!src || small) return;
  const copy = document.createElement('img');
  copy.src = src;
  copy.alt = img.alt || '';
  copy.loading = 'lazy';
  out.appendChild(copy);
};

const build = (src, out, title, depth = 0) => {
  if (depth > 40) return;
  for (const node of src.childNodes) {
    if (node.nodeType === Node.TEXT_NODE) {
      out.appendChild(document.createTextNode(node.textContent));
      continue;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) continue;
    const el = node;
    const tag = el.tagName;
    if (SKIP.has(tag) || hidden(el)) continue;
    if (tag === 'IMG') {
      addImage(el, out);
      continue;
    }
    if (tag === 'PICTURE') {
      const img = el.querySelector('img');
      if (img) addImage(img, out);
      continue;
    }
    if (tag === 'A') {
      const a = document.createElement('a');
      const href = absolute(el.getAttribute('href') || '');
      if (href) a.href = href;
      build(el, a, title, depth + 1);
      if (clean(a.textContent) || a.querySelector('img')) out.appendChild(a);
      continue;
    }
    const as = KEEP[tag];
    if (!as) {
      if (NOISE.test(`${el.id} ${el.getAttribute('class') || ''}`) && textLen(el) < 2000) continue;
      // Блок с одним текстом (без абзацев внутри) — сам абзац, иначе — только содержимое.
      if (/^(DIV|SECTION|ARTICLE|MAIN|HEADER)$/.test(tag) && !el.querySelector(BLOCKS) && textLen(el)) {
        const p = document.createElement('p');
        build(el, p, title, depth + 1);
        out.appendChild(p);
      } else {
        build(el, out, title, depth + 1);
      }
      continue;
    }
    // Заголовок статьи уже наверху.
    if (tag === 'H1' && clean(el.textContent) === title) continue;
    const copy = document.createElement(as);
    if (as !== 'br' && as !== 'hr') build(el, copy, title, depth + 1);
    if (as === 'br' || as === 'hr' || clean(copy.textContent) || copy.querySelector('img')) out.appendChild(copy);
  }
};

// ---------- окно чтения ----------
const STYLE = `
  :host { all: initial; position: fixed; inset: 0; z-index: 2147483647; }
  .wrap { position: absolute; inset: 0; overflow: auto; outline: none; background: var(--bg); color: var(--fg);
    animation: in .18s ease-out; -webkit-font-smoothing: antialiased; }
  .wrap[data-theme=light] { --bg: #fbfbf8; --fg: #1d1d1f; --mute: #6e6e73; --line: #e4e4e0; --link: #2457d6; }
  .wrap[data-theme=sepia] { --bg: #f4ecd8; --fg: #3b2f1e; --mute: #85735a; --line: #e3d6b8; --link: #8a4b14; }
  .wrap[data-theme=dark] { --bg: #18181b; --fg: #e4e4e7; --mute: #9a9aa3; --line: #2c2c31; --link: #8fb2ff; }
  @keyframes in { from { opacity: 0; } }
  @media (prefers-reduced-motion: reduce) { .wrap { animation: none; } }
  .bar { position: sticky; top: 0; display: flex; align-items: center; gap: 12px; padding: 10px 16px;
    background: var(--bg); border-bottom: 1px solid var(--line); font: 500 13px/1.2 system-ui, 'Segoe UI', sans-serif; color: var(--mute); }
  .bar .grow { flex: 1; }
  .close { all: unset; cursor: pointer; width: 32px; height: 32px; border-radius: 8px; display: grid; place-items: center; color: var(--fg); }
  .close:hover { background: var(--line); }
  .close:focus-visible { outline: 2px solid var(--link); }
  .close svg { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 2.2; stroke-linecap: round; }
  article { box-sizing: border-box; max-width: calc(var(--w) * 1px); margin: 0 auto; padding: 40px 24px 96px;
    font-size: calc(var(--fs) * 1px); line-height: 1.7; overflow-wrap: break-word; }
  .wrap[data-font=serif] article { font-family: Georgia, 'PT Serif', 'Times New Roman', serif; }
  .wrap[data-font=sans] article { font-family: system-ui, 'Segoe UI', Roboto, 'Helvetica Neue', sans-serif; }
  .meta { margin: 0 0 10px; font: 500 13px/1.4 system-ui, 'Segoe UI', sans-serif; color: var(--mute); letter-spacing: .02em; }
  h1 { font-size: 1.75em; line-height: 1.2; margin: 0 0 1em; }
  h2 { font-size: 1.35em; line-height: 1.3; margin: 1.6em 0 .6em; }
  h3, h4 { font-size: 1.12em; line-height: 1.35; margin: 1.4em 0 .5em; }
  p, ul, ol, dl, blockquote, pre, figure, table { margin: 0 0 1.05em; }
  img { display: block; max-width: 100%; height: auto; margin: 1.2em auto; border-radius: 6px; }
  a { color: var(--link); text-underline-offset: 2px; }
  blockquote { margin-left: 0; padding-left: 1em; border-left: 3px solid var(--line); color: var(--mute); }
  pre { overflow: auto; padding: 12px 14px; border-radius: 8px; background: var(--line); font-size: .85em; line-height: 1.5; }
  code { font-family: Consolas, 'Cascadia Mono', monospace; font-size: .9em; }
  figcaption { font-size: .85em; color: var(--mute); text-align: center; }
  table { border-collapse: collapse; display: block; overflow: auto; font-size: .9em; }
  td, th { border: 1px solid var(--line); padding: 6px 10px; text-align: left; }
  hr { border: 0; border-top: 1px solid var(--line); margin: 2em 0; }
  .empty { color: var(--mute); font-style: italic; }
`;

const apply = () => {
  if (!open) return;
  open.wrap.dataset.theme = settings.theme;
  open.wrap.dataset.font = settings.font;
  open.wrap.style.setProperty('--fs', settings.size);
  open.wrap.style.setProperty('--w', settings.width);
};

const close = () => {
  if (!open) return;
  open.host.remove();
  document.documentElement.style.overflow = open.overflow;
  removeEventListener('keydown', open.onKey, true);
  open = null;
};

const show = () => {
  const article = findArticle();
  const title = pageTitle(article);
  const host = document.createElement('axiom-reader');
  const root = host.attachShadow({ mode: 'closed' });
  // Стили — листом (adoptedStyleSheets): <style> в Shadow DOM строгая CSP сайта не пустит.
  const sheet = new CSSStyleSheet();
  sheet.replaceSync(STYLE);
  root.adoptedStyleSheets = [sheet];
  const wrap = document.createElement('div');
  wrap.className = 'wrap';
  wrap.tabIndex = -1;
  const site = document.querySelector('meta[property="og:site_name"]');
  const bar = document.createElement('div');
  bar.className = 'bar';
  bar.innerHTML = '<span class="site"></span><span class="grow"></span><button class="close" title="Закрыть (Esc)" aria-label="Закрыть режим чтения"><svg viewBox="0 0 24 24"><path d="M7 7l10 10M17 7L7 17"/></svg></button>';
  bar.querySelector('.site').textContent = clean(site && site.content) || location.hostname.replace(/^www\./, '');
  const body = document.createElement('article');
  const h1 = document.createElement('h1');
  h1.textContent = title;
  const content = document.createElement('div');
  build(article, content, title);
  const words = clean(content.textContent).split(' ').filter(Boolean).length;
  const meta = document.createElement('p');
  meta.className = 'meta';
  meta.textContent = `${Math.max(1, Math.round(words / 200))} мин чтения`;
  body.append(meta, h1, content);
  if (words < 40) {
    const note = document.createElement('p');
    note.className = 'empty';
    note.textContent = 'Похоже, на этой странице нет статьи — показано всё, что удалось найти.';
    body.appendChild(note);
  }
  wrap.append(bar, body);
  root.append(wrap);
  bar.querySelector('.close').addEventListener('click', close);
  const onKey = (e) => {
    if (e.key !== 'Escape') return;
    e.stopImmediatePropagation();
    close();
  };
  addEventListener('keydown', onKey, true);
  open = { host, wrap, onKey, overflow: document.documentElement.style.overflow };
  apply();
  document.documentElement.appendChild(host);
  // Прокрутка — только у окна чтения, страница под ним стоит.
  document.documentElement.style.overflow = 'hidden';
  wrap.focus({ preventScroll: true });
};

axiom.on('action', () => (open ? close() : show()));
axiom.on('settings', (next) => {
  settings = next;
  apply();
});
