use std::cmp::Ordering;
use std::collections::HashSet;
use std::error::Error;
use std::io::Stdout;
use std::time::{Duration, Instant};

use bollard::Docker;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::TableState;
use tokio::sync::mpsc;

use crate::docker::{self, ContainerInfo, Snapshot, StatsManager};
use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Cpu,
    Mem,
    Name,
    Pids,
    Net,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Cpu => "CPU",
            SortKey::Mem => "MEM",
            SortKey::Name => "NAME",
            SortKey::Pids => "PIDS",
            SortKey::Net => "NET",
        }
    }

    pub fn ascending(self) -> bool {
        matches!(self, SortKey::Name)
    }
}

pub struct Entry {
    pub info: ContainerInfo,
    pub snap: Option<Snapshot>,
    pub history: Vec<u64>,
}

impl Entry {
    pub fn running(&self) -> bool {
        self.info.state == "running"
    }

    /// Sparkline ceiling: per-container CPU can exceed 100% on multi-core, so
    /// scale the graph to the observed peak (at least 100).
    pub fn spark_max(&self) -> u64 {
        self.history.iter().copied().max().unwrap_or(100).max(100)
    }

    fn metric(&self, key: SortKey) -> f64 {
        match (&self.snap, key) {
            (Some(s), SortKey::Cpu) => s.cpu_percent,
            (Some(s), SortKey::Mem) => s.mem_usage as f64,
            (Some(s), SortKey::Pids) => s.pids as f64,
            (Some(s), SortKey::Net) => (s.net_rx + s.net_tx) as f64,
            _ => -1.0,
        }
    }
}

pub struct App {
    pub docker: Docker,
    pub engine: String,
    pub show_all: bool,
    pub paused: bool,
    pub sort: SortKey,
    pub entries: Vec<Entry>,
    pub selected_id: Option<String>,
    pub help: bool,
    pub loaded: bool,
    pub daemon_ok: bool,
    pub message: Option<String>,
    pub list_every: Duration,
    pub last_request: Instant,
    pub manager: StatsManager,
    pub table_state: TableState,
    pub frame: u64,
    list_tx: mpsc::Sender<bool>,
    list_rx: mpsc::Receiver<Result<Vec<ContainerInfo>, String>>,
    force_apply_list: bool,
}

impl App {
    pub fn new(
        docker: Docker,
        engine: String,
        show_all: bool,
        list_every: Duration,
    ) -> (Self, mpsc::Receiver<(String, Snapshot)>) {
        // The daemon call itself runs on a worker task with a hard timeout so a
        // wedged daemon can never block the draw/event loop.
        let (list_tx, mut req_rx) = mpsc::channel::<bool>(1);
        let (res_tx, list_rx) = mpsc::channel(1);
        let worker_docker = docker.clone();
        tokio::spawn(async move {
            while let Some(all) = req_rx.recv().await {
                let result = match tokio::time::timeout(
                    Duration::from_secs(10),
                    docker::list(&worker_docker, all),
                )
                .await
                {
                    Ok(Ok(infos)) => Ok(infos),
                    Ok(Err(e)) => Err(format!("{e}")),
                    Err(_) => Err("timed out after 10s".into()),
                };
                if res_tx.send(result).await.is_err() {
                    break;
                }
            }
        });

        let (rx, manager) = StatsManager::new();
        (
            Self {
                docker,
                engine,
                show_all,
                paused: false,
                sort: SortKey::Cpu,
                entries: Vec::new(),
                selected_id: None,
                help: false,
                loaded: false,
                daemon_ok: true,
                message: None,
                list_every,
                last_request: Instant::now(),
                manager,
                table_state: TableState::default(),
                frame: 0,
                list_tx,
                list_rx,
                force_apply_list: false,
            },
            rx,
        )
    }

    pub async fn run(
        mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        mut rx: mpsc::Receiver<(String, Snapshot)>,
    ) -> Result<(), Box<dyn Error>> {
        self.request_list();
        let mut events = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            terminal.draw(|f| ui::draw(f, &mut self))?;
            self.frame += 1;
            tokio::select! {
                ev = events.next() => {
                    let Some(Ok(event)) = ev else { return Ok(()) };
                    if let Event::Key(key) = event {
                        if key.kind != KeyEventKind::Release && self.on_key(key) {
                            return Ok(());
                        }
                    }
                }
                recv = rx.recv() => {
                    if let Some((id, snap)) = recv {
                        if !self.paused {
                            self.apply_snapshot(&id, snap);
                        }
                    }
                }
                result = self.list_rx.recv() => {
                    if let Some(result) = result {
                        let force = std::mem::take(&mut self.force_apply_list);
                        if !self.paused || force {
                            self.apply_list(result);
                        }
                    }
                }
                _ = tick.tick() => {
                    if !self.paused && self.last_request.elapsed() >= self.list_every {
                        self.request_list();
                    }
                }
            }
        }
    }

    /// Returns true when the app should quit.
    fn on_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }
        if self.help {
            self.help = false;
            return key.code == KeyCode::Char('q');
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            // bare space only: Ctrl+Space (NUL byte) is terminal noise
            KeyCode::Char(' ') if key.modifiers.is_empty() => self.paused = !self.paused,
            KeyCode::Char('a') => {
                self.show_all = !self.show_all;
                self.request_list();
                self.force_apply_list = true;
            }
            KeyCode::Char('r') => {
                self.request_list();
                self.force_apply_list = true;
            }
            KeyCode::Char('h') | KeyCode::Char('?') => self.help = true,
            KeyCode::Char('c') => self.set_sort(SortKey::Cpu),
            KeyCode::Char('m') => self.set_sort(SortKey::Mem),
            KeyCode::Char('n') => self.set_sort(SortKey::Name),
            KeyCode::Char('p') => self.set_sort(SortKey::Pids),
            KeyCode::Char('u') => self.set_sort(SortKey::Net),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home | KeyCode::Char('g') => self.move_selection_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_selection_to(self.entries.len().saturating_sub(1)),
            _ => {}
        }
        false
    }

    fn set_sort(&mut self, key: SortKey) {
        self.sort = key;
        self.sort_entries();
    }

    fn move_selection(&mut self, delta: i64) {
        if self.entries.is_empty() {
            return;
        }
        let cur = self.selected_index() as i64;
        let next = (cur + delta).clamp(0, self.entries.len() as i64 - 1) as usize;
        self.move_selection_to(next);
    }

    fn move_selection_to(&mut self, idx: usize) {
        if let Some(e) = self.entries.get(idx) {
            self.selected_id = Some(e.info.id.clone());
        }
    }

    pub fn selected_index(&self) -> usize {
        self.entries
            .iter()
            .position(|e| Some(&e.info.id) == self.selected_id.as_ref())
            .unwrap_or(0)
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected_index())
    }

    fn request_list(&mut self) {
        self.last_request = Instant::now();
        let _ = self.list_tx.try_send(self.show_all);
    }

    fn apply_snapshot(&mut self, id: &str, snap: Snapshot) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.info.id == id) {
            e.history.push(snap.cpu_percent.round().max(0.0) as u64);
            if e.history.len() > 120 {
                e.history.remove(0);
            }
            e.snap = Some(snap);
            if !self.sort.ascending() {
                self.sort_entries();
            }
        }
    }

    fn apply_list(&mut self, result: Result<Vec<ContainerInfo>, String>) {
        self.loaded = true;
        match result {
            Ok(infos) => {
                self.daemon_ok = true;
                self.message = None;
                let mut live = HashSet::new();
                for info in infos {
                    live.insert(info.id.clone());
                    if let Some(e) = self.entries.iter_mut().find(|e| e.info.id == info.id) {
                        e.info = info;
                    } else {
                        self.entries.push(Entry {
                            info,
                            snap: None,
                            history: Vec::new(),
                        });
                    }
                }
                self.entries.retain(|e| live.contains(&e.info.id));
                // a container that is no longer running must not keep showing
                // (or contributing to totals with) its last live readings
                for e in &mut self.entries {
                    if !e.running() {
                        e.snap = None;
                        e.history.clear();
                    }
                }
                let running: HashSet<String> = self
                    .entries
                    .iter()
                    .filter(|e| e.running())
                    .map(|e| e.info.id.clone())
                    .collect();
                for id in &running {
                    self.manager.ensure(&self.docker, id);
                }
                self.manager.reconcile(&running);
                self.sort_entries();
            }
            Err(e) => {
                self.daemon_ok = false;
                self.message = Some(format!("daemon unreachable: {e}"));
            }
        }
    }

    fn sort_entries(&mut self) {
        let key = self.sort;
        self.entries.sort_by(|a, b| {
            let ord = if key.ascending() {
                a.info
                    .name
                    .to_lowercase()
                    .cmp(&b.info.name.to_lowercase())
            } else {
                b.metric(key)
                    .partial_cmp(&a.metric(key))
                    .unwrap_or(Ordering::Equal)
            };
            ord.then_with(|| a.info.name.to_lowercase().cmp(&b.info.name.to_lowercase()))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::ContainerInfo;

    fn entry(history: Vec<u64>) -> Entry {
        Entry {
            info: ContainerInfo {
                id: "abc".into(),
                name: "test".into(),
                image: "img".into(),
                state: "running".into(),
                status: "Up".into(),
                created: 0,
                ports: vec![],
            },
            snap: None,
            history,
        }
    }

    #[test]
    fn spark_max_floors_at_100() {
        assert_eq!(entry(vec![3, 42, 97]).spark_max(), 100);
        assert_eq!(entry(vec![]).spark_max(), 100);
    }

    #[test]
    fn spark_max_tracks_multi_core_peaks() {
        assert_eq!(entry(vec![100, 320, 40]).spark_max(), 320);
    }
}
