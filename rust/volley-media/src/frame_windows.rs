//! Deterministic bounded 9-frame raw-video windowing for native ONNX input.
//! FFmpeg can stream one decoded video through this reader without repeated
//! random seeks. It never owns a source path or creates an inference result.
use std::{
    collections::VecDeque,
    io::{self, Read},
};

pub const CLIP_FRAMES: usize = 9;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_FRAMES: u64 = 48 * 3600 * 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawWindow {
    pub first_frame: u64,
    pub data: Vec<u8>,
}

pub struct FrameWindows<R: Read> {
    source: R,
    frame_bytes: usize,
    stride: usize,
    queue: VecDeque<Vec<u8>>,
    emitted: bool,
    eof: bool,
    decoded: u64,
    next_start: u64,
}

impl<R: Read> FrameWindows<R> {
    pub fn new(source: R, frame_bytes: usize, stride: usize) -> io::Result<Self> {
        if !(1..=MAX_FRAME_BYTES).contains(&frame_bytes)
            || !(1..=CLIP_FRAMES).contains(&stride)
            || frame_bytes.checked_mul(CLIP_FRAMES).is_none()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid frame byte count or temporal window stride",
            ));
        }
        Ok(Self {
            source,
            frame_bytes,
            stride,
            queue: VecDeque::with_capacity(CLIP_FRAMES),
            emitted: false,
            eof: false,
            decoded: 0,
            next_start: 0,
        })
    }

    fn read_frame(&mut self) -> io::Result<Option<Vec<u8>>> {
        if self.decoded >= MAX_FRAMES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Decoded more than the supported 48 hours of 30-fps video",
            ));
        }
        let mut frame = vec![0u8; self.frame_bytes];
        let mut filled = 0;
        while filled < frame.len() {
            match self.source.read(&mut frame[filled..]) {
                Ok(0) if filled == 0 => return Ok(None),
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "FFmpeg ended inside a raw frame; refusing a partial model window",
                    ))
                }
                Ok(n) => filled += n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        self.decoded += 1;
        Ok(Some(frame))
    }

    /// Emits only full, ordered, 9-frame windows; never pads a trailing
    /// partial clip without an explicit inference sampling policy.
    pub fn next_window(&mut self) -> io::Result<Option<RawWindow>> {
        if self.eof {
            return Ok(None);
        }
        if self.emitted {
            for _ in 0..self.stride {
                self.queue.pop_front();
            }
            self.next_start += self.stride as u64;
        }
        while self.queue.len() < CLIP_FRAMES {
            match self.read_frame()? {
                Some(frame) => self.queue.push_back(frame),
                None => {
                    self.eof = true;
                    break;
                }
            }
        }
        if self.queue.len() < CLIP_FRAMES {
            return Ok(None);
        }
        self.emitted = true;
        let mut data = Vec::with_capacity(self.frame_bytes * CLIP_FRAMES);
        for frame in &self.queue {
            data.extend_from_slice(frame);
        }
        Ok(Some(RawWindow {
            first_frame: self.next_start,
            data,
        }))
    }

    pub fn decoded_frames(&self) -> u64 {
        self.decoded
    }
}

/// Pixel-normalization contract shared by clip diagnostics and future full
/// model streaming: grayscale [T,H,W] and RAVEL RGB [T,C,H,W], both [0,1].
pub fn normalized_model_window(
    raw: &[u8],
    width: usize,
    height: usize,
    channels: usize,
) -> Result<Vec<f32>, String> {
    if width == 0 || height == 0 || !matches!(channels, 1 | 3) {
        return Err("Invalid model pixel dimensions".into());
    }
    let each = width
        .checked_mul(height)
        .ok_or("Invalid image plane dimensions")?;
    let total = each
        .checked_mul(channels)
        .and_then(|n| n.checked_mul(CLIP_FRAMES))
        .filter(|n| *n <= MAX_FRAME_BYTES * CLIP_FRAMES)
        .ok_or("Model tensor exceeds input limit")?;
    if raw.len() != total {
        return Err(format!(
            "Expected {total} bytes for exactly 9 frames; got {}",
            raw.len()
        ));
    }
    if channels == 1 {
        return Ok(raw.iter().map(|&byte| byte as f32 / 255.0).collect());
    }
    let mut tensor = vec![0.0_f32; total];
    for frame in 0..CLIP_FRAMES {
        for pos in 0..each {
            for channel in 0..channels {
                let src = (frame * each + pos) * channels + channel;
                let dst = (frame * channels + channel) * each + pos;
                tensor[dst] = raw[src] as f32 / 255.0;
            }
        }
    }
    Ok(tensor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn sliding_windows_preserve_full_nine_frame_context() {
        let bytes: Vec<_> = (0..15).collect();
        let mut windows = FrameWindows::new(Cursor::new(bytes), 1, 3).unwrap();
        let a = windows.next_window().unwrap().unwrap();
        let b = windows.next_window().unwrap().unwrap();
        let c = windows.next_window().unwrap().unwrap();
        assert_eq!((a.first_frame, b.first_frame, c.first_frame), (0, 3, 6));
        assert_eq!(a.data, (0..9).collect::<Vec<u8>>());
        assert_eq!(b.data, (3..12).collect::<Vec<u8>>());
        assert_eq!(c.data, (6..15).collect::<Vec<u8>>());
        assert!(windows.next_window().unwrap().is_none());
        assert_eq!(windows.decoded_frames(), 15);
    }

    #[test]
    fn nonoverlapping_and_short_clips_are_explicit() {
        let mut blocks = FrameWindows::new(Cursor::new(vec![7u8; 18]), 1, 9).unwrap();
        assert_eq!(blocks.next_window().unwrap().unwrap().first_frame, 0);
        assert_eq!(blocks.next_window().unwrap().unwrap().first_frame, 9);
        assert!(blocks.next_window().unwrap().is_none());
        let mut short = FrameWindows::new(Cursor::new(vec![0u8; 8]), 1, 1).unwrap();
        assert!(short.next_window().unwrap().is_none());
        assert!(FrameWindows::new(Cursor::new(vec![0u8; 8]), 0, 1).is_err());
        assert!(FrameWindows::new(Cursor::new(vec![0u8; 8]), 1, 10).is_err());
    }

    #[test]
    fn truncated_last_frame_is_not_silently_discarded() {
        let bytes = vec![0u8; 3 * 9 + 1];
        let mut stream = FrameWindows::new(Cursor::new(bytes), 3, 1).unwrap();
        assert!(stream.next_window().unwrap().is_some());
        assert_eq!(
            stream.next_window().unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn rgb_is_temporal_chw_with_independent_pixel_channels() {
        let raw = vec![0, 127, 255, 10, 20, 30]
            .into_iter()
            .cycle()
            .take(9 * 2 * 3)
            .collect::<Vec<u8>>();
        let tensor = normalized_model_window(&raw, 2, 1, 3).unwrap();
        assert!((tensor[0] - 0.0).abs() < 1e-6);
        assert!((tensor[1] - 10.0 / 255.0).abs() < 1e-6);
        assert!((tensor[2] - 127.0 / 255.0).abs() < 1e-6);
        assert!((tensor[3] - 20.0 / 255.0).abs() < 1e-6);
        assert!((tensor[4] - 1.0).abs() < 1e-6);
        assert!((tensor[5] - 30.0 / 255.0).abs() < 1e-6);
        assert_eq!(
            normalized_model_window(&[255; 9], 1, 1, 1).unwrap(),
            vec![1.0; 9]
        );
        assert!(normalized_model_window(&[0; 8], 1, 1, 1).is_err());
    }
}
