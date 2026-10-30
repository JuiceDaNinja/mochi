//! Mochi's local voice: starts the `mochi-tts` sidecar (see `tts/mochi_tts.py`) on a free
//! localhost port and tells the webview where it is. The sidecar downloads the Kokoro model
//! on first run and answers `POST /tts`; the webview's `HostedTtsVoice` falls back to the
//! Windows voice whenever the sidecar is absent or not ready yet.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

pub struct Tts {
    child: Mutex<Option<Child>>,
    pub port: Option<u16>,
    /// Why there is no local voice, for the Settings panel (None when running).
    pub unavailable: Option<String>,
}

impl Tts {
    /// Start the sidecar if it is bundled; never fails the app.
    pub fn start(resource_dir: &Path, data_dir: &Path) -> Tts {
        let Some(exe) = find_sidecar(resource_dir) else {
            return Tts { child: Mutex::new(None), port: None, unavailable: Some("local voice not bundled in this build".into()) };
        };
        let port = match free_port() {
            Some(p) => p,
            None => return Tts { child: Mutex::new(None), port: None, unavailable: Some("no free port".into()) },
        };
        let mut cmd = Command::new(&exe);
        cmd.arg("--port").arg(port.to_string())
            .arg("--data-dir").arg(data_dir)
            .arg("--parent-pid").arg(std::process::id().to_string())
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        match cmd.spawn() {
            Ok(child) => Tts { child: Mutex::new(Some(child)), port: Some(port), unavailable: None },
            Err(e) => Tts { child: Mutex::new(None), port: None, unavailable: Some(format!("could not start {}: {e}", exe.display())) },
        }
    }

    pub fn endpoint(&self) -> Option<String> {
        self.port.map(|p| format!("http://127.0.0.1:{p}"))
    }

    pub fn stop(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Tts {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `binaries/mochi-tts/mochi-tts(.exe)` under the resource dir (PyInstaller one-folder build),
/// or next to the executable in dev.
fn find_sidecar(resource_dir: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "mochi-tts.exe" } else { "mochi-tts" };
    let mut candidates = vec![resource_dir.join("binaries").join("mochi-tts").join(name)];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("binaries").join("mochi-tts").join(name));
        }
    }
    candidates.into_iter().find(|p| p.exists())
}

fn free_port() -> Option<u16> {
    std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?.local_addr().ok().map(|a| a.port())
}
