//! Native MP4 byte-range playback and FFmpeg JPEG preview.
use crate::{
    preparation,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, Response, StatusCode},
    Json,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, SeekFrom},
};
use tokio_util::io::ReaderStream;
use uuid::Uuid;
use volley_media::preview::{render_jpeg, FrameRequest, PreviewMode};

fn range_request(value: Option<&str>, size: u64) -> Result<(u64, u64, bool), ()> {
    if size == 0 {
        return Err(());
    }
    let Some(value) = value else {
        return Ok((0, size - 1, false));
    };
    let value = value.strip_prefix("bytes=").ok_or(())?;
    if value.contains(',') {
        return Err(());
    }
    let (first, last) = value.split_once('-').ok_or(())?;
    if first.is_empty() {
        let suffix = last.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        Ok((size.saturating_sub(suffix), size - 1, true))
    } else {
        let start = first.parse::<u64>().map_err(|_| ())?;
        if start >= size {
            return Err(());
        }
        let end = if last.is_empty() {
            size - 1
        } else {
            last.parse::<u64>().map_err(|_| ())?.min(size - 1)
        };
        if end < start {
            return Err(());
        }
        Ok((start, end, true))
    }
}

async fn serve_mp4(path: PathBuf, headers: &HeaderMap) -> Result<Response<Body>, ApiError> {
    let mut file = fs::File::open(&path)
        .await
        .map_err(|_| storage::not_found("Видео не найдено"))?;
    let size = file
        .metadata()
        .await
        .map_err(|error| storage::internal(format!("Media stat failed: {error}")))?
        .len();
    let requested = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let (start, end, partial) = match range_request(requested, size) {
        Ok(value) => value,
        Err(()) => {
            return Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                .header(header::CONTENT_LENGTH, "0")
                .body(Body::empty())
                .expect("Static response headers"));
        }
    };
    file.seek(SeekFrom::Start(start))
        .await
        .map_err(|error| storage::internal(format!("Media seek failed: {error}")))?;
    let bytes = end - start + 1;
    let body = Body::from_stream(ReaderStream::new(file.take(bytes)));
    let mut response = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, bytes.to_string())
        .header(header::CACHE_CONTROL, "private, max-age=120");
    if partial {
        response = response.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    Ok(response.body(body).expect("Static response headers"))
}

fn required_query<'a>(query: &'a HashMap<String, String>, name: &str) -> Result<&'a str, ApiError> {
    query
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| storage::bad_request(format!("Не указан параметр {name}")))
}

pub async fn video_media(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response<Body>, ApiError> {
    let id = required_query(&query, "project_id")?.to_owned();
    let name = required_query(&query, "video")?.to_owned();
    let reading = state.clone();
    let plan = tokio::task::spawn_blocking(move || preparation::plan_for(&reading, &id, &name))
        .await
        .map_err(|error| storage::internal(format!("Video-media plan failed: {error}")))??;
    let prepared = plan
        .ready_path()
        .ok_or_else(|| storage::conflict("Подготовка видео ещё не завершена"))?;
    serve_mp4(prepared.to_path_buf(), &headers).await
}

pub async fn video_view(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Value> {
    let id = required_query(&query, "project_id")?.to_owned();
    let name = required_query(&query, "video")?.to_owned();
    let status = preparation::collect_status(&state, &id).await?;
    let selected = status["videos"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["video"].as_str() == Some(&name)))
        .ok_or_else(|| storage::bad_request("Видео не принадлежит проекту"))?;
    let ready = selected["state"].as_str() == Some("ready");
    let stage = selected["state"].as_str().unwrap_or("missing");
    let reading = state.clone();
    let item = name.clone();
    let info = tokio::task::spawn_blocking(move || {
        let source = reading.repository_root.join("project").join(&item);
        volley_media::probe(&source).map_err(storage::bad_request)
    })
    .await
    .map_err(|error| storage::internal(format!("Video-info task failed: {error}")))??;
    let name_encoded = utf8_percent_encode(&name, NON_ALPHANUMERIC);
    let media_url = format!("/api/video-media?project_id={id}&video={name_encoded}");
    let poster = format!("/api/video-preview?video={name_encoded}&time=2&mode=source");
    Ok(Json(json!({
        "video": name,
        "duration_sec": info.duration_sec,
        "media_url": if ready {Some(media_url.as_str())} else {None},
        "browser_preview": if ready {"prepared"} else {"preparing"},
        "preparation_state": stage,
        "segment_seconds": 18,
        "preview_url": if ready {Some(media_url.as_str())} else {None},
        "poster_url": if ready {Some(poster.as_str())} else {None},
        "fallback_warning": if ready {""} else {"Подготовка видео ещё не завершена."},
    })))
}

fn file_key(name: &str, source: &Path) -> Result<String, ApiError> {
    let stat = source.metadata().map_err(|error| {
        storage::internal(format!("Video metadata for preview failed: {error}"))
    })?;
    let nanos = stat
        .modified()
        .map_err(|error| storage::internal(format!("Preview timestamp failed: {error}")))?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| storage::internal(format!("Preview timestamp out of range: {error}")))?
        .as_nanos();
    let signature = format!("{name}:{}:{nanos}", stat.len());
    let sha = Sha256::digest(signature.as_bytes());
    Ok(format!("{sha:x}")[..20].to_owned())
}

async fn cached_jpeg(path: &Path) -> Option<Vec<u8>> {
    let stat = fs::metadata(path).await.ok()?;
    if stat.len() > 9_000_000 || stat.len() < 10 {
        return None;
    }
    let bytes = fs::read(path).await.ok()?;
    bytes.starts_with(&[0xff, 0xd8]).then_some(bytes)
}

pub async fn video_preview(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response<Body>, ApiError> {
    let name = required_query(&query, "video")?.to_owned();
    storage::video_name(&name)?;
    let input = state.repository_root.join("project").join(&name);
    let source = input
        .canonicalize()
        .map_err(|_| storage::not_found("Видео не найдено"))?;
    if !source.is_file() {
        return Err(storage::not_found("Видео не найдено"));
    }

    let mode = PreviewMode::parse(query.get("mode").map(String::as_str).unwrap_or("source"))
        .map_err(storage::bad_request)?;
    let timestamp = query
        .get("time")
        .map(String::as_str)
        .unwrap_or("0")
        .parse::<f64>()
        .map_err(|_| storage::bad_request("Некорректное время кадра"))?;
    let width = match query.get("width").map(String::as_str) {
        None | Some("auto") | Some("") => None,
        Some(value) => Some(
            value
                .parse::<u32>()
                .map_err(|_| storage::bad_request("Некорректная ширина"))?,
        ),
    };

    let source_for_probe = source.clone();
    let info = tokio::task::spawn_blocking(move || volley_media::probe(&source_for_probe))
        .await
        .map_err(|error| storage::internal(format!("Preview probe failed: {error}")))?
        .map_err(storage::bad_request)?;
    let frame = FrameRequest::new(&info, mode, timestamp, width).map_err(storage::bad_request)?;
    let signature = file_key(&name, &source)?;
    let target = state
        .data_root
        .join("ui_previews")
        .join(signature)
        .join(format!(
            "{}_{}_{}.jpg",
            mode.as_str(),
            frame.frame_width,
            frame.frame_index
        ));
    if let Some(bytes) = cached_jpeg(&target).await {
        return Ok(jpeg_response(bytes));
    }

    // Limit competing video decodes on a desktop. After admission recheck the
    // cache so simultaneous requests for the same frame do not both encode it.
    let _permit = state
        .preview_limit
        .acquire()
        .await
        .map_err(|error| storage::internal(format!("Preview semaphore closed: {error}")))?;
    if let Some(bytes) = cached_jpeg(&target).await {
        return Ok(jpeg_response(bytes));
    }
    let jpeg = render_jpeg(&source, &frame)
        .await
        .map_err(storage::bad_request)?;
    if state.writes_enabled {
        if let Some(parent) = target.parent() {
            if fs::create_dir_all(parent).await.is_ok() {
                let scratch = target.with_extension(format!("{}.tmp", Uuid::new_v4().simple()));
                if fs::write(&scratch, &jpeg).await.is_ok() {
                    if fs::rename(&scratch, &target).await.is_err() {
                        let _ = fs::remove_file(&scratch).await;
                    }
                }
            }
        }
    }
    Ok(jpeg_response(jpeg))
}

fn jpeg_response(bytes: Vec<u8>) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .header(header::CACHE_CONTROL, "private, max-age=120")
        .body(Body::from(bytes))
        .expect("Static JPEG headers")
}

#[cfg(test)]
mod tests {
    use super::range_request;
    #[test]
    fn single_byte_ranges_and_suffixes_are_bounded() {
        assert_eq!(range_request(Some("bytes=0-0"), 100), Ok((0, 0, true)));
        assert_eq!(range_request(Some("bytes=98-"), 100), Ok((98, 99, true)));
        assert_eq!(range_request(Some("bytes=-3"), 100), Ok((97, 99, true)));
        assert!(range_request(Some("bytes=120-"), 100).is_err());
        assert!(range_request(Some("bytes=0-1,2-4"), 100).is_err());
    }
}
