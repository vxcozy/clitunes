//! clitunesd — clitunes audio daemon.
//!
//! Unit 9 delivers the lifecycle skeleton:
//!   * argv parsing (`--foreground`, `--help`)
//!   * flock-based singleton at `$runtime_dir/clitunesd.lock`
//!   * double-fork detach + stdio → /dev/null (unless `--foreground`)
//!   * size-rotated log file at `~/.cache/clitunes/clitunesd.log`
//!   * idle-exit loop that polls the shared `IdleTimer` state and exits
//!     cleanly 30 s after the last client disconnects.
//!
//! The control-socket protocol (Unit 10) and PCM ring bridge (Unit 11)
//! plug into this skeleton in later units. Until then the daemon
//! happily boots, waits out its idle window, and exits — which is
//! exactly what the auto-spawn tests expect.
//!
//! D15: this binary must never pull wgpu/ratatui/crossterm. The
//! `clitunes-engine::daemon` module lives under the `daemon` feature
//! for that reason. See `crates/clitunesd/Cargo.toml`.

use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clitunes_engine::daemon::{
    acquire_at, default_log_path, runtime_dir, set_socket_umask, write_pidfile, AcquireOutcome,
    DetachOutcome, IdleTimer, RotatingLog, Tick,
};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter};

/// Poll interval for the idle loop. The idle window is 30 s so 500 ms
/// ticks give plenty of resolution without burning cycles.
const IDLE_POLL: Duration = Duration::from_millis(500);

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            // We might be pre-tracing-init; always also fprintf to stderr
            // so a hard failure isn't swallowed by the rotating log.
            eprintln!("clitunesd: {e:#}");
            tracing::error!(error = %e, "clitunesd fatal");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode> {
    let args = CliArgs::parse_from_env()?;

    if args.help {
        print_help();
        return Ok(ExitCode::SUCCESS);
    }

    // Acquire the singleton lock *before* forking so the calling shell
    // sees an immediate "already running" exit if another daemon owns
    // the lock. Locking after fork would leak a detached process.
    let runtime = runtime_dir().context("resolve runtime dir")?;
    let lock_path = runtime.join("clitunesd.lock");
    let lock = match acquire_at(&lock_path).context("acquire singleton lock")? {
        AcquireOutcome::Acquired(lock) => lock,
        AcquireOutcome::AlreadyRunning => {
            // Exit 0 silently — auto-spawn races and deliberate double
            // starts should both be benign.
            return Ok(ExitCode::SUCCESS);
        }
    };

    // Apply the SEC-001 umask *before* any socket bind so the socket
    // inode is mode 0600 atomically. Unit 10 will inherit this when it
    // binds the control socket; we set it now so the fix lives in the
    // lifecycle layer rather than being easy to forget downstream.
    let _prior_umask = set_socket_umask();

    if !args.foreground {
        // SAFETY: we haven't spawned any threads or tokio runtimes yet.
        // `detach` must be called single-threaded.
        match unsafe { clitunes_engine::daemon::detach() }.context("daemon detach")? {
            DetachOutcome::Parent { child_pid: _ } => {
                // The parent holds no state the grandchild cares about;
                // exit cleanly so the shell returns.
                return Ok(ExitCode::SUCCESS);
            }
            DetachOutcome::Daemon => {
                // Fall through — we're the detached grandchild.
            }
        }
    }

    // Logging. After detach, stderr is /dev/null, so we need the file
    // subscriber; in --foreground we want both stderr and file.
    let log_path = default_log_path();
    init_tracing(&log_path, args.foreground).context("init tracing")?;

    tracing::info!(
        target: "clitunesd",
        version = env!("CARGO_PKG_VERSION"),
        foreground = args.foreground,
        lock_path = %lock.path().display(),
        log_path = %log_path.display(),
        "clitunesd booted",
    );

    // Informational pidfile. Real singleton enforcement is the flock
    // above; this file is just for `ps`/observability. If writing it
    // fails we log and move on.
    let pid_path = runtime.join("clitunesd.pid");
    // SAFETY: getpid is always safe.
    let pid = unsafe { libc::getpid() };
    if let Err(e) = write_pidfile(&pid_path, pid) {
        tracing::warn!(target: "clitunesd", error = %e, path = %pid_path.display(), "pidfile write failed");
    }

    // Install signal handler so SIGTERM / SIGINT trip the shutdown flag
    // and the idle loop exits on its next tick.
    let stop = Arc::new(AtomicBool::new(false));
    install_signal_handler(Arc::clone(&stop))?;

    let idle = Arc::new(IdleTimer::new());

    let exit = run_idle_loop(idle, stop);

    tracing::info!(target: "clitunesd", "clitunesd graceful shutdown");
    // Lock is released on drop, which the kernel would do for us on
    // exit anyway; explicit drop keeps the intent obvious.
    drop(lock);
    Ok(exit)
}

/// The idle loop. Until Unit 10 wires up the control socket, no client
/// will ever call `on_client_connected`, so the daemon naturally exits
/// after its idle window. That's exactly the behaviour we want for the
/// auto-spawn-then-nothing-connects path.
fn run_idle_loop(idle: Arc<IdleTimer>, stop: Arc<AtomicBool>) -> ExitCode {
    loop {
        if stop.load(Ordering::SeqCst) {
            tracing::info!(target: "clitunesd", "shutdown signal observed");
            return ExitCode::SUCCESS;
        }
        match idle.tick() {
            Tick::Busy => {}
            Tick::Idle { remaining: _ } => {}
            Tick::Expired => {
                tracing::info!(
                    target: "clitunesd",
                    window_secs = idle.window().as_secs(),
                    "idle window elapsed; exiting"
                );
                return ExitCode::SUCCESS;
            }
        }
        thread::sleep(IDLE_POLL);
    }
}

fn init_tracing(log_path: &Path, foreground: bool) -> Result<()> {
    let default_filter = "clitunesd=info,clitunes_engine=info,warn";
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let rotating = RotatingLog::open(log_path.to_path_buf())
        .with_context(|| format!("open log file {}", log_path.display()))?;
    let rotating: &'static RotatingLog = Box::leak(Box::new(rotating));

    let file_layer = fmt::layer()
        .with_target(true)
        .with_ansi(false)
        .with_writer(rotating);

    if foreground {
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(fmt::layer().with_target(true).with_writer(std::io::stderr))
            .try_init()
            .map_err(|e| anyhow::anyhow!("tracing init: {e}"))?;
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .try_init()
            .map_err(|e| anyhow::anyhow!("tracing init: {e}"))?;
    }
    Ok(())
}

fn install_signal_handler(stop: Arc<AtomicBool>) -> Result<()> {
    extern "C" fn handler(_sig: libc::c_int) {
        HANDLED.store(true, Ordering::SeqCst);
    }
    static HANDLED: AtomicBool = AtomicBool::new(false);

    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as *const () as usize;
        sa.sa_flags = 0;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
    }

    thread::Builder::new()
        .name("clitunesd-signal".into())
        .spawn(move || loop {
            if HANDLED.load(Ordering::SeqCst) {
                stop.store(true, Ordering::SeqCst);
                return;
            }
            thread::sleep(Duration::from_millis(50));
        })
        .context("spawn signal watcher")?;
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct CliArgs {
    foreground: bool,
    help: bool,
}

impl CliArgs {
    fn parse_from_env() -> Result<Self> {
        let mut out = Self::default();
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "-f" | "--foreground" => out.foreground = true,
                "-h" | "--help" => out.help = true,
                other => anyhow::bail!("unknown argument: {other}"),
            }
        }
        Ok(out)
    }
}

fn print_help() {
    println!(
        "\
clitunesd — clitunes audio daemon

USAGE:
    clitunesd [--foreground]

OPTIONS:
    -f, --foreground    Do not fork; log to stderr as well as the rotating log file
    -h, --help          Show this help

The daemon acquires an exclusive flock at $XDG_RUNTIME_DIR/clitunes/clitunesd.lock
(or $TMPDIR/$USER/clitunes/clitunesd.lock on macOS). A second invocation while
the lock is held exits 0 silently.

After the last control client disconnects, the daemon waits 30 seconds and then
exits cleanly."
    );
}
