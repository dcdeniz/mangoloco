//! The whisper models the app offers, where they live on disk, and how they are downloaded.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::Cancelled;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    /// Short stable identifier, e.g. `"base.en"`.
    pub id: &'static str,
    /// File name inside [`models_dir`], e.g. `"ggml-base.en.bin"`.
    pub file: &'static str,
    /// Text for the model picker.
    pub label: &'static str,
    /// Exact size of the file in bytes; a download is only accepted when it matches.
    pub bytes: u64,
    /// False for the English-only (`.en`) models.
    pub multilingual: bool,
    /// Where the file is downloaded from (`https://`, `http://` or `file://`).
    pub url: String,
}

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// The models offered for download: ggml files from the official whisper.cpp repository on
/// Hugging Face, smallest first within each group. Sizes are the published file sizes.
pub fn catalog() -> Vec<ModelInfo> {
    let entry = |id: &'static str, file: &'static str, label: &'static str, bytes: u64, multilingual: bool| ModelInfo {
        id,
        file,
        label,
        bytes,
        multilingual,
        url: format!("{BASE_URL}/{file}"),
    };
    vec![
        entry("tiny.en", "ggml-tiny.en.bin", "Tiny (English) - fastest, least accurate, 78 MB", 77_704_715, false),
        entry("base.en", "ggml-base.en.bin", "Base (English) - fast, good for clear speech, 148 MB", 147_964_211, false),
        entry("small.en", "ggml-small.en.bin", "Small (English) - balanced, 488 MB", 487_614_201, false),
        entry("base", "ggml-base.bin", "Base (multilingual) - fast, 148 MB", 147_951_465, true),
        entry("small", "ggml-small.bin", "Small (multilingual) - balanced, 488 MB", 487_601_967, true),
        entry("medium", "ggml-medium.bin", "Medium (multilingual) - accurate, slower, 1.5 GB", 1_533_763_059, true),
        entry(
            "large-v3-turbo",
            "ggml-large-v3-turbo.bin",
            "Large v3 Turbo (multilingual) - most accurate, 1.6 GB",
            1_624_555_275,
            true,
        ),
    ]
}

/// Folder that holds downloaded models: `MANGO_MODELS_DIR` if set, otherwise
/// `<data dir>/models`.
pub fn models_dir() -> PathBuf {
    match std::env::var_os("MANGO_MODELS_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => mango_core::tools::data_dir().join("models"),
    }
}

/// ggml files start with the little-endian magic 0x67676d6c.
fn looks_like_model(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    path.is_file()
        && File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok()
        && u32::from_le_bytes(magic) == 0x6767_6d6c
}

/// Catalogue models present in [`models_dir`], in catalogue order.
pub fn installed() -> Vec<(ModelInfo, PathBuf)> {
    let dir = models_dir();
    catalog()
        .into_iter()
        .filter_map(|info| {
            let path = dir.join(info.file);
            looks_like_model(&path).then_some((info, path))
        })
        .collect()
}

/// Path of an installed model by id (`"base.en"`). Ids outside the catalogue are looked up
/// as `ggml-<id>.bin`, so models the user copied in by hand can be used too.
pub fn find(id: &str) -> Option<PathBuf> {
    let id = id.trim();
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        return None;
    }
    let dir = models_dir();
    let file = match catalog().into_iter().find(|m| m.id == id) {
        Some(info) => info.file.to_string(),
        None => format!("ggml-{id}.bin"),
    };
    let path = dir.join(file);
    looks_like_model(&path).then_some(path)
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    dest.with_file_name(name)
}

/// Downloads a model into [`models_dir`] and returns its path.
///
/// Data goes to `<file>.part`, which is renamed into place only after the size (and the ggml
/// header) check out, so an interrupted download is never mistaken for a model. A `.part`
/// left by an earlier attempt is resumed when the server supports ranges and restarted
/// otherwise. `on_progress` receives `0..=1`. Setting `cancel` stops within one network read
/// and returns [`Cancelled`]; the `.part` file is kept so the download can resume later.
pub fn download(info: &ModelInfo, on_progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<PathBuf> {
    if info.file.is_empty() || info.file.contains(['/', '\\']) || info.file.contains("..") {
        bail!("invalid model file name {:?}", info.file);
    }
    let dir = models_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create the models folder {}", dir.display()))?;
    let dest = dir.join(info.file);
    let part = part_path(&dest);

    let mut last = -1.0f32;
    let mut report = |done: u64| {
        let v = if info.bytes > 0 { (done as f64 / info.bytes as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
        if v > last {
            last = v;
            on_progress(v);
        }
    };

    if looks_like_model(&dest) && (info.bytes == 0 || std::fs::metadata(&dest).map(|m| m.len()).ok() == Some(info.bytes)) {
        report(info.bytes);
        if info.bytes == 0 {
            on_progress(1.0);
        }
        return Ok(dest);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled.into());
    }

    // What an earlier attempt left behind.
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if info.bytes > 0 && have > info.bytes {
        let _ = std::fs::remove_file(&part);
        have = 0;
    }
    report(0);

    if !(info.bytes > 0 && have == info.bytes) {
        let (mut source, resumed) = open_source(&info.url, have)?;
        let mut file = if resumed {
            let mut f = OpenOptions::new().append(true).open(&part).with_context(|| format!("could not open {}", part.display()))?;
            f.seek(SeekFrom::End(0))?;
            f
        } else {
            have = 0;
            File::create(&part).with_context(|| format!("could not create {}", part.display()))?
        };
        report(have);
        let mut buf = vec![0u8; 128 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = file.flush();
                return Err(Cancelled.into());
            }
            let n = match source.read(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let _ = file.flush();
                    return Err(anyhow!(e).context(format!(
                        "the download of {} was interrupted; try again and it will continue where it stopped",
                        info.file
                    )));
                }
            };
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).with_context(|| format!("could not write {} (is the disk full?)", part.display()))?;
            have += n as u64;
            if info.bytes > 0 && have > info.bytes {
                drop(file);
                let _ = std::fs::remove_file(&part);
                bail!("the server sent more data than the expected {} bytes for {}; nothing was installed", info.bytes, info.file);
            }
            report(have);
        }
        file.flush()?;
        file.sync_all().ok();
    }

    let size = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if info.bytes > 0 && size != info.bytes {
        if size < info.bytes {
            // Keep the partial data: the next attempt resumes from here.
            bail!(
                "the download of {} stopped early ({size} of {} bytes); try again and it will continue where it stopped",
                info.file,
                info.bytes
            );
        }
        let _ = std::fs::remove_file(&part);
        bail!("downloaded file for {} has the wrong size ({size} bytes, expected {})", info.file, info.bytes);
    }
    if !looks_like_model(&part) {
        let _ = std::fs::remove_file(&part);
        bail!("the file downloaded from {} is not a whisper model; nothing was installed", info.url);
    }
    std::fs::rename(&part, &dest).with_context(|| format!("could not move the download into place at {}", dest.display()))?;
    if info.bytes == 0 {
        on_progress(1.0);
    } else {
        report(info.bytes);
    }
    Ok(dest)
}

/// Opens the model source, asking to start at byte `offset`. Returns the reader and whether
/// it really starts at `offset` (true) or at the beginning (false).
fn open_source(url: &str, offset: u64) -> Result<(Box<dyn Read>, bool)> {
    if let Some(path) = url.strip_prefix("file://") {
        let mut f = File::open(path).with_context(|| format!("could not open {path}"))?;
        let len = f.metadata()?.len();
        if offset > 0 && offset <= len {
            f.seek(SeekFrom::Start(offset))?;
            return Ok((Box::new(f), true));
        }
        return Ok((Box::new(f), false));
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        bail!("unsupported download address {url}");
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .user_agent("MangoLoco")
        .build()
        .into();
    let mut request = agent.get(url).header("Accept-Encoding", "identity");
    if offset > 0 {
        request = request.header("Range", format!("bytes={offset}-"));
    }
    let response = request.call().map_err(|e| anyhow!("could not reach the model server ({e}). Check the internet connection."))?;
    let status = response.status().as_u16();
    match status {
        206 if offset > 0 => {
            // Only trust a resume when the server confirms the range starts where we asked.
            let confirmed = response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.trim().starts_with(&format!("bytes {offset}-")));
            if confirmed {
                Ok((Box::new(response.into_body().into_reader()), true))
            } else {
                drop(response);
                open_source(url, 0)
            }
        }
        200 => Ok((Box::new(response.into_body().into_reader()), false)),
        // The partial file does not fit what the server has: start over.
        416 if offset > 0 => {
            drop(response);
            open_source(url, 0)
        }
        other => bail!("the model server answered with HTTP {other} for {url}"),
    }
}
