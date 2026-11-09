//! Native-source montage encoding plan. FFmpeg remains the codec executable;
//! no Python runner is needed for clip extraction, concatenation or music.

use crate::{ffprobe, probe};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const ANALYSIS_FPS: f64 = 30.0;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedClip {
    pub video: String,
    pub start_frame: u64,
    pub end_frame: u64,
    #[serde(default)]
    pub analysis_offset_sec: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct OriginalClip {
    pub source: PathBuf,
    pub start_frame: u64,
    pub end_frame: u64,
    pub analysis_offset_sec: f64,
    pub native_fps: f64,
    pub native_rate: String,
    pub native_width: u32,
    pub native_height: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClipReceipt {
    pub source: String,
    pub start_sec: f64,
    pub frames: u64,
    pub duration_sec: f64,
    pub native_fps: f64,
    pub native_rate: String,
    pub original_resolution: bool,
    pub native_width: u32,
    pub native_height: u32,
}

fn parse_fraction(raw: &str) -> Option<(u64, u64)> {
    let (numerator, denominator) = raw.split_once('/').unwrap_or((raw, "1"));
    let numerator = numerator.parse::<u64>().ok()?;
    let denominator = denominator.parse::<u64>().ok()?;
    (numerator > 0 && denominator > 0).then_some((numerator, denominator))
}

fn fps_for(source: &Path) -> Result<(f64, String), String> {
    let json = ffprobe(source)?;
    let stream = json["streams"]
        .as_array()
        .and_then(|streams| {
            streams
                .iter()
                .find(|v| v["codec_type"].as_str() == Some("video"))
        })
        .ok_or("Исходник не содержит видеопотока")?;
    let native = stream["r_frame_rate"]
        .as_str()
        .and_then(parse_fraction)
        .and_then(|(n, d)| {
            let fps = n as f64 / d as f64;
            (fps >= 1.0 && fps <= 240.0).then_some((fps, format!("{n}/{d}")))
        });
    let avg = stream["avg_frame_rate"]
        .as_str()
        .and_then(parse_fraction)
        .and_then(|(n, d)| {
            let fps = n as f64 / d as f64;
            (fps >= 1.0 && fps <= 240.0).then_some((fps, format!("{n}/{d}")))
        });
    match (avg, native) {
        (Some(average), Some(nominal)) if (average.0 - nominal.0).abs() < 0.2 => Ok(nominal),
        (Some(average), _) => Ok(average),
        (None, Some(nominal)) => Ok(nominal),
        _ => Err("Не удалось определить кадровую частоту оригинала".into()),
    }
}

impl OriginalClip {
    /// Original time = analysis offset + frame index / 30. No resized analysis
    /// video is ever used to produce montage pixels.
    pub fn from_analysis(
        source: &Path,
        start_frame: u64,
        end_frame: u64,
        analysis_offset_sec: f64,
    ) -> Result<Self, String> {
        if end_frame < start_frame || end_frame - start_frame > 30 * 600 {
            return Err("Некорректная длина выбранного эпизода".into());
        }
        if !analysis_offset_sec.is_finite() || analysis_offset_sec < 0.0 {
            return Err("Некорректное смещение анализа".into());
        }
        let source = source
            .canonicalize()
            .map_err(|error| format!("Исходный файл недоступен: {error}"))?;
        let (native_fps, native_rate) = fps_for(&source)?;
        let info = probe(&source)?;
        let start_sec = analysis_offset_sec + start_frame as f64 / ANALYSIS_FPS;
        let duration_sec = (end_frame - start_frame + 1) as f64 / ANALYSIS_FPS;
        if start_sec >= info.duration_sec + 0.001
            || start_sec + duration_sec > info.duration_sec + 1.0 / ANALYSIS_FPS
        {
            return Err("Выбранный эпизод выходит за границы оригинала".into());
        }
        Ok(Self {
            source,
            start_frame,
            end_frame,
            analysis_offset_sec,
            native_fps,
            native_rate,
            native_width: info.width,
            native_height: info.height,
        })
    }

    pub fn duration_sec(&self) -> f64 {
        (self.end_frame - self.start_frame + 1) as f64 / ANALYSIS_FPS
    }

    pub fn receipt(&self) -> ClipReceipt {
        let duration = self.duration_sec();
        ClipReceipt {
            source: self.source.to_string_lossy().to_string(),
            start_sec: self.analysis_offset_sec + self.start_frame as f64 / ANALYSIS_FPS,
            frames: ((duration * self.native_fps).round() as u64).max(1),
            duration_sec: duration,
            native_fps: self.native_fps,
            native_rate: self.native_rate.clone(),
            original_resolution: true,
            native_width: self.native_width,
            native_height: self.native_height,
        }
    }

    pub fn extract_args(&self, output: &Path, hardware: bool) -> Vec<String> {
        let receipt = self.receipt();
        let mut args = vec![
            "-nostdin".into(),
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-y".into(),
            "-ss".into(),
            format!("{:.6}", receipt.start_sec),
            "-i".into(),
            self.source.to_string_lossy().to_string(),
            "-map".into(),
            "0:v:0".into(),
            "-an".into(),
            "-vf".into(),
            format!("fps={},setsar=1", self.native_rate),
            "-frames:v".into(),
            receipt.frames.to_string(),
        ];
        args.extend(encode_flags(hardware));
        args.extend([
            "-movflags".into(),
            "+faststart".into(),
            output.to_string_lossy().into_owned(),
        ]);
        args
    }
}

pub fn encode_flags(hardware: bool) -> Vec<String> {
    if hardware {
        [
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p5",
            "-rc",
            "vbr",
            "-cq",
            "17",
            "-b:v",
            "0",
            "-pix_fmt",
            "yuv420p",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
    } else {
        [
            "-c:v", "libx264", "-preset", "fast", "-crf", "15", "-threads", "8", "-pix_fmt",
            "yuv420p",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
    }
}

pub fn native_clip_format(path: &Path) -> Result<(u32, u32, String, f64), String> {
    let payload = ffprobe(path)?;
    let video = payload["streams"]
        .as_array()
        .and_then(|streams| {
            streams
                .iter()
                .find(|v| v["codec_type"].as_str() == Some("video"))
        })
        .ok_or("В монтажном фрагменте нет видеопотока")?;
    let width = video["width"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or("Нет ширины видео")?;
    let height = video["height"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or("Нет высоты видео")?;
    let codec = video["codec_name"].as_str().unwrap_or("").to_owned();
    let fps = video["r_frame_rate"]
        .as_str()
        .and_then(parse_fraction)
        .map(|(n, d)| n as f64 / d as f64)
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(30.0);
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err("Неподдерживаемый размер FFmpeg-фрагмента".into());
    }
    Ok((width, height, codec, fps))
}

/// The same profile is used by both native and legacy CPU rally selectors.
/// Preserve every original's width AND height independently. A wide and a
/// tall source must not downscale either source just because the other source
/// has a larger pixel area. Padding is allowed; resampling resolution is not.
pub fn output_profile(formats: &[(u32, u32, String, f64)]) -> Result<(u32, u32, f64), String> {
    if formats.is_empty() || formats.len() > 80 {
        return Err("Нет фрагментов для определения исходного разрешения".into());
    }
    let mut width = 0u32;
    let mut height = 0u32;
    let mut fps = 0.0f64;
    for (w, h, _, rate) in formats {
        if *w == 0
            || *h == 0
            || *w > 8192
            || *h > 8192
            || !rate.is_finite()
            || !(1.0..=240.0).contains(rate)
        {
            return Err("Некорректное исходное разрешение или FPS".into());
        }
        width = width.max(*w);
        height = height.max(*h);
        fps = fps.max(*rate);
    }
    let width = width + width % 2;
    let height = height + height % 2;
    if u64::from(width) * u64::from(height) > 32_000_000 {
        return Err("Комбинация исходных размеров превышает безопасный лимит: уменьшать качество автоматически нельзя".into());
    }
    Ok((width, height, fps))
}

/// Refuse to claim an original-resolution extract when FFmpeg produced a
/// scaled clip, wrong codec or incorrect source frame rate.
pub fn verify_original_segment(
    original: &OriginalClip,
    format: &(u32, u32, String, f64),
) -> Result<(), String> {
    if format.0 != original.native_width || format.1 != original.native_height {
        return Err(format!(
            "Рендер уменьшил/изменил разрешение оригинала {}x{} -> {}x{}",
            original.native_width, original.native_height, format.0, format.1
        ));
    }
    if format.2 != "h264" || (format.3 - original.native_fps).abs() > 0.1 {
        return Err("FFmpeg изменил FPS или кодек оригинального фрагмента".into());
    }
    Ok(())
}
pub fn can_stream_copy(formats: &[(u32, u32, String, f64)]) -> bool {
    let Some(first) = formats.first() else {
        return false;
    };
    first.2 == "h264"
        && formats.iter().all(|item| {
            item.0 == first.0
                && item.1 == first.1
                && item.2 == first.2
                && (item.3 - first.3).abs() < 0.00001
        })
}

/// FFmpeg concat demuxer list. Each path is canonicalized and quoted so
/// apostrophes and whitespace never escape the manifest syntax.
pub fn concat_playlist(paths: &[PathBuf]) -> Result<String, String> {
    if paths.is_empty() {
        return Err("Нет фрагментов монтажа".into());
    }
    let mut rows = String::new();
    for path in paths {
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("Cannot locate clip for concat: {error}"))?;
        let text = canonical.to_string_lossy();
        rows.push_str("file '");
        rows.push_str(&text.replace('\\', "\\\\").replace('\'', "'\\''"));
        rows.push_str("'\n");
    }
    Ok(rows)
}

/// Native resolution retained. When original clips differ in dimensions,
/// pad to the largest native frame; never downsample to an arbitrary 720p.
pub fn compose_args(
    clips: &[PathBuf],
    formats: &[(u32, u32, String, f64)],
    output: &Path,
    playlist: Option<&Path>,
    hardware: bool,
) -> Result<Vec<String>, String> {
    let (max_width, max_height, target_fps) = output_profile(formats)?;
    if clips.is_empty() || clips.len() != formats.len() {
        return Err("Некорректные исходные фрагменты монтажа".into());
    }
    let mut args: Vec<String> = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
    ];
    if can_stream_copy(formats) && playlist.is_some() {
        args.extend([
            "-f".into(),
            "concat".into(),
            "-safe".into(),
            "0".into(),
            "-i".into(),
            playlist.unwrap().to_string_lossy().into_owned(),
            "-map".into(),
            "0:v:0".into(),
            "-c:v".into(),
            "copy".into(),
        ]);
    } else {
        for clip in clips {
            args.extend(["-i".into(), clip.to_string_lossy().into_owned()]);
        }
        let mut filters = Vec::new();
        let mut refs = String::new();
        for (index, _) in clips.iter().enumerate() {
            filters.push(format!(
                "[{index}:v:0]setpts=PTS-STARTPTS,fps={target_fps:.8},\
                 pad={max_width}:{max_height}:(ow-iw)/2:(oh-ih)/2,\
                 setsar=1,format=yuv420p[v{index}]"
            ));
            refs.push_str(&format!("[v{index}]"));
        }
        let mapped = if clips.len() == 1 {
            "[v0]".to_owned()
        } else {
            filters.push(format!("{refs}concat=n={}:v=1:a=0[combined]", clips.len()));
            "[combined]".to_owned()
        };
        args.extend([
            "-filter_complex".into(),
            filters.join(";"),
            "-map".into(),
            mapped,
        ]);
        args.extend(encode_flags(hardware));
    }
    args.extend([
        "-movflags".into(),
        "+faststart".into(),
        output.to_string_lossy().into_owned(),
    ]);
    Ok(args)
}

/// Add a looped soundtrack while keeping the encoded video bit-for-bit.
/// Since clip extraction discards source audio, soundtrack is the only audio.
pub fn soundtrack_args(
    video: &Path,
    soundtrack: &Path,
    output: &Path,
    duration_sec: f64,
    volume: f64,
) -> Result<Vec<String>, String> {
    if !duration_sec.is_finite()
        || duration_sec <= 0.0
        || !volume.is_finite()
        || !(0.0..=100.0).contains(&volume)
    {
        return Err("Некорректная длительность или громкость".into());
    }
    let fade_at = (duration_sec - (duration_sec / 4.0).min(1.5)).max(0.0);
    let fade_duration = (duration_sec - fade_at).max(0.01);
    let audio_filter = format!(
        "[1:a:0]volume={:.4},atrim=0:{:.4},asetpts=PTS-STARTPTS,\
         afade=t=out:st={:.4}:d={:.4}[music]",
        volume / 100.0,
        duration_sec,
        fade_at,
        fade_duration
    );
    Ok(vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        video.to_string_lossy().into_owned(),
        "-stream_loop".into(),
        "-1".into(),
        "-i".into(),
        soundtrack.to_string_lossy().into_owned(),
        "-filter_complex".into(),
        audio_filter,
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "[music]".into(),
        "-c:v".into(),
        "copy".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        "-t".into(),
        format!("{duration_sec:.4}"),
        "-movflags".into(),
        "+faststart".into(),
        output.to_string_lossy().into_owned(),
    ])
}

pub fn soundtrack_is_audio(path: &Path) -> Result<bool, String> {
    let payload: Value = ffprobe(path)?;
    Ok(payload["streams"].as_array().is_some_and(|streams| {
        streams
            .iter()
            .any(|item| item["codec_type"].as_str() == Some("audio"))
    }))
}

#[cfg(test)]
mod tests {
    use super::{concat_playlist, soundtrack_args, SelectedClip};
    use std::path::Path;

    #[test]
    fn request_bounds_deserialize() {
        let request: SelectedClip =
            serde_json::from_str(r#"{"video":"match.mp4","start_frame":100,"end_frame":199}"#)
                .unwrap();
        assert_eq!(request.end_frame - request.start_frame + 1, 100);
    }

    #[test]
    fn soundtrack_rejects_invalid_volume() {
        assert!(soundtrack_args(
            Path::new("a.mp4"),
            Path::new("b.mp3"),
            Path::new("out.mp4"),
            10.0,
            101.0
        )
        .is_err());
    }

    #[test]
    fn empty_playlist_is_rejected() {
        assert!(concat_playlist(&[]).is_err());
    }
}
