//! One running Aether per session. The first instance owns a Unix socket in
//! `$XDG_RUNTIME_DIR` (0700, so only this user can reach it); launching again
//! just asks it to show its window, which matters once closing the window
//! leaves an effect running in the background.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("aether-he.sock"))
}

/// Ask a running instance to show itself. True if one answered.
pub fn signal_existing(path: &Path) -> bool {
    match UnixStream::connect(path) {
        Ok(mut s) => s.write_all(b"show\n").is_ok(),
        Err(_) => false,
    }
}

/// Own the socket (replacing a stale one) and call `on_show` for every
/// "show" request, on a background thread.
pub fn listen(path: &Path, on_show: impl Fn() + Send + 'static) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::thread::Builder::new().name("aether-instance".into()).spawn(move || {
        for conn in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(conn).read_line(&mut line).is_ok() && line.trim() == "show" {
                on_show();
            }
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn second_launch_reaches_the_first() {
        let path = std::env::temp_dir().join(format!("aether-inst-{}.sock", std::process::id()));
        assert!(!signal_existing(&path), "nobody listening yet");
        let (tx, rx) = channel();
        listen(&path, move || {
            let _ = tx.send(());
        })
        .unwrap();
        assert!(signal_existing(&path));
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
        let _ = std::fs::remove_file(path);
    }
}
