//! Громкость вкладки: 0–100 %, своя у каждой вкладки.
//!
//! У WebView2 громкости на webview нет (только «выключить звук»), а звук всех вкладок
//! идёт через общий аудиопроцесс. Поэтому громкость делает сама страница: скрипт
//! [`SCRIPT`] встаёт раньше её скриптов во всех фреймах и умножает звук на множитель
//! вкладки (0…1):
//!
//! - `<audio>` и `<video>`: свойство `volume` подменено на прототипе. Страница ставит
//!   и читает свою громкость (её ползунок работает как обычно), а у элемента на самом
//!   деле своя × множитель. Громче 100 % не бывает: усиление шло бы через Web Audio,
//!   а `createMediaElementSource` у чужого медиа без CORS даёт тишину.
//! - Web Audio: всё, что страница подключает к `destination` живого `AudioContext`,
//!   на деле идёт через наш `GainNode` перед ним. Сам `destination` настоящий: его
//!   `maxChannelCount`, `channelCount` и `instanceof` не меняются. `OfflineAudioContext`
//!   не трогаем: это расчёт, а не звук. Громкость элемента действует и внутри Web Audio,
//!   поэтому `<audio>`/`<video>`, ушедшие туда (`createMediaElementSource`), звучат
//!   со своей громкостью, а множитель им даёт наш `GainNode` — иначе вышло бы k².
//!
//! Браузер задаёт множитель через [`apply`] (`script::run`, без жеста пользователя)
//! в главном фрейме; тот пересылает его своим фреймам `postMessage` с меткой
//! `__axiomVolume`, фреймы — своим. Новый фрейм сам спрашивает множитель у родителя.
//! Фрейм принимает множитель только от родителя, поэтому страница своим сообщением
//! может поменять громкость только себе и своим фреймам. Сообщения с меткой страницы
//! не видят, а ошибки скрипта не выходят наружу и не пишутся в консоль.
//!
//! При 100 % скрипт ничего не меняет: родные сеттеры вызываются как есть, `GainNode`
//! стоит с усилением 1.

/// Громкость новой вкладки и «как было».
pub const FULL: u8 = 100;

/// Скрипт в каждом фрейме вкладки (см. описание модуля).
pub const SCRIPT: &str = r#"(() => {
  const W = window;
  const TAG = '__axiomVolume';
  const call = Reflect.apply;
  const desc = Object.getOwnPropertyDescriptor;
  const quiet = (f) => { try { return f(); } catch {} };
  let k = 1;
  // Что пересчитать при новом множителе: свои части добавляют медиа и Web Audio.
  const updates = [];
  // Элемент ушёл в Web Audio (`createMediaElementSource`): см. ниже.
  let route = () => {};

  // ---------- <audio> и <video>: на деле своя громкость × множитель ----------
  quiet(() => {
    const P = HTMLMediaElement.prototype;
    const d = desc(P, 'volume');
    const get = d.get, set = d.set;
    const real = (el) => call(get, el, []);
    const target = desc(Event.prototype, 'target').get;
    const all = Document.prototype.querySelectorAll;
    // Громкость, которую поставила страница, — пока элемент приглушён.
    const own = new WeakMap();
    // Все встреченные элементы, в том числе не в документе (`new Audio()`).
    const seen = new WeakSet();
    const known = new Set();
    let limit = 512;
    const see = (el) => {
      if (seen.has(el)) return;
      seen.add(el);
      known.add(new WeakRef(el));
      if (known.size > limit) {
        for (const r of known) if (!r.deref()) known.delete(r);
        limit = Math.max(512, known.size * 2);
      }
    };
    // Громкость элемента действует и внутри Web Audio, поэтому у ушедшего туда элемента
    // она своя, а множитель даёт наш GainNode (иначе вышло бы k²).
    const routed = new WeakSet();
    const factor = (el) => (routed.has(el) ? 1 : k);
    const fit = (el) => {
      see(el);
      const m = factor(el);
      if (!own.has(el)) {
        if (m === 1) return;
        own.set(el, real(el));
      }
      const want = own.get(el) * m;
      if (real(el) !== want) call(set, el, [want]);
      if (m === 1) own.delete(el);
    };
    route = (el) => quiet(() => {
      real(el);
      routed.add(el);
      fit(el);
    });
    Object.defineProperty(P, 'volume', {
      configurable: d.configurable,
      enumerable: d.enumerable,
      get: new Proxy(get, {
        apply(fn, self, args) {
          const v = call(fn, self, args);
          return own.has(self) ? own.get(self) : v;
        },
      }),
      set: new Proxy(set, {
        apply(fn, self, args) {
          const v = args[0];
          const m = factor(self);
          if (m === 1 && !own.has(self)) {
            call(fn, self, args);
          } else if (typeof v === 'number' && v >= 0 && v <= 1) {
            call(fn, self, [v * m]);
            own.set(self, v);
          } else {
            // Не число или вне 0…1: проверит (и бросит ту же ошибку) родной сеттер.
            call(fn, self, args);
            own.set(self, real(self));
            call(fn, self, [own.get(self) * m]);
          }
          if (m === 1) own.delete(self);
          quiet(() => see(self));
        },
      }),
    });
    updates.push(() => {
      for (const r of known) {
        const el = r.deref();
        if (el) quiet(() => fit(el));
        else known.delete(r);
      }
      for (const el of call(all, document, ['audio,video'])) quiet(() => fit(el));
    });
    // Элемент, который заиграл, не задав громкость, тоже получает множитель.
    const catchUp = (el) => quiet(() => {
      if (!(el instanceof HTMLMediaElement)) return;
      see(el);
      if (k !== 1) fit(el);
    });
    P.play = new Proxy(P.play, {
      apply(fn, self, args) {
        catchUp(self);
        return call(fn, self, args);
      },
    });
    for (const type of ['loadstart', 'play']) {
      W.addEventListener(type, (e) => catchUp(call(target, e, [])), true);
    }
  });

  // ---------- Web Audio: destination живого контекста — через наш GainNode ----------
  quiet(() => {
    const N = AudioNode.prototype;
    const connect = N.connect, disconnect = N.disconnect;
    const ctxOf = desc(N, 'context').get;
    const channels = ['channelCount', 'channelCountMode', 'channelInterpretation'].map((p) => desc(N, p));
    const Base = W.BaseAudioContext || W.AudioContext;
    const createGain = Base.prototype.createGain;
    const stateOf = desc(Base.prototype, 'state').get;
    const timeOf = desc(Base.prototype, 'currentTime').get;
    const gainOf = desc(GainNode.prototype, 'gain').get;
    const value = desc(AudioParam.prototype, 'value').set;
    const setTarget = AudioParam.prototype.setTargetAtTime;
    const isDest = desc(AudioDestinationNode.prototype, 'maxChannelCount').get;
    const isOffline = desc(OfflineAudioContext.prototype, 'length').get;
    const Off = W.webkitOfflineAudioContext;
    const brand = (getter, x) => { try { call(getter, x, []); return true; } catch { return false; } };
    // Узел из другого фрейма того же сайта — проверка по сути, а не по прототипу.
    const dest = (x) => x instanceof AudioDestinationNode || (x !== null && typeof x === 'object'
      && !(x instanceof AudioNode) && !(x instanceof AudioParam) && brand(isDest, x));
    const offline = (c) => c instanceof OfflineAudioContext || (!!Off && c instanceof Off)
      || (!(c instanceof AudioContext) && brand(isOffline, c));
    const gains = new WeakMap();
    const live = new Set();
    const level = (g) => {
      const p = call(gainOf, g, []);
      const c = call(ctxOf, g, []);
      // Плавно, чтобы ползунок не щёлкал; у стоящего контекста время не идёт — сразу.
      if (call(stateOf, c, []) === 'running') call(setTarget, p, [k, call(timeOf, c, []), 0.015]);
      else call(value, p, [k]);
    };
    // Наш узел повторяет настройку каналов destination: смешивание то же, что без него.
    const same = (g, d) => {
      for (const c of channels) quiet(() => { const v = call(c.get, d, []); if (call(c.get, g, []) !== v) call(c.set, g, [v]); });
    };
    const via = (d) => {
      let g = gains.get(d);
      if (!g) {
        const c = call(ctxOf, d, []);
        if (offline(c)) return;
        g = call(createGain, c, []);
        call(value, call(gainOf, g, []), [k]);
        call(connect, g, [d]);
        gains.set(d, g);
        live.add(new WeakRef(g));
      }
      same(g, d);
      return g;
    };
    const rest = (g, args) => { const a = [g]; for (let i = 1; i < args.length; i++) a.push(args[i]); return a; };
    N.connect = new Proxy(connect, {
      apply(fn, self, args) {
        const d = args[0];
        const g = quiet(() => (dest(d) ? via(d) : undefined));
        if (!g) return call(fn, self, args);
        call(fn, self, rest(g, args));
        return d;
      },
    });
    N.disconnect = new Proxy(disconnect, {
      apply(fn, self, args) {
        const d = args[0];
        const g = d !== null && typeof d === 'object' ? gains.get(d) : undefined;
        return call(fn, self, g ? rest(g, args) : args);
      },
    });
    updates.push(() => {
      for (const r of live) {
        const g = r.deref();
        if (g) quiet(() => level(g));
        else live.delete(r);
      }
    });
    // `<audio>`/`<video>`, ушедшие в Web Audio, приглушает GainNode, а не их громкость.
    quiet(() => {
      const AC = (W.AudioContext || W.webkitAudioContext).prototype;
      AC.createMediaElementSource = new Proxy(AC.createMediaElementSource, {
        apply(fn, self, args) {
          const node = call(fn, self, args);
          route(args[0]);
          return node;
        },
      });
    });
    quiet(() => {
      const MES = desc(W, 'MediaElementAudioSourceNode');
      MES.value = new Proxy(MES.value, {
        construct(T, args, next) {
          const node = Reflect.construct(T, args, next);
          quiet(() => route(args[1].mediaElement));
          return node;
        },
      });
      Object.defineProperty(W, 'MediaElementAudioSourceNode', MES);
    });
  });

  // ---------- множитель: от браузера в главный фрейм, оттуда во все фреймы ----------
  quiet(() => {
    const up = W.parent;
    const top = up === W;
    const frames = desc(W, 'length');
    const count = () => (frames && frames.get ? call(frames.get, W, []) : W.length);
    const dataOf = desc(MessageEvent.prototype, 'data').get;
    const sourceOf = desc(MessageEvent.prototype, 'source').get;
    const stop = Event.prototype.stopImmediatePropagation;
    const has = Object.prototype.hasOwnProperty;
    const tell = (win, v) => quiet(() => win.postMessage({ [TAG]: v }, '*'));
    const setK = (v) => {
      if (typeof v !== 'number' || !(v >= 0 && v <= 1)) return;
      if (v !== k) {
        k = v;
        for (const u of updates) quiet(u);
      }
      for (let i = 0, n = count(); i < n; i++) quiet(() => tell(W[i], k));
    };
    W.addEventListener('message', (e) => quiet(() => {
      const d = call(dataOf, e, []);
      if (d === null || typeof d !== 'object' || !call(has, d, [TAG])) return;
      call(stop, e, []);
      const v = d[TAG];
      const from = call(sourceOf, e, []);
      // Новый фрейм спрашивает множитель; при 1 ответ не нужен.
      if (v === '?') {
        if (from && from !== W && from.parent === W && k !== 1) tell(from, k);
      } else if (!top && from === up) {
        setK(v);
      }
    }), true);
    Object.defineProperty(W, TAG, { value: (v) => { setK(v); } });
    if (!top) tell(up, '?');
  });
})();"#;

/// Громкость в допустимых пределах.
pub fn clamp(volume: u8) -> u8 {
    volume.min(FULL)
}

/// Windows: 0 % — это выключенный звук движка, а страница при этом играет на 1 %.
///
/// WebView2 считает страницу играющей (`IsDocumentPlayingAudio`), только пока в её
/// звуке что-то есть, и с множителем 0 вкладку с музыкой можно было бы усыпить как
/// молчащую — а выключенную кнопкой вкладку не усыпляют. С выключенным звуком движка
/// и 1 % в странице 0 % ведёт себя так же. Звук движка включается и выключается сразу,
/// а скрипт доходит до страницы чуть позже — поэтому не 100 %, а 1 %: щелчка громкости
/// при переходе через ноль нет. В WebKitGTK усыпление на звук не смотрит: там 0 — это 0.
pub const ZERO_IS_MUTE: bool = cfg!(windows);

/// Громкость, которую получает сама страница (см. [`ZERO_IS_MUTE`]).
pub fn page_volume(volume: u8) -> u8 {
    match clamp(volume) {
        0 if ZERO_IS_MUTE => 1,
        v => v,
    }
}

/// Звук движка выключен: кнопкой и Ctrl+M или громкостью 0 % (см. [`ZERO_IS_MUTE`]).
pub fn engine_muted(muted: bool, volume: u8) -> bool {
    muted || (ZERO_IS_MUTE && volume == 0)
}

/// Скрипт браузера для главного фрейма вкладки: задать громкость вкладки (0–100)
/// странице и всем её фреймам. Выполнять через `script::run`: без жеста пользователя.
/// Скрипт для вкладок этой платформы. На Linux громкость вкладок пока выключена:
/// после 0.5.3 у тестера на WebKitGTK видео зависало (картинка стоит, звук идёт),
/// а громкость там вживую не проверена — вернём, когда проверим.
pub const PAGE_SCRIPT: &str = if cfg!(target_os = "linux") {
    ""
} else {
    SCRIPT
};

/// Есть ли громкость вкладок на этой платформе.
pub const SUPPORTED: bool = !cfg!(target_os = "linux");

pub fn apply(volume: u8) -> String {
    let k = f64::from(page_volume(volume)) / f64::from(FULL);
    format!("typeof window.__axiomVolume === 'function' && window.__axiomVolume({k})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_passes_a_fraction() {
        assert_eq!(
            apply(40),
            "typeof window.__axiomVolume === 'function' && window.__axiomVolume(0.4)"
        );
        assert!(apply(100).ends_with("window.__axiomVolume(1)"));
        assert!(apply(5).ends_with("window.__axiomVolume(0.05)"));
        assert!(apply(1).ends_with("window.__axiomVolume(0.01)"));
        // Больше 100 % не бывает.
        assert!(apply(250).ends_with("window.__axiomVolume(1)"));
        assert_eq!(clamp(101), FULL);
        assert_eq!(clamp(35), 35);
    }

    #[test]
    fn zero_volume() {
        if ZERO_IS_MUTE {
            // Звук движка выключен, страница звучит на 1 % — WebView2 видит, что она играет.
            assert!(apply(0).ends_with("window.__axiomVolume(0.01)"));
            assert!(engine_muted(false, 0));
        } else {
            assert!(apply(0).ends_with("window.__axiomVolume(0)"));
            assert!(!engine_muted(false, 0));
        }
        assert_eq!(page_volume(7), 7);
        assert!(engine_muted(true, 50));
        assert!(!engine_muted(false, 1));
        assert!(!engine_muted(false, FULL));
    }

    #[test]
    fn script_answers_apply() {
        // Браузер зовёт ту же функцию, что ставит скрипт, и её метка — та же, что в сообщениях.
        assert!(SCRIPT.contains("const TAG = '__axiomVolume';"));
        assert!(SCRIPT.contains("Object.defineProperty(W, TAG,"));
        assert!(apply(50).contains("window.__axiomVolume("));
        // Ничего не пишет в консоль.
        assert!(!SCRIPT.contains("console."));
    }
}
