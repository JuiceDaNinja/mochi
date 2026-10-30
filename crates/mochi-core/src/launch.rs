//! Launching CLI brains the way a terminal would.
//!
//! On Windows, npm installs commands as `name.cmd`; `Command::new("name")` only finds
//! `name.exe`, so Mochi resolves the program through PATH + PATHEXT itself and runs
//! `.cmd`/`.bat` shims through `cmd /C`.

use std::path::{Path, PathBuf};

/// `(program, leading args)` to use with `Command::new(program).args(leading)`.
pub fn resolve_program(name: &str) -> (String, Vec<String>) {
    if !cfg!(windows) || Path::new(name).extension().is_some() && Path::new(name).is_absolute() {
        return (name.to_string(), vec![]);
    }
    let Some(found) = find_on_path(name) else { return (name.to_string(), vec![]) };
    let ext = found.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "cmd" || ext == "bat" {
        ("cmd".to_string(), vec!["/C".to_string(), found.to_string_lossy().to_string()])
    } else {
        (found.to_string_lossy().to_string(), vec![])
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.components().count() > 1 {
        return candidates(p).into_iter().find(|c| c.is_file());
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if let Some(hit) = candidates(&dir.join(name)).into_iter().find(|c| c.is_file()) {
            return Some(hit);
        }
    }
    None
}

/// `name`, then `name` + each PATHEXT extension (exe first, then cmd/bat, …).
fn candidates(base: &Path) -> Vec<PathBuf> {
    let mut v = vec![base.to_path_buf()];
    if base.extension().is_none() {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into());
        let mut list: Vec<String> = exts.split(';').filter(|e| !e.is_empty()).map(|e| e.to_ascii_lowercase()).collect();
        // Prefer a real exe over a shim when both exist.
        list.sort_by_key(|e| if e == ".exe" { 0 } else if e == ".cmd" { 1 } else { 2 });
        for e in list {
            let mut s = base.as_os_str().to_owned();
            s.push(&e);
            v.push(PathBuf::from(s));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_windows_passes_names_through() {
        if cfg!(windows) { return; }
        assert_eq!(resolve_program("claude"), ("claude".to_string(), vec![]));
    }

    #[test]
    fn candidates_prefer_exe_then_cmd() {
        std::env::set_var("PATHEXT", ".COM;.BAT;.CMD;.EXE");
        let c = candidates(Path::new("x"));
        let names: Vec<String> = c.iter().map(|p| p.to_string_lossy().to_string()).collect();
        assert_eq!(names[0], "x");
        assert_eq!(names[1], "x.exe");
        assert_eq!(names[2], "x.cmd");
    }
}
