// UDP presence beacon + TCP transfer engine. std networking, no async runtime.
use crate::proto::{self, FileSpec, Header, Presence, MAGIC, TCP_PORT, UDP_PORT};
use crate::store::{HistoryEntry, Store};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Shared {
    pub store: Mutex<Store>,
    pub peers: Mutex<HashMap<String, PeerInfo>>,
    pub pending: Mutex<Vec<Arc<(Mutex<PendingInner>, Condvar)>>>,
    pub transfers: Mutex<HashMap<u64, Arc<TransferShared>>>,
    pub cancel: Mutex<HashSet<u64>>,
    pub listener_status: Mutex<String>,
    pub id_counter: AtomicU64,
    pub local_ips: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PeerInfo {
    pub device: String,
    pub addr: String, // ip:port
    pub last_seen: u64,
}

#[derive(serde::Serialize)]
pub struct PendingView {
    pub id: u64,
    pub device: String,
    pub addr: String,
    pub files: Vec<FileSpec>,
    pub total_bytes: u64,
}

#[derive(serde::Serialize)]
pub struct TransferView {
    pub id: u64,
    pub direction: String,
    pub peer: String,
    pub file_names: Vec<String>,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub status: String,
    pub message: String,
}

pub struct PendingInner {
    pub id: u64,
    pub device: String,
    pub addr: String,
    pub files: Vec<FileSpec>,
    pub total_bytes: u64,
    pub stream: Option<TcpStream>,
    pub decision: Option<bool>,
}

pub struct TransferShared {
    pub id: u64,
    pub direction: String,
    pub peer: String,
    pub file_names: Vec<String>,
    pub total_bytes: u64,
    pub done_bytes: AtomicU64,
    pub status: Mutex<String>,
    pub message: Mutex<String>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn detect_local_ips() -> Vec<String> {
    let mut ips = vec!["127.0.0.1".to_string()];
    // Connecting a UDP socket sends no packets; it just picks the egress interface.
    if let Ok(s) = UdpSocket::bind("0.0.0.0:0") {
        if s.connect("8.8.8.8:80").is_ok() {
            if let Ok(a) = s.local_addr() {
                ips.push(a.ip().to_string());
            }
        }
    }
    if let Ok(host) = std::env::var("HOSTNAME").or_else(|_| std::env::var("COMPUTERNAME")) {
        if let Ok(addrs) = format!("{host}:0").to_socket_addrs() {
            for a in addrs {
                let ip = a.ip().to_string();
                if !ips.contains(&ip) {
                    ips.push(ip);
                }
            }
        }
    }
    ips
}

pub fn default_save_dir() -> PathBuf {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into());
    let downloads = PathBuf::from(&home).join("Downloads");
    if downloads.is_dir() { downloads } else { PathBuf::from(home) }
}

pub fn device_name_default() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .map(|h| proto::sanitize_display_name(&h, 64))
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "My device".into())
}

impl Shared {
    pub fn new(store: Store) -> Arc<Self> {
        Arc::new(Shared {
            store: Mutex::new(store),
            peers: Mutex::new(HashMap::new()),
            pending: Mutex::new(Vec::new()),
            transfers: Mutex::new(HashMap::new()),
            cancel: Mutex::new(HashSet::new()),
            listener_status: Mutex::new("starting".into()),
            id_counter: AtomicU64::new(now_secs().saturating_mul(1000)),
            local_ips: detect_local_ips(),
        })
    }

    pub fn next_id(&self) -> u64 {
        self.id_counter.fetch_add(1, Ordering::Relaxed)
    }

    pub fn save_dir(&self) -> PathBuf {
        let store = self.store.lock().unwrap();
        store
            .state
            .save_dir
            .clone()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(default_save_dir)
    }

    pub fn device_name(&self) -> String {
        let store = self.store.lock().unwrap();
        store.state.device_name.clone().unwrap_or_else(device_name_default)
    }

    pub fn record_history(&self, direction: &str, peer: &str, files: &[String], bytes: u64, status: &str) {
        let mut store = self.store.lock().unwrap();
        store.record(HistoryEntry {
            ts: now_secs(),
            direction: direction.into(),
            peer: peer.into(),
            files: files.to_vec(),
            bytes,
            status: status.into(),
        });
    }
}

fn transfer_view(t: &TransferShared) -> TransferView {
    TransferView {
        id: t.id,
        direction: t.direction.clone(),
        peer: t.peer.clone(),
        file_names: t.file_names.clone(),
        total_bytes: t.total_bytes,
        done_bytes: t.done_bytes.load(Ordering::Relaxed),
        status: t.status.lock().unwrap().clone(),
        message: t.message.lock().unwrap().clone(),
    }
}

pub fn transfer_views(shared: &Shared) -> Vec<TransferView> {
    let map = shared.transfers.lock().unwrap();
    let mut v: Vec<_> = map.values().map(|t| transfer_view(t)).collect();
    v.sort_by(|a, b| b.id.cmp(&a.id));
    v
}

pub fn pending_views(shared: &Shared) -> Vec<PendingView> {
    let list = shared.pending.lock().unwrap();
    list.iter()
        .map(|p| {
            let inner = p.0.lock().unwrap();
            PendingView {
                id: inner.id,
                device: inner.device.clone(),
                addr: inner.addr.clone(),
                files: inner.files.clone(),
                total_bytes: inner.total_bytes,
            }
        })
        .collect()
}

pub fn peer_views(shared: &Shared) -> Vec<PeerInfo> {
    let cutoff = now_secs().saturating_sub(30);
    let mut map = shared.peers.lock().unwrap();
    map.retain(|_, p| p.last_seen >= cutoff);
    let mut v: Vec<_> = map.values().cloned().collect();
    v.sort_by(|a, b| a.device.to_lowercase().cmp(&b.device.to_lowercase()));
    v
}

/// Start UDP beacon + TCP listener. Both run forever in background threads.
pub fn start_background_threads(shared: Arc<Shared>) {
    // Presence beacon (send)
    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let sock = match UdpSocket::bind("0.0.0.0:0").and_then(|s| {
                s.set_broadcast(true)?;
                Ok(s)
            }) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("nearsend: beacon disabled: {e}");
                    return;
                }
            };
            let dest: std::net::SocketAddr = (std::net::Ipv4Addr::BROADCAST, UDP_PORT).into();
            loop {
                let name = shared.device_name();
                let presence = Presence { magic: MAGIC.into(), device: name, port: TCP_PORT };
                if let Ok(bytes) = serde_json::to_vec(&presence) {
                    let _ = sock.send_to(&bytes, dest);
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }
    // Presence receiver
    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let sock = match UdpSocket::bind(("0.0.0.0", UDP_PORT)) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("nearsend: presence receiver disabled: {e}");
                    return;
                }
            };
            let mut buf = [0u8; 2048];
            loop {
                if let Ok((n, from)) = sock.recv_from(&mut buf) {
                    let ip = from.ip().to_string();
                    if shared.local_ips.contains(&ip) {
                        continue; // ignore our own beacon
                    }
                    if let Some((device, port)) = proto::parse_presence(&buf[..n]) {
                        shared.peers.lock().unwrap().insert(
                            ip.clone(),
                            PeerInfo { device, addr: format!("{ip}:{port}"), last_seen: now_secs() },
                        );
                    }
                }
            }
        });
    }
    // Incoming transfer listener
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(("0.0.0.0", TCP_PORT)) {
            Ok(l) => l,
            Err(e) => {
                *shared.listener_status.lock().unwrap() = format!("port {TCP_PORT} busy: {e}");
                return;
            }
        };
        *shared.listener_status.lock().unwrap() = format!("listening on {TCP_PORT}");
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    let shared = Arc::clone(&shared);
                    std::thread::spawn(move || handle_incoming(shared, stream));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    });
}

fn handle_incoming(shared: Arc<Shared>, stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_else(|_| "unknown".into());
    let ip_only = peer.rsplit_once(':').map(|(i, _)| i.to_string()).unwrap_or(peer.clone());

    // Read the header line (cap HEADER_MAX bytes).
    let mut limited = stream.try_clone().expect("clone stream").take((proto::HEADER_MAX + 1) as u64);
    let mut header_bytes = Vec::new();
    let mut byte = [0u8; 1];
    let mut too_big = false;
    while limited.read(&mut byte).unwrap_or(0) == 1 {
        if byte[0] == b'\n' { break; }
        header_bytes.push(byte[0]);
        if header_bytes.len() > proto::HEADER_MAX { too_big = true; break; }
    }
    if too_big { return; }
    let header = match proto::parse_header_line(&header_bytes) {
        Ok(h) => h,
        Err(_) => {
            let mut s = stream;
            let _ = s.write_all(b"R");
            return;
        }
    };

    let id = shared.next_id();
    let total_bytes: u64 = header.files.iter().map(|f| f.size).sum();
    let file_names: Vec<String> = header.files.iter().map(|f| f.name.clone()).collect();
    let pending = Arc::new((
        Mutex::new(PendingInner {
            id,
            device: header.device.clone(),
            addr: ip_only.clone(),
            files: header.files.clone(),
            total_bytes,
            stream: Some(stream),
            decision: None,
        }),
        Condvar::new(),
    ));
    shared.pending.lock().unwrap().push(Arc::clone(&pending));

    // Mirror into transfers for progress visibility (starts at 0%).
    let shared_transfer = Arc::new(TransferShared {
        id,
        direction: "received".into(),
        peer: header.device.clone(),
        file_names: file_names.clone(),
        total_bytes,
        done_bytes: AtomicU64::new(0),
        status: Mutex::new("waiting".into()),
        message: Mutex::new("waiting for your approval".into()),
    });
    shared.transfers.lock().unwrap().insert(id, Arc::clone(&shared_transfer));

    // Wait for the user's decision (120 s).
    let deadline = Instant::now() + Duration::from_secs(120);
    let accepted = {
        let (lock, cvar) = &*pending;
        let mut inner = lock.lock().unwrap();
        while inner.decision.is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                inner.decision = Some(false);
                break;
            }
            let (guard, _) = cvar.wait_timeout(inner, remaining).unwrap();
            inner = guard;
        }
        inner.decision.unwrap_or(false)
    };
    // Remove from pending list.
    {
        let mut list = shared.pending.lock().unwrap();
        list.retain(|p| p.0.lock().unwrap().id != id);
    }
    let mut inner = pending.0.lock().unwrap();
    let mut stream = match inner.stream.take() {
        Some(s) => s,
        None => return,
    };
    if !accepted {
        let _ = stream.write_all(b"R");
        let mut st = shared_transfer.status.lock().unwrap();
        *st = "rejected".into();
        *shared_transfer.message.lock().unwrap() = "you declined the transfer".into();
        drop(st);
        shared.record_history("received", &header.device, &file_names, 0, "rejected");
        return;
    }
    let _ = stream.write_all(b"A");

    // Receive files into a unique path inside the configured save dir.
    let save_dir = shared.save_dir();
    *shared_transfer.status.lock().unwrap() = "in_progress".into();
    let mut remaining_cancel = false;
    'files: for spec in &header.files {
        let target = match proto::unique_path(&save_dir, &spec.name) {
            Ok(p) => p,
            Err(e) => {
                *shared_transfer.status.lock().unwrap() = "failed".into();
                *shared_transfer.message.lock().unwrap() = e;
                break 'files;
            }
        };
        let mut out = match std::fs::File::create(&target) {
            Ok(f) => f,
            Err(e) => {
                *shared_transfer.status.lock().unwrap() = "failed".into();
                *shared_transfer.message.lock().unwrap() = format!("save to {}: {e}", target.display());
                break 'files;
            }
        };
        let mut hasher = Sha256::new();
        let mut left = spec.size;
        let mut buf = vec![0u8; 1024 * 1024];
        let mut file_failed = false;
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            match stream.read_exact(&mut buf[..want]) {
                Ok(()) => {
                    if out.write_all(&buf[..want]).is_err() { file_failed = true; break; }
                    hasher.update(&buf[..want]);
                    left -= want as u64;
                    shared_transfer.done_bytes.fetch_add(want as u64, Ordering::Relaxed);
                }
                Err(_) => { file_failed = true; break; }
            }
            if shared.cancel.lock().unwrap().contains(&id) {
                remaining_cancel = true;
                file_failed = true;
                break;
            }
        }
        drop(out);
        if file_failed {
            let _ = std::fs::remove_file(&target); // delete partial file
            if remaining_cancel {
                *shared_transfer.status.lock().unwrap() = "cancelled".into();
                *shared_transfer.message.lock().unwrap() = "transfer cancelled by you".into();
            } else {
                *shared_transfer.status.lock().unwrap() = "failed".into();
                *shared_transfer.message.lock().unwrap() = format!("connection lost while receiving {}", spec.name);
            }
            break 'files;
        }
        let digest = format!("{:x}", hasher.finalize());
        if !digest.eq_ignore_ascii_case(&spec.sha256) {
            let _ = std::fs::remove_file(&target);
            *shared_transfer.status.lock().unwrap() = "failed".into();
            *shared_transfer.message.lock().unwrap() = format!("checksum mismatch: {}", spec.name);
            break 'files;
        }
    }
    let final_status = shared_transfer.status.lock().unwrap().clone();
    if final_status == "in_progress" || final_status == "waiting" {
        *shared_transfer.status.lock().unwrap() = "done".into();
        *shared_transfer.message.lock().unwrap() = "received".into();
    }
    let bytes = shared_transfer.done_bytes.load(Ordering::Relaxed);
    shared.record_history(
        "received",
        &header.device,
        &file_names,
        bytes,
        &shared_transfer.status.lock().unwrap(),
    );
}

/// Kick off a send in a background thread. Returns the transfer id immediately.
pub fn start_send(shared: &Arc<Shared>, peer_addr: &str, paths: Vec<String>) -> Result<u64, String> {
    if paths.is_empty() {
        return Err("no files selected".into());
    }
    let mut files = Vec::with_capacity(paths.len());
    let mut total: u64 = 0;
    for p in &paths {
        let path = PathBuf::from(p);
        if !path.is_file() {
            return Err(format!("not a file: {p}"));
        }
        let name = proto::sanitize_file_name(
            path.file_name().and_then(|n| n.to_str()).ok_or("unreadable file name")?,
        )?;
        let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        total = total.saturating_add(size);
        files.push((path, name, size));
    }
    if files.len() > proto::MAX_FILES {
        return Err(format!("too many files (max {})", proto::MAX_FILES));
    }
    if total > proto::MAX_TOTAL_BYTES {
        return Err("transfer too large (max 4 GiB)".into());
    }
    // Hash everything up front so the header carries checksums.
    let mut specs = Vec::with_capacity(files.len());
    for (path, name, size) in &files {
        let sha256 = sha256_file(path).map_err(|e| format!("hash {}: {e}", path.display()))?;
        specs.push(FileSpec { name: name.clone(), size: *size, sha256 });
    }

    let id = shared.next_id();
    let file_names: Vec<String> = specs.iter().map(|f| f.name.clone()).collect();
    let shared_transfer = Arc::new(TransferShared {
        id,
        direction: "sent".into(),
        peer: peer_addr.to_string(),
        file_names: file_names.clone(),
        total_bytes: total,
        done_bytes: AtomicU64::new(0),
        status: Mutex::new("in_progress".into()),
        message: Mutex::new("connecting…".into()),
    });
    shared.transfers.lock().unwrap().insert(id, Arc::clone(&shared_transfer));

    let device = shared.device_name();
    let header = Header { magic: MAGIC.into(), device, files: specs.clone() };
    let peer_addr = peer_addr.to_string();
    let shared = Arc::clone(shared);
    let paths: Vec<PathBuf> = files.into_iter().map(|(p, _, _)| p).collect();

    std::thread::spawn(move || {
        let result = send_blocking(&shared, id, &peer_addr, &header, &paths, &shared_transfer);
        let status = match &result {
            Ok(()) => "done".to_string(),
            Err(msg) => {
                if msg == "cancelled" { "cancelled".into() } else { "failed".to_string() }
            }
        };
        *shared_transfer.status.lock().unwrap() = status.clone();
        *shared_transfer.message.lock().unwrap() = match &result {
            Ok(()) => "sent".into(),
            Err(msg) => msg.clone(),
        };
        let bytes = shared_transfer.done_bytes.load(Ordering::Relaxed);
        shared.record_history("sent", &peer_addr, &file_names, bytes, &status);
    });
    Ok(id)
}

fn send_blocking(
    shared: &Shared,
    id: u64,
    peer_addr: &str,
    header: &Header,
    paths: &[PathBuf],
    transfer: &Arc<TransferShared>,
) -> Result<(), String> {
    let addr = peer_addr
        .to_socket_addrs()
        .map_err(|e| format!("resolve {peer_addr}: {e}"))?
        .next()
        .ok_or_else(|| format!("cannot resolve {peer_addr}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("connect: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));

    let mut header_bytes = serde_json::to_vec(header).map_err(|e| e.to_string())?;
    header_bytes.push(b'\n');
    stream.write_all(&header_bytes).map_err(|e| format!("send header: {e}"))?;
    *transfer.message.lock().unwrap() = "waiting for their approval…".into();

    let mut resp = [0u8; 1];
    stream.read_exact(&mut resp).map_err(|e| format!("no response ({e})"))?;
    if resp[0] != b'A' {
        return Err("they declined the transfer".into());
    }
    *transfer.message.lock().unwrap() = "sending…".into();

    for (i, path) in paths.iter().enumerate() {
        let spec = &header.files[i];
        let mut file = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let mut remaining = spec.size;
        let mut buf = vec![0u8; 1024 * 1024];
        while remaining > 0 {
            if shared.cancel.lock().unwrap().contains(&id) {
                return Err("cancelled".into());
            }
            let want = remaining.min(buf.len() as u64) as usize;
            let n = file.read(&mut buf[..want]).map_err(|e| e.to_string())?;
            if n == 0 { break; }
            stream.write_all(&buf[..n]).map_err(|e| format!("network write: {e}"))?;
            remaining -= n as u64;
            transfer.done_bytes.fetch_add(n as u64, Ordering::Relaxed);
        }
    }
    Ok(())
}
