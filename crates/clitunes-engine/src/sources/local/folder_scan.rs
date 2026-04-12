use std::path::PathBuf;

use clitunes_core::Track;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::ItemKey;

const SUPPORTED_EXTENSIONS: &[&str] = &["mp3", "flac", "ogg", "opus", "wav", "m4a", "aac"];

pub fn scan_paths(paths: &[PathBuf], max_files: usize) -> (Vec<Track>, Option<String>) {
    let mut tracks = Vec::new();
    let mut warning = None;

    for path in paths {
        if tracks.len() >= max_files {
            warning = Some(format!("scan capped at {} files", max_files));
            break;
        }

        if path.is_file() {
            if is_supported(path) {
                tracks.push(read_track(path));
            }
        } else if path.is_dir() {
            let mut dir_files: Vec<PathBuf> = Vec::new();
            for entry in walkdir::WalkDir::new(path)
                .follow_links(true)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() && is_supported(entry.path()) {
                    dir_files.push(entry.into_path());
                }
            }
            dir_files.sort();

            for file_path in dir_files {
                if tracks.len() >= max_files {
                    warning = Some(format!("scan capped at {} files", max_files));
                    break;
                }
                tracks.push(read_track(&file_path));
            }
        }
    }

    (tracks, warning)
}

fn is_supported(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn read_track(path: &std::path::Path) -> Track {
    match read_tags(path) {
        Some(track) => track,
        None => Track {
            path: path.to_path_buf(),
            title: None,
            artist: None,
            album: None,
            album_artist: None,
            track_num: None,
            year: None,
            duration_secs: None,
            embedded_art: None,
        },
    }
}

fn read_tags(path: &std::path::Path) -> Option<Track> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let (title, artist, album, album_artist, track_num, year) = if let Some(tag) = tag {
        (
            tag.title().map(|s| clitunes_core::sanitize(&s)),
            tag.artist().map(|s| clitunes_core::sanitize(&s)),
            tag.album().map(|s| clitunes_core::sanitize(&s)),
            tag.get_string(&ItemKey::AlbumArtist)
                .map(clitunes_core::sanitize),
            tag.track(),
            tag.year(),
        )
    } else {
        (None, None, None, None, None, None)
    };

    let duration_secs = tagged.properties().duration().as_secs_f64();
    let duration_secs = if duration_secs > 0.0 {
        Some(duration_secs)
    } else {
        None
    };

    let embedded_art = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .and_then(|tag| tag.pictures().first())
        .map(|pic| pic.data().to_vec());

    Some(Track {
        path: path.to_path_buf(),
        title,
        artist,
        album,
        album_artist,
        track_num,
        year,
        duration_secs,
        embedded_art,
    })
}
