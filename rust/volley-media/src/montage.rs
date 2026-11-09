//! Native-source montage encoding plan. FFmpeg remains the codec executable;
//! no Python runner is needed for clip extraction, concatenation or music.

use crate::{ffprobe, probe};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const ANALYSIS_FPS: f64 = 30.0;
/// Explicit output geometry. Original is the safe unscaled default.
/// Vertical fits the entire source frame inside a portrait 9:16 canvas.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoLayout {
    #[default]
    Original,
    Vertical,
}

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
/// Derive portrait canvas from the maximum source height at original FPS.
pub fn output_profile_for_layout(
    formats: &[(u32, u32, String, f64)],
    layout: VideoLayout,
) -> Result<(u32, u32, f64), String> {
    let original = output_profile(formats)?;
    if layout == VideoLayout::Original {
        return Ok(original);
    }
    let height = original.1;
    let width = ((((height as f64) * 9.0 / 16.0) / 2.0).round() as u32 * 2).max(2);
    if u64::from(width) * u64::from(height) > 32_000_000 {
        return Err("Vertical output exceeds maximum safe canvas".into());
    }
    Ok((width, height, original.2))
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
    compose_args_for_layout(
        clips,
        formats,
        output,
        playlist,
        hardware,
        VideoLayout::Original,
    )
}

/// Both native CPU and legacy montage share this FFmpeg path. Portrait mode
/// fits the entire frame and pads borders rather than cropping the ball.
pub fn compose_args_for_layout(
    clips: &[PathBuf],
    formats: &[(u32, u32, String, f64)],
    output: &Path,
    playlist: Option<&Path>,
    hardware: bool,
    layout: VideoLayout,
) -> Result<Vec<String>, String> {
    let (max_width, max_height, target_fps) = output_profile_for_layout(formats, layout)?;
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
    if can_stream_copy(formats)
        && playlist.is_some()
        && (layout == VideoLayout::Original
            || (formats[0].0 == max_width && formats[0].1 == max_height))
    {
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
            let fit = if layout == VideoLayout::Vertical {
                format!("scale={max_width}:{max_height}:force_original_aspect_ratio=decrease:flags=lanczos,")
            } else {
                String::new()
            };
            filters.push(format!(
                "[{index}:v:0]setpts=PTS-STARTPTS,fps={target_fps:.8},\
                 {fit}pad={max_width}:{max_height}:(ow-iw)/2:(oh-ih)/2,\
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
        "[1:a:0]aresample=48000:async=1:first_pts=0,volume={:.4},atrim=0:{:.4},asetpts=PTS-STARTPTS,\
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
        "-ar".into(),
        "48000".into(),
        "-t".into(),
        format!("{duration_sec:.4}"),
        "-movflags".into(),
        "+faststart".into(),
        output.to_string_lossy().into_owned(),
    ])
}

/// FFprobe evidence for a finished MP4. This validates the VIDEO stream
/// separately from the AUDIO stream; container duration alone can hide
/// truncated video, AAC encoder delay, and missing soundtracks.
#[derive(Debug, Clone, Serialize)]
pub struct RenderQuality {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub video_duration_sec: f64,
    pub video_frames: Option<u64>,
    pub has_audio: bool,
    pub audio_duration_sec: Option<f64>,
    pub audio_video_skew_sec: Option<f64>,
    pub audio_codec: Option<String>,
}

fn ffprobe_start(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}
fn ffprobe_sec(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
}

pub fn verify_mp4_quality(
    path: &Path,
    expected: (u32, u32, f64),
    planned_duration: f64,
    with_audio: bool,
) -> Result<RenderQuality, String> {
    if !planned_duration.is_finite() || planned_duration <= 0.0 {
        return Err("Invalid planned MP4 duration".into());
    }
    let data = ffprobe(path)?;
    let streams = data["streams"]
        .as_array()
        .ok_or("MP4 has no FFprobe streams")?;
    let videos = streams
        .iter()
        .filter(|s| s["codec_type"] == "video")
        .collect::<Vec<_>>();
    let audios = streams
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .collect::<Vec<_>>();
    if videos.len() != 1
        || audios.len() != usize::from(with_audio)
        || streams.len() != 1 + usize::from(with_audio)
    {
        return Err("MP4 must contain exactly one H.264 video and the expected AAC audio".into());
    }
    let v = videos[0];
    let width = v["width"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or("Missing MP4 video width")?;
    let height = v["height"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or("Missing MP4 video height")?;
    let fps = v["avg_frame_rate"]
        .as_str()
        .and_then(parse_fraction)
        .map(|(n, d)| n as f64 / d as f64)
        .ok_or("Missing MP4 video FPS")?;
    if (width, height) != (expected.0, expected.1)
        || (fps - expected.2).abs() > 0.1
        || v["codec_name"] != "h264"
        || v["pix_fmt"] != "yuv420p"
        || !matches!(v["sample_aspect_ratio"].as_str(), Some("1:1" | "1/1"))
    {
        return Err(
            "MP4 changed original output dimensions, frame rate, SAR, codec or pixel format".into(),
        );
    }
    let video_start = ffprobe_start(&v["start_time"]).unwrap_or(0.0);
    let video_duration = ffprobe_sec(&v["duration"])
        .or_else(|| ffprobe_sec(&data["format"]["duration"]))
        .ok_or("Missing MP4 video duration")?;
    if video_start.abs() > 0.08
        || video_duration < planned_duration - 0.34
        || video_duration > planned_duration + 0.34
    {
        return Err("MP4 contains a truncated/extended video or nonzero video start time".into());
    }
    let frames = v["nb_frames"].as_str().and_then(|n| n.parse::<u64>().ok());
    if frames.is_some_and(|count| {
        let wanted = (planned_duration * expected.2).round();
        count == 0 || (count as f64 - wanted).abs() > 3.0
    }) {
        return Err("MP4 frame count differs from extracted original-frame plan".into());
    }
    let (audio_duration, skew, codec) = if let Some(a) = audios.first() {
        if a["codec_name"] != "aac" {
            return Err("MP4 soundtrack must be AAC".into());
        }
        let sample_rate = a["sample_rate"]
            .as_str()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or("Missing AAC sample rate")?;
        if !(16_000..=192_000).contains(&sample_rate) {
            return Err("Invalid AAC output sample rate".into());
        }
        let start = ffprobe_start(&a["start_time"]).unwrap_or(0.0);
        let duration = ffprobe_sec(&a["duration"]).ok_or("Missing AAC duration")?;
        let drift = (start + duration - (video_start + video_duration)).abs();
        if (start - video_start).abs() > 0.08 || drift > 0.12 || duration < planned_duration - 0.12
        {
            return Err("Audio and video start/end are out of sync".into());
        }
        (Some(duration), Some(drift), Some("aac".into()))
    } else {
        (None, None, None)
    };
    Ok(RenderQuality {
        width,
        height,
        fps,
        video_duration_sec: video_duration,
        video_frames: frames,
        has_audio: with_audio,
        audio_duration_sec: audio_duration,
        audio_video_skew_sec: skew,
        audio_codec: codec,
    })
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
