use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEvent};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

use crate::animation::Animation;
use crate::animation::external::ExternalProgram;
use crate::animation::fire::Fire;
use crate::animation::matrix::MatrixRain;
use crate::animation::purr::Purr;
use crate::animation::starfield::Starfield;
use crate::compositor::Compositor;
use crate::config::Config;
use crate::input::{encode_key_event, encode_mouse_event};
use crate::pty::PtyHandle;
use crate::terminal_state::TerminalState;

const STAR_COUNT: usize = 150;

pub struct App {
    terminal_state: TerminalState,
    pty: PtyHandle,
    application_cursor: Arc<AtomicBool>,
    animations: Vec<Box<dyn Animation>>,
    animation_index: usize, // index into animations; len == "none"
    compositor: Compositor,
    config: Config,
}

impl App {
    pub fn new(rows: u16, cols: u16, config: Config) -> Result<Self> {

        let pty = PtyHandle::spawn(rows, cols, &config.shell)?;

        let bg_rgb = config.background_rgb();
        let bg_color = ratatui::style::Color::Rgb(bg_rgb.0, bg_rgb.1, bg_rgb.2);

        let mut animations: Vec<Box<dyn Animation>> = vec![
            Box::new(Starfield::new(STAR_COUNT, bg_color)),
            Box::new(MatrixRain::new(cols, rows, bg_color)),
            Box::new(Fire::new(cols, rows, bg_color)),
            Box::new(Purr::new(bg_color)),
        ];

        // Add external animation if configured
        let mut external_index = None;
        if let Some(ref cmd) = config.external_command {
            let repo_url = config.external_command_repo.as_deref();
            let install_cmd = config.external_command_install.as_deref();
            if let Some(ext) = ExternalProgram::new(cmd, rows, cols, repo_url, install_cmd, bg_color) {
                external_index = Some(animations.len());
                animations.push(Box::new(ext));
            }
        }

        // Resolve initial animation index from config
        let animation_index = match config.animation.as_str() {
            "starfield" => 0,
            "matrix" => 1,
            "fire" => 2,
            "purr" => 3,
            "external" => external_index.unwrap_or(0),
            "none" => animations.len(), // past end = "none"
            _ => 0,
        };

        let compositor = Compositor {
            animation_enabled: animation_index < animations.len(),
            opacity: config.opacity,
            fallback_bg: bg_color,
        };

        Ok(Self {
            terminal_state: TerminalState::new(rows, cols),
            pty,
            application_cursor: Arc::new(AtomicBool::new(false)),
            animations,
            animation_index,
            compositor,
            config,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        let mut terminal = setup_terminal()?;
        let _guard = TerminalGuard;

        // Set terminal emulator background to match config (fills window padding)
        set_terminal_background(self.config.background_hex());

        // PTY reader → channel (blocking I/O bridged to async)
        let pty_reader = self.pty.try_clone_reader()?;
        let (pty_tx, mut pty_rx) = mpsc::channel::<Vec<u8>>(256);
        tokio::task::spawn_blocking(move || {
            pty_reader_task(pty_reader, pty_tx);
        });

        // PTY writer shared between input task and main loop (for mouse)
        let pty_writer = self.pty.take_writer()?;
        let pty_writer = Arc::new(std::sync::Mutex::new(pty_writer));

        // Input task
        let app_cursor = self.application_cursor.clone();
        let writer_for_input = pty_writer.clone();
        let (input_tx, mut input_rx) = mpsc::channel::<AppEvent>(64);
        tokio::spawn(async move {
            input_task(app_cursor, writer_for_input, input_tx).await;
        });

        // Tick timer from config fps
        let tick_ms = 1000 / self.config.fps.max(1) as u64;
        let tick_duration = Duration::from_millis(tick_ms);
        let mut tick = interval(tick_duration);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let dt = tick_ms as f64 / 1000.0;
        let mut running = true;
        while running {
            tokio::select! {
                // PTY output
                Some(bytes) = pty_rx.recv() => {
                    self.terminal_state.process(&bytes);
                    self.application_cursor.store(
                        self.terminal_state.screen().application_cursor(),
                        Ordering::Relaxed,
                    );
                }
                // Input events
                Some(event) = input_rx.recv() => {
                    match event {
                        AppEvent::Resize(cols, rows) => {
                            self.terminal_state.resize(rows, cols);
                            let _ = self.pty.resize(rows, cols);
                            for anim in &mut self.animations {
                                anim.resize(rows, cols);
                            }
                        }
                        AppEvent::DecreaseOpacity => {
                            self.compositor.opacity = (self.compositor.opacity - 0.1).max(0.0);
                        }
                        AppEvent::IncreaseOpacity => {
                            self.compositor.opacity = (self.compositor.opacity + 0.1).min(1.0);
                        }
                        AppEvent::ToggleAnimation => {
                            self.compositor.animation_enabled = !self.compositor.animation_enabled;
                        }
                        AppEvent::CycleAnimation => {
                            // Cycle: starfield → matrix → fire → purr → none → starfield
                            self.animation_index = (self.animation_index + 1) % (self.animations.len() + 1);
                            self.compositor.animation_enabled = self.animation_index < self.animations.len();
                        }
                        AppEvent::Mouse(mouse_event) => {
                            let screen = self.terminal_state.screen();
                            let mode = screen.mouse_protocol_mode();
                            let encoding = screen.mouse_protocol_encoding();
                            let bytes = encode_mouse_event(&mouse_event, mode, encoding);
                            if !bytes.is_empty() {
                                if let Ok(mut writer) = pty_writer.lock() {
                                    let _ = writer.write_all(&bytes);
                                    let _ = writer.flush();
                                }
                            }
                        }
                    }
                }
                // Tick — update animation + render frame
                _ = tick.tick() => {
                    // Check if child exited
                    if let Ok(Some(_)) = self.pty.try_wait() {
                        running = false;
                        continue;
                    }

                    // Suppress animation when in alternate screen (vim, htop, etc.)
                    let in_alt_screen = self.terminal_state.screen().alternate_screen();
                    let suppress = in_alt_screen && self.config.disable_animation_alt_screen;

                    // Tick the current animation forward
                    if self.compositor.animation_enabled && !suppress {
                        if let Some(anim) = self.animations.get_mut(self.animation_index) {
                            anim.tick(dt);
                        }
                    }

                    let ts = &self.terminal_state;
                    let compositor = &self.compositor;
                    // Get current animation or a dummy reference for "none" mode
                    let anim_ref: &dyn Animation = if self.animation_index < self.animations.len() {
                        self.animations[self.animation_index].as_ref()
                    } else {
                        self.animations[0].as_ref()
                    };
                    let render_suppress = suppress;
                    terminal.draw(|frame| {
                        if render_suppress {
                            // Render shell directly without animation
                            render_shell_only(ts, frame);
                        } else {
                            compositor.render(ts, anim_ref, frame);
                        }
                    })?;
                }
            }
        }

        Ok(())
    }
}

enum AppEvent {
    Resize(u16, u16),
    DecreaseOpacity,
    IncreaseOpacity,
    ToggleAnimation,
    CycleAnimation,
    Mouse(MouseEvent),
}

/// Render shell content directly without animation background.
/// Used when alternate screen apps are running (vim, htop, etc.).
fn render_shell_only(ts: &TerminalState, frame: &mut ratatui::Frame) {
    use crate::color::convert_vt100_color;
    use ratatui::layout::Position;
    use ratatui::style::{Modifier, Style};

    let area = frame.area();
    let screen = ts.screen();
    let buf = frame.buffer_mut();

    let rows = area.height.min(screen.size().0);
    let cols = area.width.min(screen.size().1);

    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };

            if cell.is_wide_continuation() {
                continue;
            }

            let pos = Position::new(col, row);
            let Some(buf_cell) = buf.cell_mut(pos) else {
                continue;
            };

            let contents = cell.contents();
            if contents.is_empty() {
                buf_cell.set_symbol(" ");
            } else {
                buf_cell.set_symbol(&contents);
            }

            let mut fg = convert_vt100_color(cell.fgcolor());
            let mut bg = convert_vt100_color(cell.bgcolor());

            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }

            buf_cell.set_fg(fg);
            buf_cell.set_bg(bg);

            let mut modifier = Modifier::empty();
            if cell.bold() {
                modifier |= Modifier::BOLD;
            }
            if cell.italic() {
                modifier |= Modifier::ITALIC;
            }
            if cell.underline() {
                modifier |= Modifier::UNDERLINED;
            }
            buf_cell.set_style(Style::default().add_modifier(modifier));
        }
    }

    if !screen.hide_cursor() {
        let (cursor_row, cursor_col) = screen.cursor_position();
        if cursor_row < area.height && cursor_col < area.width {
            frame.set_cursor_position(Position::new(cursor_col, cursor_row));
        }
    }
}

fn pty_reader_task(mut reader: Box<dyn io::Read + Send>, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break, // EOF
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

async fn input_task(
    application_cursor: Arc<AtomicBool>,
    pty_writer: Arc<std::sync::Mutex<Box<dyn io::Write + Send>>>,
    event_tx: mpsc::Sender<AppEvent>,
) {
    let mut stream = EventStream::new();
    while let Some(Ok(event)) = stream.next().await {
        match event {
            Event::Key(key_event) => {
                // crossterm sends Press, Repeat, Release — only handle Press and Repeat
                if key_event.kind == KeyEventKind::Release {
                    continue;
                }

                // Intercept Alt+key as app hotkeys
                if key_event.modifiers.contains(KeyModifiers::ALT) {
                    match key_event.code {
                        KeyCode::Char('j') => {
                            let _ = event_tx.send(AppEvent::DecreaseOpacity).await;
                            continue;
                        }
                        KeyCode::Char('k') => {
                            let _ = event_tx.send(AppEvent::IncreaseOpacity).await;
                            continue;
                        }
                        KeyCode::Char('a') => {
                            let _ = event_tx.send(AppEvent::ToggleAnimation).await;
                            continue;
                        }
                        KeyCode::Char('n') => {
                            let _ = event_tx.send(AppEvent::CycleAnimation).await;
                            continue;
                        }
                        _ => {}
                    }
                }

                let app_cursor = application_cursor.load(Ordering::Relaxed);
                let bytes = encode_key_event(&key_event, app_cursor);
                if !bytes.is_empty() {
                    if let Ok(mut writer) = pty_writer.lock() {
                        let _ = writer.write_all(&bytes);
                        let _ = writer.flush();
                    }
                }
            }
            Event::Mouse(mouse_event) => {
                let _ = event_tx.send(AppEvent::Mouse(mouse_event)).await;
            }
            Event::Resize(cols, rows) => {
                let _ = event_tx.send(AppEvent::Resize(cols, rows)).await;
            }
            _ => {}
        }
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    crossterm::terminal::enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
    )
    .context("Failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("Failed to create terminal")
}

/// RAII guard that restores terminal state on drop
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        reset_terminal_background();
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show,
        );
    }
}

/// Set the terminal emulator's background color via OSC 11.
fn set_terminal_background(hex: &str) {
    let _ = write!(io::stdout(), "\x1b]11;{}\x07", hex);
    let _ = io::stdout().flush();
}

/// Reset the terminal emulator's background to its default via OSC 110.
fn reset_terminal_background() {
    let _ = write!(io::stdout(), "\x1b]110\x07");
    let _ = io::stdout().flush();
}
