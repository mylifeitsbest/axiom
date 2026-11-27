// Время на сайтах: секунды, пока вкладка видна, по дням и сайтам.
// Ключи хранилища: «t:<день>:<сайт>» — секунды, «n:<день>:<сайт>» — о лимите уже напомнили.
const site = location.hostname.replace(/^www\./, '');
const DAYS = 7;
const FLUSH_MS = 15000;
let settings = axiom.settings;
// Сколько записано в хранилище на начало счёта и сколько набежало с тех пор.
let base = 0;
let pending = 0;
let since = null;
let day = '';
let ready = false;

const today = (d = new Date()) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
const key = (d) => `t:${d}:${site}`;
const visible = () => document.visibilityState === 'visible';

const format = (sec) => {
  const m = Math.floor(sec / 60);
  if (m < 1) return 'меньше минуты';
  const h = Math.floor(m / 60);
  return h ? `${h} ч ${m % 60} мин` : `${m} мин`;
};

// Набежавшее время — в счётчик (не в хранилище).
const tick = () => {
  if (since != null) {
    const now = Date.now();
    pending += (now - since) / 1000;
    since = visible() ? now : null;
  }
};

// Перевалило за полночь — новый день начинается с нуля.
const rollover = () => {
  const d = today();
  if (d === day) return false;
  day = d;
  base = 0;
  pending = 0;
  return true;
};

const flush = () => {
  tick();
  if (!ready || pending < 1) return;
  const total = Math.round(base + pending);
  axiom.storage.set(key(day), total).catch(() => {});
  base = total;
  pending = 0;
  checkLimit(total).catch(() => {});
};

const limited = () => {
  const list = String(settings.sites || '')
    .toLowerCase()
    .split(/[\s,;]+/)
    .map((s) => s.replace(/^https?:\/\//, '').replace(/^www\./, '').replace(/\/.*$/, ''))
    .filter(Boolean);
  return !list.length || list.some((s) => site === s || site.endsWith(`.${s}`));
};

const checkLimit = async (total) => {
  if (!settings.limit || total < settings.limit * 60 || !limited()) return;
  const mark = `n:${day}:${site}`;
  if (await axiom.storage.get(mark)) return;
  await axiom.storage.set(mark, true);
  await axiom.notify(`сегодня на ${site} уже ${format(total)} — лимит ${format(settings.limit * 60)}`);
};

// Счёт начинается с того, что уже записано (на этом сайте могли быть и другие вкладки).
const resume = async () => {
  rollover();
  const saved = await axiom.storage.get(key(day));
  base = typeof saved === 'number' ? saved : 0;
  ready = true;
};

// Раз в день — убрать дни старше недели.
const prune = async () => {
  const d = today();
  if ((await axiom.storage.get('pruned')) === d) return;
  const oldest = today(new Date(Date.now() - (DAYS - 1) * 86400000));
  const all = await axiom.storage.get();
  const old = Object.keys(all).filter((k) => /^[tn]:/.test(k) && k.slice(2, 12) < oldest);
  if (old.length) await axiom.storage.remove(old);
  await axiom.storage.set('pruned', d);
};

document.addEventListener('visibilitychange', () => {
  if (visible()) {
    // Вкладка снова на экране: пока её не было, время сайта могли досчитать другие вкладки.
    since = Date.now();
    ready = false;
    resume().catch(() => {});
  } else {
    flush();
  }
});
addEventListener('pagehide', flush);
setInterval(() => {
  if (rollover()) {
    since = visible() ? Date.now() : null;
    return;
  }
  if (visible()) flush();
}, FLUSH_MS);

since = visible() ? Date.now() : null;
resume().then(prune).catch(() => {});

axiom.on('settings', (next) => {
  settings = next;
});

axiom.on('action', async () => {
  flush();
  const all = await axiom.storage.get();
  const prefix = `t:${day}:`;
  let sum = 0;
  for (const [k, v] of Object.entries(all)) {
    if (k.startsWith(prefix) && typeof v === 'number') sum += v;
  }
  const here = Math.round(base + pending);
  // Свой сайт — по счётчику этой вкладки: в хранилище может быть на секунды меньше.
  sum += here - (typeof all[key(day)] === 'number' ? all[key(day)] : 0);
  await axiom.notify(`сегодня на ${site} — ${format(here)}, на всех сайтах — ${format(sum)}`);
});
