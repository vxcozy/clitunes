//! clitunesd — clitunes audio daemon.
//!
//! Lifecycle: singleton flock → double-fork detach → rotating log →
//! tokio runtime → daemon event loop (control bus + source pipeline +
//! SPMC PCM ring + idle-exit timer).
//!
//! D15: this binary must never pull wgpu/ratatui/crossterm.

use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clitunes_engine::daemon::event_loop::DaemonEventLoop;
use clitunes_engine::daemon::{
    acquire_at, default_log_path, runtime_dir, set_socket_umask, write_pidfile, AcquireOutcome,
    DetachOutcome, IdleTimer, RotatingLog,
};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
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

    let runtime = runtime_dir().context("resolve runtime dir")?;
    let lock_path = runtime.join("clitunesd.lock");
    let lock = match acquire_at(&lock_path).context("acquire singleton lock")? {
        AcquireOutcome::Acquired(lock) => lock,
        AcquireOutcome::AlreadyRunning => {
            return Ok(ExitCode::SUCCESS);
        }
    };

    let _prior_umask = set_socket_umask();

    if !args.foreground {
        match unsafe { clitunes_engine::daemon::detach() }.context("daemon detach")? {
            DetachOutcome::Parent { child_pid: _ } => {
                return Ok(ExitCode::SUCCESS);
            }
            DetachOutcome::Daemon => {}
        }
    }

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

    let pid_path = runtime.join("clitunesd.pid");
    let pid = unsafe { libc::getpid() };
    if let Err(e) = write_pidfile(&pid_path, pid) {
        tracing::warn!(target: "clitunesd", error = %e, path = %pid_path.display(), "pidfile write failed");
    }

    let stop = Arc::new(AtomicBool::new(false));
    install_signal_handler(Arc::clone(&stop))?;

    let idle = Arc::new(match args.idle_timeout_secs {
        Some(secs) => IdleTimer::with_window(Duration::from_secs(secs)),
        None => IdleTimer::new(),
    });
    let socket_path = runtime.join("clitunesd.sock");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("build tokio runtime")?;

    let event_loop = DaemonEventLoop::new(socket_path, Arc::clone(&idle), Arc::clone(&stop));
    let result = rt.block_on(event_loop.run());

    if let Err(e) = &result {
        tracing::error!(target: "clitunesd", error = %e, "event loop error");
    }

    tracing::info!(target: "clitunesd", "clitunesd graceful shutdown");
    drop(lock);

    match result {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(_) => Ok(ExitCode::from(1)),
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
    idle_timeout_secs: Option<u64>,
}

impl CliArgs {
    fn parse_from_env() -> Result<Self> {
        let mut out = Self::default();
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-f" | "--foreground" => out.foreground = true,
                "-h" | "--help" => out.help = true,
                "--idle-timeout" => {
                    i += 1;
                    let val = args
                        .get(i)
                        .ok_or_else(|| anyhow::anyhow!("--idle-timeout requires a value"))?;
                    out.idle_timeout_secs = Some(
                        val.parse()
                            .map_err(|_| anyhow::anyhow!("invalid idle timeout: {val}"))?,
                    );
                }
                other => anyhow::bail!("unknown argument: {other}"),
            }
            i += 1;
        }
        Ok(out)
    }
}

fn print_help() {
    println!(
        "\
clitunesd — clitunes audio daemon

USAGE:
    clitunesd [--foreground] [--idle-timeout <seconds>]

OPTIONS:
    -f, --foreground            Do not fork; log to stderr as well as the rotating log file
        --idle-timeout <secs>   Override idle shutdown timeout (default: 30s)
    -h, --help                  Show this help

The daemon acquires an exclusive flock at $XDG_RUNTIME_DIR/clitunes/clitunesd.lock
(or $TMPDIR/$USER/clitunes/clitunesd.lock on macOS). A second invocation while
the lock is held exits 0 silently.

After the last control client disconnects, the daemon waits 30 seconds and then
exits cleanly."
    );
}
