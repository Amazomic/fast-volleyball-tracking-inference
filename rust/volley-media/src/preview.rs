//! Render browser preview JPEGs with native FFmpeg orchestration.

use crate::{target_width, VideoInfo};
use std::{path::Path, time::Duration};
use tokio::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewMode {
    Source,
    Analysis,
}

impl PreviewMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "source" => Ok(Self::Source),
            "analysis" => Ok(Self::Analysis),
            _ => Err("Неверный режим предпросмотра".into()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Analysis => "analysis",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FrameRequest {
    pub frame_index: u64,
    pub frame_width: u32,
    pub mode: PreviewMode,
}

impl FrameRequest {
    pub fn new(
        info: &VideoInfo,
        mode: PreviewMode,
        timestamp: f64,
        requested_width: Option<u32>,
    ) -> Result<Self, String> {
        if !timestamp.is_finite() || timestamp < 0.0 || timestamp > info.duration_sec.max(0.0) {
            return Err("Время кадра вне длительности видео".into());
        }
        let frame_width = match mode {
            PreviewMode::Source => (info.width.min(1920) / 2 * 2).max(2),
            PreviewMode::Analysis => target_width(info, requested_width)?,
        };
        Ok(Self {
            frame_index: (timestamp * 30.0).round() as u64,
            frame_width,
            mode,
        })
    }

    pub fn filter(&self) -> String {
        match self.mode {
            PreviewMode::Source => format!("scale={}:-2,setsar=1", self.frame_width),
            PreviewMode::Analysis => format!("fps=30,scale={}:-2,setsar=1", self.frame_width),
        }
    }
}

pub async fn render_jpeg(source: &Path, frame: &FrameRequest) -> Result<Vec<u8>, String> {
    let time = format!("{:.4}", frame.frame_index as f64 / 30.0);
    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-ss",
        &time,
    ]);
    cmd.arg("-i").arg(source);
    cmd.args([
        "-vf",
        &frame.filter(),
        "-frames:v",
        "1",
        "-an",
        "-c:v",
        "mjpeg",
        "-q:v",
        "4",
        "-f",
        "image2pipe",
        "pipe:1",
    ]);
    cmd.kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_secs(60), cmd.output())
        .await
        .map_err(|_| "Таймаут получения кадра из FFmpeg".to_string())?
        .map_err(|error| format!("FFmpeg не запущен: {error}"))?;
    if !result.status.success()
        || !result.stdout.starts_with(&[0xff, 0xd8])
        || result.stdout.len() > 9_000_000
    {
        let detail = String::from_utf8_lossy(&result.stderr);
        let detail = detail
            .chars()
            .rev()
            .take(350)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        return Err(format!("Не удалось создать предпросмотр кадра: {detail}"));
    }
    Ok(result.stdout)
}

#[cfg(test)]
mod tests {
    use super::{FrameRequest, PreviewMode};
    use crate::VideoInfo;

    #[test]
    fn orientation_and_auto_resolution_are_stable() {
        let info = VideoInfo {
            fps: Some(30.0),
            width: 1080,
            height: 1920,
            coded_width: 1080,
            coded_height: 1920,
            rotation: 0,
            duration_sec: 40.0,
            auto_width: 1080,
            auto_height: 1920,
            codec: "h264".into(),
        };
        let source = FrameRequest::new(&info, PreviewMode::Source, 2.0, None).unwrap();
        let analysis = FrameRequest::new(&info, PreviewMode::Analysis, 2.0, None).unwrap();
        assert_eq!(source.frame_index, 60);
        assert_eq!(analysis.frame_width, 1080);
        assert!(analysis.filter().starts_with("fps=30"));
    }
}
