//! Decoded-audio cache: correctness of the samples, cache behaviour, errors and cancellation.

mod common;
use common::*;
use mango_playback::pcm::{Cancelled, Pcm, PcmCache};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

fn ensure(cache: &PcmCache, path: &Path) -> anyhow::Result<Option<Arc<Pcm>>> {
    cache.ensure(path, &mut |_| {}, &AtomicBool::new(false))
}

/// What ffmpeg itself decodes to 48 kHz stereo s16.
fn reference_decode(path: &Path) -> Vec<[i16; 2]> {
    let out = Command::new(tool("ffmpeg"))
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-ac", "2", "-ar", "48000", "-f", "s16le", "-c:a", "pcm_s16le", "pipe:1"])
        .output()
        .unwrap();
    assert!(out.status.success());
    out.stdout.as_chunks::<4>().0.iter().map(|b| [i16::from_le_bytes([b[0], b[1]]), i16::from_le_bytes([b[2], b[3]])]).collect()
}

fn read_all(pcm: &Pcm) -> Vec<[f32; 2]> {
    let mut v = vec![[0.0f32; 2]; pcm.frames() as usize];
    assert_eq!(pcm.read(0, &mut v), v.len());
    v
}

fn files_with_ext(dir: &Path, ext: &str) -> Vec<std::path::PathBuf> {
    match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == ext)).collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn length_matches_ffprobe_duration() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PcmCache::with_dir(dir.path());
    let wav = tone_wav(8);
    let pcm = ensure(&cache, &wav).unwrap().expect("has audio");
    assert_eq!(pcm.frames(), 8 * 48_000);
    assert!((pcm.duration() - probe_duration(&wav)).abs() < 1e-6);

    // Compressed audio in a video container: within one AAC frame of the container duration.
    let mp4 = numbered_video(6, true);
    let pcm = ensure(&cache, &mp4).unwrap().expect("has audio");
    assert!((pcm.duration() - probe_duration(&mp4)).abs() < 0.03, "{} vs {}", pcm.duration(), probe_duration(&mp4));
}

#[test]
fn samples_match_an_independent_decode() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PcmCache::with_dir(dir.path());

    // 48 kHz stereo PCM source: bit exact.
    let wav = tone_wav(8);
    let pcm = ensure(&cache, &wav).unwrap().unwrap();
    let reference = reference_decode(&wav);
    assert_eq!(reference.len() as u64, pcm.frames());
    let ours = read_all(&pcm);
    for (i, (o, r)) in ours.iter().zip(&reference).enumerate() {
        assert_eq!([o[0], o[1]], [r[0] as f32 / 32768.0, r[1] as f32 / 32768.0], "frame {i}");
    }
    // And it is the tone we generated: source second 3 is 600 Hz.
    assert_eq!(dominant_freq(span(&ours, 3.1, 3.9), 0), step_freq(3));

    // 44.1 kHz mono source: resampled and upmixed like ffmpeg does it.
    let mono = sine_wav("mono450at441.wav", 450, 2.0, 44_100, 1);
    let pcm = ensure(&cache, &mono).unwrap().unwrap();
    let reference = reference_decode(&mono);
    assert!((pcm.frames() as i64 - reference.len() as i64).abs() <= 1, "{} vs {}", pcm.frames(), reference.len());
    assert!((pcm.duration() - 2.0).abs() < 0.001);
    let ours = read_all(&pcm);
    let mut worst = 0.0f32;
    for (o, r) in ours.iter().zip(&reference) {
        worst = worst.max((o[0] - r[0] as f32 / 32768.0).abs()).max((o[1] - r[1] as f32 / 32768.0).abs());
    }
    assert!(worst <= 2.0 / 32768.0, "resampled audio differs from ffmpeg's by {worst}");
    assert_eq!(dominant_freq(span(&ours, 0.5, 1.5), 1), 450.0);
    assert!((rms(span(&ours, 0.5, 1.5), 0) - rms(span(&ours, 0.5, 1.5), 1)).abs() < 1e-6, "mono is centred");

    // AAC: compare against ffmpeg's own decode of the same file.
    let mp4 = numbered_video(6, true);
    let pcm = ensure(&cache, &mp4).unwrap().unwrap();
    let reference = reference_decode(&mp4);
    let ours = read_all(&pcm);
    assert!((ours.len() as i64 - reference.len() as i64).abs() <= 1);
    let mut worst = 0.0f32;
    for (o, r) in ours.iter().zip(&reference) {
        worst = worst.max((o[0] - r[0] as f32 / 32768.0).abs());
    }
    assert!(worst <= 2.0 / 32768.0, "AAC decode differs from ffmpeg's by {worst}");
}

#[test]
fn read_zero_fills_past_the_end() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PcmCache::with_dir(dir.path());
    let pcm = ensure(&cache, &tone_wav(8)).unwrap().unwrap();
    let n = pcm.frames();
    let mut buf = vec![[9.0f32; 2]; 100];
    assert_eq!(pcm.read(n - 40, &mut buf), 40);
    assert!(buf[..40].iter().any(|s| s[0] != 0.0));
    assert!(buf[..40].iter().all(|s| s[0].abs() <= 1.0));
    assert!(buf[40..].iter().all(|s| *s == [0.0, 0.0]));
    assert_eq!(pcm.read(n, &mut buf), 0);
    assert!(buf.iter().all(|s| *s == [0.0, 0.0]));
    buf.fill([9.0; 2]);
    assert_eq!(pcm.read(u64::MAX - 5, &mut buf), 0);
    assert!(buf.iter().all(|s| *s == [0.0, 0.0]));
    assert_eq!(pcm.read(0, &mut []), 0);
}

#[test]
fn second_ensure_is_a_cache_hit_and_survives_a_new_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PcmCache::with_dir(dir.path());
    let wav = sine_wav("long60.wav", 330, 60.0, 48_000, 2);
    assert!(cache.get(&wav).is_none(), "nothing is loaded before ensure");

    let mut progress = Vec::new();
    let t = Instant::now();
    let first = cache.ensure(&wav, &mut |p| progress.push(p), &AtomicBool::new(false)).unwrap().unwrap();
    let decode_time = t.elapsed();
    assert!(progress.len() > 10, "progress is reported while decoding: {progress:?}");
    assert!(progress.windows(2).all(|w| w[0] <= w[1]), "progress never goes backwards");
    assert!(progress.iter().all(|p| (0.0..=1.0).contains(p)));
    assert_eq!(*progress.last().unwrap(), 1.0);

    let files = files_with_ext(dir.path(), "pcm");
    assert_eq!(files.len(), 1);
    let inode = std::fs::metadata(&files[0]).unwrap().ino();

    // Same handle: instant, same mapping.
    let t = Instant::now();
    let mut hits = Vec::new();
    let again = cache.ensure(&wav, &mut |p| hits.push(p), &AtomicBool::new(false)).unwrap().unwrap();
    let hit_time = t.elapsed();
    assert!(Arc::ptr_eq(&first, &again));
    assert_eq!(hits, [1.0]);
    assert!(Arc::ptr_eq(&first, &cache.get(&wav).unwrap()));
    assert!(Arc::ptr_eq(&first, &cache.clone().get(&wav).unwrap()), "clones share what is loaded");

    // A new cache on the same folder (a new app session): no decode, same file on disk.
    let fresh = PcmCache::with_dir(dir.path());
    assert!(fresh.get(&wav).is_none());
    let t = Instant::now();
    let reloaded = fresh.ensure(&wav, &mut |_| {}, &AtomicBool::new(false)).unwrap().unwrap();
    let reload_time = t.elapsed();
    assert_eq!(reloaded.frames(), first.frames());
    let files = files_with_ext(dir.path(), "pcm");
    assert_eq!(files.len(), 1);
    assert_eq!(std::fs::metadata(&files[0]).unwrap().ino(), inode, "the cache file was reused, not rewritten");
    let mut a = vec![[0.0f32; 2]; 4800];
    let mut b = a.clone();
    first.read(1_000_000, &mut a);
    reloaded.read(1_000_000, &mut b);
    assert_eq!(a, b);

    println!("MEASURE pcm decode of 60 s wav: {decode_time:?}; cache hit (memory): {hit_time:?}; cache hit (disk, new PcmCache): {reload_time:?}");
    println!("MEASURE pcm cache size: {} bytes for 60 s = {:.1} MB per hour", std::fs::metadata(&files[0]).unwrap().len(), std::fs::metadata(&files[0]).unwrap().len() as f64 * 60.0 / 1e6);
    assert!(hit_time < Duration::from_millis(20), "memory hit took {hit_time:?}");
    assert!(reload_time < Duration::from_millis(100), "disk hit took {reload_time:?}");
    assert_eq!(std::fs::metadata(&files[0]).unwrap().len(), 64 + 60 * 48_000 * 4);
}

#[test]
fn a_changed_file_is_decoded_again() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PcmCache::with_dir(dir.path().join("cache"));
    let src = dir.path().join("take.wav");

    std::fs::copy(sine_wav("a2.wav", 400, 2.0, 48_000, 2), &src).unwrap();
    let a = ensure(&cache, &src).unwrap().unwrap();
    assert_eq!(a.frames(), 2 * 48_000);

    // New contents with a different size.
    std::fs::copy(sine_wav("b3.wav", 800, 3.0, 48_000, 2), &src).unwrap();
    let b = ensure(&cache, &src).unwrap().unwrap();
    assert_eq!(b.frames(), 3 * 48_000);
    assert_eq!(dominant_freq(&read_all(&b)[..48_000], 0), 800.0);
    assert!(Arc::ptr_eq(&b, &cache.get(&src).unwrap()), "get returns the new audio");
    // The old mapping stays readable for whoever still holds it.
    assert_eq!(dominant_freq(&read_all(&a)[..48_000], 0), 400.0);
    assert_eq!(files_with_ext(cache.dir(), "pcm").len(), 1, "the stale cache file is removed");

    // Same size, different contents and a newer modification time.
    std::fs::copy(sine_wav("c3.wav", 1200, 3.0, 48_000, 2), &src).unwrap();
    std::fs::File::options().write(true).open(&src).unwrap().set_modified(SystemTime::now() + Duration::from_secs(5)).unwrap();
    let c = ensure(&cache, &src).unwrap().unwrap();
    assert_eq!(c.frames(), 3 * 48_000);
    assert_eq!(dominant_freq(&read_all(&c)[..48_000], 0), 1200.0);

    // Also for a cache that only knows the file from disk.
    let fresh = PcmCache::with_dir(cache.dir());
    std::fs::copy(sine_wav("a2.wav", 400, 2.0, 48_000, 2), &src).unwrap();
    let d = ensure(&fresh, &src).unwrap().unwrap();
    assert_eq!(d.frames(), 2 * 48_000);
}

#[test]
fn a_file_without_audio_gives_none() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PcmCache::with_dir(dir.path());
    let silent = numbered_video(6, false);
    assert!(ensure(&cache, &silent).unwrap().is_none());
    assert!(cache.get(&silent).is_none());
    assert!(cache.is_loaded(&silent));
    // Remembered, in memory and on disk.
    let mut calls = Vec::new();
    assert!(cache.ensure(&silent, &mut |p| calls.push(p), &AtomicBool::new(false)).unwrap().is_none());
    assert_eq!(calls, [1.0]);
    let fresh = PcmCache::with_dir(dir.path());
    let t = Instant::now();
    assert!(ensure(&fresh, &silent).unwrap().is_none());
    assert!(t.elapsed() < Duration::from_millis(100));
}

#[test]
fn missing_and_unreadable_files_are_clear_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PcmCache::with_dir(dir.path().join("cache"));

    let missing = dir.path().join("nope.wav");
    let e = ensure(&cache, &missing).unwrap_err().to_string();
    assert!(e.contains("not found") && e.contains("nope.wav"), "{e}");

    let e = ensure(&cache, dir.path()).unwrap_err().to_string();
    assert!(e.contains("not a media file"), "{e}");

    let garbage = dir.path().join("garbage.mp3");
    std::fs::write(&garbage, b"this is not audio at all, just some text\n".repeat(200)).unwrap();
    let e = ensure(&cache, &garbage).unwrap_err().to_string();
    assert!(e.contains("garbage.mp3"), "{e}");
    assert!(cache.get(&garbage).is_none());
    assert!(!cache.is_loaded(&garbage));

    assert!(files_with_ext(cache.dir(), "pcm").is_empty(), "failures leave no cache entries");
    assert!(files_with_ext(cache.dir(), "tmp").is_empty(), "failures leave no temporary files");
}

#[test]
fn cancel_leaves_no_cache_entry() {
    let dir = tempfile::tempdir().unwrap();
    let _cache = PcmCache::with_dir(dir.path());
    // A private copy, so the process check below only sees this test's decoder.
    let wav = dir.path().join("cancel-me.wav");
    std::fs::copy(sine_wav("long60.wav", 330, 60.0, 48_000, 2), &wav).unwrap();
    let cache = PcmCache::with_dir(dir.path().join("cache"));

    // Cancelled before it starts.
    let cancel = AtomicBool::new(true);
    let e = cache.ensure(&wav, &mut |_| {}, &cancel).unwrap_err();
    assert_eq!(e.downcast_ref::<Cancelled>(), Some(&Cancelled));

    // Cancelled in the middle of the decode.
    let cancel = AtomicBool::new(false);
    let mut seen = Vec::new();
    let e = cache
        .ensure(
            &wav,
            &mut |p| {
                seen.push(p);
                if p >= 0.2 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert_eq!(e.downcast_ref::<Cancelled>(), Some(&Cancelled), "{e}");
    assert!(seen.iter().all(|p| *p < 0.5), "decoding stopped promptly: {seen:?}");
    assert!(cache.get(&wav).is_none());
    assert!(!cache.is_loaded(&wav));
    let left: Vec<_> = std::fs::read_dir(cache.dir()).unwrap().flatten().map(|e| e.path()).collect();
    assert!(left.is_empty(), "cancel left files behind: {left:?}");
    assert_eq!(children_mentioning("cancel-me.wav"), 0, "the decoder process is gone");

    // The next attempt decodes the whole file.
    let pcm = ensure(&cache, &wav).unwrap().unwrap();
    assert_eq!(pcm.frames(), 60 * 48_000);
    let mut tail = vec![[0.0f32; 2]; 4800];
    assert_eq!(pcm.read(60 * 48_000 - 4800, &mut tail), 4800);
    assert!(peak(&tail, 0) > 0.05, "the end of the file is really there");
}

#[test]
fn a_damaged_cache_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PcmCache::with_dir(dir.path());
    let wav = tone_wav(8);
    let good = ensure(&cache, &wav).unwrap().unwrap();
    let file = good.cache_file().to_path_buf();
    drop(good);

    // Truncated behind our back (e.g. the disk filled up in an older version).
    for damage in [&b"MANGOPCM"[..], &[0u8; 5000][..], &[][..]] {
        std::fs::remove_file(&file).unwrap();
        std::fs::write(&file, damage).unwrap();
        let fresh = PcmCache::with_dir(dir.path());
        let pcm = ensure(&fresh, &wav).unwrap().unwrap();
        assert_eq!(pcm.frames(), 8 * 48_000);
        assert_eq!(dominant_freq(span(&read_all(&pcm), 5.1, 5.9), 0), step_freq(5));
    }
}

#[test]
fn concurrent_ensure_decodes_once() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PcmCache::with_dir(dir.path());
    let wav = sine_wav("long60.wav", 330, 60.0, 48_000, 2);
    let results: Vec<Arc<Pcm>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..4).map(|_| s.spawn(|| ensure(&cache, &wav).unwrap().unwrap())).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(results.iter().all(|p| Arc::ptr_eq(p, &results[0])), "every caller gets the same mapping");
    assert_eq!(files_with_ext(dir.path(), "pcm").len(), 1);
    assert!(files_with_ext(dir.path(), "tmp").is_empty());
}

#[test]
fn audio_that_starts_late_is_padded_to_source_time() {
    // The audio stream starts one second into the file; sample n must still be source time n / 48000.
    let late = generated("late_audio.mp4", |p| {
        ffmpeg(&[
            "-f", "lavfi", "-i", "color=black:s=64x64:r=30:d=4",
            "-itsoffset", "1.0", "-f", "lavfi", "-i", "sine=f=500:r=48000:d=3",
            "-map", "0:v", "-map", "1:a", "-c:v", "libx264", "-c:a", "aac", p.to_str().unwrap(),
        ]);
    });
    let dir = tempfile::tempdir().unwrap();
    let cache = PcmCache::with_dir(dir.path());
    let pcm = ensure(&cache, &late).unwrap().unwrap();
    let all = read_all(&pcm);
    assert!((pcm.duration() - 4.0).abs() < 0.05, "{}", pcm.duration());
    assert!(peak(span(&all, 0.0, 0.95), 0) < 0.01, "the first second is silence");
    assert_eq!(dominant_freq(span(&all, 1.1, 1.9), 0), 500.0);
    assert!(rms(span(&all, 1.1, 1.9), 0) > 0.05);
}

#[test]
fn prune_removes_least_recently_used_files_but_not_loaded_ones() {
    let dir = tempfile::tempdir().unwrap();
    let a = sine_wav("a2.wav", 400, 2.0, 48_000, 2);
    let b = sine_wav("b3.wav", 800, 3.0, 48_000, 2);
    {
        let earlier = PcmCache::with_dir(dir.path());
        ensure(&earlier, &a).unwrap().unwrap();
        ensure(&earlier, &b).unwrap().unwrap();
    }
    let cache = PcmCache::with_dir(dir.path());
    assert_eq!(cache.disk_usage(), 2 * 64 + 5 * 48_000 * 4);
    let kept = ensure(&cache, &b).unwrap().unwrap();
    assert_eq!(cache.prune(u64::MAX), 0);
    // Everything should go, but `b` is in use.
    assert_eq!(cache.prune(0), 64 + 2 * 48_000 * 4);
    assert_eq!(files_with_ext(dir.path(), "pcm"), [kept.cache_file().to_path_buf()]);
    assert_eq!(cache.disk_usage(), 64 + 3 * 48_000 * 4);
    // `a` simply decodes again when needed.
    assert_eq!(ensure(&cache, &a).unwrap().unwrap().frames(), 2 * 48_000);
}
