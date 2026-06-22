//! Scans a folder of pad audio files and maps them to musical keys.
//!
//! Matches by whole filename token (e.g. "C.mp3", "Pad C.flac", "01 - C.mp3");
//! a token must be exactly a key spelling. Files with no key or a same-key
//! conflict go to `Preset::unmapped` for manual resolution instead of being
//! dropped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::model::{Key, Preset};

const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "ogg", "m4a", "aac"];

fn has_audio_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// First filename-stem token that names a key.
fn key_from_filename(path: &Path) -> Option<Key> {
    let stem = path.file_stem()?.to_str()?.to_lowercase();
    stem.split(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
        .filter(|t| !t.is_empty())
        .find_map(Key::parse)
}

/// Sorted audio files in `folder`.
fn audio_files(folder: &Path) -> Result<Vec<PathBuf>, String> {
    if !folder.is_dir() {
        return Err(format!("not a folder: {}", folder.display()));
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(folder)
        .map_err(|e| format!("read folder: {e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && has_audio_ext(p))
        .collect();
    paths.sort();
    Ok(paths)
}

fn folder_display_name(folder: &Path) -> String {
    folder
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Preset")
        .to_string()
}

/// Scan `folder`: keys claimed by exactly one file are auto-assigned; ambiguous
/// files go to `unmapped`. `name` overrides the display name (default: folder name).
pub fn scan_preset(folder: &Path, name: Option<String>) -> Result<Preset, String> {
    let paths = audio_files(folder)?;

    // Group files by the key their name implies.
    let mut by_key: HashMap<Key, Vec<PathBuf>> = HashMap::new();
    let mut unmapped: Vec<PathBuf> = Vec::new();
    for path in paths {
        match key_from_filename(&path) {
            Some(key) => by_key.entry(key).or_default().push(path),
            None => unmapped.push(path),
        }
    }

    // One claimant → auto-assign; conflicts → `unmapped`.
    let mut files: HashMap<Key, PathBuf> = HashMap::new();
    for (key, mut candidates) in by_key {
        if candidates.len() == 1 {
            files.insert(key, candidates.pop().unwrap());
        } else {
            unmapped.extend(candidates);
        }
    }
    unmapped.sort();

    Ok(Preset {
        id: folder.to_string_lossy().to_string(),
        name: name.unwrap_or_else(|| folder_display_name(folder)),
        folder: folder.to_path_buf(),
        files,
        unmapped,
    })
}

/// Re-scan a preset's folder, preserving the user's manual key assignments.
/// Manual choices win; only files still on disk are kept.
pub fn rescan_preserving(old: &Preset, name: Option<String>) -> Result<Preset, String> {
    let fresh = scan_preset(&old.folder, name.or_else(|| Some(old.name.clone())))?;
    Ok(merge_scan(old, fresh))
}

/// In-memory merge of a fresh scan onto an existing preset's manual mappings.
/// Split from `rescan_preserving` so the slow scan can run outside the settings
/// lock, then reconcile against live state under the lock.
pub fn merge_scan(old: &Preset, fresh: Preset) -> Preset {
    // All audio files currently on disk.
    let universe: Vec<PathBuf> = fresh
        .files
        .values()
        .cloned()
        .chain(fresh.unmapped.iter().cloned())
        .collect();

    // Fresh auto-mapping, overridden by surviving manual choices.
    let mut files = fresh.files;
    for (key, path) in &old.files {
        if universe.contains(path) {
            files.insert(*key, path.clone());
        }
    }

    // Unassigned files are unmapped.
    let mut unmapped: Vec<PathBuf> = universe
        .into_iter()
        .filter(|p| !files.values().any(|v| v == p))
        .collect();
    unmapped.sort();
    unmapped.dedup();

    Preset {
        unmapped,
        files,
        ..fresh
    }
}
