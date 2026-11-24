//! Locating the external tools the app shells out to.

use std::path::PathBuf;

/// Finds `ffmpeg`, `ffprobe` and similar. Order: `MANGO_<NAME>` environment
/// variable, next to the executable, the app bundle's `Resources`, common
/// install locations, then `PATH`.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let env_key = format!("MANGO_{}", name.to_uppercase().replace('-', "_"));
    if let Some(p) = std::env::var_os(&env_key).map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
            dirs.push(dir.join("../Resources"));
            dirs.push(dir.join("../Resources/bin"));
        }
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// Per-user cache directory for derived files (waveforms, decoded audio).
pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("MANGO_CACHE_DIR").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        if cfg!(target_os = "macos") { home.join("Library/Caches/MangoLoco") } else { home.join(".cache/mangoloco") }
    });
    let _ = std::fs::create_dir_all(&base);
    base
}

/// Per-user data directory (speech models, settings).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("MANGO_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        if cfg!(target_os = "macos") { home.join("Library/Application Support/MangoLoco") } else { home.join(".local/share/mangoloco") }
    });
    let _ = std::fs::create_dir_all(&base);
    base
}
