//! Manual server storage and injection.
//!
//! Handles user-added SMB servers: address parsing, TCP reachability checks,
//! persistence to `manual-servers.json`, and injection into the discovery state.

use crate::network::{HostSource, NetworkHost, on_host_found, on_host_lost};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Runtime};

const DEFAULT_SMB_PORT: u16 = 445;
const REACHABILITY_TIMEOUT_SECS: u64 = 5;
const MANUAL_SERVERS_FILENAME: &str = "manual-servers.json";

/// Protects the read-modify-write cycle on `manual-servers.json`.
/// Without this, concurrent `add_manual_server` / `remove_manual_server` calls
/// can read the same on-disk state and one write clobbers the other.
static STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn get_store_lock() -> &'static Mutex<()> {
    STORE_LOCK.get_or_init(|| Mutex::new(()))
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A parsed server address from user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAddress {
    pub host: String,
    pub port: u16,
    pub share_path: Option<String>,
}

/// Error from parsing a server address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    UnsupportedProtocol(String),
    Ipv6NotSupported,
    InvalidPort(String),
    Malformed(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Empty => write!(f, "Enter a server address"),
            ParseError::UnsupportedProtocol(proto) => {
                write!(f, "Only SMB shares are supported right now (got {}://)", proto)
            }
            ParseError::Ipv6NotSupported => {
                write!(
                    f,
                    "IPv6 addresses aren't supported yet. Use an IPv4 address or hostname."
                )
            }
            ParseError::InvalidPort(msg) => write!(f, "{}", msg),
            ParseError::Malformed(msg) => write!(f, "{}", msg),
        }
    }
}

/// A persisted manual server entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualServerEntry {
    pub id: String,
    pub display_name: String,
    pub address: String,
    pub port: u16,
    pub added_at: String,
}

/// The on-disk store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManualServersStore {
    #[serde(default)]
    servers: Vec<ManualServerEntry>,
}

/// Result of successfully adding a manual server.
///
/// Only serialized (Rust → frontend); no `Deserialize` needed.
/// `NetworkHost` has no `Deserialize` either (prevents specta type-split issues).
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ManualConnectResult {
    pub host: NetworkHost,
    pub share_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Address parsing
// ---------------------------------------------------------------------------

/// Parses user input into a structured address.
///
/// Accepts bare hostnames/IPs, host:port, and `smb://` URLs.
/// Rejects unsupported protocols and IPv6.
pub fn parse_server_address(input: &str) -> Result<ParsedAddress, ParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseError::Empty);
    }

    // Check for unsupported protocols
    if let Some(proto) = extract_protocol(trimmed) {
        let proto_lower = proto.to_lowercase();
        if proto_lower != "smb" {
            return Err(ParseError::UnsupportedProtocol(proto_lower));
        }
        return parse_smb_url(trimmed);
    }

    // Check for IPv6 (contains colons that aren't a single host:port separator, or starts with [)
    if trimmed.starts_with('[') {
        return Err(ParseError::Ipv6NotSupported);
    }

    // Count colons to distinguish IPv6 from host:port
    let colon_count = trimmed.chars().filter(|c| *c == ':').count();
    if colon_count > 1 {
        return Err(ParseError::Ipv6NotSupported);
    }

    // Bare host or host:port
    if colon_count == 1 {
        let (host_part, port_str) = trimmed.split_once(':').expect("colon exists");
        let host = host_part.trim().to_string();
        if host.is_empty() {
            return Err(ParseError::Malformed(
                "Couldn't parse this address. Try a hostname, IP, or smb:// URL.".to_string(),
            ));
        }
        let port = parse_port(port_str.trim())?;
        validate_host(&host)?;
        Ok(ParsedAddress {
            host,
            port,
            share_path: None,
        })
    } else {
        let host = trimmed.to_string();
        validate_host(&host)?;
        Ok(ParsedAddress {
            host,
            port: DEFAULT_SMB_PORT,
            share_path: None,
        })
    }
}

/// Extracts the protocol prefix (before `://`) if present.
fn extract_protocol(input: &str) -> Option<String> {
    let lower = input.to_lowercase();
    if let Some(idx) = lower.find("://") {
        let proto = &input[..idx];
        // Only consider it a protocol if it's all alphabetic
        if proto.chars().all(|c| c.is_ascii_alphabetic()) {
            return Some(proto.to_string());
        }
    }
    None
}

/// Parses an `smb://` URL.
fn parse_smb_url(input: &str) -> Result<ParsedAddress, ParseError> {
    // Strip the scheme. The caller only routes here after `extract_protocol`
    // matched a `smb://` prefix, so `://` is present; fall back to a Malformed
    // error rather than panicking if that ever stops holding.
    let scheme_end = input.find("://").ok_or_else(|| {
        ParseError::Malformed("Couldn't parse this address. Try a hostname, IP, or smb:// URL.".to_string())
    })?;
    let after_scheme = &input[scheme_end + 3..];

    if after_scheme.is_empty() {
        return Err(ParseError::Malformed(
            "Couldn't parse this address. Try a hostname, IP, or smb:// URL.".to_string(),
        ));
    }

    // Strip user info (user@ or user:pass@)
    let after_userinfo = if let Some(at_idx) = after_scheme.find('@') {
        // Only treat @ as userinfo separator if it's before the first /
        let slash_idx = after_scheme.find('/').unwrap_or(after_scheme.len());
        if at_idx < slash_idx {
            &after_scheme[at_idx + 1..]
        } else {
            after_scheme
        }
    } else {
        after_scheme
    };

    // Split host:port from path
    let (host_port, path) = if let Some(slash_idx) = after_userinfo.find('/') {
        let path_part = &after_userinfo[slash_idx + 1..];
        let share_path = if path_part.is_empty() {
            None
        } else {
            Some(path_part.trim_end_matches('/').to_string())
        };
        (&after_userinfo[..slash_idx], share_path)
    } else {
        (after_userinfo, None)
    };

    // Parse host and optional port
    let (host, port) = if let Some(colon_idx) = host_port.rfind(':') {
        let host_part = &host_port[..colon_idx];
        let port_str = &host_port[colon_idx + 1..];
        if port_str.is_empty() {
            (host_part.to_string(), DEFAULT_SMB_PORT)
        } else {
            (host_part.to_string(), parse_port(port_str)?)
        }
    } else {
        (host_port.to_string(), DEFAULT_SMB_PORT)
    };

    if host.is_empty() {
        return Err(ParseError::Malformed(
            "Couldn't parse this address. Try a hostname, IP, or smb:// URL.".to_string(),
        ));
    }

    validate_host(&host)?;

    Ok(ParsedAddress {
        host,
        port,
        share_path: path,
    })
}

/// Validates a host string (rejects IPv6, empty, obviously invalid).
fn validate_host(host: &str) -> Result<(), ParseError> {
    if host.is_empty() {
        return Err(ParseError::Malformed(
            "Couldn't parse this address. Try a hostname, IP, or smb:// URL.".to_string(),
        ));
    }
    // If it looks like an IPv6 address
    if host.contains(':') || host.starts_with('[') {
        return Err(ParseError::Ipv6NotSupported);
    }
    // Basic character validation: alphanumeric, dots, dashes
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(ParseError::Malformed(
            "Couldn't parse this address. Try a hostname, IP, or smb:// URL.".to_string(),
        ));
    }
    Ok(())
}

/// Parses and validates a port string.
fn parse_port(s: &str) -> Result<u16, ParseError> {
    match s.parse::<u32>() {
        Ok(p) if (1..=65535).contains(&p) => Ok(p as u16),
        Ok(_) => Err(ParseError::InvalidPort("Port must be between 1 and 65535".to_string())),
        Err(_) => Err(ParseError::InvalidPort("Port must be between 1 and 65535".to_string())),
    }
}

// ---------------------------------------------------------------------------
// ID generation
// ---------------------------------------------------------------------------

/// Generates a deterministic ID for a manual server.
///
/// Format: `manual-{address}-{port}` with dots/colons replaced by dashes.
pub fn generate_server_id(address: &str, port: u16) -> String {
    let sanitized = address.replace(['.', ':'], "-");
    format!("manual-{}-{}", sanitized, port)
}

// ---------------------------------------------------------------------------
// Display name
// ---------------------------------------------------------------------------

/// Generates a display name for a manual server.
///
/// Bare address for default port, address:port for non-default.
fn display_name(address: &str, port: u16) -> String {
    if port == DEFAULT_SMB_PORT {
        address.to_string()
    } else {
        format!("{}:{}", address, port)
    }
}

// ---------------------------------------------------------------------------
// NetworkHost mapping
// ---------------------------------------------------------------------------

/// Whether the address looks like an IP address.
fn is_ip_address(host: &str) -> bool {
    host.parse::<IpAddr>().is_ok()
}

/// Creates a `NetworkHost` from parsed address info.
pub fn create_network_host(address: &str, port: u16) -> NetworkHost {
    let id = generate_server_id(address, port);
    let name = display_name(address, port);
    let is_ip = is_ip_address(address);

    NetworkHost {
        id,
        name,
        // hostname is always set so the share listing pipeline picks it up
        hostname: Some(address.to_string()),
        ip_address: if is_ip { Some(address.to_string()) } else { None },
        port,
        source: HostSource::Manual,
    }
}

// ---------------------------------------------------------------------------
// TCP reachability
// ---------------------------------------------------------------------------

/// Checks that the host:port is reachable via TCP with a timeout.
pub async fn check_reachability(host: &str, port: u16) -> Result<(), String> {
    use tokio::net::TcpStream;
    use tokio::time::{Duration, timeout};

    let addr = format!("{}:{}", host, port);
    debug!("Checking TCP reachability: {}", addr);

    // Try to resolve + connect. For hostnames, tokio::net::TcpStream::connect
    // does DNS resolution internally.
    match timeout(
        Duration::from_secs(REACHABILITY_TIMEOUT_SECS),
        TcpStream::connect(&addr),
    )
    .await
    {
        Ok(Ok(_stream)) => {
            debug!("Reachable: {}", addr);
            Ok(())
        }
        Ok(Err(e)) => {
            debug!("Unreachable: {} ({})", addr, e);
            Err(format!("Couldn't reach {}: {}", addr, e))
        }
        Err(_) => {
            debug!("Timed out connecting to {}", addr);
            Err(format!(
                "Couldn't reach {}: connection timed out after {}s",
                addr, REACHABILITY_TIMEOUT_SECS
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Atomic file writes
// ---------------------------------------------------------------------------

/// Durably writes content to a file using write-to-temp + fsync + rename + parent-dir fsync.
/// On failure, the original file (if any) remains intact. The fsyncs make the write survive a
/// power loss, not just process death: `manual-servers.json` holds user-entered SMB servers that
/// aren't rediscoverable via mDNS, so a torn / zero-length write here loses real config. See
/// `crate::config::durable_write_json` for the durability rationale.
fn atomic_write_json(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    crate::config::durable_write_json(path, &tmp, content)
}

/// Removes a stale `.tmp` file left over from a crash during atomic write.
fn cleanup_tmp_file(path: &Path) {
    let tmp = path.with_extension("json.tmp");
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Returns the path to the manual servers store file.
fn get_store_path<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    crate::config::resolved_app_data_dir(app)
        .ok()
        .map(|dir| dir.join(MANUAL_SERVERS_FILENAME))
}

/// Every manually-typed SMB server, as the store holds it.
///
/// ❗ Reads the FILE. There is no in-memory mirror here (the discovery host map
/// is where a loaded entry ends up, and that map is about what's reachable
/// rather than what's saved), so a caller on a hot path wants to ask once.
pub fn all<R: Runtime>(app: &AppHandle<R>) -> Vec<ManualServerEntry> {
    read_store(app).servers
}

/// Reads the store from a path on disk.
fn read_store_from_path(path: &Path) -> ManualServersStore {
    cleanup_tmp_file(path);

    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => ManualServersStore::default(),
    }
}

/// Writes the store to a path on disk.
fn write_store_to_path(path: &Path, store: &ManualServersStore) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    match serde_json::to_string_pretty(store) {
        Ok(json) => {
            if let Err(e) = atomic_write_json(path, &json) {
                warn!("Couldn't write manual servers store: {}", e);
            }
        }
        Err(e) => warn!("Couldn't serialize manual servers store: {}", e),
    }
}

/// Loads the store from disk.
fn read_store<R: Runtime>(app: &AppHandle<R>) -> ManualServersStore {
    let Some(path) = get_store_path(app) else {
        return ManualServersStore::default();
    };
    read_store_from_path(&path)
}

/// Adds a server entry to the store file at the given path, protected by `STORE_LOCK`.
/// Extracted so it can be tested without an `AppHandle`.
fn add_server_entry_to_path(path: &Path, entry: ManualServerEntry) {
    let _guard = get_store_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut store = read_store_from_path(path);
    if let Some(existing) = store.servers.iter_mut().find(|s| s.id == entry.id) {
        *existing = entry;
    } else {
        store.servers.push(entry);
    }
    write_store_to_path(path, &store);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Adds a manual server: parses input, checks reachability, persists, and injects into discovery
/// state.
pub async fn add_manual_server<R: Runtime>(
    input: &str,
    app_handle: &AppHandle<R>,
) -> Result<ManualConnectResult, String> {
    let parsed = parse_server_address(input).map_err(|e| e.to_string())?;

    // Check TCP reachability
    check_reachability(&parsed.host, parsed.port).await?;

    // Build the network host
    let host = create_network_host(&parsed.host, parsed.port);

    // Persist to disk
    if let Some(path) = get_store_path(app_handle) {
        let entry = ManualServerEntry {
            id: host.id.clone(),
            display_name: host.name.clone(),
            address: parsed.host.clone(),
            port: parsed.port,
            added_at: chrono::Utc::now().to_rfc3339(),
        };
        add_server_entry_to_path(&path, entry);
    }

    info!("Added manual server: {} (id={})", host.name, host.id);

    // Inject into discovery state
    on_host_found(host.clone(), app_handle);

    Ok(ManualConnectResult {
        host,
        share_path: parsed.share_path,
    })
}

/// Removes a server entry by ID from the store file at the given path, protected by `STORE_LOCK`.
/// Returns `true` if the server was found and removed, `false` if not found.
fn remove_server_entry_from_path(path: &Path, server_id: &str) -> bool {
    let _guard = get_store_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut store = read_store_from_path(path);
    let original_len = store.servers.len();
    store.servers.retain(|s| s.id != server_id);
    if store.servers.len() == original_len {
        return false;
    }
    write_store_to_path(path, &store);
    true
}

/// Removes a manual server by ID from storage and discovery state.
pub fn remove_manual_server<R: Runtime>(server_id: &str, app_handle: &AppHandle<R>) -> Result<(), String> {
    let Some(path) = get_store_path(app_handle) else {
        return Err(format!("Server '{}' not found", server_id));
    };

    if !remove_server_entry_from_path(&path, server_id) {
        return Err(format!("Server '{}' not found", server_id));
    }

    // Remove from discovery state and notify frontend
    on_host_lost(server_id, app_handle);

    info!("Removed manual server: {}", server_id);
    Ok(())
}

/// Loads persisted manual servers and injects them into discovery state.
///
/// Called at startup, before the frontend subscribes to events.
pub fn load_manual_servers<R: Runtime>(app_handle: &AppHandle<R>) {
    let store = read_store(app_handle);

    if store.servers.is_empty() {
        return;
    }

    info!("Loading {} persisted manual server(s)", store.servers.len());

    for entry in &store.servers {
        let host = create_network_host(&entry.address, entry.port);
        on_host_found(host, app_handle);
        debug!("Loaded manual server: {} (id={})", entry.display_name, entry.id);
    }
}

#[cfg(test)]
#[path = "manual_servers_test.rs"]
mod tests;
