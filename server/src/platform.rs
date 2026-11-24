//! Cross-platform host lookups: home directories, the T3 data directory,
//! the device name, and the external programs the T3 adapter runs.
//!
//! Windows differs from Linux/macOS in ways that silently broke live mode:
//! `HOME` is usually unset, `/etc/hostname` does not exist, the T3 CLI is a
//! `t3.cmd` shim that `Command::new("t3")` will not find, and `python3` is
//! often the Microsoft Store stub rather than a working interpreter.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The current user's home directory (`HOME`, then `USERPROFILE`).
pub fn home_dir() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// T3's base directory: `T3CODE_HOME`, else `~/.t3`.
pub fn t3_home() -> Option<PathBuf> {
    std::env::var_os("T3CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".t3")))
}

/// This machine's name for the home airport banner.
pub fn device_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "local".to_string())
}

/// Executable suffixes to try for a bare program name.
fn executable_suffixes() -> Vec<String> {
    if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        pathext.split(';').filter(|ext| !ext.is_empty()).map(str::to_ascii_lowercase).collect()
    } else {
        vec![String::new()]
    }
}

/// Find `name` in `dirs`, trying each platform executable suffix.
pub fn find_in(name: &str, dirs: impl IntoIterator<Item = PathBuf>, suffixes: &[String]) -> Option<PathBuf> {
    dirs.into_iter().find_map(|dir| {
        suffixes
            .iter()
            .map(|ext| dir.join(format!("{name}{ext}")))
            .find(|candidate| candidate.is_file())
    })
}

/// Locate `name` on `PATH`, then in `extra` directories.
fn find_program(name: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::env::split_paths(&path).chain(extra.iter().cloned());
    find_in(name, dirs, &executable_suffixes())
}

/// Path to the T3 CLI: `SKYWATCH_T3_CLI`, `PATH` (including `t3.cmd` on
/// Windows), then the launcher T3 Code writes to `<T3 home>/bin`.
pub fn t3_cli() -> Option<&'static Path> {
    static CLI: OnceLock<Option<PathBuf>> = OnceLock::new();
    CLI.get_or_init(|| {
        let found = std::env::var_os("SKYWATCH_T3_CLI")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| find_program("t3", &t3_home().map(|h| vec![h.join("bin")]).unwrap_or_default()));
        match &found {
            Some(path) => tracing::info!(path = %path.display(), "using t3 CLI"),
            None => tracing::warn!("t3 CLI not found on PATH or in the T3 home bin directory; set SKYWATCH_T3_CLI"),
        }
        found
    })
    .as_deref()
}

/// A command that runs a working Python 3: `SKYWATCH_PYTHON`, then the first
/// of `python3`, `python`, or the Windows `py -3` launcher that actually runs.
pub fn python() -> Option<Command> {
    static PYTHON: OnceLock<Option<Vec<OsString>>> = OnceLock::new();
    let argv = PYTHON.get_or_init(|| {
        let candidates: Vec<Vec<OsString>> = match std::env::var_os("SKYWATCH_PYTHON").filter(|v| !v.is_empty()) {
            Some(explicit) => vec![vec![explicit]],
            None if cfg!(windows) => vec![vec!["py".into(), "-3".into()], vec!["python".into()], vec!["python3".into()]],
            None => vec![vec!["python3".into()], vec!["python".into()]],
        };
        let found = candidates.into_iter().find(|argv| {
            let mut probe = command(argv);
            probe.args(["-c", "import sys; sys.exit(sys.version_info[0] != 3)"]);
            probe.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            probe.status().map(|s| s.success()).unwrap_or(false)
        });
        match &found {
            Some(argv) => tracing::info!(python = ?argv, "using Python for the T3 MCP bridge"),
            None => tracing::warn!("no working Python 3 found for linked devices; set SKYWATCH_PYTHON"),
        }
        found
    });
    argv.as_ref().map(|argv| command(argv))
}

fn command(argv: &[OsString]) -> Command {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    // Python on Windows defaults to the ANSI code page for stdio and files.
    cmd.env("PYTHONUTF8", "1").env("PYTHONIOENCODING", "utf-8");
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_windows_shims_by_suffix_and_skips_directories() {
        let dir = std::env::temp_dir().join(format!("skywatch-find-{}", std::process::id()));
        let empty = dir.join("empty");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(bin.join("t3")).unwrap();
        std::fs::write(bin.join("t3.cmd"), "@echo off").unwrap();
        let suffixes = vec![String::new(), ".exe".into(), ".cmd".into()];
        assert_eq!(find_in("t3", [empty.clone(), bin.clone()], &suffixes), Some(bin.join("t3.cmd")));
        assert_eq!(find_in("t3", [empty], &suffixes), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
