//! Tauri commands: thin wrappers around the `kp_*` crates.
//!
//! Commands are `async` so they run off the main (UI) thread; CPU-heavy work
//! goes through `spawn_blocking`. Errors are French strings shown as is.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use kp_geom::{JobRequest, JobResult, PathData, Tile, TileRequest, WeedRequest};
use kp_import::{FontInfo, ImageInfo, Imported, TextOutline, TextRequest, TraceRequest};
use kp_link::relay::{self, RelayInfo, RelayStatus};
use kp_link::{Destination, JobInfo, PortInfo, SendOptions, SendOutcome};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// Runs blocking work on Tauri's thread pool.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("Erreur interne : {e}"))?
}

static FONTS: OnceLock<Vec<FontInfo>> = OnceLock::new();

#[tauri::command]
async fn list_fonts() -> Result<Vec<FontInfo>, String> {
    // Waits for the warm-up scan started in `setup` if it is still running.
    blocking(|| Ok(FONTS.get_or_init(kp_import::list_fonts).clone())).await
}

#[tauri::command]
async fn text_outline(req: TextRequest) -> Result<TextOutline, String> {
    blocking(move || kp_import::text_outline(&req)).await
}

#[tauri::command]
async fn import_file(path: PathBuf) -> Result<Imported, String> {
    blocking(move || kp_import::import_file(&path)).await
}

#[tauri::command]
async fn load_image(path: PathBuf) -> Result<ImageInfo, String> {
    blocking(move || kp_import::load_image(&path)).await
}

#[tauri::command]
async fn trace_image(req: TraceRequest) -> Result<Imported, String> {
    blocking(move || kp_import::trace_image(&req)).await
}

#[tauri::command]
async fn make_weeding(req: WeedRequest) -> Result<PathData, String> {
    blocking(move || kp_geom::make_weeding(&req)).await
}

#[tauri::command]
async fn make_tiles(req: TileRequest) -> Result<Vec<Tile>, String> {
    blocking(move || kp_geom::make_tiles(&req)).await
}

#[tauri::command]
async fn build_job(req: JobRequest) -> Result<JobResult, String> {
    blocking(move || kp_geom::build_job(&req)).await
}

#[tauri::command]
async fn list_ports() -> Result<Vec<PortInfo>, String> {
    blocking(|| Ok(kp_link::list_ports())).await
}

#[tauri::command]
async fn list_printers() -> Result<Vec<String>, String> {
    blocking(|| Ok(kp_link::list_printers())).await
}

/// KI Plotter relays (Raspberry Pi) answering on the local networks.
#[tauri::command]
async fn discover_relays() -> Result<Vec<RelayInfo>, String> {
    blocking(|| Ok(relay::discover(Duration::from_millis(900)))).await
}

#[tauri::command]
async fn relay_status(host: String, port: u16) -> Result<RelayStatus, String> {
    blocking(move || relay::status(&host, port)).await
}

#[tauri::command]
async fn read_text_file(path: PathBuf) -> Result<String, String> {
    blocking(move || {
        std::fs::read_to_string(&path)
            .map_err(|e| format!("Impossible de lire « {} » : {e}", path.display()))
    })
    .await
}

#[tauri::command]
async fn write_text_file(path: PathBuf, contents: String) -> Result<(), String> {
    blocking(move || {
        std::fs::write(&path, contents)
            .map_err(|e| format!("Impossible d'écrire « {} » : {e}", path.display()))
    })
    .await
}

// ---- rolls inventory

fn rolls_file(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Dossier de données introuvable : {e}"))?;
    Ok(dir.join("rouleaux.json"))
}

/// The rolls (shared by every project), or None before the first save.
#[tauri::command]
async fn load_rolls(app: AppHandle) -> Result<Option<String>, String> {
    let path = rolls_file(&app)?;
    blocking(move || match std::fs::read_to_string(&path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Impossible de lire les rouleaux : {e}")),
    })
    .await
}

/// Writes the rolls; the previous file is kept as `rouleaux.json.bak`.
#[tauri::command]
async fn save_rolls(app: AppHandle, contents: String) -> Result<(), String> {
    let path = rolls_file(&app)?;
    blocking(move || {
        let fail = |e: std::io::Error| format!("Impossible d'enregistrer les rouleaux : {e}");
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        if path.exists() {
            let _ = std::fs::copy(&path, path.with_extension("json.bak"));
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, contents).map_err(fail)?;
        std::fs::rename(&tmp, &path).map_err(fail)
    })
    .await
}

// ---- sending

#[derive(Default)]
struct Sender {
    busy: AtomicBool,
    cancel: Arc<AtomicBool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendRequest {
    destination: Destination,
    data: String,
    #[serde(default)]
    chunk_size: usize,
    #[serde(default)]
    chunk_delay_ms: u64,
    /// Name, size and settings of the job, shown on the relay's screen.
    #[serde(default)]
    info: JobInfo,
}

#[derive(Clone, Serialize)]
struct JobProgress {
    sent: usize,
    total: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobFinished {
    ok: bool,
    cancelled: bool,
    error: Option<String>,
    /// The relay keeps the job in its queue (started from its screen).
    queued: bool,
    position: usize,
}

/// Starts sending in the background; progress and the outcome arrive as
/// `job-progress` and `job-finished` events.
#[tauri::command]
fn send_job(app: AppHandle, state: State<'_, Arc<Sender>>, req: SendRequest) -> Result<(), String> {
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("Un envoi est déjà en cours.".into());
    }
    state.cancel.store(false, Ordering::SeqCst);
    let sender = Arc::clone(&state);
    std::thread::spawn(move || {
        let opts = SendOptions {
            chunk_size: req.chunk_size,
            chunk_delay_ms: req.chunk_delay_ms,
            info: req.info,
        };
        let mut last = Instant::now() - Duration::from_secs(1);
        let mut progress = |sent: usize, total: usize| {
            // Throttle: a file destination can report thousands of chunks a second.
            if sent == total || last.elapsed() >= Duration::from_millis(50) {
                last = Instant::now();
                let _ = app.emit("job-progress", JobProgress { sent, total });
            }
        };
        let result = kp_link::send(
            &req.destination,
            req.data.as_bytes(),
            &opts,
            &mut progress,
            &sender.cancel,
        );
        let done = |ok, cancelled, error, queued, position| JobFinished {
            ok,
            cancelled,
            error,
            queued,
            position,
        };
        let finished = match result {
            Ok(SendOutcome::Completed) => done(true, false, None, false, 0),
            Ok(SendOutcome::Queued { position }) => done(true, false, None, true, position),
            Ok(SendOutcome::Cancelled) => done(false, true, None, false, 0),
            Err(e) => done(false, false, Some(e), false, 0),
        };
        sender.busy.store(false, Ordering::SeqCst);
        let _ = app.emit("job-finished", finished);
    });
    Ok(())
}

#[tauri::command]
fn cancel_job(state: State<'_, Arc<Sender>>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(Sender::default()))
        .invoke_handler(tauri::generate_handler![
            list_fonts,
            text_outline,
            import_file,
            load_image,
            trace_image,
            make_weeding,
            make_tiles,
            build_job,
            list_ports,
            list_printers,
            discover_relays,
            relay_status,
            load_rolls,
            save_rolls,
            read_text_file,
            write_text_file,
            send_job,
            cancel_job,
        ])
        .setup(|_| {
            // Warm the font list so the first text tool use is instant.
            std::thread::spawn(|| {
                let _ = FONTS.get_or_init(kp_import::list_fonts);
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("KI Plotter n'a pas pu démarrer");
}
