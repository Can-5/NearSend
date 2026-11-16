mod proto;
mod store;
mod transfers;

use proto::TCP_PORT;
use store::{HistoryEntry, Store};
use tauri::Manager;
use transfers::{Shared, TransferView, PeerInfo, PendingView};

#[derive(serde::Serialize)]
struct StateView {
    device_name: String,
    save_dir: String,
    listener_status: String,
    my_code: String,
    primary_ip: String,
    local_ips: Vec<String>,
}

#[tauri::command]
fn get_state(shared: tauri::State<'_, std::sync::Arc<Shared>>) -> StateView {
    let primary = shared
        .local_ips
        .iter()
        .find(|i| i.as_str() != "127.0.0.1")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".into());
    StateView {
        device_name: shared.device_name(),
        save_dir: shared.save_dir().to_string_lossy().into_owned(),
        listener_status: shared.listener_status.lock().unwrap().clone(),
        my_code: format!("nearsend://{primary}:{TCP_PORT}"),
        primary_ip: primary,
        local_ips: shared.local_ips.clone(),
    }
}

#[tauri::command]
fn set_device_name(shared: tauri::State<'_, std::sync::Arc<Shared>>, name: String) -> Result<(), String> {
    let clean = proto::sanitize_display_name(&name, 64);
    if clean.is_empty() {
        return Err("name cannot be empty".into());
    }
    let mut store = shared.store.lock().unwrap();
    store.state.device_name = Some(clean);
    store.save()
}

#[tauri::command]
fn set_save_dir(shared: tauri::State<'_, std::sync::Arc<Shared>>, dir: String) -> Result<String, String> {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return Err("empty path".into());
    }
    let path = std::path::PathBuf::from(trimmed);
    let canonical = path.canonicalize().map_err(|e| format!("cannot resolve: {e}"))?;
    if !canonical.is_dir() {
        return Err("not a directory".into());
    }
    let mut store = shared.store.lock().unwrap();
    store.state.save_dir = Some(canonical.to_string_lossy().into_owned());
    store.save()?;
    Ok(canonical.to_string_lossy().into_owned())
}

#[tauri::command]
fn peers(shared: tauri::State<'_, std::sync::Arc<Shared>>) -> Vec<PeerInfo> {
    transfers::peer_views(&shared)
}

#[tauri::command]
fn add_peer_code(shared: tauri::State<'_, std::sync::Arc<Shared>>, code: String) -> Result<PeerInfo, String> {
    let addr = proto::parse_invite_code(&code)?;
    let info = PeerInfo {
        device: "manual entry".into(),
        addr: addr.clone(),
        last_seen: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    shared.peers.lock().unwrap().insert(addr.split(':').next().unwrap_or(&addr).to_string(), info.clone());
    Ok(info)
}

#[tauri::command]
fn pending_requests(shared: tauri::State<'_, std::sync::Arc<Shared>>) -> Vec<PendingView> {
    transfers::pending_views(&shared)
}

#[tauri::command]
fn respond(shared: tauri::State<'_, std::sync::Arc<Shared>>, id: u64, accept: bool) -> bool {
    let list = shared.pending.lock().unwrap();
    for p in list.iter() {
        let (lock, cvar) = &**p;
        let mut inner = lock.lock().unwrap();
        if inner.id == id && inner.decision.is_none() {
            inner.decision = Some(accept);
            cvar.notify_all();
            return true;
        }
    }
    false
}

#[derive(serde::Serialize)]
struct PathInfo {
    path: String,
    name: String,
    size: u64,
}

#[tauri::command]
fn inspect_paths(paths: Vec<String>) -> Vec<PathInfo> {
    paths
        .into_iter()
        .filter_map(|p| {
            let path = std::path::PathBuf::from(&p);
            if !path.is_file() {
                return None;
            }
            let name = path.file_name()?.to_str()?.to_string();
            let size = std::fs::metadata(&path).ok()?.len();
            Some(PathInfo { path: p, name, size })
        })
        .collect()
}

#[tauri::command]
fn send_files(
    shared: tauri::State<'_, std::sync::Arc<Shared>>,
    addr: String,
    paths: Vec<String>,
) -> Result<u64, String> {
    let addr = addr.trim().to_string();
    if addr.is_empty() || addr.len() > 253 || !addr.chars().all(|c| c.is_ascii_alphanumeric() || ".:-_".contains(c)) {
        return Err("invalid peer address".into());
    }
    transfers::start_send(&shared, &addr, paths)
}

#[tauri::command]
fn transfers(shared: tauri::State<'_, std::sync::Arc<Shared>>) -> Vec<TransferView> {
    transfers::transfer_views(&shared)
}

#[tauri::command]
fn cancel_transfer(shared: tauri::State<'_, std::sync::Arc<Shared>>, id: u64) -> bool {
    shared.cancel.lock().unwrap().insert(id);
    // Also reject it if it is still waiting for approval.
    let list = shared.pending.lock().unwrap();
    for p in list.iter() {
        let (lock, cvar) = &**p;
        let mut inner = lock.lock().unwrap();
        if inner.id == id && inner.decision.is_none() {
            inner.decision = Some(false);
            cvar.notify_all();
        }
    }
    true
}

#[tauri::command]
fn clear_transfers(shared: tauri::State<'_, std::sync::Arc<Shared>>) {
    let mut map = shared.transfers.lock().unwrap();
    map.retain(|_, t| {
        let s = t.status.lock().unwrap().clone();
        s == "in_progress" || s == "waiting"
    });
    let mut cancel = shared.cancel.lock().unwrap();
    let live: std::collections::HashSet<u64> = map.keys().copied().collect();
    cancel.retain(|id| live.contains(id));
}

#[tauri::command]
fn history(shared: tauri::State<'_, std::sync::Arc<Shared>>) -> Vec<HistoryEntry> {
    shared.store.lock().unwrap().state.history.clone()
}

#[tauri::command]
fn clear_history(shared: tauri::State<'_, std::sync::Arc<Shared>>) {
    let mut store = shared.store.lock().unwrap();
    store.state.history.clear();
    let _ = store.save();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("nearsend"));
            std::fs::create_dir_all(&dir).ok();
            let store = Store::load(&dir);
            let shared = Shared::new(store);
            transfers::start_background_threads(std::sync::Arc::clone(&shared));
            app.manage(shared);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            inspect_paths,
            set_device_name,
            set_save_dir,
            peers,
            add_peer_code,
            pending_requests,
            respond,
            send_files,
            transfers,
            cancel_transfer,
            clear_transfers,
            history,
            clear_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running NearSend");
}
