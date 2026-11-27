//! Диспетчер задач (`axiom://tasks`, Shift+Esc): сколько памяти и процессора
//! занимает каждая вкладка и сам браузер.
//!
//! Замер — по процессам Axiom (`os`): сам Axiom и все его потомки. Чьи они, говорит
//! движок (`engine`): процессы страниц — вкладкам по их главным фреймам, остальное —
//! службам браузера. Процесс, который делят несколько вкладок, делится между ними
//! поровну, поэтому строки в сумме дают итог.

pub mod engine;
pub mod os;

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use serde::Serialize;

use self::{
    engine::{EngineProc, Kind},
    os::Proc,
};
use crate::tabs::TabId;

/// Чья страница: вкладки, интерфейса Axiom или панели ИИ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    Tab(TabId),
    Ui,
    Ai,
}

/// Часть самого браузера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// Процесс axiom.exe: окно, вкладки, загрузки, история.
    App,
    /// Главный процесс движка WebView2.
    Engine,
    /// Видеокарта.
    Gpu,
    /// Сеть, звук и другие службы движка.
    Utility,
    /// Страница интерфейса: панель вкладок, адресная строка, служебные вкладки.
    Ui,
    /// Панель ИИ.
    Ai,
    /// Процессы страниц без вкладки: запасной, фоновые скрипты сайтов. На Linux — все
    /// процессы страниц (чей из них чей, WebKitGTK не говорит).
    Pages,
    /// Прочие потомки Axiom (например, сборщик отчётов о вылетах).
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Usage {
    /// Частная память в оперативной памяти, байты.
    pub memory: u64,
    /// Процессор, % от всех ядер компьютера.
    pub cpu: f32,
    /// Сколько процессов (доля общего процесса считается целым).
    pub procs: u32,
}

impl Usage {
    fn add(&mut self, memory: f64, cpu: f64) {
        self.memory += memory.round() as u64;
        self.cpu += cpu as f32;
        self.procs += 1;
    }
}

#[derive(Debug, Serialize)]
pub struct TabUse {
    pub id: TabId,
    #[serde(flatten)]
    pub usage: Usage,
    /// Процесс страницы общий с другими вкладками: здесь его доля.
    pub shared: bool,
}

#[derive(Debug, Serialize)]
pub struct PartUse {
    pub part: Part,
    #[serde(flatten)]
    pub usage: Usage,
}

/// Замер для страницы диспетчера.
#[derive(Debug, Serialize)]
pub struct View {
    /// Память по вкладкам известна (WebView2). На Linux — только итог и службы.
    pub per_tab: bool,
    pub tabs: Vec<TabUse>,
    pub parts: Vec<PartUse>,
    /// Весь Axiom: каждый процесс один раз.
    pub total: Usage,
    /// Процессор уже посчитан: для этого нужны два замера подряд.
    pub cpu_ready: bool,
    /// Оперативная память компьютера: всего и доступно, байты (0 — неизвестно).
    pub ram_total: u64,
    pub ram_free: u64,
    pub cpus: u32,
}

/// Время процессора по процессам с прошлого замера: из разницы — проценты.
#[derive(Debug, Default)]
pub struct Sampler {
    prev: HashMap<(u32, u64), u64>,
    prev_at: Option<Instant>,
}

/// Чаще замеры не сравниваем: за миллисекунды разница времени — шум.
const MIN_GAP: Duration = Duration::from_millis(250);

impl Sampler {
    /// Процент процессора каждого процесса `tree` с прошлого замера (`None` — первый).
    fn cpu(&mut self, tree: &[Proc], cpus: u32, now: Instant) -> Option<HashMap<u32, f64>> {
        let gap = self.prev_at.map(|at| now.saturating_duration_since(at));
        if gap.is_some_and(|g| g < MIN_GAP) {
            // Слишком часто: прошлый замер остаётся точкой отсчёта.
            return None;
        }
        let out = gap.map(|gap| {
            let span = gap.as_nanos() as f64 * f64::from(cpus.max(1));
            tree.iter()
                .map(|p| {
                    let before = self.prev.get(&(p.pid, p.started)).copied();
                    // Новый процесс: его время — с запуска, а запущен он после прошлого замера.
                    let used = p.cpu_ns.saturating_sub(before.unwrap_or(p.cpu_ns));
                    (p.pid, (used as f64 / span * 100.0).clamp(0.0, 100.0))
                })
                .collect()
        });
        self.prev = tree
            .iter()
            .map(|p| ((p.pid, p.started), p.cpu_ns))
            .collect();
        self.prev_at = Some(now);
        out
    }

    /// Раскладывает процессы `tree` (Axiom первым) по вкладкам и частям браузера.
    /// `engine` — ответы движка: главный процесс окружения → его процессы;
    /// `owners` — (главный процесс окружения, главный фрейм) → чья страница.
    pub fn measure(
        &mut self,
        tree: &[Proc],
        engine: &[(u32, Vec<EngineProc>)],
        owners: &HashMap<(u32, u32), Owner>,
        now: Instant,
    ) -> View {
        let cpus = os::cpus();
        let cpu = self.cpu(tree, cpus, now);
        let usage_of = |pid: u32| -> Option<(f64, f64)> {
            let p = tree.iter().find(|p| p.pid == pid)?;
            let c = cpu
                .as_ref()
                .and_then(|c| c.get(&pid))
                .copied()
                .unwrap_or(0.0);
            Some((p.memory as f64, c))
        };

        let mut tabs: Vec<TabUse> = Vec::new();
        let mut parts: Vec<PartUse> = Vec::new();
        let mut part = |kind: Part, memory: f64, cpu: f64| {
            let at = match parts.iter().position(|p| p.part == kind) {
                Some(at) => at,
                None => {
                    parts.push(PartUse {
                        part: kind,
                        usage: Usage::default(),
                    });
                    parts.len() - 1
                }
            };
            parts[at].usage.add(memory, cpu);
        };
        let mut seen: Vec<u32> = Vec::new();

        for (browser, procs) in engine {
            for proc in procs {
                if seen.contains(&proc.pid) {
                    continue;
                }
                let Some((memory, cpu)) = usage_of(proc.pid) else {
                    continue;
                };
                seen.push(proc.pid);
                let kind = match proc.kind {
                    Kind::Browser => Part::Engine,
                    Kind::Gpu => Part::Gpu,
                    Kind::Utility => Part::Utility,
                    Kind::Renderer => {
                        let mut who: Vec<Owner> = Vec::new();
                        for root in &proc.roots {
                            if let Some(&owner) = owners.get(&(*browser, *root))
                                && !who.contains(&owner)
                            {
                                who.push(owner);
                            }
                        }
                        if who.is_empty() {
                            Part::Pages
                        } else {
                            let n = who.len() as f64;
                            for owner in who.iter() {
                                let (memory, cpu) = (memory / n, cpu / n);
                                match *owner {
                                    Owner::Ui => part(Part::Ui, memory, cpu),
                                    Owner::Ai => part(Part::Ai, memory, cpu),
                                    Owner::Tab(id) => {
                                        let at = match tabs.iter().position(|t| t.id == id) {
                                            Some(at) => at,
                                            None => {
                                                tabs.push(TabUse {
                                                    id,
                                                    usage: Usage::default(),
                                                    shared: false,
                                                });
                                                tabs.len() - 1
                                            }
                                        };
                                        tabs[at].usage.add(memory, cpu);
                                        tabs[at].shared |= who.len() > 1;
                                    }
                                }
                            }
                            continue;
                        }
                    }
                };
                part(kind, memory, cpu);
            }
        }

        // Остальные процессы Axiom: сам он, а без ответа движка — все по имени.
        let me = tree.first().map(|p| p.pid);
        for p in tree {
            if seen.contains(&p.pid) {
                continue;
            }
            let (memory, cpu) = usage_of(p.pid).unwrap_or_default();
            let kind = if Some(p.pid) == me {
                Part::App
            } else {
                by_name(&p.name)
            };
            part(kind, memory, cpu);
        }

        let mut total = Usage::default();
        for p in tree {
            let (memory, cpu) = usage_of(p.pid).unwrap_or_default();
            total.add(memory, cpu);
        }

        View {
            per_tab: !engine.is_empty(),
            tabs,
            parts,
            total,
            cpu_ready: cpu.is_some(),
            ram_total: 0,
            ram_free: 0,
            cpus,
        }
    }
}

/// Процесс, о котором движок не рассказал: WebKitGTK (Linux) или потомок без роли.
/// В `/proc/<pid>/stat` имя обрезано до 15 букв.
fn by_name(name: &str) -> Part {
    if name.starts_with("WebKitWebProces") {
        Part::Pages
    } else if name.starts_with("WebKitNetwork") {
        Part::Utility
    } else if name.starts_with("WebKitGPU") {
        Part::Gpu
    } else {
        Part::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, mb: u64, cpu_ms: u64, name: &str) -> Proc {
        Proc {
            pid,
            parent,
            started: 1,
            memory: mb << 20,
            cpu_ns: cpu_ms * 1_000_000,
            name: name.into(),
        }
    }

    fn r(pid: u32, kind: Kind, roots: &[u32]) -> EngineProc {
        EngineProc {
            pid,
            kind,
            roots: roots.to_vec(),
        }
    }

    const MB: u64 = 1 << 20;

    #[test]
    fn processes_go_to_their_tabs() {
        let tree = vec![
            p(1, 0, 40, 0, "axiom.exe"),
            p(10, 1, 100, 0, "msedgewebview2.exe"),
            p(11, 10, 200, 0, "msedgewebview2.exe"),
            p(12, 10, 300, 0, "msedgewebview2.exe"),
            p(13, 10, 50, 0, "msedgewebview2.exe"),
            p(14, 10, 80, 0, "msedgewebview2.exe"),
            p(15, 10, 20, 0, "msedgewebview2.exe"),
            p(16, 10, 60, 0, "msedgewebview2.exe"),
            p(17, 10, 30, 0, "msedgewebview2.exe"),
            p(18, 10, 10, 0, "msedgewebview2.exe"),
        ];
        let engine = vec![(
            10,
            vec![
                r(10, Kind::Browser, &[]),
                // Вкладка 7 и её фрейм в отдельном процессе (реклама с другого сайта).
                r(11, Kind::Renderer, &[701]),
                r(12, Kind::Renderer, &[701]),
                // Вкладки 8 и 9 делят процесс.
                r(13, Kind::Renderer, &[801, 901]),
                r(14, Kind::Renderer, &[1]),
                r(15, Kind::Gpu, &[]),
                r(16, Kind::Utility, &[]),
                // Запасной процесс: ничьих фреймов нет.
                r(17, Kind::Renderer, &[]),
            ],
        )];
        let owners = HashMap::from([
            ((10, 701), Owner::Tab(7)),
            ((10, 801), Owner::Tab(8)),
            ((10, 901), Owner::Tab(9)),
            ((10, 1), Owner::Ui),
        ]);
        let mut sampler = Sampler::default();
        let view = sampler.measure(&tree, &engine, &owners, Instant::now());
        assert!(view.per_tab && !view.cpu_ready);
        let tab = |id| {
            view.tabs
                .iter()
                .find(|t| t.id == id)
                .map(|t| (t.usage.memory, t.usage.procs, t.shared))
        };
        assert_eq!(tab(7), Some((500 * MB, 2, false)));
        assert_eq!(tab(8), Some((25 * MB, 1, true)));
        assert_eq!(tab(9), Some((25 * MB, 1, true)));
        let part = |k| {
            view.parts
                .iter()
                .find(|p| p.part == k)
                .map(|p| p.usage.memory)
        };
        assert_eq!(part(Part::Ui), Some(80 * MB));
        assert_eq!(part(Part::Engine), Some(100 * MB));
        assert_eq!(part(Part::Gpu), Some(20 * MB));
        assert_eq!(part(Part::Utility), Some(60 * MB));
        assert_eq!(part(Part::Pages), Some(30 * MB));
        assert_eq!(part(Part::App), Some(40 * MB));
        // Потомок, о котором движок молчит (crashpad).
        assert_eq!(part(Part::Other), Some(10 * MB));
        assert_eq!(view.total.memory, 890 * MB);
        assert_eq!(view.total.procs, 10);
        let sum: u64 = view.tabs.iter().map(|t| t.usage.memory).sum::<u64>()
            + view.parts.iter().map(|p| p.usage.memory).sum::<u64>();
        assert_eq!(sum, view.total.memory);
    }

    #[test]
    fn cpu_needs_two_samples() {
        let mut sampler = Sampler::default();
        let t0 = Instant::now();
        let cpus = 4;
        assert_eq!(sampler.cpu(&[p(1, 0, 1, 0, "")], cpus, t0), None);
        // За секунду процесс занял 2 секунды процессора (два ядра из четырёх) — 50 %.
        let next = [p(1, 0, 1, 2000, ""), p(2, 1, 1, 900, "")];
        let cpu = sampler
            .cpu(&next, cpus, t0 + Duration::from_secs(1))
            .unwrap_or_default();
        let of = |pid| cpu.get(&pid).copied().unwrap_or(-1.0);
        assert!((of(1) - 50.0).abs() < 1e-6);
        // Новый процесс: прошлого замера нет — не 100 %, а ноль до следующего.
        assert_eq!(of(2), 0.0);
        // Слишком часто — не считаем.
        let soon = t0 + Duration::from_millis(1100);
        assert_eq!(sampler.cpu(&next, cpus, soon), None);
    }

    #[test]
    fn linux_without_engine_by_name() {
        let tree = vec![
            p(1, 0, 90, 0, "axiom"),
            p(2, 1, 300, 0, "WebKitWebProces"),
            p(3, 1, 200, 0, "WebKitWebProces"),
            p(4, 1, 40, 0, "WebKitNetworkPr"),
        ];
        let view = Sampler::default().measure(&tree, &[], &HashMap::new(), Instant::now());
        assert!(!view.per_tab && view.tabs.is_empty());
        let part = |k| view.parts.iter().find(|p| p.part == k).map(|p| p.usage);
        assert_eq!(
            part(Part::Pages).map(|u| (u.memory, u.procs)),
            Some((500 * MB, 2))
        );
        assert_eq!(part(Part::Utility).map(|u| u.memory), Some(40 * MB));
        assert_eq!(part(Part::App).map(|u| u.memory), Some(90 * MB));
        assert_eq!(view.total.memory, 630 * MB);
    }
}
