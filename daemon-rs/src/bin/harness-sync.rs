//! harness-sync: Rust replacement for `python daemon/harness_sync.py`.
//! Modes: `--status` prints the mirror view; `--once` runs one sync pass.
//! Continuous polling is the same pass repeated on an interval (not yet wired).

use harness_sync::install::{install_journal, InstallTarget};
use harness_sync::lock::{with_claim, ClaimOutcome};
use harness_sync::mirror::{file_hash, mirror_uuid};
use harness_sync::pass::{format_status, PassSource};
use harness_sync::scan::{should_sync, ScanInput, MIN_BYTES};
use harness_sync::state::{Mirror, Source, State};
use harness_sync::{discovery, titles};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

struct Env {
    state_dir: PathBuf,
    omp: PathBuf,
    pi: PathBuf,
    claude: PathBuf,
    opencode_db: PathBuf,
    opencode_bin: PathBuf,
    hsconv: PathBuf,
    hslock: PathBuf,
    quiet_secs: f64,
    max_age_secs: f64,
}

impl Env {
    fn from_env() -> Env {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let get = |k: &str, d: PathBuf| std::env::var_os(k).map(PathBuf::from).unwrap_or(d);
        let num = |k: &str, d: f64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        Env {
            state_dir: get("HARNESS_SYNC_DIR", home.join(".local/share/harness-sync")),
            omp: get("OMP_AGENT_DIR", home.join(".omp/agent")),
            pi: get("PI_AGENT_DIR", home.join(".pi/agent")),
            claude: get("CLAUDE_CONFIG_DIR", home.join(".claude")),
            opencode_db: get("OPENCODE_DB", home.join(".local/share/opencode/opencode.db")),
            opencode_bin: get("OPENCODE_BIN", PathBuf::from("opencode")),
            hsconv: get("HSCONV", home.join("bin/hsconv")),
            hslock: get("HSLOCK", home.join("bin/hslock")),
            quiet_secs: num("HARNESS_SYNC_QUIET", 20.0),
            max_age_secs: num("HARNESS_SYNC_MAX_AGE_DAYS", 14.0) * 86400.0,
        }
    }
    fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }
    fn lock_dir(&self) -> PathBuf {
        std::env::var_os("HARNESS_SYNC_LOCK_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.state_dir.join("locks"))
    }
    fn store_for(&self, fmt: &str) -> &Path {
        if fmt == "omp" { &self.omp } else { &self.pi }
    }
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn mtime_secs(p: &Path) -> f64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Mirrors the daemon created, so a change to them is never treated as a source.
fn is_mirror(state: &State, key: &str) -> bool {
    state.mirror_keys.contains(key)
}

/// Find sources that need a sync, and the title to give their mirror.
fn scan(env: &Env, state: &mut State) -> Vec<PassSource> {
    let now = now_secs();
    let mut out = Vec::new();
    for (fmt, dir) in [("omp", &env.omp), ("pi", &env.pi)] {
        if !dir.is_dir() {
            continue;
        }
        let other = if fmt == "omp" { "pi" } else { "omp" };
        let other_present = if other == "omp" { env.omp.is_dir() } else { env.pi.is_dir() };
        for path in discovery::jsonl_sessions(dir) {
            let key = path.display().to_string();
            if is_mirror(state, &key) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let mtime = mtime_secs(&path);
            let src = state.sources.entry(key.clone()).or_insert_with(|| Source::new(fmt, &key));
            if mtime > src.last_seen_change {
                src.last_seen_change = mtime;
            }
            let stamp = format!("{}:{}", meta.len(), mtime);
            let hash = if stamp == src.last_stamp && !src.last_hash.is_empty() {
                src.last_hash.clone()
            } else {
                let h = file_hash(&path).unwrap_or_default();
                src.last_stamp = stamp;
                h
            };
            let hash_changed = hash != src.last_hash;
            let live: Vec<&Mirror> = src.mirrors.values().filter(|m| !m.diverged).collect();
            let live_current = live.iter().all(|m| m.source_hash == hash);
            let wanted = if other_present { 1 } else { 0 };
            let input = ScanInput {
                size: meta.len(),
                mtime,
                now,
                is_mirror: false,
                max_age_secs: env.max_age_secs,
                quiet_secs: env.quiet_secs,
                hash_changed,
                live_mirrors_current: live_current,
                mirror_count: src.mirrors.len(),
                wanted_mirrors: wanted,
            };
            if !should_sync(&input) || meta.len() < MIN_BYTES {
                continue;
            }
            let title = std::fs::read_to_string(&path)
                .map(|text| {
                    let lines: Vec<&str> = text.lines().collect();
                    titles::omp_title_from_lines(&lines).0
                })
                .unwrap_or_default();
            src.last_hash = hash;
            out.push(PassSource { key, title: if title.is_empty() { "untitled".into() } else { title } });
        }
    }
    out.extend(scan_opencode(env, state, now));
    out
}

/// O-penCode sessions that changed. Same rules as the Python: live sessions only,
/// no sessions older than the window, wait for quiet, and re-sync only when
/// time_updated moved. The title is the store's title, or the first prompt when the
/// store has only the default name.
fn scan_opencode(env: &Env, state: &mut State, now: f64) -> Vec<PassSource> {
    let mut out = Vec::new();
    if !env.opencode_db.exists() {
        return out;
    }
    for s in harness_sync::opencode::sessions(&env.opencode_db) {
        if state.mirror_keys.contains(&s.id) || now - s.updated > env.max_age_secs {
            continue;
        }
        let src = state.sources.entry(s.id.clone()).or_insert_with(|| Source::new("opencode", &s.id));
        if s.updated > src.last_seen_change {
            src.last_seen_change = s.updated;
        }
        if now - src.last_seen_change < env.quiet_secs {
            continue;
        }
        let stamp = format!("{:.3}", s.updated);
        if stamp == src.last_hash {
            continue;
        }
        src.last_hash = stamp;
        let title = harness_sync::opencode::clean_title(&s.title);
        let title = if title.is_empty() {
            let p = harness_sync::opencode::first_prompt(&env.opencode_db, &s.id);
            if p.is_empty() { "untitled session".to_string() } else { p }
        } else {
            title
        };
        out.push(PassSource { key: s.id.clone(), title });
    }
    out
}

/// Mirror an O-penCode session into the omp store. The session is exported and
/// converted to a journal, then written as an omp file with the same id rule as the
/// file-store mirrors. An O-penCode session has no file, so there is no cycle risk.
fn sync_opencode(env: &Env, state: &mut State, s: &PassSource) -> Result<(), String> {
    let target = "omp";
    let mirror_id = mirror_uuid(&s.key, target);
    let scratch = env.state_dir.join("scratch").join("oc");
    let conv = s.key.clone();
    let outcome = with_claim(&env.hslock, None, &conv, "daemon", std::process::id(), &s.key, || {
        let journal = harness_sync::opencode::export_and_convert(&env.opencode_bin, &env.hsconv, &s.key, &scratch)?;
        install_journal(
            &env.hsconv, &journal,
            InstallTarget::Omp { agent_dir: env.omp.clone(), home: env.omp.parent().unwrap_or(&env.omp).to_path_buf() },
            "/", &mirror_id, &s.title,
        )
    });
    let installed = match outcome {
        ClaimOutcome::Skipped(holder) => return Err(format!("held by {holder}; will retry")),
        ClaimOutcome::Ran(r) => r?,
    };
    let src = state.sources.get_mut(&s.key).ok_or("unknown source")?;
    let path = installed.display().to_string();
    state.mirror_keys.insert(path.clone());
    src.mirrors.insert(target.to_string(), Mirror {
        target: target.to_string(),
        path,
        source_hash: src.last_hash.clone(),
        installed_hash: file_hash(&installed).unwrap_or_default(),
        diverged: false,
    });
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}

/// Mirror one changed source into the other store. Claims the conversation while
/// it writes, so a harness turn cannot start mid-mirror.
fn sync_one(env: &Env, state: &mut State, s: &PassSource) -> Result<(), String> {
    if state.sources.get(&s.key).map(|src| src.fmt.as_str()) == Some("opencode") {
        return sync_opencode(env, state, s);
    }
    let (fmt, id) = {
        let src = state.sources.get(&s.key).ok_or("unknown source")?;
        (src.fmt.clone(), discovery::parse_omp_name(Path::new(&s.key).file_name().and_then(|n| n.to_str()).unwrap_or("")).unwrap_or_default())
    };
    let target = if fmt == "omp" { "pi" } else { "omp" };
    let target_store = if target == "pi" { env.pi.clone() } else { env.omp.clone() };
    let mirror_id = mirror_uuid(&s.key, target);
    let conv = if id.is_empty() { s.key.clone() } else { id };
    let src_path = PathBuf::from(&s.key);
    let cwd = "/".to_string();

    let outcome = with_claim(&env.hslock, None, &conv, "daemon", std::process::id(), &s.key, || {
        let t = if target == "pi" {
            InstallTarget::Pi { agent_dir: target_store.clone() }
        } else {
            InstallTarget::Omp { agent_dir: target_store.clone(), home: target_store.parent().unwrap_or(&target_store).to_path_buf() }
        };
        install_journal(&env.hsconv, &src_path, t, &cwd, &mirror_id, &s.title)
    });
    let installed = match outcome {
        ClaimOutcome::Skipped(holder) => return Err(format!("held by {holder}; will retry")),
        ClaimOutcome::Ran(r) => r?,
    };
    let src = state.sources.get_mut(&s.key).ok_or("unknown source")?;
    let source_hash = src.last_hash.clone();
    let path = installed.display().to_string();
    state.mirror_keys.insert(path.clone());
    src.mirrors.insert(target.to_string(), Mirror {
        target: target.to_string(),
        path: path.clone(),
        source_hash,
        installed_hash: file_hash(&installed).unwrap_or_default(),
        diverged: false,
    });
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = Env::from_env();
    let _ = std::fs::create_dir_all(&env.state_dir);
    let mut state = match State::load(&env.state_file()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("harness-sync: cannot read state {}: {e}", env.state_file().display());
            std::process::exit(1);
        }
    };
    if args.iter().any(|a| a == "--tui") {
        let roots = harness_sync::manage::Roots {
            omp: env.omp.join("sessions"),
            pi: env.pi.join("sessions"),
            claude: env.claude.join("projects"),
        };
        if let Err(e) = run_tui(roots, state, env.lock_dir(), env.hslock.clone()) {
            eprintln!("harness-sync: tui failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    if args.iter().any(|a| a == "--status") {
        print!("{}", format_status(&state));
        return;
    }
    if args.iter().any(|a| a == "--once") {
        let changed = scan(&env, &mut state);
        let attempted = changed.len();
        for s in &changed {
            if let Err(e) = sync_one(&env, &mut state, s) {
                if let Some(src) = state.sources.get_mut(&s.key) {
                    src.failures += 1;
                }
                eprintln!("prepare {} failed: {e}", s.key);
            }
        }
        if let Err(e) = state.save(&env.state_file()) {
            eprintln!("harness-sync: cannot save state: {e}");
            std::process::exit(1);
        }
        println!("one pass done, {attempted} source(s) synced");
        return;
    }
    eprintln!("usage: harness-sync --status | --once");
    std::process::exit(2);
}

/// Interactive manage screen. Raw mode and the alternate screen are restored on
/// every exit path, including a panic, so the terminal is never left unusable.
fn run_tui(roots: harness_sync::manage::Roots, state: State, lock_dir: PathBuf, hslock: PathBuf) -> std::io::Result<()> {
    use crossterm::event::{self, Event, KeyCode, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use crossterm::ExecutableCommand;
    use harness_sync::app::{App, Ev};
    use ratatui::{backend::CrosstermBackend, Terminal};
    use std::time::{Duration, Instant};

    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
            let _ = std::io::stdout().execute(LeaveAlternateScreen);
        }
    }

    enable_raw_mode()?;
    let _restore = Restore;
    std::io::stdout().execute(EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let mut app = App::new(roots, state, lock_dir, hslock);

    loop {
        term.draw(|f| harness_sync::ui::draw(f, &app))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != crossterm::event::KeyEventKind::Press {
            continue;
        }
        let ev = match (key.code, key.modifiers) {
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => Ev::CtrlD,
            (KeyCode::Char(c), _) => Ev::Char(c),
            (KeyCode::Backspace, _) => Ev::Backspace,
            (KeyCode::Enter, _) => Ev::Enter,
            (KeyCode::Esc, _) => Ev::Esc,
            _ => continue,
        };
        app.handle(ev, Instant::now());
        if app.should_quit() {
            return Ok(());
        }
    }
}
