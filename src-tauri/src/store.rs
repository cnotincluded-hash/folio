//! The journal folder on disk. Plain file work with no Tauri types, so it can be tested on its own.
//!
//! Layout of a journal folder:
//!
//! ```text
//! folio.json                         journals and settings
//! Entries/2026/2026-10-18 Title (a1b2c3).md   one Markdown file per entry (JSON front matter)
//! Attachments/<id>.jpg|.mp3|…        photos, drawings and recordings
//! Attachments/Thumbnails/<id>-t.jpg  small copies for the timeline
//! Recently Deleted/2026-10-18 …      deleted entries and attachments, kept for 30 days
//! ```

use std::{
    fmt::Display,
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;

pub const META_FILE: &str = "folio.json";
pub const ENTRIES_DIR: &str = "Entries";
pub const ATTACH_DIR: &str = "Attachments";
pub const THUMBS_DIR: &str = "Thumbnails";
pub const DELETED_DIR: &str = "Recently Deleted";
pub const SUBFOLDER: &str = "Folio";
pub const KEEP_DELETED_DAYS: i64 = 30;

const MEDIA_TYPES: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("png", "image/png"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
    ("mp3", "audio/mpeg"),
    ("m4a", "audio/mp4"),
    ("wav", "audio/wav"),
    ("webm", "audio/webm"),
    ("ogg", "audio/ogg"),
    ("flac", "audio/flac"),
    ("aac", "audio/aac"),
    ("bin", "application/octet-stream"),
];

/// Changes to these numbers mean the journal changed on disk.
#[derive(Serialize, Clone, Copy, Default, PartialEq, Debug)]
pub struct Stamp {
    pub mtime: u64,
    pub count: u64,
}

#[derive(Serialize, Debug)]
pub struct EntryFile {
    pub rel: String,
    pub text: String,
}

#[derive(Serialize, Debug)]
pub struct Loaded {
    pub meta: Option<String>,
    pub entries: Vec<EntryFile>,
    pub unreadable: Vec<String>,
    pub warning: Option<String>,
    pub stamp: Stamp,
}

pub fn err<E: Display>(e: E) -> String {
    e.to_string()
}

fn ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn mtime(p: &Path) -> u64 {
    fs::metadata(p).and_then(|m| m.modified()).map(ms).unwrap_or(0)
}

/// Resolve a path the page gave us inside the journal folder, refusing anything that could leave it.
pub fn inside(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    if rel.trim().is_empty() || p.is_absolute() {
        return Err(format!("refused the path “{rel}”."));
    }
    for c in p.components() {
        match c {
            Component::Normal(_) => {}
            _ => return Err(format!("refused the path “{rel}”.")),
        }
    }
    Ok(root.join(p))
}

fn valid_id(id: &str) -> bool {
    (3..=120).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || "_-~".contains(c))
}

/// Write to a temporary file, then rename over the target, so a crash never leaves half a file.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("couldn’t create {}: {e}", dir.display()))?;
    }
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = target.with_file_name(format!(".{name}.tmp"));
    fs::write(&tmp, bytes).map_err(|e| format!("couldn’t write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("couldn’t save {}: {e}", target.display())
    })
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let hidden = p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with('.')).unwrap_or(true);
        if hidden {
            continue;
        }
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn entry_files(root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    walk(&root.join(ENTRIES_DIR), &mut v);
    v.retain(|p| p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("md")).unwrap_or(false));
    v.sort();
    v
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str().map(String::from),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub fn stamp(root: &Path) -> Stamp {
    let files = entry_files(root);
    let newest = files.iter().map(|p| mtime(p)).max().unwrap_or(0);
    Stamp { mtime: newest.max(mtime(&root.join(META_FILE))), count: files.len() as u64 }
}

pub fn has_journal(dir: &Path) -> bool {
    dir.join(META_FILE).is_file() || dir.join(ENTRIES_DIR).is_dir()
}

fn effectively_empty(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(rd) => rd.flatten().all(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.starts_with('.') || n == "desktop.ini" || n == "thumbs.db"
        }),
        Err(_) => true,
    }
}

/// Where a picked folder's journal lives: the folder itself if it is new, empty, already a journal
/// or called "Folio"; otherwise a "Folio" folder inside it, so entries never mix with other files.
pub fn resolve_target(dir: &Path) -> PathBuf {
    let named_folio = dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case(SUBFOLDER))
        .unwrap_or(false);
    if !dir.exists() || has_journal(dir) || effectively_empty(dir) || named_folio {
        dir.to_path_buf()
    } else {
        dir.join(SUBFOLDER)
    }
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

pub fn check_writable(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Folio can’t create that folder ({e})."))?;
    let probe = dir.join(".folio-write-test");
    fs::write(&probe, b"ok").map_err(|e| format!("Folio can’t save in that folder ({e})."))?;
    let _ = fs::remove_file(probe);
    Ok(())
}

/// Read the whole journal. Never creates anything: a folder that doesn't exist yet reads as empty.
pub fn load_all(root: &Path, today: i64) -> Result<Loaded, String> {
    prune_deleted(root, today);
    let mut warning = None;
    let meta_path = root.join(META_FILE);
    let meta = match fs::read_to_string(&meta_path) {
        Ok(s) if serde_json::from_str::<serde_json::Value>(&s).is_ok() => Some(s),
        Ok(_) => {
            let aside = root.join(format!("folio.unreadable-{}.json", ms(SystemTime::now())));
            fs::rename(&meta_path, &aside).map_err(err)?;
            warning = Some(format!(
                "Your journal list couldn’t be read, so it was moved to {}. Your entries are untouched.",
                aside.display()
            ));
            None
        }
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(format!("couldn’t read {}: {e}", meta_path.display())),
    };
    let mut entries = Vec::new();
    let mut unreadable = Vec::new();
    for p in entry_files(root) {
        match fs::read_to_string(&p) {
            Ok(text) => entries.push(EntryFile { rel: rel_of(root, &p), text }),
            Err(_) => unreadable.push(rel_of(root, &p)),
        }
    }
    Ok(Loaded { meta, entries, unreadable, warning, stamp: stamp(root) })
}

pub fn write_text(root: &Path, rel: &str, text: &str) -> Result<Stamp, String> {
    let target = inside(root, rel)?;
    write_atomic(&target, text.as_bytes())?;
    Ok(stamp(root))
}

/// Remove a file from the journal. With `trash`, it goes to Recently Deleted instead.
pub fn remove_path(root: &Path, rel: &str, trash: bool, today: i64) -> Result<Stamp, String> {
    let target = inside(root, rel)?;
    if target.is_file() {
        if trash {
            move_to_deleted(root, &target, today)?;
        } else {
            fs::remove_file(&target).map_err(|e| format!("couldn’t remove {}: {e}", target.display()))?;
        }
    }
    Ok(stamp(root))
}

fn media_dir(root: &Path, id: &str) -> PathBuf {
    if id.ends_with("-t") {
        root.join(ATTACH_DIR).join(THUMBS_DIR)
    } else {
        root.join(ATTACH_DIR)
    }
}

fn find_media(root: &Path, id: &str) -> Option<(PathBuf, &'static str)> {
    let dir = media_dir(root, id);
    MEDIA_TYPES.iter().find_map(|(ext, mime)| {
        let p = dir.join(format!("{id}.{ext}"));
        p.is_file().then_some((p, *mime))
    })
}

/// Save a `data:` URL as a real file (photo.jpg, memo.mp3, …).
pub fn write_media(root: &Path, id: &str, data_url: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err(format!("refused the attachment name “{id}”."));
    }
    let rest = data_url.strip_prefix("data:").ok_or("that attachment isn’t a data URL.")?;
    let (head, b64) = rest.split_once(',').ok_or("that attachment is damaged.")?;
    if !head.ends_with(";base64") {
        return Err("that attachment isn’t base64.".into());
    }
    let mime = head.trim_end_matches(";base64").split(';').next().unwrap_or("").to_ascii_lowercase();
    let ext = match mime.as_str() {
        "audio/mp3" | "audio/mpeg3" => "mp3",
        "audio/x-m4a" | "audio/m4a" => "m4a",
        "audio/x-wav" | "audio/wave" => "wav",
        m => MEDIA_TYPES.iter().find(|(_, t)| *t == m).map(|(e, _)| *e).unwrap_or("bin"),
    };
    let bytes = STANDARD.decode(b64.trim()).map_err(|e| format!("that attachment is damaged ({e})."))?;
    let dir = media_dir(root, id);
    for (other, _) in MEDIA_TYPES.iter().filter(|(e, _)| *e != ext) {
        let _ = fs::remove_file(dir.join(format!("{id}.{other}")));
    }
    write_atomic(&dir.join(format!("{id}.{ext}")), &bytes)
}

pub fn read_media(root: &Path, id: &str) -> Result<Option<String>, String> {
    if !valid_id(id) {
        return Ok(None);
    }
    match find_media(root, id) {
        None => Ok(None),
        Some((p, mime)) => {
            let bytes = fs::read(&p).map_err(|e| format!("couldn’t read {}: {e}", p.display()))?;
            Ok(Some(format!("data:{mime};base64,{}", STANDARD.encode(bytes))))
        }
    }
}

/// Full-size attachments go to Recently Deleted; thumbnails are simply removed.
pub fn remove_media(root: &Path, id: &str, today: i64) -> Result<(), String> {
    if !valid_id(id) {
        return Ok(());
    }
    if let Some((p, _)) = find_media(root, id) {
        if id.ends_with("-t") {
            fs::remove_file(&p).map_err(err)?;
        } else {
            move_to_deleted(root, &p, today)?;
        }
    }
    Ok(())
}

fn move_to_deleted(root: &Path, file: &Path, today: i64) -> Result<(), String> {
    let dir = root.join(DELETED_DIR);
    fs::create_dir_all(&dir).map_err(err)?;
    let (y, m, d) = civil_from_days(today);
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let mut target = dir.join(format!("{y:04}-{m:02}-{d:02} {name}"));
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("{y:04}-{m:02}-{d:02} ({n}) {name}"));
        n += 1;
    }
    if fs::rename(file, &target).is_err() {
        fs::copy(file, &target).map_err(err)?;
        fs::remove_file(file).map_err(err)?;
    }
    Ok(())
}

/// Empty Recently Deleted of anything deleted more than 30 days ago.
pub fn prune_deleted(root: &Path, today: i64) {
    let Ok(rd) = fs::read_dir(root.join(DELETED_DIR)) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let date = name.get(..10).and_then(|s| {
            let mut it = s.split('-');
            let y = it.next()?.parse::<i64>().ok()?;
            let m = it.next()?.parse::<u32>().ok()?;
            let d = it.next()?.parse::<u32>().ok()?;
            Some(days_from_civil(y, m, d))
        });
        if let Some(day) = date {
            if today - day > KEEP_DELETED_DAYS && e.path().is_file() {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

fn copy_tree_missing(src: &Path, dst: &Path) -> Result<u64, String> {
    let mut files = Vec::new();
    walk(src, &mut files);
    let mut n = 0;
    for f in files {
        let rel = f.strip_prefix(src).map_err(err)?;
        let to = dst.join(rel);
        if to.exists() {
            continue;
        }
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir).map_err(err)?;
        }
        fs::copy(&f, &to).map_err(|e| format!("couldn’t copy {}: {e}", f.display()))?;
        n += 1;
    }
    Ok(n)
}

/// Copy this journal into another folder without overwriting anything already there.
pub fn copy_missing(src: &Path, dst: &Path) -> Result<u64, String> {
    if dst != src && dst.starts_with(src) {
        return Err("choose a folder outside your current journal folder.".into());
    }
    let mut n = 0;
    let meta = src.join(META_FILE);
    if meta.is_file() && !dst.join(META_FILE).exists() {
        fs::create_dir_all(dst).map_err(err)?;
        fs::copy(&meta, dst.join(META_FILE)).map_err(err)?;
        n += 1;
    }
    n += copy_tree_missing(&src.join(ENTRIES_DIR), &dst.join(ENTRIES_DIR))?;
    n += copy_tree_missing(&src.join(ATTACH_DIR), &dst.join(ATTACH_DIR))?;
    Ok(n)
}

/// Days since 1970-01-01 to a calendar date (UTC).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe as i64 + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

/// Calendar date (UTC) to days since 1970-01-01.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

pub fn today() -> i64 {
    (SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) / 86_400) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("folio-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn dates_round_trip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_744), (2026, 10, 18));
        for day in [-1000, 0, 11_016, 20_744, 30_000] {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
        }
    }

    #[test]
    fn refuses_paths_outside_the_folder() {
        let root = Path::new("/journal");
        assert!(inside(root, "Entries/2026/a.md").is_ok());
        for bad in ["", "../x.md", "Entries/../../x", "/etc/passwd", "./a", "Entries//../a"] {
            assert!(inside(root, bad).is_err(), "{bad}");
        }
        assert!(write_media(root, "../evil", "data:image/jpeg;base64,AA==").is_err());
    }

    #[test]
    fn entries_write_load_rename_and_delete() {
        let root = tmp("entries");
        assert!(load_all(&root, 20_744).unwrap().entries.is_empty());
        assert!(!root.exists(), "loading must not create the folder");
        let s1 = write_text(&root, "Entries/2026/2026-10-18 First (abc123).md", "---\n{}\n---\nhi").unwrap();
        assert_eq!(s1.count, 1);
        write_text(&root, META_FILE, "{\"journals\":[]}").unwrap();
        let l = load_all(&root, 20_744).unwrap();
        assert_eq!(l.entries.len(), 1);
        assert_eq!(l.entries[0].rel, "Entries/2026/2026-10-18 First (abc123).md");
        assert!(l.meta.is_some());
        // rename = write the new name, then remove the old one without trashing it
        write_text(&root, "Entries/2026/2026-10-18 Renamed (abc123).md", "---\n{}\n---\nhi").unwrap();
        let s = remove_path(&root, "Entries/2026/2026-10-18 First (abc123).md", false, 20_744).unwrap();
        assert_eq!(s.count, 1);
        assert!(!root.join(DELETED_DIR).exists());
        // delete = move to Recently Deleted
        let s = remove_path(&root, "Entries/2026/2026-10-18 Renamed (abc123).md", true, 20_744).unwrap();
        assert_eq!(s.count, 0);
        assert!(root.join(DELETED_DIR).join("2026-10-18 2026-10-18 Renamed (abc123).md").is_file());
        // pruned after 30 days
        prune_deleted(&root, 20_744 + 31);
        assert_eq!(fs::read_dir(root.join(DELETED_DIR)).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unreadable_meta_is_moved_aside() {
        let root = tmp("meta");
        write_text(&root, META_FILE, "not json").unwrap();
        let l = load_all(&root, 20_744).unwrap();
        assert!(l.meta.is_none() && l.warning.is_some());
        assert!(!root.join(META_FILE).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn media_round_trip() {
        let root = tmp("media");
        write_media(&root, "m1abc", "data:image/jpeg;base64,/9j/AA==").unwrap();
        write_media(&root, "m1abc-t", "data:image/jpeg;base64,/9j/AA==").unwrap();
        write_media(&root, "m2abc", "data:audio/x-m4a;base64,AAAA").unwrap();
        assert!(root.join("Attachments/m1abc.jpg").is_file());
        assert!(root.join("Attachments/Thumbnails/m1abc-t.jpg").is_file());
        assert!(root.join("Attachments/m2abc.m4a").is_file());
        assert_eq!(read_media(&root, "m1abc").unwrap().unwrap(), "data:image/jpeg;base64,/9j/AA==");
        assert_eq!(read_media(&root, "m2abc").unwrap().unwrap(), "data:audio/mp4;base64,AAAA");
        assert_eq!(read_media(&root, "nothere").unwrap(), None);
        remove_media(&root, "m1abc", 20_744).unwrap();
        remove_media(&root, "m1abc-t", 20_744).unwrap();
        assert!(read_media(&root, "m1abc").unwrap().is_none());
        assert!(root.join(DELETED_DIR).join("2026-10-18 m1abc.jpg").is_file());
        assert_eq!(fs::read_dir(root.join(DELETED_DIR)).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn picking_a_busy_folder_uses_a_folio_subfolder() {
        let base = tmp("pick");
        fs::create_dir_all(&base).unwrap();
        assert_eq!(resolve_target(&base), base, "empty folder is used as is");
        fs::write(base.join("notes.txt"), "x").unwrap();
        assert_eq!(resolve_target(&base), base.join(SUBFOLDER));
        write_text(&base, META_FILE, "{}").unwrap();
        assert_eq!(resolve_target(&base), base, "an existing journal is used as is");
        assert_eq!(resolve_target(&base.join("new")), base.join("new"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn copying_never_overwrites() {
        let a = tmp("copy-a");
        let b = tmp("copy-b");
        write_text(&a, META_FILE, "{\"from\":\"a\"}").unwrap();
        write_text(&a, "Entries/2026/x.md", "a").unwrap();
        write_media(&a, "m1abc", "data:image/png;base64,AA==").unwrap();
        write_text(&b, META_FILE, "{\"from\":\"b\"}").unwrap();
        write_text(&b, "Entries/2026/x.md", "b").unwrap();
        assert_eq!(copy_missing(&a, &b).unwrap(), 1);
        assert_eq!(fs::read_to_string(b.join(META_FILE)).unwrap(), "{\"from\":\"b\"}");
        assert_eq!(fs::read_to_string(b.join("Entries/2026/x.md")).unwrap(), "b");
        assert!(b.join("Attachments/m1abc.png").is_file());
        assert!(copy_missing(&a, &a.join("Entries/inner")).is_err());
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }
}
