use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use brain::decider::Client;

/// Starts the decider sidecar unless one already answers on the socket.
/// The returned child is killed when dropped.
pub struct Sidecar {
    child: Option<Child>,
    pub socket: PathBuf,
    pub log: PathBuf,
}

impl Sidecar {
    pub fn ensure(root: &Path, socket: PathBuf) -> Result<Self> {
        let log = std::env::temp_dir().join("digital-standin-decider.log");
        if Client::connect(&socket, Duration::from_secs(2)).and_then(|mut c| c.ping()).is_ok() {
            tracing::info!("decider already running on {}", socket.display());
            return Ok(Self { child: None, socket, log });
        }
        let dir = root.join("sidecar/decider");
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
            .context("starting the decider sidecar with uv")?;
        tracing::info!("decider sidecar starting, log at {}", log.display());
        // Ctrl+C and SIGTERM bypass every destructor, and the sidecar's own process group does not
        // receive the terminal's Ctrl+C. Without this it would keep 10 GB of GPU memory.
        let group = child.id() as i32;
        let _ = ctrlc::set_handler(move || {
            tracing::info!("stopping the decider sidecar");
            let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(group), nix::sys::signal::Signal::SIGTERM);
            std::process::exit(130);
        });
        Ok(Self { child: Some(child), socket, log })
    }

    pub fn exited(&mut self) -> Option<String> {
        let status = self.child.as_mut()?.try_wait().ok()??;
        self.child = None;
        Some(format!("decider exited with {status}; see {}", self.log.display()))
    }
}

impl Sidecar {
    /// Stops the sidecar if this process started it; one that was already running is left alone.
    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            tracing::info!("stopping the decider sidecar");
            let group = nix::unistd::Pid::from_raw(c.id() as i32);
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGTERM);
            let _ = c.wait();
        }
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        self.stop();
    }
}
