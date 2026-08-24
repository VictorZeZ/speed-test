use crate::history::{self, TestRecord};
use crate::net::{self, ConnectionInfo, LatencyStats, Metrics, Phase, Profile, TestEvent};
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Test,
    History,
    Help,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Test, Tab::History, Tab::Help];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Idle,
    Running,
    Finished,
}

pub struct App {
    pub tab: Tab,
    pub should_quit: bool,
    pub profile: Profile,

    pub run_state: RunState,
    pub phase: Phase,
    pub error: Option<String>,

    pub connection: Option<ConnectionInfo>,
    pub pings: Vec<f64>,
    pub latency: Option<LatencyStats>,
    pub down_instant: f64,
    pub down_avg: f64,
    pub up_instant: f64,
    pub up_avg: f64,
    pub down_samples: Vec<u64>,
    pub up_samples: Vec<u64>,
    pub metrics: Option<Metrics>,

    pub spinner_frame: usize,

    test_tx: Option<UnboundedReceiver<TestEvent>>,
    cancel_flag: Option<Arc<AtomicBool>>,
    pub pending_meta: Option<UnboundedReceiver<Result<ConnectionInfo, String>>>,

    pub history: Vec<TestRecord>,
    pub history_selected: usize,
}

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl App {
    pub fn new() -> Self {
        Self {
            tab: Tab::Test,
            should_quit: false,
            profile: Profile::Standard,
            run_state: RunState::Idle,
            phase: Phase::Connect,
            error: None,
            connection: None,
            pings: Vec::new(),
            latency: None,
            down_instant: 0.0,
            down_avg: 0.0,
            up_instant: 0.0,
            up_avg: 0.0,
            down_samples: Vec::new(),
            up_samples: Vec::new(),
            metrics: None,
            spinner_frame: 0,
            test_tx: None,
            cancel_flag: None,
            pending_meta: None,
            history: Vec::new(),
            history_selected: 0,
        }
    }

    pub fn load_history(&mut self) {
        self.history = history::load();
        self.history_selected = self.history.len().saturating_sub(1);
    }

    pub fn grade(down_mbps: f64) -> (char, Color) {
        match down_mbps {
            x if x >= 300.0 => ('S', Color::LightGreen),
            x if x >= 100.0 => ('A', Color::Green),
            x if x >= 50.0 => ('B', Color::Yellow),
            x if x >= 25.0 => ('C', Color::LightYellow),
            x if x >= 10.0 => ('D', Color::LightRed),
            _ => ('F', Color::Red),
        }
    }

    pub fn start_test(&mut self) {
        self.run_state = RunState::Running;
        self.phase = Phase::Connect;
        self.error = None;
        self.pings.clear();
        self.latency = None;
        self.down_instant = 0.0;
        self.down_avg = 0.0;
        self.up_instant = 0.0;
        self.up_avg = 0.0;
        self.down_samples.clear();
        self.up_samples.clear();
        self.metrics = None;

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = unbounded_channel();
        self.cancel_flag = Some(cancel.clone());
        self.test_tx = Some(rx);

        let profile = self.profile;
        tokio::spawn(net::run_full_test(profile, cancel, tx));
    }

    pub fn cancel_test(&mut self) {
        if let Some(flag) = &self.cancel_flag {
            flag.store(true, Ordering::Relaxed);
        }
        self.test_tx = None;
        self.run_state = RunState::Idle;
    }

    pub fn tick(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER.len();
        if self.test_tx.is_some() {
            let mut done = false;
            while let Ok(ev) = self.test_tx.as_mut().unwrap().try_recv() {
                match ev {
                    TestEvent::Phase(p) => self.phase = p,
                    TestEvent::PingSample(ms) => self.pings.push(ms),
                    TestEvent::LatencyDone(stats) => self.latency = Some(stats),
                    TestEvent::Throughput {
                        instant_mbps,
                        avg_mbps,
                    } => match self.phase {
                        Phase::Download => {
                            self.down_instant = instant_mbps;
                            self.down_avg = avg_mbps;
                            self.down_samples.push(instant_mbps.max(0.05).round() as u64);
                        }
                        Phase::Upload => {
                            self.up_instant = instant_mbps;
                            self.up_avg = avg_mbps;
                            self.up_samples.push(instant_mbps.max(0.05).round() as u64);
                        }
                        _ => {}
                    },
                    TestEvent::PhaseDone { .. } => {}
                    TestEvent::Finished(metrics) => {
                        self.metrics = Some(metrics.clone());
                        self.run_state = RunState::Finished;
                        done = true;
                        let (grade, _) = Self::grade(metrics.down_mbps);
                        let record = TestRecord {
                            timestamp: Utc::now(),
                            profile: self.profile.name().to_string(),
                            down_mbps: metrics.down_mbps,
                            up_mbps: metrics.up_mbps,
                            ping_ms: metrics.latency.avg_ms,
                            jitter_ms: metrics.latency.jitter_ms,
                            grade,
                        };
                        self.history.push(record);
                        self.history_selected = self.history.len().saturating_sub(1);
                        if let Err(e) = history::save(&self.history) {
                            self.error = Some(format!("failed to save history: {e}"));
                        }
                    }
                    TestEvent::Failed(msg) => {
                        self.error = Some(msg);
                        self.run_state = RunState::Idle;
                        done = true;
                    }
                }
            }
            if done {
                self.test_tx = None;
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }
        if key.code == KeyCode::Char('q')
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && key.code == KeyCode::Char('c'))
        {
            if self.run_state == RunState::Running {
                self.cancel_test();
            }
            self.should_quit = true;
            return;
        }

        match key.code {
            KeyCode::Tab => self.tab = self.tab.next(),
            KeyCode::BackTab => self.tab = self.tab.prev(),
            _ => {}
        }

        match self.tab {
            Tab::Test => self.handle_test_key(key),
            Tab::History => self.handle_history_key(key),
            Tab::Help => {}
        }
    }

    fn handle_test_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('p') | KeyCode::Right | KeyCode::Down => {
                if self.run_state != RunState::Running {
                    self.profile = self.profile.next();
                }
            }
            KeyCode::Up => {
                if self.run_state != RunState::Running {
                    // cycle backwards through profiles
                    let idx = Profile::ALL
                        .iter()
                        .position(|p| *p == self.profile)
                        .unwrap_or(0);
                    self.profile =
                        Profile::ALL[(idx + Profile::ALL.len() - 1) % Profile::ALL.len()];
                }
            }
            KeyCode::Enter => {
                if self.run_state != RunState::Running {
                    self.start_test();
                }
            }
            KeyCode::Esc => {
                if self.run_state == RunState::Running {
                    self.cancel_test();
                } else {
                    self.should_quit = true;
                }
            }
            _ => {}
        }
    }

    fn handle_history_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.history_selected = self.history_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.history.is_empty() {
                    self.history_selected =
                        (self.history_selected + 1).min(self.history.len() - 1);
                }
            }
            KeyCode::Delete => {
                if !self.history.is_empty() {
                    self.history.clear();
                    self.history_selected = 0;
                    let _ = history::save(&self.history);
                }
            }
            KeyCode::Char('d') => {
                if !self.history.is_empty() {
                    self.history.remove(self.history_selected.min(self.history.len() - 1));
                    self.history_selected = self
                        .history_selected
                        .saturating_sub(1)
                        .min(self.history.len().saturating_sub(1));
                    let _ = history::save(&self.history);
                }
            }
            _ => {}
        }
    }
}
