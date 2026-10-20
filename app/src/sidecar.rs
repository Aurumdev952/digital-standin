use std::fs::File;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Once;
use std::time::Duration;

use anyhow::{Context, Result};
use brain::decider::Client;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use parking_lot::Mutex;

/// The Python servers the app runs, each from `sidecar/<name>/server.py`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Decider,
    Diarizer,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Decider => "decider",
            Kind::Diarizer => "diarizer",
        }
    }

    pub fn default_socket(self) -> PathBuf {
        match self {
            Kind::Decider => brain::decider::default_socket(),
            Kind::Diarizer => {
                std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| "/tmp".into()).join("digital-standin-diarizer.sock")
            }
        }
    }

    /// Whether a server already answers on `socket`.
    fn running(self, socket: &Path) -> bool {
        match self {
            Kind::Decider => Client::connect(socket, Duration::from_secs(2)).and_then(|mut c| c.ping()).is_ok(),
            Kind::Diarizer => UnixStream::connect(socket).is_ok(),
        }
    }
}

/// Process groups of the sidecars this process started, for the Ctrl+C handler.
static GROUPS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Ctrl+C and SIGTERM bypass every destructor, and a sidecar's own process group does not receive
/// the terminal's Ctrl+C. Without this they would keep their GPU memory. `ctrlc` accepts only one
/// handler per process, so one handler stops every sidecar.
fn install_stop_handler() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = ctrlc::set_handler(|| {
            for g in GROUPS.lock().iter() {
                let _ = killpg(Pid::from_raw(*g), Signal::SIGTERM);
            }
            crate::exit_now(130);
        });
    });
}

/// A sidecar server, started unless one already answers on its socket.
/// One this process started is stopped when dropped.
pub struct Sidecar {
    pub kind: Kind,
    child: Option<Child>,
    pub socket: PathBuf,
    pub log: PathBuf,
}

impl Sidecar {
    pub fn ensure(root: &Path, kind: Kind) -> Result<Self> {
        let name = kind.name();
        let socket = kind.default_socket();
        let log = std::env::temp_dir().join(format!("digital-standin-{name}.log"));
        if kind.running(&socket) {
            tracing::info!("{name} already running on {}", socket.display());
            return Ok(Self { kind, child: None, socket, log });
        }
        let dir = root.join("sidecar").join(name);
        install_stop_handler();
        let child = Command::new("uv")
            .args(["run", "--project"])
            .arg(&dir)
            .arg("python")
            .arg(dir.join("server.py"))
            .arg("--socket")
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(File::create(&log)?)
            .stderr(File::create(&log)?)
            // Its own process group, so stopping it also stops the Python child that `uv` starts.
            .process_group(0)
            .spawn()
            .with_context(|| format!("starting the {name} sidecar with uv"))?;
        tracing::info!("{name} sidecar starting, log at {}", log.display());
        GROUPS.lock().push(child.id() as i32);
        Ok(Self { kind, child: Some(child), socket, log })
    }

    pub fn exited(&mut self) -> Option<String> {
        let status = self.child.as_mut()?.try_wait().ok()??;
        self.child = None;
        Some(format!("{} exited with {status}; see {}", self.kind.name(), self.log.display()))
    }

    /// Stops the sidecar if this process started it; one that was already running is left alone.
    /// Signals the whole group and returns at once; Python takes seconds to release CUDA and needs
    /// no help from us to finish.
    pub fn stop(&mut self) {
        if let Some(c) = self.child.take() {
            tracing::info!("stopping the {} sidecar", self.kind.name());
            let group = c.id() as i32;
            let _ = killpg(Pid::from_raw(group), Signal::SIGTERM);
            GROUPS.lock().retain(|g| *g != group);
        }
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        self.stop();
    }
}
