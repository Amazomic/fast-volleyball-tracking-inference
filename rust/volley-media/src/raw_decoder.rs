//! One bounded FFmpeg raw-video stream feeding overlapped 9-frame Rust
//! windows. Never seeks for every window or writes an analysis cache.
//! Input paths must be validated by the calling project/preparation layer.
use crate::frame_windows::{FrameWindows, RawWindow, CLIP_FRAMES, MAX_FRAMES};
use serde::Serialize;
use std::{
    collections::VecDeque,
    io::{self, Read},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub enum ModelPixels {
    BallGray,
    PlayersRgb,
}
impl ModelPixels {
    pub fn dimensions(self) -> (usize, usize, usize, &'static str) {
        match self {
            Self::BallGray => (512, 288, 1, "gray"),
            Self::PlayersRgb => (640, 360, 3, "rgb24"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DecodeSummary {
    pub first_frame: u64,
    pub windows: usize,
    pub decoded_frames: u64,
    pub stride: usize,
}

struct ManagedChild(Child);
impl Drop for ManagedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

const TAIL_BYTES: usize = 4096;

/// A hung FFmpeg decoder must not pin the inference worker on a blocking pipe
/// read forever. A scoped thread drains the pipe into bounded chunks while
/// the consumer waits with a timeout: cancellation interrupts within 200 ms
/// and a fully silent decoder fails after the idle limit. Dropping the reader
/// disconnects the channel, which also stops the drain thread.
struct GuardedPipe<'a> {
    receiver: Receiver<io::Result<Vec<u8>>>,
    cancelled: &'a AtomicBool,
    idle_limit: Duration,
    pending: Vec<u8>,
    offset: usize,
}
impl Read for GuardedPipe<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut last_activity = Instant::now();
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            if self.offset < self.pending.len() {
                let count = (self.pending.len() - self.offset).min(buf.len());
                buf[..count].copy_from_slice(&self.pending[self.offset..self.offset + count]);
                self.offset += count;
                if self.offset == self.pending.len() {
                    self.pending.clear();
                    self.offset = 0;
                }
                return Ok(count);
            }
            match self.receiver.recv_timeout(Duration::from_millis(200)) {
                Ok(Ok(chunk)) => {
                    if chunk.is_empty() {
                        return Ok(0);
                    }
                    self.pending = chunk;
                    self.offset = 0;
                    last_activity = Instant::now();
                }
                Ok(Err(error)) => return Err(error),
                Err(RecvTimeoutError::Timeout) => {
                    if last_activity.elapsed() >= self.idle_limit {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "FFmpeg decoder produced no data within the idle limit",
                        ));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
    }
}
const DECODER_IDLE_LIMIT: Duration = Duration::from_secs(120);

fn stderr_tail(mut error_pipe: impl Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut tail = VecDeque::with_capacity(TAIL_BYTES);
        let mut chunk = [0_u8; 1024];
        while let Ok(count) = error_pipe.read(&mut chunk) {
            if count == 0 {
                break;
            }
            for &byte in &chunk[..count] {
                if tail.len() == TAIL_BYTES {
                    tail.pop_front();
                }
                tail.push_back(byte);
            }
        }
        String::from_utf8_lossy(&tail.into_iter().collect::<Vec<_>>()).into_owned()
    })
}

fn checked_total_frames(first: u64, windows: usize, stride: usize) -> Result<usize, String> {
    if !(1..=120).contains(&windows) || !(1..=CLIP_FRAMES).contains(&stride) {
        return Err("Select 1..120 windows with a temporal stride of 1..9".into());
    }
    let total = CLIP_FRAMES
        .checked_add(
            (windows - 1)
                .checked_mul(stride)
                .ok_or("Window count overflow")?,
        )
        .ok_or("Window count overflow")?;
    let last = first
        .checked_add(total as u64)
        .ok_or("Frame index overflow")?;
    if last > MAX_FRAMES {
        return Err("Requested frames exceed the maximum 48-hour 30fps source".into());
    }
    Ok(total)
}

/// The callback is invoked synchronously per 9-frame window, allowing one
/// persistent ONNX session to consume a stream with bounded memory. A failed
/// callback or cancellation kills and reaps FFmpeg; no partial result is valid.
pub fn decode_windows(
    source: &Path,
    pixels: ModelPixels,
    first_frame: u64,
    windows: usize,
    stride: usize,
    cancelled: &AtomicBool,
    mut consume: impl FnMut(RawWindow) -> Result<(), String>,
) -> Result<DecodeSummary, String> {
    let total_frames = checked_total_frames(first_frame, windows, stride)?;
    if cancelled.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }
    if !source.is_file() {
        return Err("Prepared source video is not a file".into());
    }
    let (w, h, channels, format) = pixels.dimensions();
    let bytes_per_frame = w * h * channels;
    let seek = first_frame as f64 / 30.0;
    let mut ff = Command::new("ffmpeg");
    ff.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-threads",
        "4",
    ])
    .arg("-i")
    .arg(source)
    .args(["-ss", &format!("{seek:.6}")])
    .args(["-map", "0:v:0", "-an", "-sn", "-dn"])
    .args([
        "-vf",
        &format!("scale={w}:{h}:flags=bilinear,format={format}"),
    ])
    .args(["-frames:v", &total_frames.to_string()])
    .args(["-pix_fmt", format, "-f", "rawvideo", "pipe:1"]);
    ff.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ManagedChild(
        ff.spawn()
            .map_err(|e| format!("FFmpeg launch failed: {e}"))?,
    );
    let mut stdout = child.0.stdout.take().ok_or("FFmpeg stdout missing")?;
    let stderr = child.0.stderr.take().ok_or("FFmpeg stderr missing")?;
    let error_reader = stderr_tail(stderr);
    let result = thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<io::Result<Vec<u8>>>(8);
        scope.spawn(move || {
            let mut chunk = vec![0_u8; 64 * 1024];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => {
                        let _ = sender.send(Ok(Vec::new()));
                        break;
                    }
                    Ok(count) => {
                        if sender.send(Ok(chunk[..count].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        });
        let guarded = GuardedPipe {
            receiver,
            cancelled,
            idle_limit: DECODER_IDLE_LIMIT,
            pending: Vec::new(),
            offset: 0,
        };
        let outcome = (|| -> Result<DecodeSummary, String> {
            let mut reader = FrameWindows::new(guarded, bytes_per_frame, stride)
                .map_err(|e| format!("Raw frame reader: {e}"))?;
            let mut produced = 0;
            while produced < windows {
                if cancelled.load(Ordering::Acquire) {
                    return Err("cancelled".into());
                }
                let Some(window) = reader
                    .next_window()
                    .map_err(|e| format!("Decode raw frame: {e}"))?
                else {
                    return Err(
                        "Decoder ended before the requested number of complete windows".into(),
                    );
                };
                if window.first_frame != produced as u64 * stride as u64 {
                    return Err("Decoder frame sequence is inconsistent".into());
                }
                consume(RawWindow {
                    first_frame: first_frame + window.first_frame,
                    data: window.data,
                })?;
                produced += 1;
            }
            // FFmpeg has a finite frame limit; its normal EOF and exit code are
            // verified before any result is published.
            let status = child
                .0
                .wait()
                .map_err(|e| format!("Wait for FFmpeg: {e}"))?;
            if !status.success() {
                return Err(format!("FFmpeg returned {status}"));
            }
            Ok(DecodeSummary {
                first_frame,
                windows: produced,
                decoded_frames: reader.decoded_frames(),
                stride,
            })
        })();
        if outcome.is_err() {
            // Kill before the scope joins the drain thread: a hung child would
            // otherwise keep the pipe open and block the join forever.
            let _ = child.0.kill();
            let _ = child.0.wait();
        }
        outcome
    });
    let errors = error_reader
        .join()
        .unwrap_or_else(|_| "FFmpeg stderr thread panicked".into());
    result.map_err(|e| {
        if errors.trim().is_empty() {
            e
        } else {
            format!(
                "{e}: {}",
                errors.trim().chars().take(500).collect::<String>()
            )
        }
    })
}

/// Stream an entire prepared video without loading it into RAM or asking
/// FFmpeg to seek for each window. Only whole nine-frame clips are emitted;
/// any final 1..8 frames that cannot form a full window are left unused.
/// The result is valid only after the FFmpeg process exits successfully.
pub fn decode_full_video(
    source: &Path,
    pixels: ModelPixels,
    stride: usize,
    cancelled: &AtomicBool,
    mut consume: impl FnMut(RawWindow) -> Result<(), String>,
) -> Result<DecodeSummary, String> {
    if !(1..=CLIP_FRAMES).contains(&stride) {
        return Err("Full-video decode stride must be between 1 and 9".into());
    }
    if cancelled.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }
    if !source.is_file() {
        return Err("Prepared source video is not a file".into());
    }
    let (w, h, channels, format) = pixels.dimensions();
    let mut ff = Command::new("ffmpeg");
    ff.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-threads",
        "4",
    ])
    .arg("-i")
    .arg(source)
    .args(["-map", "0:v:0", "-an", "-sn", "-dn"])
    .args([
        "-vf",
        &format!("scale={w}:{h}:flags=bilinear,format={format}"),
    ])
    .args(["-pix_fmt", format, "-f", "rawvideo", "pipe:1"]);
    ff.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ManagedChild(
        ff.spawn()
            .map_err(|e| format!("FFmpeg full-video launch failed: {e}"))?,
    );
    let mut stdout = child.0.stdout.take().ok_or("FFmpeg stdout missing")?;
    let stderr = child.0.stderr.take().ok_or("FFmpeg stderr missing")?;
    let errors_reader = stderr_tail(stderr);
    let result = thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<io::Result<Vec<u8>>>(8);
        scope.spawn(move || {
            let mut chunk = vec![0_u8; 64 * 1024];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => {
                        let _ = sender.send(Ok(Vec::new()));
                        break;
                    }
                    Ok(count) => {
                        if sender.send(Ok(chunk[..count].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        });
        let guarded = GuardedPipe {
            receiver,
            cancelled,
            idle_limit: DECODER_IDLE_LIMIT,
            pending: Vec::new(),
            offset: 0,
        };
        let outcome = (|| -> Result<DecodeSummary, String> {
            let mut reader = FrameWindows::new(guarded, w * h * channels, stride)
                .map_err(|e| format!("Video frame reader: {e}"))?;
            let mut produced = 0usize;
            loop {
                if cancelled.load(Ordering::Acquire) {
                    return Err("cancelled".into());
                }
                let Some(window) = reader
                    .next_window()
                    .map_err(|e| format!("Video raw-frame decoding: {e}"))?
                else {
                    break;
                };
                if window.first_frame != (produced as u64) * stride as u64 {
                    return Err("Full-video frame sequence is not monotonic".into());
                }
                consume(window)?;
                produced += 1;
            }
            if cancelled.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            if produced == 0 {
                return Err("Video does not contain a complete nine-frame model window".into());
            }
            let status = child.0.wait().map_err(|e| format!("Wait FFmpeg: {e}"))?;
            if !status.success() {
                return Err(format!("FFmpeg exited with {status}"));
            }
            Ok(DecodeSummary {
                first_frame: 0,
                windows: produced,
                decoded_frames: reader.decoded_frames(),
                stride,
            })
        })();
        if outcome.is_err() {
            // Kill before the scope joins the drain thread: a hung child would
            // otherwise keep the pipe open and block the join forever.
            let _ = child.0.kill();
            let _ = child.0.wait();
        }
        outcome
    });
    let reported = errors_reader
        .join()
        .unwrap_or_else(|_| "FFmpeg stderr reader panicked".into());
    result.map_err(|e| {
        if reported.trim().is_empty() {
            e
        } else {
            format!(
                "{e}: {}",
                reported.trim().chars().take(500).collect::<String>()
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};
    #[test]
    fn full_video_stream_covers_complete_windows_only_and_can_cancel() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("volley-full-raw-windows-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("synthetic.mp4");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=30:duration=0.6",
                "-an",
                "-c:v",
                "mpeg4",
                "-y",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        let cancelled = AtomicBool::new(false);
        let mut frame_ids = Vec::new();
        let summary = decode_full_video(&source, ModelPixels::BallGray, 4, &cancelled, |win| {
            assert_eq!(win.data.len(), 9 * 512 * 288);
            frame_ids.push(win.first_frame);
            Ok(())
        })
        .unwrap();
        assert_eq!(frame_ids, vec![0, 4, 8]);
        assert_eq!(summary.windows, 3);
        assert_eq!(summary.decoded_frames, 18);
        // A separate invocation fails instead of inventing a complete
        // full-video result from an interrupted callback.
        let aborted = AtomicBool::new(false);
        let result = decode_full_video(&source, ModelPixels::BallGray, 9, &aborted, |_| {
            aborted.store(true, Ordering::Release);
            Ok(())
        });
        assert!(result.unwrap_err().contains("cancelled"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bounded_window_schedule_checks_range() {
        assert_eq!(checked_total_frames(0, 3, 9).unwrap(), 27);
        assert_eq!(checked_total_frames(100, 3, 3).unwrap(), 15);
        assert!(checked_total_frames(0, 121, 9).is_err());
        assert!(checked_total_frames(MAX_FRAMES, 1, 1).is_err());
        assert!(checked_total_frames(0, 2, 0).is_err());
    }
    #[test]
    fn guarded_pipe_times_out_when_the_decoder_is_silent() {
        let (_sender, receiver) = std::sync::mpsc::sync_channel(8);
        let cancelled = AtomicBool::new(false);
        let mut pipe = GuardedPipe {
            receiver,
            cancelled: &cancelled,
            idle_limit: Duration::from_millis(50),
            pending: Vec::new(),
            offset: 0,
        };
        let mut buffer = [0_u8; 16];
        let error = pipe.read(&mut buffer).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
    #[test]
    fn guarded_pipe_interrupts_a_blocked_read_on_cancel() {
        let (_sender, receiver) = std::sync::mpsc::sync_channel(8);
        let cancelled = AtomicBool::new(true);
        let mut pipe = GuardedPipe {
            receiver,
            cancelled: &cancelled,
            idle_limit: Duration::from_secs(120),
            pending: Vec::new(),
            offset: 0,
        };
        let mut buffer = [0_u8; 16];
        let error = pipe.read(&mut buffer).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    }
    #[test]
    fn guarded_pipe_preserves_bytes_across_chunk_and_buffer_boundaries() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(8);
        let cancelled = AtomicBool::new(false);
        let mut pipe = GuardedPipe {
            receiver,
            cancelled: &cancelled,
            idle_limit: Duration::from_secs(120),
            pending: Vec::new(),
            offset: 0,
        };
        sender.send(Ok(vec![1, 2, 3, 4, 5])).unwrap();
        sender.send(Ok(Vec::new())).unwrap();
        let mut out = Vec::new();
        let mut buffer = [0_u8; 2];
        loop {
            let count = pipe.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..count]);
        }
        assert_eq!(out, vec![1, 2, 3, 4, 5]);
    }
    #[test]
    fn ffmpeg_streams_one_process_across_ordered_windows() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("volley-ffmpeg-windows-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("synthetic.mp4");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=30:duration=0.6",
                "-an",
                "-c:v",
                "mpeg4",
                "-y",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        let cancelled = AtomicBool::new(false);
        let mut starts = Vec::new();
        let summary = decode_windows(
            &source,
            ModelPixels::BallGray,
            0,
            3,
            3,
            &cancelled,
            |window| {
                starts.push(window.first_frame);
                assert_eq!(window.data.len(), 512 * 288 * 9);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(starts, vec![0, 3, 6]);
        assert_eq!(summary.decoded_frames, 15);
        let cancelled = AtomicBool::new(true);
        assert!(
            decode_windows(&source, ModelPixels::BallGray, 0, 1, 1, &cancelled, |_| Ok(
                ()
            ))
            .is_err()
        );
        let _ = fs::remove_dir_all(root);
    }
}
