//! Rust-native video probing and conservative analysis-ready classification.
//! FFmpeg/FFprobe are external native codecs, not Python application code.
pub mod frame_windows;
pub mod montage;
pub mod preparation;
pub mod preview;

use serde::Serialize;
use serde_json::Value;
use std::{path::Path, process::Command};

const MAX_AUTO_WIDTH: u32 = 1280;

#[derive(Clone, Debug, Serialize)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub rotation: i32,
    pub coded_width: u32,
    pub coded_height: u32,
    pub duration_sec: f64,
    pub auto_width: u32,
    pub auto_height: u32,
    pub codec: String,
}

fn as_number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|v| v.parse::<f64>().ok()))
        .filter(|v| v.is_finite())
}

fn as_positive_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok().filter(|n| *n > 0))
}

fn video_stream(payload: &Value) -> Option<&Value> {
    payload["streams"]
        .as_array()?
        .iter()
        .find(|row| row["codec_type"].as_str() == Some("video"))
}

fn rotation(video: &Value) -> Option<f64> {
    if let Some(sides) = video["side_data_list"].as_array() {
        for item in sides {
            if let Some(value) = as_number(&item["rotation"]) {
                return Some(value);
            }
        }
    }
    as_number(&video["tags"]["rotate"]).or(Some(0.0))
}

fn output_height(width: u32, src_width: u32, src_height: u32) -> u32 {
    if src_width == 0 {
        return 2;
    }
    let scaled = (width as f64 * src_height as f64 / src_width as f64 / 2.0).round() as u32;
    (scaled * 2).max(2)
}

fn rounded_millis(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

pub fn inspect_probe(payload: &Value) -> Result<VideoInfo, String> {
    let video = video_stream(payload).ok_or("В файле нет видеодорожки")?;
    let coded_width = as_positive_u32(&video["width"]).ok_or("Нет ширины видеодорожки")?;
    let coded_height = as_positive_u32(&video["height"]).ok_or("Нет высоты видеодорожки")?;
    let rotate = rotation(video).ok_or("Некорректный поворот видео")?;
    if !rotate.is_finite() || rotate.abs() > i32::MAX as f64 {
        return Err("Недопустимый угол поворота".into());
    }
    let rotation = rotate as i32;
    let swapped = rotation.rem_euclid(180) != 0;
    let (width, height) = if swapped {
        (coded_height, coded_width)
    } else {
        (coded_width, coded_height)
    };
    let auto_width = (width.min(MAX_AUTO_WIDTH) / 2 * 2).max(2);
    let duration = as_number(&payload["format"]["duration"])
        .or_else(|| as_number(&video["duration"]))
        .unwrap_or(0.0);
    Ok(VideoInfo {
        width,
        height,
        rotation,
        coded_width,
        coded_height,
        duration_sec: rounded_millis(duration),
        auto_width,
        auto_height: output_height(auto_width, width, height),
        codec: video["codec_name"].as_str().unwrap_or("unknown").to_owned(),
    })
}

pub fn ffprobe(source: &Path) -> Result<Value, String> {
    let result = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(source)
        .output()
        .map_err(|err| format!("FFprobe недоступен: {err}"))?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(format!(
            "Не удалось прочитать параметры видео: {}",
            stderr
                .chars()
                .rev()
                .take(350)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        ));
    }
    if result.stdout.len() > 4_000_000 {
        return Err("Слишком большой ответ FFprobe".into());
    }
    serde_json::from_slice(&result.stdout)
        .map_err(|err| format!("Некорректный формат данных FFprobe: {err}"))
}

pub fn probe(source: &Path) -> Result<VideoInfo, String> {
    inspect_probe(&ffprobe(source)?)
}

fn exact_fps30(value: &Value) -> bool {
    let Some(rate) = value.as_str() else {
        return false;
    };
    let (top, bottom) = rate
        .split_once('/')
        .map_or((rate, "1"), |(top, bottom)| (top, bottom));
    let (Ok(top), Ok(bottom)) = (top.parse::<u64>(), bottom.parse::<u64>()) else {
        return false;
    };
    bottom > 0 && bottom.checked_mul(30) == Some(top)
}

pub fn analysis_ready_probe(payload: &Value, source: &Path, requested_width: u32) -> bool {
    if !source
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("mp4"))
    {
        return false;
    }
    if !payload["format"]["format_name"]
        .as_str()
        .is_some_and(|formats| formats.split(',').any(|part| part == "mp4"))
    {
        return false;
    }
    let Some(streams) = payload["streams"].as_array() else {
        return false;
    };
    let videos: Vec<&Value> = streams
        .iter()
        .filter(|row| row["codec_type"].as_str() == Some("video"))
        .collect();
    let audios: Vec<&Value> = streams
        .iter()
        .filter(|row| row["codec_type"].as_str() == Some("audio"))
        .collect();
    if videos.len() != 1 || audios.len() > 1 || videos.len() + audios.len() != streams.len() {
        return false;
    }
    let video = videos[0];
    if video["codec_name"].as_str() != Some("h264")
        || video["pix_fmt"].as_str() != Some("yuv420p")
        || as_positive_u32(&video["width"]) != Some(requested_width)
        || as_positive_u32(&video["height"]).is_none_or(|height| height % 2 != 0)
        || !matches!(video["sample_aspect_ratio"].as_str(), Some("1:1" | "1/1"))
        || !matches!(
            video["field_order"].as_str().unwrap_or("progressive"),
            "progressive" | "unknown"
        )
        || audios
            .iter()
            .any(|stream| stream["codec_name"].as_str() != Some("aac"))
        || !exact_fps30(&video["r_frame_rate"])
        || !exact_fps30(&video["avg_frame_rate"])
        || as_number(&video["start_time"]).unwrap_or(0.0).abs() > 0.02
    {
        return false;
    }
    rotation(video).is_some_and(|rot| rot.rem_euclid(360.0) == 0.0)
}

pub fn is_analysis_ready(source: &Path, requested_width: u32) -> Result<bool, String> {
    Ok(analysis_ready_probe(
        &ffprobe(source)?,
        source,
        requested_width,
    ))
}

/// Only widths that are valid for explicit user-selected analysis.
pub fn target_width(info: &VideoInfo, requested: Option<u32>) -> Result<u32, String> {
    let Some(width) = requested else {
        return Ok(info.auto_width);
    };
    if width % 2 != 0 || !(320..=1920).contains(&width) {
        return Err("Ручная ширина: чётное число 320–1920 пикселей".into());
    }
    Ok(width)
}

#[cfg(test)]
mod tests {
    use super::{analysis_ready_probe, inspect_probe};
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn probe_rotation_preserves_display_dimensions() {
        let payload = json!({
            "streams":[{"codec_type":"video","width":1080,"height":1920,
                        "side_data_list":[{"rotation":90}]}],
            "format":{"duration":"10.035"}
        });
        let info = inspect_probe(&payload).unwrap();
        assert_eq!((info.width, info.height), (1920, 1080));
        assert_eq!(info.auto_width, 1280);
        assert_eq!(info.duration_sec, 10.035);
    }

    #[test]
    fn ready_video_needs_real_h264_aac_30fps_cfr() {
        let payload = json!({
            "format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2"},
            "streams":[
                {"codec_type":"video","codec_name":"h264","pix_fmt":"yuv420p",
                 "width":1280,"height":720,"sample_aspect_ratio":"1:1",
                 "r_frame_rate":"30/1","avg_frame_rate":"30/1",
                 "start_time":"0.00","field_order":"progressive"},
                {"codec_type":"audio","codec_name":"aac"}]
        });
        assert!(analysis_ready_probe(
            &payload,
            Path::new("/tmp/clip.mp4"),
            1280
        ));
        assert!(!analysis_ready_probe(
            &payload,
            Path::new("/tmp/clip.mov"),
            1280
        ));
        assert!(!analysis_ready_probe(
            &payload,
            Path::new("/tmp/clip.mp4"),
            640
        ));
    }
}
