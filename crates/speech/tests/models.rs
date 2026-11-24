use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use mango_speech::models::{self, ModelInfo};

/// `MANGO_MODELS_DIR` is process-wide; tests that set it take this lock.
static ENV: Mutex<()> = Mutex::new(());

struct ModelsDir {
    dir: tempfile::TempDir,
    _guard: MutexGuard<'static, ()>,
}

impl ModelsDir {
    fn new() -> Self {
        let guard = ENV.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: every test that touches the environment holds ENV.
        unsafe { std::env::set_var("MANGO_MODELS_DIR", dir.path().join("models")) };
        Self { dir, _guard: guard }
    }
    fn path(&self) -> PathBuf {
        self.dir.path().join("models")
    }
    fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.path())
            .map(|d| d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }
}

/// Something that passes for a ggml file: the magic number followed by noise.
fn fake_model(len: usize) -> Vec<u8> {
    let mut bytes = 0x6767_6d6cu32.to_le_bytes().to_vec();
    let mut x = 99u32;
    while bytes.len() < len {
        x = x.wrapping_mul(1664525).wrapping_add(1013904223);
        bytes.push((x >> 16) as u8);
    }
    bytes
}

#[derive(Clone, Default)]
struct Behaviour {
    ranges: bool,
    /// Pause after every 16 KiB, to give a test time to cancel.
    slow: bool,
    /// Close the connection after this many body bytes.
    drop_after: Option<usize>,
    status: Option<u16>,
}

struct Server {
    url: String,
    requests: Arc<AtomicUsize>,
    range_headers: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// A tiny HTTP/1.1 server for one file.
fn serve(body: Vec<u8>, behaviour: Behaviour) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/ggml-test.bin", listener.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let range_headers = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (rq, rh, st) = (requests.clone(), range_headers.clone(), stop.clone());
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        while !st.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let (body, behaviour, rq, rh) = (body.clone(), behaviour.clone(), rq.clone(), rh.clone());
                    std::thread::spawn(move || handle(stream, &body, &behaviour, &rq, &rh));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    });
    Server { url, requests, range_headers, stop }
}

fn handle(stream: TcpStream, body: &[u8], behaviour: &Behaviour, requests: &AtomicUsize, ranges: &Mutex<Vec<String>>) {
    stream.set_nonblocking(false).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut stream = stream;
    let mut range: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("range:") {
            ranges.lock().unwrap().push(value.trim().to_string());
            range = value.trim().strip_prefix("bytes=").and_then(|v| v.trim_end_matches('-').parse().ok());
        }
    }
    requests.fetch_add(1, Ordering::SeqCst);
    if let Some(status) = behaviour.status {
        let _ = write!(stream, "HTTP/1.1 {status} Nope\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    let (from, head) = match range {
        Some(from) if behaviour.ranges && from < body.len() => (
            from,
            format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {from}-{}/{}\r\n", body.len() - 1, body.len()),
        ),
        Some(_) if behaviour.ranges => {
            let _ = write!(stream, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        _ => (0, "HTTP/1.1 200 OK\r\n".to_string()),
    };
    let payload = &body[from..];
    let _ = write!(stream, "{head}Content-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n", payload.len());
    let mut sent = 0;
    for chunk in payload.chunks(16 * 1024) {
        if let Some(limit) = behaviour.drop_after {
            if sent >= limit {
                return; // connection drops mid-body
            }
        }
        if stream.write_all(chunk).is_err() {
            return;
        }
        let _ = stream.flush();
        sent += chunk.len();
        if behaviour.slow {
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

fn info(url: &str, bytes: u64) -> ModelInfo {
    ModelInfo { id: "test", file: "ggml-test.bin", label: "Test model", bytes, multilingual: false, url: url.to_string() }
}

fn download(info: &ModelInfo) -> (anyhow::Result<PathBuf>, Vec<f32>) {
    let mut seen = Vec::new();
    let result = models::download(info, &mut |p| seen.push(p), &AtomicBool::new(false));
    (result, seen)
}

fn assert_progress(seen: &[f32], complete: bool) {
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|p| (0.0..=1.0).contains(p)), "{seen:?}");
    assert!(seen.windows(2).all(|w| w[1] >= w[0]), "progress went backwards: {seen:?}");
    if complete {
        assert_eq!(*seen.last().unwrap(), 1.0, "{seen:?}");
    } else {
        assert!(*seen.last().unwrap() < 1.0, "{seen:?}");
    }
}

#[test]
fn catalog_is_well_formed() {
    let catalog = models::catalog();
    let ids: Vec<&str> = catalog.iter().map(|m| m.id).collect();
    assert_eq!(ids, ["tiny.en", "base.en", "small.en", "base", "small", "medium", "large-v3-turbo"]);
    for m in &catalog {
        assert_eq!(m.file, format!("ggml-{}.bin", m.id));
        assert_eq!(m.url, format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}", m.file));
        assert!(!m.label.trim().is_empty() && m.label.is_ascii(), "{m:?}");
        assert!(m.bytes > 50_000_000 && m.bytes < 4_000_000_000, "{m:?}");
        assert_eq!(m.multilingual, !m.id.ends_with(".en"), "{m:?}");
    }
    let size = |id: &str| catalog.iter().find(|m| m.id == id).unwrap().bytes;
    assert!(size("tiny.en") < size("base.en") && size("base.en") < size("small.en") && size("small") < size("medium") && size("medium") < size("large-v3-turbo"));
    // The catalogue sizes agree with the real files used for development.
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models");
    for id in ["tiny.en", "base.en"] {
        let m = catalog.iter().find(|m| m.id == id).unwrap();
        if let Ok(meta) = std::fs::metadata(dev.join(m.file)) {
            assert_eq!(meta.len(), m.bytes, "catalogue size of {id} does not match the real file");
        }
    }
}

#[test]
fn models_dir_honours_the_environment() {
    let guard = ENV.lock().unwrap_or_else(|p| p.into_inner());
    let previous_data = std::env::var_os("MANGO_DATA_DIR");
    let data = tempfile::tempdir().unwrap();
    // SAFETY: ENV is held.
    unsafe {
        std::env::set_var("MANGO_MODELS_DIR", "/tmp/some where/models");
        assert_eq!(models::models_dir(), PathBuf::from("/tmp/some where/models"));
        std::env::remove_var("MANGO_MODELS_DIR");
        std::env::set_var("MANGO_DATA_DIR", data.path());
        assert_eq!(models::models_dir(), data.path().join("models"));
        match previous_data {
            Some(v) => std::env::set_var("MANGO_DATA_DIR", v),
            None => std::env::remove_var("MANGO_DATA_DIR"),
        }
    }
    drop(guard);
}

#[test]
fn find_and_installed_see_only_real_model_files() {
    let dir = ModelsDir::new();
    assert!(models::installed().is_empty(), "a missing folder means nothing is installed");
    assert_eq!(models::find("base.en"), None);

    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(dir.path().join("ggml-base.en.bin"), fake_model(4096)).unwrap();
    std::fs::write(dir.path().join("ggml-small.bin"), b"<html>404 not found</html>").unwrap(); // not a model
    std::fs::write(dir.path().join("ggml-medium.bin"), b"").unwrap(); // empty
    std::fs::write(dir.path().join("ggml-tiny.en.bin.part"), fake_model(4096)).unwrap(); // unfinished download
    std::fs::write(dir.path().join("ggml-my-finetune.bin"), fake_model(4096)).unwrap(); // user-supplied
    std::fs::create_dir_all(dir.path().join("ggml-base.bin")).unwrap(); // a folder, not a file

    assert_eq!(models::find("base.en"), Some(dir.path().join("ggml-base.en.bin")));
    assert_eq!(models::find(" base.en "), Some(dir.path().join("ggml-base.en.bin")));
    for missing in ["small", "medium", "tiny.en", "base", "large-v3-turbo", "nonsense", "", "../models/ggml-base.en", "/etc/passwd"] {
        assert_eq!(models::find(missing), None, "{missing:?} must not be found");
    }
    assert_eq!(models::find("my-finetune"), Some(dir.path().join("ggml-my-finetune.bin")));
    let installed = models::installed();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].0.id, "base.en");
    assert_eq!(installed[0].1, dir.path().join("ggml-base.en.bin"));
}

#[test]
fn download_installs_the_model_and_reports_progress() {
    let dir = ModelsDir::new();
    let body = fake_model(700_000);
    let server = serve(body.clone(), Behaviour::default());
    let (result, seen) = download(&info(&server.url, body.len() as u64));
    let path = result.unwrap();
    assert_eq!(path, dir.path().join("ggml-test.bin"));
    assert_eq!(std::fs::read(&path).unwrap(), body);
    assert_progress(&seen, true);
    assert!(seen.len() >= 4, "progress should advance in steps: {seen:?}");
    assert_eq!(dir.entries(), ["ggml-test.bin"], "no .part file may remain");
    assert_eq!(models::find("test"), Some(path.clone()));

    // Already installed: nothing is fetched again.
    let before = server.requests.load(Ordering::SeqCst);
    let (again, seen) = download(&info(&server.url, body.len() as u64));
    assert_eq!(again.unwrap(), path);
    assert_eq!(*seen.last().unwrap(), 1.0);
    assert_eq!(server.requests.load(Ordering::SeqCst), before);
}

#[test]
fn cancel_leaves_no_model_and_the_download_resumes() {
    let dir = ModelsDir::new();
    let body = fake_model(1_500_000);
    let model = {
        let server = serve(body.clone(), Behaviour { ranges: true, slow: true, ..Default::default() });
        let model = info(&server.url, body.len() as u64);
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let started = std::time::Instant::now();
        let mut cancelled_at = None;
        let result = models::download(
            &model,
            &mut |p| {
                seen.push(p);
                if p > 0.3 && !cancel.swap(true, Ordering::SeqCst) {
                    cancelled_at = Some(std::time::Instant::now());
                }
            },
            &cancel,
        );
        let err = result.expect_err("cancelled download must not succeed");
        assert!(mango_speech::is_cancelled(&err), "expected Cancelled, got: {err}");
        assert!(cancelled_at.unwrap().elapsed() < Duration::from_millis(500), "cancel must be prompt");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_progress(&seen, false);
        model
    };
    assert!(!dir.path().join("ggml-test.bin").exists(), "no model file after cancel");
    assert_eq!(models::find("test"), None);
    assert!(models::installed().is_empty());
    assert_eq!(dir.entries(), ["ggml-test.bin.part"]);
    let partial = std::fs::metadata(dir.path().join("ggml-test.bin.part")).unwrap().len();
    assert!(partial > 0 && partial < body.len() as u64);

    // Second attempt continues from where the first stopped.
    let server = serve(body.clone(), Behaviour { ranges: true, ..Default::default() });
    let (result, seen) = download(&ModelInfo { url: server.url.clone(), ..model });
    let path = result.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), body, "resumed file must be byte-identical");
    assert_eq!(server.range_headers.lock().unwrap().as_slice(), [format!("bytes={partial}-")]);
    assert_progress(&seen, true);
    assert!(seen[0] > 0.0 || seen[1] >= partial as f32 / body.len() as f32 - 0.01, "progress should start from the resumed position: {seen:?}");
    assert_eq!(dir.entries(), ["ggml-test.bin"]);
}

#[test]
fn restart_is_clean_when_the_server_cannot_resume() {
    let dir = ModelsDir::new();
    let body = fake_model(300_000);
    std::fs::create_dir_all(dir.path()).unwrap();
    // A stale partial file with different content.
    std::fs::write(dir.path().join("ggml-test.bin.part"), vec![0xAAu8; 100_000]).unwrap();
    let server = serve(body.clone(), Behaviour { ranges: false, ..Default::default() });
    let (result, seen) = download(&info(&server.url, body.len() as u64));
    assert_eq!(std::fs::read(result.unwrap()).unwrap(), body, "stale partial data must not end up in the model");
    assert_progress(&seen, true);
    assert_eq!(dir.entries(), ["ggml-test.bin"]);

    // A partial file larger than the model is discarded too.
    std::fs::remove_file(dir.path().join("ggml-test.bin")).unwrap();
    std::fs::write(dir.path().join("ggml-test.bin.part"), vec![0xAAu8; 400_000]).unwrap();
    let ranged = serve(body.clone(), Behaviour { ranges: true, ..Default::default() });
    let (result, _) = download(&info(&ranged.url, body.len() as u64));
    assert_eq!(std::fs::read(result.unwrap()).unwrap(), body);
}

#[test]
fn dropped_connection_is_an_error_and_the_retry_completes() {
    let dir = ModelsDir::new();
    let body = fake_model(600_000);
    let flaky = serve(body.clone(), Behaviour { ranges: true, drop_after: Some(200_000), ..Default::default() });
    let (result, seen) = download(&info(&flaky.url, body.len() as u64));
    let err = result.expect_err("a short download must not be installed");
    assert!(!mango_speech::is_cancelled(&err));
    assert!(err.to_string().contains("try again"), "unclear error: {err}");
    assert_progress(&seen, false);
    assert_eq!(models::find("test"), None);
    assert!(!dir.path().join("ggml-test.bin").exists());

    let healthy = serve(body.clone(), Behaviour { ranges: true, ..Default::default() });
    let (result, _) = download(&info(&healthy.url, body.len() as u64));
    assert_eq!(std::fs::read(result.unwrap()).unwrap(), body);
    assert_eq!(healthy.range_headers.lock().unwrap().len(), 1, "the retry should resume, not restart");
}

#[test]
fn wrong_size_wrong_content_and_http_errors_install_nothing() {
    let dir = ModelsDir::new();
    let body = fake_model(200_000);

    // Server has more data than the catalogue says.
    let server = serve(body.clone(), Behaviour::default());
    let (result, _) = download(&info(&server.url, 150_000));
    let err = result.unwrap_err();
    assert!(err.to_string().contains("150000"), "unclear error: {err}");
    assert!(dir.entries().is_empty(), "{:?}", dir.entries());

    // Right size, but it is an HTML error page rather than a model.
    let html = vec![b'<'; 200_000];
    let server = serve(html, Behaviour::default());
    let (result, _) = download(&info(&server.url, 200_000));
    let err = result.unwrap_err();
    assert!(err.to_string().contains("not a whisper model"), "unclear error: {err}");
    assert!(dir.entries().is_empty(), "{:?}", dir.entries());

    let server = serve(body.clone(), Behaviour { status: Some(404), ..Default::default() });
    let (result, _) = download(&info(&server.url, body.len() as u64));
    let err = result.unwrap_err();
    assert!(err.to_string().contains("404"), "unclear error: {err}");

    // Nobody listening.
    let (result, _) = download(&info("http://127.0.0.1:9/ggml-test.bin", body.len() as u64));
    assert!(result.unwrap_err().to_string().contains("could not reach"));

    let (result, _) = download(&info("ftp://example.invalid/ggml-test.bin", 10));
    assert!(result.unwrap_err().to_string().contains("unsupported"));

    let bad_name = ModelInfo { file: "../escape.bin", ..info(&server.url, 10) };
    assert!(models::download(&bad_name, &mut |_| {}, &AtomicBool::new(false)).is_err());

    assert_eq!(models::find("test"), None);
    assert!(dir.entries().iter().all(|n| n.ends_with(".part") || n.is_empty()), "no model may be installed: {:?}", dir.entries());
    assert!(!dir.dir.path().join("escape.bin").exists());
}

#[test]
fn file_urls_work_and_precancelled_download_does_nothing() {
    let dir = ModelsDir::new();
    let body = fake_model(250_000);
    let source = dir.dir.path().join("source.bin");
    std::fs::write(&source, &body).unwrap();
    let model = info(&format!("file://{}", source.display()), body.len() as u64);

    let err = models::download(&model, &mut |_| {}, &AtomicBool::new(true)).unwrap_err();
    assert!(mango_speech::is_cancelled(&err));
    assert_eq!(models::find("test"), None);

    let (result, seen) = download(&model);
    assert_eq!(std::fs::read(result.unwrap()).unwrap(), body);
    assert_progress(&seen, true);
    assert_eq!(dir.entries(), ["ggml-test.bin"]);
}
