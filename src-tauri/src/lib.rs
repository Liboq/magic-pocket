use std::{
    borrow::Cow,
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use arboard::{Clipboard, ImageData, SetExtWindows};
use chrono::{DateTime, Utc};
use image::{ImageBuffer, RgbaImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_global_shortcut::{Builder as ShortcutBuilder, ShortcutEvent, ShortcutState};
use uuid::Uuid;

#[cfg(target_os = "windows")]
use window_vibrancy::{apply_blur, apply_mica};

const STORAGE_FILE: &str = "clipboard-history.json";
const IMAGE_DIR: &str = "clipboard-images";
const DEFAULT_MAX_ENTRIES: usize = 100;
const MAX_ALLOWED_ENTRIES: usize = 500;
const CLIPBOARD_EVENT: &str = "clipboard-updated";
const TOGGLE_SHORTCUT: &str = "Ctrl+Shift+Space";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardKind {
    Text,
    Image,
    Files,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardRecord {
    id: String,
    kind: ClipboardKind,
    content: String,
    searchable_text: String,
    created_at: DateTime<Utc>,
    favorite: bool,
    tags: Vec<String>,
    text: Option<String>,
    image_path: Option<String>,
    image_width: Option<u32>,
    image_height: Option<u32>,
    file_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ClipboardPayload {
    entries: Vec<ClipboardRecord>,
    max_entries: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedClipboardState {
    max_entries: usize,
    entries: Vec<ClipboardRecord>,
}

#[derive(Debug)]
struct ClipboardStore {
    entries: VecDeque<ClipboardRecord>,
    max_entries: usize,
    last_seen_signature: Option<String>,
    storage_path: PathBuf,
    image_dir: PathBuf,
}

#[derive(Debug)]
enum ClipboardCapture {
    Text {
        signature: String,
        display: String,
        searchable_text: String,
        text: String,
    },
    Image {
        signature: String,
        display: String,
        searchable_text: String,
        png_bytes: Vec<u8>,
        width: u32,
        height: u32,
    },
    Files {
        signature: String,
        display: String,
        searchable_text: String,
        file_paths: Vec<String>,
    },
}

impl ClipboardStore {
    fn load(storage_path: PathBuf, image_dir: PathBuf) -> Self {
        let restored = fs::read_to_string(&storage_path)
            .ok()
            .and_then(|content| serde_json::from_str::<PersistedClipboardState>(&content).ok());

        let mut store = match restored {
            Some(state) => Self {
                last_seen_signature: state
                    .entries
                    .first()
                    .map(ClipboardStore::signature_for_record),
                entries: VecDeque::from(state.entries),
                max_entries: normalize_limit(state.max_entries),
                storage_path,
                image_dir,
            },
            None => Self {
                entries: VecDeque::new(),
                max_entries: DEFAULT_MAX_ENTRIES,
                last_seen_signature: None,
                storage_path,
                image_dir,
            },
        };

        store.remove_missing_assets();
        store.enforce_limit();
        store.persist();
        store
    }

    fn payload(&self) -> ClipboardPayload {
        ClipboardPayload {
            entries: self.entries.iter().cloned().collect(),
            max_entries: self.max_entries,
        }
    }

    fn list(&self) -> Vec<ClipboardRecord> {
        self.entries.iter().cloned().collect()
    }

    fn persist(&self) {
        if let Some(parent) = self.storage_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::create_dir_all(&self.image_dir);

        let snapshot = PersistedClipboardState {
            max_entries: self.max_entries,
            entries: self.entries.iter().cloned().collect(),
        };

        if let Ok(serialized) = serde_json::to_string_pretty(&snapshot) {
            let _ = fs::write(&self.storage_path, serialized);
        }
    }

    fn upsert_capture(&mut self, capture: ClipboardCapture) -> bool {
        let signature = capture.signature().to_string();
        if self.last_seen_signature.as_deref() == Some(signature.as_str()) {
            return false;
        }

        self.last_seen_signature = Some(signature.clone());

        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| Self::signature_for_record(entry) == signature)
        {
            if let Some(mut existing) = self.entries.remove(index) {
                existing.created_at = Utc::now();
                match capture {
                    ClipboardCapture::Text {
                        display,
                        searchable_text,
                        text,
                        ..
                    } => {
                        existing.kind = ClipboardKind::Text;
                        existing.content = display;
                        existing.searchable_text = searchable_text;
                        existing.text = Some(text);
                        existing.image_path = None;
                        existing.image_width = None;
                        existing.image_height = None;
                        existing.file_paths = Vec::new();
                    }
                    ClipboardCapture::Image {
                        display,
                        searchable_text,
                        png_bytes,
                        width,
                        height,
                        ..
                    } => {
                        let image_path = self.write_image_asset(&existing.id, &png_bytes);
                        existing.kind = ClipboardKind::Image;
                        existing.content = display;
                        existing.searchable_text = searchable_text;
                        existing.text = None;
                        existing.image_path = Some(image_path.to_string_lossy().to_string());
                        existing.image_width = Some(width);
                        existing.image_height = Some(height);
                        existing.file_paths = Vec::new();
                    }
                    ClipboardCapture::Files {
                        display,
                        searchable_text,
                        file_paths,
                        ..
                    } => {
                        existing.kind = ClipboardKind::Files;
                        existing.content = display;
                        existing.searchable_text = searchable_text;
                        existing.text = None;
                        existing.image_path = None;
                        existing.image_width = None;
                        existing.image_height = None;
                        existing.file_paths = file_paths;
                    }
                }
                self.entries.push_front(existing);
            }
        } else {
            let id = Uuid::new_v4().to_string();
            self.entries.push_front(self.new_record(id, capture));
        }

        self.enforce_limit();
        self.persist();
        true
    }

    fn new_record(&self, id: String, capture: ClipboardCapture) -> ClipboardRecord {
        match capture {
            ClipboardCapture::Text {
                display,
                searchable_text,
                text,
                ..
            } => ClipboardRecord {
                id,
                kind: ClipboardKind::Text,
                content: display,
                searchable_text,
                created_at: Utc::now(),
                favorite: false,
                tags: Vec::new(),
                text: Some(text),
                image_path: None,
                image_width: None,
                image_height: None,
                file_paths: Vec::new(),
            },
            ClipboardCapture::Image {
                display,
                searchable_text,
                png_bytes,
                width,
                height,
                ..
            } => {
                let image_path = self.write_image_asset(&id, &png_bytes);
                ClipboardRecord {
                    id,
                    kind: ClipboardKind::Image,
                    content: display,
                    searchable_text,
                    created_at: Utc::now(),
                    favorite: false,
                    tags: Vec::new(),
                    text: None,
                    image_path: Some(image_path.to_string_lossy().to_string()),
                    image_width: Some(width),
                    image_height: Some(height),
                    file_paths: Vec::new(),
                }
            }
            ClipboardCapture::Files {
                display,
                searchable_text,
                file_paths,
                ..
            } => ClipboardRecord {
                id,
                kind: ClipboardKind::Files,
                content: display,
                searchable_text,
                created_at: Utc::now(),
                favorite: false,
                tags: Vec::new(),
                text: None,
                image_path: None,
                image_width: None,
                image_height: None,
                file_paths,
            },
        }
    }

    fn toggle_favorite(&mut self, id: &str) -> Option<Vec<ClipboardRecord>> {
        let record = self.entries.iter_mut().find(|entry| entry.id == id)?;
        record.favorite = !record.favorite;
        self.persist();
        Some(self.list())
    }

    fn update_tags(&mut self, id: &str, tags: Vec<String>) -> Option<Vec<ClipboardRecord>> {
        let record = self.entries.iter_mut().find(|entry| entry.id == id)?;
        record.tags = dedupe_tags(tags);
        self.persist();
        Some(self.list())
    }

    fn set_limit(&mut self, limit: usize) -> Vec<ClipboardRecord> {
        self.max_entries = normalize_limit(limit);
        self.enforce_limit();
        self.persist();
        self.list()
    }

    fn delete_record(&mut self, id: &str) -> Option<Vec<ClipboardRecord>> {
        let index = self.entries.iter().position(|entry| entry.id == id)?;
        if let Some(record) = self.entries.remove(index) {
            self.delete_record_assets(&record);
        }
        self.last_seen_signature = self.entries.front().map(Self::signature_for_record);
        self.persist();
        Some(self.list())
    }

    fn get_by_id(&self, id: &str) -> Option<ClipboardRecord> {
        self.entries.iter().find(|entry| entry.id == id).cloned()
    }

    fn mark_last_seen_from_record(&mut self, record: &ClipboardRecord) {
        self.last_seen_signature = Some(Self::signature_for_record(record));
    }

    fn enforce_limit(&mut self) {
        while self.entries.len() > self.max_entries {
            if let Some(record) = self.entries.pop_back() {
                self.delete_record_assets(&record);
            }
        }
    }

    fn remove_missing_assets(&mut self) {
        self.entries.retain(|record| match &record.image_path {
            Some(path) => Path::new(path).exists(),
            None => true,
        });
    }

    fn delete_record_assets(&self, record: &ClipboardRecord) {
        if let Some(path) = &record.image_path {
            let _ = fs::remove_file(path);
        }
    }

    fn write_image_asset(&self, id: &str, png_bytes: &[u8]) -> PathBuf {
        let _ = fs::create_dir_all(&self.image_dir);
        let path = self.image_dir.join(format!("{id}.png"));
        let _ = fs::write(&path, png_bytes);
        path
    }

    fn signature_for_record(record: &ClipboardRecord) -> String {
        match record.kind {
            ClipboardKind::Text => format!("text:{}", record.text.as_deref().unwrap_or_default()),
            ClipboardKind::Image => {
                let path = record.image_path.as_deref().unwrap_or_default();
                let hash = fs::read(path)
                    .ok()
                    .map(|bytes| hash_bytes(&bytes))
                    .unwrap_or_default();
                format!("image:{hash}")
            }
            ClipboardKind::Files => format!("files:{}", record.file_paths.join("|")),
        }
    }
}

impl ClipboardCapture {
    fn signature(&self) -> &str {
        match self {
            ClipboardCapture::Text { signature, .. } => signature,
            ClipboardCapture::Image { signature, .. } => signature,
            ClipboardCapture::Files { signature, .. } => signature,
        }
    }
}

fn dedupe_tags(tags: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();

    for tag in tags {
        let cleaned = tag.trim();
        if cleaned.is_empty() || deduped.iter().any(|item| item == cleaned) {
            continue;
        }
        deduped.push(cleaned.to_string());
    }

    deduped
}

fn normalize_limit(limit: usize) -> usize {
    limit.clamp(20, MAX_ALLOWED_ENTRIES)
}

fn summarize_text(text: &str) -> String {
    let single_line = text.replace('\n', " ").replace('\r', " ");
    let trimmed = single_line.trim();
    if trimmed.chars().count() <= 180 {
        trimmed.to_string()
    } else {
        let summary: String = trimmed.chars().take(180).collect();
        format!("{summary}…")
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn capture_clipboard(clipboard: &mut Clipboard) -> Option<ClipboardCapture> {
    if let Ok(file_paths) = clipboard.get().file_list() {
        let file_paths: Vec<String> = file_paths
            .into_iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();

        if !file_paths.is_empty() {
            let filenames: Vec<String> = file_paths
                .iter()
                .filter_map(|path| {
                    Path::new(path)
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                })
                .collect();

            let summary = if filenames.len() == 1 {
                filenames[0].clone()
            } else {
                format!("{} 个文件", filenames.len())
            };

            let searchable = format!("{} {}", filenames.join(" "), file_paths.join(" "));
            return Some(ClipboardCapture::Files {
                signature: format!("files:{}", file_paths.join("|")),
                display: summary,
                searchable_text: searchable,
                file_paths,
            });
        }
    }

    if let Ok(image) = clipboard.get_image() {
        let width = image.width as u32;
        let height = image.height as u32;
        if let Some(png_bytes) = image_to_png(image) {
            let hash = hash_bytes(&png_bytes);
            return Some(ClipboardCapture::Image {
                signature: format!("image:{hash}"),
                display: format!("图片 {width} × {height}"),
                searchable_text: format!("图片 {width} {height}"),
                png_bytes,
                width,
                height,
            });
        }
    }

    if let Ok(text) = clipboard.get_text() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(ClipboardCapture::Text {
                signature: format!("text:{trimmed}"),
                display: summarize_text(trimmed),
                searchable_text: trimmed.to_string(),
                text: trimmed.to_string(),
            });
        }
    }

    None
}

fn image_to_png(image: ImageData<'static>) -> Option<Vec<u8>> {
    let width = image.width as u32;
    let height = image.height as u32;
    let bytes = image.bytes.into_owned();
    let rgba: RgbaImage = ImageBuffer::from_raw(width, height, bytes)?;

    let mut png_bytes = Vec::new();
    let dynamic = image::DynamicImage::ImageRgba8(rgba);
    let mut cursor = std::io::Cursor::new(&mut png_bytes);
    dynamic
        .write_to(&mut cursor, image::ImageFormat::Png)
        .ok()?;
    Some(png_bytes)
}

fn png_to_image_data(path: &Path) -> Result<ImageData<'static>, String> {
    let image = image::open(path)
        .map_err(|error| error.to_string())?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Ok(ImageData {
        width: width as usize,
        height: height as usize,
        bytes: Cow::Owned(image.into_raw()),
    })
}

type SharedClipboardStore = Arc<Mutex<ClipboardStore>>;

#[tauri::command]
fn get_clipboard_history(
    state: State<'_, SharedClipboardStore>,
) -> Result<Vec<ClipboardRecord>, String> {
    state
        .lock()
        .map_err(|_| "clipboard store lock poisoned".to_string())
        .map(|store| store.list())
}

#[tauri::command]
fn get_max_entries(state: State<'_, SharedClipboardStore>) -> Result<usize, String> {
    state
        .lock()
        .map_err(|_| "clipboard store lock poisoned".to_string())
        .map(|store| store.max_entries)
}

#[tauri::command]
fn get_toggle_shortcut() -> &'static str {
    TOGGLE_SHORTCUT
}

#[tauri::command]
fn toggle_favorite(
    id: String,
    state: State<'_, SharedClipboardStore>,
) -> Result<Vec<ClipboardRecord>, String> {
    let updated = state
        .lock()
        .map_err(|_| "clipboard store lock poisoned".to_string())?
        .toggle_favorite(&id)
        .ok_or_else(|| "record not found".to_string())?;

    Ok(updated)
}

#[tauri::command]
fn update_tags(
    id: String,
    tags: Vec<String>,
    state: State<'_, SharedClipboardStore>,
) -> Result<Vec<ClipboardRecord>, String> {
    let updated = state
        .lock()
        .map_err(|_| "clipboard store lock poisoned".to_string())?
        .update_tags(&id, tags)
        .ok_or_else(|| "record not found".to_string())?;

    Ok(updated)
}

#[tauri::command]
fn set_max_entries(
    limit: usize,
    state: State<'_, SharedClipboardStore>,
) -> Result<Vec<ClipboardRecord>, String> {
    let updated = state
        .lock()
        .map_err(|_| "clipboard store lock poisoned".to_string())?
        .set_limit(limit);

    Ok(updated)
}

#[tauri::command]
fn delete_record(
    id: String,
    state: State<'_, SharedClipboardStore>,
) -> Result<Vec<ClipboardRecord>, String> {
    let updated = state
        .lock()
        .map_err(|_| "clipboard store lock poisoned".to_string())?
        .delete_record(&id)
        .ok_or_else(|| "record not found".to_string())?;

    Ok(updated)
}

#[tauri::command]
fn copy_record_to_clipboard(
    id: String,
    state: State<'_, SharedClipboardStore>,
) -> Result<(), String> {
    let record = state
        .lock()
        .map_err(|_| "clipboard store lock poisoned".to_string())?
        .get_by_id(&id)
        .ok_or_else(|| "record not found".to_string())?;

    let mut clipboard = Clipboard::new().map_err(|error| error.to_string())?;

    match record.kind {
        ClipboardKind::Text => {
            clipboard
                .set()
                .exclude_from_history()
                .text(record.text.clone().unwrap_or_default())
                .map_err(|error| error.to_string())?;
        }
        ClipboardKind::Image => {
            let path = record
                .image_path
                .as_deref()
                .ok_or_else(|| "image asset not found".to_string())?;
            let image = png_to_image_data(Path::new(path))?;
            clipboard
                .set()
                .exclude_from_history()
                .image(image)
                .map_err(|error| error.to_string())?;
        }
        ClipboardKind::Files => {
            let paths: Vec<PathBuf> = record.file_paths.iter().map(PathBuf::from).collect();
            clipboard
                .set()
                .exclude_from_history()
                .file_list(&paths)
                .map_err(|error| error.to_string())?;
        }
    }

    let mut store = state
        .lock()
        .map_err(|_| "clipboard store lock poisoned".to_string())?;
    store.mark_last_seen_from_record(&record);

    Ok(())
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

fn emit_snapshot(app: &AppHandle, store: &SharedClipboardStore) {
    if let Ok(guard) = store.lock() {
        let _ = app.emit(CLIPBOARD_EVENT, guard.payload());
    }
}

fn spawn_clipboard_listener(app: AppHandle, store: SharedClipboardStore) {
    thread::spawn(move || {
        let mut clipboard = match Clipboard::new() {
            Ok(clipboard) => clipboard,
            Err(_) => return,
        };

        loop {
            if let Some(capture) = capture_clipboard(&mut clipboard) {
                let changed = {
                    if let Ok(mut guard) = store.lock() {
                        guard.upsert_capture(capture)
                    } else {
                        false
                    }
                };

                if changed {
                    emit_snapshot(&app, &store);
                }
            }

            thread::sleep(Duration::from_millis(650));
        }
    });
}

fn apply_window_chrome(window: &WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        let _ =
            apply_mica(window, None).or_else(|_| apply_blur(window, Some((248, 248, 250, 140))));
    }
}

fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or(true);
        let minimized = window.is_minimized().unwrap_or(false);

        if visible && !minimized && window.is_focused().unwrap_or(false) {
            let _ = window.hide();
            return;
        }

        let _ = window.show();
        if minimized {
            let _ = window.unminimize();
        }
        let _ = window.set_focus();
    }
}

fn handle_shortcut_event(
    app: &AppHandle,
    _shortcut: &tauri_plugin_global_shortcut::Shortcut,
    event: ShortcutEvent,
) {
    if event.state == ShortcutState::Pressed {
        toggle_main_window(app);
    }
}

pub fn run() {
    let shortcut_plugin = ShortcutBuilder::new()
        .with_shortcut(TOGGLE_SHORTCUT)
        .expect("invalid global shortcut")
        .with_handler(handle_shortcut_event)
        .build();

    tauri::Builder::default()
        .plugin(shortcut_plugin)
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            let storage_path = app_data_dir.join(STORAGE_FILE);
            let image_dir = app_data_dir.join(IMAGE_DIR);

            let store = Arc::new(Mutex::new(ClipboardStore::load(storage_path, image_dir)));
            app.manage(store.clone());

            if let Some(window) = app.get_webview_window("main") {
                apply_window_chrome(&window);
            }

            spawn_clipboard_listener(app.handle().clone(), store.clone());
            emit_snapshot(&app.handle().clone(), &store);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_clipboard_history,
            get_max_entries,
            get_toggle_shortcut,
            toggle_favorite,
            update_tags,
            set_max_entries,
            delete_record,
            copy_record_to_clipboard,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
