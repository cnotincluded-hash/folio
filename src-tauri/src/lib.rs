//! Folio desktop shell.
//!
//! The whole app is the page in `dist/`. This side only does what a web page can't: keep the
//! journal as files in a folder you choose (one Markdown file per entry, attachments as real
//! image and audio files), move it to another folder, and show native Save dialogs.

mod store;

use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use store::{err, EntryFile, Stamp};

const SETTINGS_FILE: &str = "settings.json";

#[derive(Serialize, Deserialize, Default)]
struct Settings {
    /// Folder holding the journal. None means the default folder (Documents/Folio).
    data_dir: Option<PathBuf>,
    /// True once someone has confirmed where the journal lives.
    configured: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderInfo {
    dir: String,
    default_dir: String,
    is_default: bool,
    configured: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoadResult {
    info: FolderInfo,
    meta: Option<String>,
    entries: Vec<EntryFile>,
    unreadable: Vec<String>,
    warning: Option<String>,
    stamp: Stamp,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Inspect {
    target: String,
    has_journal: bool,
    same: bool,
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(err)?;
    fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir.join(SETTINGS_FILE))
}

fn read_settings(app: &AppHandle) -> Settings {
    settings_path(app)
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_settings(app: &AppHandle, s: &Settings) -> Result<(), String> {
    let json = serde_json::to_string_pretty(s).map_err(err)?;
    store::write_atomic(&settings_path(app)?, json.as_bytes())
}

fn default_dir(app: &AppHandle) -> Result<PathBuf, String> {
    match app.path().document_dir() {
        Ok(d) => Ok(d.join(store::SUBFOLDER)),
        Err(_) => app.path().app_data_dir().map_err(err),
    }
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    match read_settings(app).data_dir {
        Some(d) if d.is_dir() => Ok(d),
        _ => default_dir(app),
    }
}

fn info(app: &AppHandle) -> Result<FolderInfo, String> {
    let dir = data_dir(app)?;
    let def = default_dir(app)?;
    Ok(FolderInfo {
        dir: dir.to_string_lossy().into_owned(),
        default_dir: def.to_string_lossy().into_owned(),
        is_default: store::same_path(&dir, &def),
        configured: read_settings(app).configured,
    })
}

#[tauri::command]
async fn load_all(app: AppHandle) -> Result<LoadResult, String> {
    let l = store::load_all(&data_dir(&app)?, store::today())?;
    Ok(LoadResult {
        info: info(&app)?,
        meta: l.meta,
        entries: l.entries,
        unreadable: l.unreadable,
        warning: l.warning,
        stamp: l.stamp,
    })
}

#[tauri::command]
async fn folder_stamp(app: AppHandle) -> Result<Stamp, String> {
    Ok(store::stamp(&data_dir(&app)?))
}

#[tauri::command]
async fn write_text(app: AppHandle, rel: String, text: String) -> Result<Stamp, String> {
    store::write_text(&data_dir(&app)?, &rel, &text)
}

#[tauri::command]
async fn remove_path(app: AppHandle, rel: String, trash: bool) -> Result<Stamp, String> {
    store::remove_path(&data_dir(&app)?, &rel, trash, store::today())
}

#[tauri::command]
async fn write_media(app: AppHandle, id: String, data: String) -> Result<(), String> {
    store::write_media(&data_dir(&app)?, &id, &data)
}

#[tauri::command]
async fn read_media(app: AppHandle, id: String) -> Result<Option<String>, String> {
    store::read_media(&data_dir(&app)?, &id)
}

#[tauri::command]
async fn remove_media(app: AppHandle, id: String) -> Result<(), String> {
    store::remove_media(&data_dir(&app)?, &id, store::today())
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    let picked = app
        .dialog()
        .file()
        .set_title("Choose where Folio keeps your journal")
        .blocking_pick_folder();
    Ok(picked.and_then(|p| p.as_path().map(|x| x.to_string_lossy().into_owned())))
}

/// What choosing this folder would mean: the folder the journal would really go in, whether a
/// journal is already there, and whether it's the folder in use now.
#[tauri::command]
async fn inspect_folder(app: AppHandle, dir: String) -> Result<Inspect, String> {
    let target = store::resolve_target(&PathBuf::from(dir));
    Ok(Inspect {
        same: store::same_path(&target, &data_dir(&app)?),
        has_journal: store::has_journal(&target),
        target: target.to_string_lossy().into_owned(),
    })
}

/// Point the app at another folder. `mode` is "open" (use the journal already there, or start
/// one) or "bring" (copy this journal there first, never overwriting files already there).
#[tauri::command]
async fn set_folder(app: AppHandle, dir: String, mode: String) -> Result<FolderInfo, String> {
    let target = PathBuf::from(dir);
    store::check_writable(&target)?;
    match mode.as_str() {
        "open" => {}
        "bring" => {
            let current = data_dir(&app)?;
            if !store::same_path(&current, &target) && current.exists() {
                store::copy_missing(&current, &target)?;
            }
        }
        other => return Err(format!("unknown folder action “{other}”.")),
    }
    let is_default = default_dir(&app).map(|d| store::same_path(&d, &target)).unwrap_or(false);
    write_settings(&app, &Settings { data_dir: if is_default { None } else { Some(target) }, configured: true })?;
    info(&app)
}

#[tauri::command]
async fn reveal_folder(app: AppHandle) -> Result<(), String> {
    let dir = data_dir(&app)?;
    fs::create_dir_all(&dir).map_err(err)?;
    tauri_plugin_opener::open_path(dir.to_string_lossy().into_owned(), None::<&str>).map_err(err)
}

#[tauri::command]
async fn save_export(app: AppHandle, filename: String, contents: String) -> Result<Option<String>, String> {
    let ext = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();
    let mut dialog = app.dialog().file().set_file_name(&filename);
    if !ext.is_empty() {
        dialog = dialog.add_filter(ext.to_uppercase(), &[ext.as_str()]);
    }
    match dialog.blocking_save_file() {
        None => Ok(None),
        Some(choice) => {
            let path = choice
                .as_path()
                .ok_or_else(|| "that location can’t be saved to.".to_string())?
                .to_path_buf();
            fs::write(&path, contents).map_err(|e| format!("couldn’t write {}: {e}", path.display()))?;
            Ok(Some(path.to_string_lossy().into_owned()))
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            load_all,
            folder_stamp,
            write_text,
            remove_path,
            write_media,
            read_media,
            remove_media,
            pick_folder,
            inspect_folder,
            set_folder,
            reveal_folder,
            save_export
        ])
        .run(tauri::generate_context!())
        .expect("error while running Folio");
}
