use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use std::{
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::broadcast;
#[derive(Clone)]
pub(super) struct EncodedFrame {
    pub data: Bytes,
    pub duration: Duration,
}
struct RawFrame {
    rgba: Vec<u8>,
    captured: Instant,
}
pub(super) struct Encoder {
    child: Child,
    frames: Option<SyncSender<RawFrame>>,
    writer: Option<thread::JoinHandle<()>>,
    reader: Option<thread::JoinHandle<()>>,
    failure: Arc<Mutex<Option<String>>>,
    pub samples: broadcast::Sender<EncodedFrame>,
}
fn options(kind: &str, fps: u32, device: &Path) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-filter_threads",
        "1",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    if kind == "vaapi" {
        args.extend([
            "-vaapi_device".into(),
            device.to_string_lossy().into_owned(),
        ]);
    }
    args.extend(
        [
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            "480x360",
            "-framerate",
        ]
        .map(String::from),
    );
    args.extend([fps.to_string(), "-i".into(), "pipe:0".into(), "-an".into()]);
    let profile = match kind {
        "nvenc" => vec![
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p1",
            "-tune",
            "ull",
            "-rc",
            "cbr",
            "-zerolatency",
            "1",
            "-pix_fmt",
            "yuv420p",
        ],
        "vaapi" => vec![
            "-vf",
            "format=nv12,hwupload",
            "-c:v",
            "h264_vaapi",
            "-profile:v",
            "constrained_baseline",
        ],
        _ => vec![
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
            "-x264-params",
            "repeat-headers=1:aud=1:scenecut=0",
        ],
    };
    args.extend(profile.into_iter().map(String::from));
    args.extend([
        "-threads".into(),
        "2".into(),
        "-b:v".into(),
        "4M".into(),
        "-bf".into(),
        "0".into(),
        "-g".into(),
        fps.to_string(),
        "-bsf:v".into(),
        "h264_metadata=aud=insert".into(),
        "-flush_packets".into(),
        "1".into(),
        "-f".into(),
        "h264".into(),
        "pipe:1".into(),
    ]);
    args
}
fn probe(kind: &str, fps: u32, device: &Path) -> bool {
    let mut args = options(kind, fps, device);
    args.truncate(args.len() - 1);
    args.extend(["-frames:v".into(), "1".into(), "pipe:1".into()]);
    let Ok(mut child) = Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
    else {
        return false;
    };
    // A single small raw frame cannot fill a normal pipe once FFmpeg is reading.
    // The writer is separate so the hardware probe has a bounded deadline.
    let mut stdin = child.stdin.take().unwrap();
    let writer = thread::spawn(move || {
        let _ = stdin.write_all(&vec![0; 480 * 360 * 4]);
    });
    let started = std::time::Instant::now();
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    eprintln!("Encoder probe {kind} failed: {status}");
                }
                break status.success();
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
            _ => {}
        }
        if started.elapsed() > Duration::from_secs(10) {
            eprintln!("Encoder probe {kind} timed out");
            let _ = child.kill();
            let _ = child.wait();
            break false;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let _ = writer.join();
    success
}
impl Encoder {
    pub fn start(requested: &str, fps: u32, device: &Path) -> Result<Self> {
        ensure!(
            matches!(requested, "auto" | "nvenc" | "vaapi" | "software"),
            "encoder must be auto, nvenc, vaapi, or software"
        );
        let candidates: Vec<&str> = if requested == "auto" {
            vec!["nvenc", "vaapi", "software"]
        } else {
            vec![requested]
        };
        let kind=candidates.into_iter().find(|kind|probe(kind,fps,device)).context("no usable FFmpeg H.264 encoder; install FFmpeg with libx264, NVENC, or VA-API support")?;
        eprintln!("Encoder: {kind} (B-frames disabled, one-second keyframe interval)");
        let mut child = Command::new("ffmpeg")
            .args(options(kind, fps, device))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start FFmpeg")?;
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (frames, rx) = mpsc::sync_channel::<RawFrame>(1);
        let (timing_tx, timing_rx) = mpsc::sync_channel::<Instant>(8);
        let (samples, _) = broadcast::channel(8);
        let failure = Arc::new(Mutex::new(None));
        let fail = failure.clone();
        let writer = thread::Builder::new()
            .name("video-encoder-input".into())
            .spawn(move || {
                while let Ok(frame) = rx.recv() {
                    if let Err(error) = stdin.write_all(&frame.rgba) {
                        *fail.lock().unwrap() = Some(format!("FFmpeg input failed: {error}"));
                        break;
                    }
                    if timing_tx.send(frame.captured).is_err() {
                        break;
                    }
                }
            })?;
        let tx = samples.clone();
        let fail = failure.clone();
        let reader = thread::Builder::new()
            .name("video-encoder-output".into())
            .spawn(move || {
                let mut parser = AccessUnits::default();
                let mut buf = [0u8; 16 * 1024];
                let mut previous = None;
                loop {
                    match stdout.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            for data in parser.feed(&buf[..n]) {
                                let Ok(captured) = timing_rx.recv() else {
                                    return;
                                };
                                let duration = previous
                                    .map(|p| captured.saturating_duration_since(p))
                                    .unwrap_or(Duration::from_secs_f64(1.0 / fps as f64));
                                previous = Some(captured);
                                let _ = tx.send(EncodedFrame {
                                    data: Bytes::from(data),
                                    duration,
                                });
                            }
                        }
                        Err(e) => {
                            *fail.lock().unwrap() = Some(format!("FFmpeg output failed: {e}"));
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            child,
            frames: Some(frames),
            writer: Some(writer),
            reader: Some(reader),
            failure,
            samples,
        })
    }
    pub fn submit(&mut self, frame: Vec<u8>) -> Result<()> {
        if let Some(error) = self.failure.lock().unwrap().as_ref() {
            bail!("{error}");
        }
        if let Some(status) = self.child.try_wait()? {
            bail!("FFmpeg exited with {status}");
        }
        match self.frames.as_ref().unwrap().try_send(RawFrame {
            rgba: frame,
            captured: Instant::now(),
        }) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => bail!("FFmpeg writer disconnected"),
        }
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        self.frames.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
// Annex-B access units are delimited by FFmpeg's inserted AUD NALs. Keep
// incomplete start codes across pipe reads; never treat individual NALs as frames.
#[derive(Default)]
struct AccessUnits {
    buffer: Vec<u8>,
    scan: usize,
    first_aud: bool,
}
impl AccessUnits {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = vec![];
        while self.scan + 4 <= self.buffer.len() {
            let i = self.scan;
            let start = if self.buffer[i..].starts_with(&[0, 0, 0, 1]) {
                Some(4)
            } else if self.buffer[i..].starts_with(&[0, 0, 1]) {
                Some(3)
            } else {
                None
            };
            if let Some(size) = start {
                if i + size >= self.buffer.len() {
                    break;
                }
                if self.buffer[i + size] & 0x1f == 9 {
                    if self.first_aud && i > 0 {
                        frames.push(self.buffer.drain(..i).collect());
                        self.scan = 0;
                    }
                    self.first_aud = true;
                }
                self.scan += size + 1;
            } else {
                self.scan += 1;
            }
        }
        // Bound damage from broken encoders producing no access-unit delimiters.
        if self.buffer.len() > 8 * 1024 * 1024 {
            self.buffer.clear();
            self.scan = 0;
            self.first_aud = false;
        }
        frames
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn annex_b_split_at_every_byte() {
        let au = [0, 0, 0, 1, 9, 0xf0, 0, 0, 1, 0x65, 1, 2, 3];
        let mut parser = AccessUnits::default();
        let mut output = vec![];
        for byte in au.iter().chain(au.iter()).chain(au.iter()) {
            output.extend(parser.feed(&[*byte]));
        }
        assert_eq!(output, vec![au.to_vec(), au.to_vec()]);
    }
}
