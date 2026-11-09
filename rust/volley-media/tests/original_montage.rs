//! Actual offline FFmpeg compatibility tests for BOTH source adapters:
//! native CPU preview uses offset=0; legacy CPU preview may use an
//! analysis-relative non-zero offset. All pixels come from original files.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};
use volley_media::{
    montage::{
        can_stream_copy, compose_args, concat_playlist, native_clip_format, output_profile,
        verify_original_segment, OriginalClip,
    },
    probe,
};

fn ffmpeg_ready() -> bool {
    ["ffmpeg", "ffprobe"].iter().all(|bin| {
        Command::new(bin)
            .arg("-version")
            .output()
            .is_ok_and(|result| result.status.success())
    })
}
fn command(bin: &str, args: &[String]) -> Output {
    let result = Command::new(bin).args(args).output().unwrap();
    assert!(
        result.status.success(),
        "{bin} failed: {}\n{}",
        args.join(" "),
        String::from_utf8_lossy(&result.stderr)
    );
    result
}
fn source(root: &Path, file: &str, width: u32, height: u32, fps: u32) -> PathBuf {
    let output = root.join(file);
    command(
        "ffmpeg",
        &[
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ]
        .into_iter()
        .map(String::from)
        .chain([format!(
            "testsrc2=size={width}x{height}:rate={fps}:duration=2"
        )])
        .chain(
            [
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "10",
                "-pix_fmt",
                "yuv420p",
                "-an",
            ]
            .into_iter()
            .map(String::from),
        )
        .chain([output.to_string_lossy().into_owned()])
        .collect::<Vec<_>>(),
    );
    output
}
fn extract(clip: &OriginalClip, target: &Path) -> (u32, u32, String, f64) {
    command("ffmpeg", &clip.extract_args(target, false));
    let format = native_clip_format(target).unwrap();
    verify_original_segment(clip, &format).unwrap();
    format
}
fn temp_root() -> PathBuf {
    let ticks = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("volley-montage-75-{}-{ticks}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn encoder_profile_never_downscales_mixed_portrait_and_landscape() {
    let formats = [
        (3840, 2160, "h264".into(), 30.0),
        (2160, 3840, "h264".into(), 25.0),
    ];
    let (width, height, fps) = output_profile(&formats).unwrap();
    assert_eq!((width, height, fps), (3840, 3840, 30.0));
    let args = compose_args(
        &[PathBuf::from("a.mp4"), PathBuf::from("b.mp4")],
        &formats,
        Path::new("joined.mp4"),
        None,
        false,
    )
    .unwrap();
    let filter = args
        .iter()
        .skip_while(|x| x.as_str() != "-filter_complex")
        .nth(1)
        .unwrap();
    assert!(filter.contains("pad=3840:3840"));
    assert!(!filter.contains("scale="));
    assert!(!can_stream_copy(&formats));
    assert!(output_profile(&[(8192, 8192, "h264".into(), 30.0)]).is_err());
    assert!(output_profile(&[(640, 360, "h264".into(), f64::INFINITY)]).is_err());
    assert!(output_profile(&[]).is_err());
}

#[test]
fn original_source_clips_and_mixed_orientation_export_preserve_dimensions_and_quality() {
    if !ffmpeg_ready() {
        eprintln!("SKIP original FFmpeg video-export fixture: ffmpeg/ffprobe unavailable");
        return;
    }
    let root = temp_root();
    let original_native = source(&root, "original_native.mp4", 640, 360, 30);
    let original_legacy = source(&root, "original_legacy.mp4", 360, 640, 25);

    // Native CPU boundaries are zero-offset 30fps analysis frames.
    let native = OriginalClip::from_analysis(&original_native, 0, 29, 0.0).unwrap();
    // Legacy CPU boundaries can carry an offset into the original recording.
    let legacy = OriginalClip::from_analysis(&original_legacy, 0, 29, 0.4).unwrap();
    assert_eq!((native.native_width, native.native_height), (640, 360));
    assert_eq!((legacy.native_width, legacy.native_height), (360, 640));
    assert_eq!(native.receipt().start_sec, 0.0);
    assert_eq!(legacy.receipt().start_sec, 0.4);
    assert_eq!(native.receipt().frames, 30);
    assert_eq!(legacy.receipt().frames, 25);
    assert!(native.receipt().original_resolution);
    assert_eq!(
        (
            native.receipt().native_width,
            native.receipt().native_height
        ),
        (640, 360)
    );
    assert!(OriginalClip::from_analysis(&original_native, 55, 100, 0.0).is_err());

    let first = root.join("native_segment.mp4");
    let second = root.join("legacy_segment.mp4");
    let first_fmt = extract(&native, &first);
    let second_fmt = extract(&legacy, &second);
    assert!(!can_stream_copy(&[first_fmt.clone(), second_fmt.clone()]));
    let formats = vec![first_fmt, second_fmt];
    let profile = output_profile(&formats).unwrap();
    assert_eq!(profile, (640, 640, 30.0));

    let assembled = root.join("joined.mp4");
    let args = compose_args(
        &[first.clone(), second.clone()],
        &formats,
        &assembled,
        None,
        false,
    )
    .unwrap();
    command("ffmpeg", &args);
    let (w, h, codec, fps) = native_clip_format(&assembled).unwrap();
    assert_eq!((w, h), (640, 640));
    assert_eq!(codec, "h264");
    assert!((fps - 30.0).abs() < 0.05);
    let duration = probe(&assembled).unwrap().duration_sec;
    assert!(
        (duration - 2.0).abs() < 0.15,
        "Output truncated: duration={duration}"
    );

    // Original-to-extracted quality using the native source's actual pixels.
    // PSNR catches accidental proxy/720p source usage or a poor re-encode.
    let compared = command(
        "ffmpeg",
        &[
            "-nostdin",
            "-hide_banner",
            "-i",
            &original_native.to_string_lossy(),
            "-i",
            &first.to_string_lossy(),
            "-filter_complex",
            "[0:v:0][1:v:0]psnr",
            "-frames:v",
            "30",
            "-f",
            "null",
            "-",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>(),
    );
    let logs = String::from_utf8_lossy(&compared.stderr);
    let average = logs
        .rsplit("average:")
        .next()
        .and_then(|part| part.trim_start().split_whitespace().next())
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    assert!(
        average > 30.0,
        "Native source quality degraded: PSNR={average:.2} dB"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn identical_native_fps_and_size_uses_lossless_stream_copy_for_concat_stage() {
    if !ffmpeg_ready() {
        eprintln!("SKIP original FFmpeg stream-copy fixture: ffmpeg/ffprobe unavailable");
        return;
    }
    let root = temp_root();
    let original = source(&root, "same_source.mp4", 320, 240, 30);
    let a = OriginalClip::from_analysis(&original, 0, 29, 0.0).unwrap();
    let b = OriginalClip::from_analysis(&original, 30, 59, 0.0).unwrap();
    let first = root.join("a.mp4");
    let second = root.join("b.mp4");
    let formats = vec![extract(&a, &first), extract(&b, &second)];
    assert!(can_stream_copy(&formats));
    let playlist = root.join("parts.txt");
    fs::write(
        &playlist,
        concat_playlist(&[first.clone(), second.clone()]).unwrap(),
    )
    .unwrap();
    let joined = root.join("copy_joined.mp4");
    let args = compose_args(&[first, second], &formats, &joined, Some(&playlist), false).unwrap();
    assert!(args.windows(2).any(|v| v == ["-c:v", "copy"]));
    command("ffmpeg", &args);
    let info = probe(&joined).unwrap();
    assert_eq!((info.width, info.height), (320, 240));
    assert!((info.duration_sec - 2.0).abs() < 0.15);
    fs::remove_dir_all(root).unwrap();
}
