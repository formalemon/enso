//! What a file holds, as ffprobe reads it: how long it lasts, the size and
//! rate of its pictures, its sound, and the names it carries.

use std::{path::Path, time::Duration};

use serde_json::Value;

use crate::{Error, ffmpeg};

/// A media file, as far as playing it goes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    pub duration: Option<Duration>,
    /// Moving pictures, if it has any. A song's cover isn't one.
    pub video: Option<Video>,
    pub audio: Option<Audio>,
    pub tags: Tags,
    /// It carries a picture: a song's cover.
    pub cover: bool,
    /// What it is, in a word: `MP4`, `Matroska`, `MP3`, `FLAC`.
    pub format: String,
}

/// A video's pictures.
#[derive(Clone, Debug, PartialEq)]
pub struct Video {
    /// As shown: a phone's upright video is taller than wide, whichever way
    /// its pixels are stored.
    pub width: u32,
    pub height: u32,
    /// Frames a second.
    pub rate: f32,
    pub codec: String,
}

/// A file's sound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Audio {
    pub rate: u32,
    pub channels: u16,
    pub codec: String,
}

/// The names a file carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Which track of the album: `3`, or `3/12`.
    pub track: Option<String>,
    pub date: Option<String>,
}

impl Media {
    /// Whether it's for listening to: sound and no moving pictures.
    pub fn is_sound(&self) -> bool {
        self.video.is_none() && self.audio.is_some()
    }
}

/// Read what `path` holds.
pub fn probe(path: &Path) -> Result<Media, Error> {
    let out = ffmpeg::output(
        "ffprobe",
        [
            "-v".into(),
            "error".into(),
            "-print_format".into(),
            "json".into(),
            "-show_format".into(),
            "-show_streams".into(),
            ffmpeg::input(path),
        ],
    )?;
    parse(&String::from_utf8_lossy(&out))
}

/// ffprobe's JSON, read.
pub fn parse(json: &str) -> Result<Media, Error> {
    let root: Value =
        serde_json::from_str(json).map_err(|_| Error::new("ffprobe said something unreadable"))?;
    let format = &root["format"];
    let mut media = Media {
        duration: seconds(&format["duration"]),
        tags: tags(&format["tags"]),
        format: kind(format),
        ..Media::default()
    };
    for stream in root["streams"].as_array().into_iter().flatten() {
        let codec = text(&stream["codec_name"]).unwrap_or_default();
        match stream["codec_type"].as_str() {
            Some("video") if stream["disposition"]["attached_pic"].as_i64() == Some(1) => {
                media.cover = true;
            }
            Some("video") if media.video.is_none() => {
                let (Some(width), Some(height)) =
                    (stream["width"].as_u64(), stream["height"].as_u64())
                else {
                    continue;
                };
                let rate = rate(&stream["avg_frame_rate"])
                    .or_else(|| rate(&stream["r_frame_rate"]))
                    .unwrap_or(30.0);
                // Turned a quarter, it's shown the other way round.
                let (width, height) = if rotation(stream).rem_euclid(180) == 90 {
                    (height, width)
                } else {
                    (width, height)
                };
                media.video = Some(Video {
                    width: width as u32,
                    height: height as u32,
                    rate,
                    codec,
                });
                if media.duration.is_none() {
                    media.duration = seconds(&stream["duration"]);
                }
            }
            Some("audio") if media.audio.is_none() => {
                media.audio = Some(Audio {
                    rate: text(&stream["sample_rate"])
                        .and_then(|rate| rate.parse().ok())
                        .unwrap_or(48_000),
                    channels: stream["channels"].as_u64().unwrap_or(2) as u16,
                    codec,
                });
                if media.duration.is_none() {
                    media.duration = seconds(&stream["duration"]);
                }
                // An Ogg file's names ride on its stream.
                let own = tags(&stream["tags"]);
                media.tags.title = media.tags.title.or(own.title);
                media.tags.artist = media.tags.artist.or(own.artist);
                media.tags.album = media.tags.album.or(own.album);
                media.tags.track = media.tags.track.or(own.track);
                media.tags.date = media.tags.date.or(own.date);
            }
            _ => {}
        }
    }
    if media.video.is_none() && media.audio.is_none() {
        return Err(Error::new("neither pictures nor sound in it"));
    }
    Ok(media)
}

fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn seconds(value: &Value) -> Option<Duration> {
    text(value)?
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(Duration::from_secs_f64)
}

/// `30000/1001` frames a second.
fn rate(value: &Value) -> Option<f32> {
    let text = value.as_str()?;
    let (num, den) = text.split_once('/').unwrap_or((text, "1"));
    let (num, den) = (num.parse::<f32>().ok()?, den.parse::<f32>().ok()?);
    (den > 0.0 && num > 0.0)
        .then(|| num / den)
        .filter(|rate| (1.0..=240.0).contains(rate))
}

/// How far a video is turned, in degrees: as a display matrix says it, or
/// an older `rotate` tag.
fn rotation(stream: &Value) -> i64 {
    let matrix = stream["side_data_list"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|data| data["rotation"].as_f64())
        .map(|degrees| degrees.round() as i64);
    matrix
        .or_else(|| text(&stream["tags"]["rotate"]).and_then(|degrees| degrees.parse().ok()))
        .unwrap_or(0)
}

/// The names, whatever case the file writes their keys in.
fn tags(value: &Value) -> Tags {
    let Some(map) = value.as_object() else {
        return Tags::default();
    };
    let get = |keys: &[&str]| {
        keys.iter().find_map(|key| {
            map.iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .and_then(|(_, value)| text(value))
        })
    };
    Tags {
        title: get(&["title"]),
        artist: get(&["artist", "album_artist", "performer", "composer"]),
        album: get(&["album"]),
        track: get(&["track", "tracknumber"]),
        date: get(&["date", "year", "creation_time"]),
    }
}

/// The container, in a word people know it by.
fn kind(format: &Value) -> String {
    let name = format["format_name"].as_str().unwrap_or_default();
    let first = name.split(',').next().unwrap_or_default();
    match first {
        "mov" if name.contains("mp4") => "MP4".to_owned(),
        "mov" => "QuickTime".to_owned(),
        "matroska" if name.contains("webm") => "Matroska".to_owned(),
        "matroska" => "Matroska".to_owned(),
        "webm" => "WebM".to_owned(),
        "mp3" => "MP3".to_owned(),
        "flac" => "FLAC".to_owned(),
        "ogg" => "Ogg".to_owned(),
        "wav" => "WAV".to_owned(),
        "avi" => "AVI".to_owned(),
        "aac" => "AAC".to_owned(),
        other => other.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_video_stands_upright() {
        let json = r#"{
            "streams": [
                {"codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080,
                 "avg_frame_rate": "30000/1001",
                 "side_data_list": [{"side_data_type": "Display Matrix", "rotation": -90}]},
                {"codec_type": "audio", "codec_name": "aac", "sample_rate": "48000",
                 "channels": 2}
            ],
            "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "12.512000",
                       "tags": {"creation_time": "2026-11-02T10:00:00.000000Z"}}
        }"#;
        let media = parse(json).unwrap();
        let video = media.video.clone().unwrap();
        assert_eq!((video.width, video.height), (1080, 1920));
        assert!((video.rate - 29.97).abs() < 0.01);
        assert_eq!(media.audio.unwrap().rate, 48_000);
        assert_eq!(media.duration, Some(Duration::from_millis(12_512)));
        assert_eq!(media.format, "MP4");
        assert!(!media.cover);
    }

    #[test]
    fn a_song_has_its_names_and_its_cover_is_no_video() {
        let json = r#"{
            "streams": [
                {"codec_type": "audio", "codec_name": "mp3", "sample_rate": "44100",
                 "channels": 2},
                {"codec_type": "video", "codec_name": "mjpeg", "width": 600, "height": 600,
                 "disposition": {"attached_pic": 1}}
            ],
            "format": {"format_name": "mp3", "duration": "241.3",
                       "tags": {"TITLE": "Weightless", "Artist": "Marconi Union",
                                "album": "Weightless", "track": "1/1"}}
        }"#;
        let media = parse(json).unwrap();
        assert!(media.is_sound() && media.cover);
        assert_eq!(media.tags.title.as_deref(), Some("Weightless"));
        assert_eq!(media.tags.artist.as_deref(), Some("Marconi Union"));
        assert_eq!(media.tags.track.as_deref(), Some("1/1"));
        assert!(parse(r#"{"streams": [], "format": {}}"#).is_err());
        assert!(parse("not json").is_err());
    }
}
