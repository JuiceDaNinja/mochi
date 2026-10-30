//! The desktop side of the phone link.
//!
//! One WebSocket server on the LAN, advertised with mDNS as `_mochi._tcp`. A new phone
//! pairs with an X25519 key exchange; both screens show a 6-digit code derived from the
//! shared secret and **both** must approve. Later connections authenticate with an HMAC
//! over a fresh nonce. The link stays open until the network drops; pings every 5 s,
//! three misses close the socket and the phone reconnects on its own.
//!
//! `PhoneCrypto` is the client half (used by tests and mirrored in the Android app).

pub mod crypto;
mod devices;

pub use crypto::PhoneCrypto;
pub use devices::{DeviceStore, PairedDevice};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use mochi_core::protocol::*;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
/// Mochi's usual port. The phone also tries this first when a saved address is stale.
pub const DEFAULT_PORT: u16 = 47321;

#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("mdns: {0}")]
    Mdns(String),
}

#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// Shown on the phone while pairing. Defaults to the hostname.
    pub desktop_name: String,
    /// Preferred port; 0 picks a free one. If the preferred port is taken, a free one is used.
    pub port: u16,
    /// Where paired devices are stored (JSON).
    pub devices_path: PathBuf,
    pub advertise: bool,
    pub ping_interval: Duration,
    pub pairing_timeout: Duration,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            desktop_name: hostname::get().map(|h| h.to_string_lossy().to_string()).unwrap_or_else(|_| "Mochi desktop".into()),
            port: DEFAULT_PORT,
            devices_path: PathBuf::from("devices.json"),
            advertise: true,
            ping_interval: Duration::from_secs(PING_INTERVAL_SECS),
            pairing_timeout: Duration::from_secs(90),
        }
    }
}

/// What the app hears from the link.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkEvent {
    /// A new phone wants to pair. Show `code` and Approve/Deny; answer with `LinkHandle::approve`.
    PairRequest { session: u64, device_name: String, code: String },
    /// One side approved; the other is still pending. For logs and a progress hint.
    PairProgress { session: u64, phone_approved: bool, desktop_approved: bool },
    Paired { device_id: String, device_name: String },
    PairFailed { session: u64, reason: String },
    Connected { device_id: String, device_name: String },
    Disconnected { device_id: String },
    Wake,
    Utterance { text: String, is_final: bool },
    UserSpeaking,
    Answer { yes: bool },
    Stop,
}

/// The app's side: send to the phone, answer pairing prompts, manage devices.
#[derive(Clone)]
pub struct LinkHandle {
    out: broadcast::Sender<DesktopMessage>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<bool>>>>,
    devices: Arc<DeviceStore>,
    connected: Arc<Mutex<Option<(String, String)>>>,
}

impl LinkHandle {
    /// Fan out to the connected phone (no-op when none).
    pub fn send(&self, msg: DesktopMessage) {
        let _ = self.out.send(msg);
    }
    pub fn approve(&self, session: u64, yes: bool) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&session) {
            let _ = tx.send(yes);
        }
    }
    pub fn devices(&self) -> Vec<PairedDevice> {
        self.devices.list()
    }
    pub fn forget(&self, device_id: &str) {
        self.devices.remove(device_id);
    }
    /// `(device_id, name)` of the phone currently connected.
    pub fn connected(&self) -> Option<(String, String)> {
        self.connected.lock().unwrap().clone()
    }
}

pub struct Link {
    pub addr: SocketAddr,
    pub handle: LinkHandle,
    _mdns: Option<mdns_sd::ServiceDaemon>,
}

impl Link {
    /// The mDNS daemon lives as long as this value does; keep it alongside the handle.
    pub fn _mdns_keepalive(self) -> Option<mdns_sd::ServiceDaemon> {
        self._mdns
    }
}

impl Link {
    pub async fn start(cfg: LinkConfig, events: mpsc::Sender<LinkEvent>) -> Result<Link, LinkError> {
        // A fixed port keeps the phone's saved address valid across restarts; fall back
        // to a random one only if it is taken.
        let listener = match TcpListener::bind(("0.0.0.0", cfg.port)).await {
            Ok(l) => l,
            Err(e) if cfg.port != 0 => {
                eprintln!("[mochi-link] port {} unavailable ({e}); using a random port", cfg.port);
                TcpListener::bind(("0.0.0.0", 0)).await?
            }
            Err(e) => return Err(e.into()),
        };
        let addr = listener.local_addr()?;
        let (out, _) = broadcast::channel(64);
        let handle = LinkHandle {
            out,
            pending: Arc::new(Mutex::new(HashMap::new())),
            devices: Arc::new(DeviceStore::open(&cfg.devices_path)),
            connected: Arc::new(Mutex::new(None)),
        };
        let mdns = if cfg.advertise { advertise(&cfg.desktop_name, addr.port()).ok() } else { None };

        let h = handle.clone();
        let cfg = Arc::new(cfg);
        let next_session = Arc::new(AtomicU64::new(1));
        tokio::spawn(async move {
            loop {
                let Ok((stream, peer)) = listener.accept().await else { break };
                let (h, cfg, ev) = (h.clone(), cfg.clone(), events.clone());
                let session = next_session.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    if let Err(e) = serve(stream, peer, session, h, cfg, ev).await {
                        eprintln!("[mochi-link] {peer}: {e}");
                    }
                });
            }
        });
        Ok(Link { addr, handle, _mdns: mdns })
    }
}

fn advertise(name: &str, port: u16) -> Result<mdns_sd::ServiceDaemon, LinkError> {
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| LinkError::Mdns(e.to_string()))?;
    let instance = name.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == ' ').collect::<String>();
    let host = format!("{}.local.", instance.replace(' ', "-").to_lowercase());
    let info = mdns_sd::ServiceInfo::new(SERVICE_TYPE, &instance, &host, (), port, &[("name", name), ("v", "1")][..])
        .map_err(|e| LinkError::Mdns(e.to_string()))?
        .enable_addr_auto();
    daemon.register(info).map_err(|e| LinkError::Mdns(e.to_string()))?;
    Ok(daemon)
}

fn nonce() -> [u8; 32] {
    let mut n = [0u8; 32];
    getrandom::fill(&mut n).expect("os randomness");
    n
}

async fn serve(stream: TcpStream, _peer: SocketAddr, session: u64, h: LinkHandle, cfg: Arc<LinkConfig>, ev: mpsc::Sender<LinkEvent>) -> Result<(), String> {
    let mut ws = tokio_tungstenite::accept_async(stream).await.map_err(|e| e.to_string())?;

    let nonce = nonce();
    send(&mut ws, &DesktopMessage::Hello { desktop_name: cfg.desktop_name.clone(), nonce_b64: B64.encode(nonce) }).await?;

    // First frame decides: pairing or authentication.
    let first = match tokio::time::timeout(Duration::from_secs(15), read_phone(&mut ws)).await {
        Ok(Some(Ok(m))) => m,
        Ok(Some(Err(e))) => return Err(e),
        _ => return Err("no first frame".into()),
    };
    let device: PairedDevice = match first {
        PhoneMessage::Auth { device_id, hmac_b64 } => {
            let ok = h.devices.get(&device_id).map_or(false, |d| crypto::verify_auth(&d.secret(), &nonce, &hmac_b64));
            if !ok {
                send(&mut ws, &DesktopMessage::AuthFailed).await?;
                return Err("auth failed".into());
            }
            send(&mut ws, &DesktopMessage::AuthOk).await?;
            h.devices.get(&device_id).unwrap()
        }
        PhoneMessage::PairRequest { device_name, public_key_b64 } => {
            let (secret, our_pub) = crypto::desktop_pairing(&public_key_b64).ok_or("bad phone public key")?;
            let code = pairing_code(&secret);
            send(&mut ws, &DesktopMessage::PairChallenge { desktop_name: cfg.desktop_name.clone(), code: code.clone(), desktop_public_key_b64: our_pub }).await?;
            let (tx, rx) = oneshot::channel();
            // One phone at a time: a new request supersedes any pairing still waiting
            // (a phone that reconnected mid-pairing would otherwise split the approvals).
            {
                let mut pending = h.pending.lock().unwrap();
                for (_, old) in pending.drain() {
                    let _ = old.send(false);
                }
                pending.insert(session, tx);
            }
            let _ = ev.send(LinkEvent::PairRequest { session, device_name: device_name.clone(), code }).await;

            // Both approvals, in any order, within the timeout.
            let mut desktop_ok: Option<bool> = None;
            let mut phone_ok = false;
            let deadline = tokio::time::sleep(cfg.pairing_timeout);
            tokio::pin!(deadline);
            let mut rx = Some(rx);
            let approved = loop {
                if desktop_ok == Some(true) && phone_ok { break true; }
                if desktop_ok == Some(false) { break false; }
                tokio::select! {
                    r = async { rx.as_mut().unwrap().await }, if rx.is_some() => {
                        desktop_ok = Some(r.unwrap_or(false));
                        rx = None;
                        let _ = ev.send(LinkEvent::PairProgress { session, phone_approved: phone_ok, desktop_approved: desktop_ok == Some(true) }).await;
                    }
                    m = read_phone(&mut ws) => match m {
                        Some(Ok(PhoneMessage::PairApprove)) => {
                            phone_ok = true;
                            let _ = ev.send(LinkEvent::PairProgress { session, phone_approved: true, desktop_approved: desktop_ok == Some(true) }).await;
                        }
                        Some(Ok(_)) => {}
                        _ => break false,
                    },
                    _ = &mut deadline => break false,
                }
            };
            h.pending.lock().unwrap().remove(&session);
            if !approved {
                let _ = send(&mut ws, &DesktopMessage::PairDenied).await;
                let _ = ev.send(LinkEvent::PairFailed { session, reason: "not approved on both devices".into() }).await;
                return Err("pairing not approved".into());
            }
            let d = h.devices.add(&device_name, &secret);
            send(&mut ws, &DesktopMessage::Paired { device_id: d.device_id.clone() }).await?;
            let _ = ev.send(LinkEvent::Paired { device_id: d.device_id.clone(), device_name: d.name.clone() }).await;
            d
        }
        _ => return Err("expected auth or pair_request".into()),
    };

    *h.connected.lock().unwrap() = Some((device.device_id.clone(), device.name.clone()));
    let _ = ev.send(LinkEvent::Connected { device_id: device.device_id.clone(), device_name: device.name.clone() }).await;
    let result = session_loop(&mut ws, &h, &cfg, &ev).await;
    *h.connected.lock().unwrap() = None;
    let _ = ev.send(LinkEvent::Disconnected { device_id: device.device_id }).await;
    result
}

async fn session_loop(ws: &mut tokio_tungstenite::WebSocketStream<TcpStream>, h: &LinkHandle, cfg: &LinkConfig, ev: &mpsc::Sender<LinkEvent>) -> Result<(), String> {
    let mut out = h.out.subscribe();
    let mut ping = tokio::time::interval(cfg.ping_interval);
    ping.tick().await; // the first tick fires immediately; skip it
    let mut missed = 0u32;
    loop {
        tokio::select! {
            m = read_phone(ws) => match m {
                Some(Ok(msg)) => {
                    missed = 0;
                    let e = match msg {
                        PhoneMessage::Pong => continue,
                        PhoneMessage::Wake => LinkEvent::Wake,
                        PhoneMessage::Utterance { text, is_final } => LinkEvent::Utterance { text, is_final },
                        PhoneMessage::UserSpeaking => LinkEvent::UserSpeaking,
                        PhoneMessage::Answer { yes } => LinkEvent::Answer { yes },
                        PhoneMessage::Stop => LinkEvent::Stop,
                        PhoneMessage::PairRequest { .. } | PhoneMessage::PairApprove | PhoneMessage::Auth { .. } => continue,
                    };
                    let _ = ev.send(e).await;
                }
                Some(Err(e)) => return Err(e),
                None => return Ok(()),
            },
            m = out.recv() => match m {
                Ok(msg) => { let s = serde_json::to_string(&msg).unwrap(); ws.send(Message::Text(s.into())).await.map_err(|e| e.to_string())?; }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return Ok(()),
            },
            _ = ping.tick() => {
                missed += 1;
                if missed > MISSED_PINGS_BEFORE_RECONNECT { return Err("phone stopped answering pings".into()); }
                let s = serde_json::to_string(&DesktopMessage::Ping).unwrap();
                ws.send(Message::Text(s.into())).await.map_err(|e| e.to_string())?;
            }
        }
    }
}

async fn send(ws: &mut tokio_tungstenite::WebSocketStream<TcpStream>, m: &DesktopMessage) -> Result<(), String> {
    let s = serde_json::to_string(m).unwrap();
    ws.send(Message::Text(s.into())).await.map_err(|e| e.to_string())
}

/// Next phone message, or None when the socket closed cleanly.
async fn read_phone(ws: &mut tokio_tungstenite::WebSocketStream<TcpStream>) -> Option<Result<PhoneMessage, String>> {
    loop {
        match ws.next().await {
            None => return None,
            Some(Err(e)) => return Some(Err(e.to_string())),
            Some(Ok(Message::Text(t))) => {
                if t.len() > MAX_FRAME_BYTES {
                    return Some(Err("frame too large".into()));
                }
                return Some(serde_json::from_str::<PhoneMessage>(&t).map_err(|e| format!("bad frame: {e}")));
            }
            Some(Ok(Message::Close(_))) => return None,
            Some(Ok(Message::Ping(p))) => { let _ = ws.send(Message::Pong(p)).await; }
            Some(Ok(_)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::connect_async;

    type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

    async fn start(ping: Duration) -> (Link, mpsc::Receiver<LinkEvent>, tempfile_path::Path) {
        let dir = tempfile_path::Path::new();
        let (tx, rx) = mpsc::channel(32);
        let link = Link::start(
            LinkConfig { desktop_name: "DESK".into(), port: 0, devices_path: dir.join("devices.json"), advertise: false, ping_interval: ping, pairing_timeout: Duration::from_secs(5) },
            tx,
        )
        .await
        .unwrap();
        (link, rx, dir)
    }
    async fn connect(link: &Link) -> Client {
        let (ws, _) = connect_async(format!("ws://127.0.0.1:{}", link.addr.port())).await.unwrap();
        ws
    }
    async fn recv(ws: &mut Client) -> DesktopMessage {
        loop {
            match ws.next().await.unwrap().unwrap() {
                Message::Text(t) => return serde_json::from_str(&t).unwrap(),
                _ => {}
            }
        }
    }
    async fn send(ws: &mut Client, m: PhoneMessage) {
        ws.send(Message::Text(serde_json::to_string(&m).unwrap().into())).await.unwrap();
    }
    /// Next event that is not a progress note.
    async fn ev(events: &mut mpsc::Receiver<LinkEvent>) -> LinkEvent {
        loop {
            match events.recv().await.unwrap() {
                LinkEvent::PairProgress { .. } => continue,
                e => return e,
            }
        }
    }

    /// Minimal temp dir without pulling in a crate.
    mod tempfile_path {
        pub struct Path(std::path::PathBuf);
        impl Path {
            pub fn new() -> Self {
                let p = std::env::temp_dir().join(format!("mochi-link-test-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
                std::fs::create_dir_all(&p).unwrap();
                Path(p)
            }
            pub fn join(&self, s: &str) -> std::path::PathBuf { self.0.join(s) }
        }
        impl Drop for Path { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    }

    #[tokio::test]
    async fn pair_with_both_approvals_then_talk_then_reconnect_with_hmac() {
        let (link, mut events, _dir) = start(Duration::from_secs(30)).await;

        // --- pairing ---
        let mut ws = connect(&link).await;
        assert!(matches!(recv(&mut ws).await, DesktopMessage::Hello { .. }));
        let phone = PhoneCrypto::new();
        send(&mut ws, PhoneMessage::PairRequest { device_name: "Pixel".into(), public_key_b64: phone.public_key_b64() }).await;
        let DesktopMessage::PairChallenge { desktop_name, code, desktop_public_key_b64 } = recv(&mut ws).await else { panic!("expected challenge") };
        assert_eq!(desktop_name, "DESK");
        let secret = phone.shared_secret(&desktop_public_key_b64).unwrap();
        assert_eq!(code, pairing_code(&secret), "both screens compute the same code from the shared secret");

        let LinkEvent::PairRequest { session, device_name, code: shown } = ev(&mut events).await else { panic!() };
        assert_eq!((device_name.as_str(), shown.as_str()), ("Pixel", code.as_str()));

        // Phone approves first; nothing happens until the desktop approves too.
        send(&mut ws, PhoneMessage::PairApprove).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(matches!(events.try_recv(), Ok(LinkEvent::PairProgress { phone_approved: true, desktop_approved: false, .. })));
        assert!(events.try_recv().is_err(), "nothing else until the desktop approves");
        link.handle.approve(session, true);
        let DesktopMessage::Paired { device_id } = recv(&mut ws).await else { panic!("expected paired") };
        assert!(matches!(ev(&mut events).await, LinkEvent::Paired { .. }));
        assert!(matches!(ev(&mut events).await, LinkEvent::Connected { .. }));
        assert_eq!(link.handle.connected().map(|c| c.1), Some("Pixel".into()));

        // --- talk both ways ---
        send(&mut ws, PhoneMessage::Utterance { text: "reply to Sara".into(), is_final: true }).await;
        assert_eq!(ev(&mut events).await, LinkEvent::Utterance { text: "reply to Sara".into(), is_final: true });
        link.handle.send(DesktopMessage::Narrate { text: "Opening Gmail…".into(), audio_url: None });
        assert!(matches!(recv(&mut ws).await, DesktopMessage::Narrate { text, .. } if text == "Opening Gmail…"));
        send(&mut ws, PhoneMessage::Answer { yes: true }).await;
        assert_eq!(ev(&mut events).await, LinkEvent::Answer { yes: true });

        // --- drop and reconnect with HMAC auth ---
        drop(ws);
        assert!(matches!(ev(&mut events).await, LinkEvent::Disconnected { .. }));
        let mut ws = connect(&link).await;
        let DesktopMessage::Hello { nonce_b64, .. } = recv(&mut ws).await else { panic!() };
        send(&mut ws, PhoneMessage::Auth { device_id: device_id.clone(), hmac_b64: PhoneCrypto::auth_hmac(&secret, &nonce_b64) }).await;
        assert!(matches!(recv(&mut ws).await, DesktopMessage::AuthOk));
        assert!(matches!(ev(&mut events).await, LinkEvent::Connected { .. }));

        // Wrong secret is refused.
        let mut bad = connect(&link).await;
        let DesktopMessage::Hello { nonce_b64, .. } = recv(&mut bad).await else { panic!() };
        send(&mut bad, PhoneMessage::Auth { device_id, hmac_b64: PhoneCrypto::auth_hmac(&[9u8; 32], &nonce_b64) }).await;
        assert!(matches!(recv(&mut bad).await, DesktopMessage::AuthFailed));

        assert_eq!(link.handle.devices().len(), 1);
    }

    #[tokio::test]
    async fn a_second_pair_request_supersedes_the_first() {
        let (link, mut events, _dir) = start(Duration::from_secs(30)).await;
        let mut ws1 = connect(&link).await;
        let _ = recv(&mut ws1).await;
        let p1 = PhoneCrypto::new();
        send(&mut ws1, PhoneMessage::PairRequest { device_name: "Pixel".into(), public_key_b64: p1.public_key_b64() }).await;
        let _ = recv(&mut ws1).await;
        let LinkEvent::PairRequest { session: s1, .. } = events.recv().await.unwrap() else { panic!() };
        send(&mut ws1, PhoneMessage::PairApprove).await;
        assert!(matches!(events.recv().await.unwrap(), LinkEvent::PairProgress { phone_approved: true, desktop_approved: false, .. }));

        // The phone reconnects and asks again before the desktop answered.
        let mut ws2 = connect(&link).await;
        let _ = recv(&mut ws2).await;
        let p2 = PhoneCrypto::new();
        send(&mut ws2, PhoneMessage::PairRequest { device_name: "Pixel".into(), public_key_b64: p2.public_key_b64() }).await;
        let _ = recv(&mut ws2).await;
        // The first session is denied and closed.
        assert!(matches!(recv(&mut ws1).await, DesktopMessage::PairDenied));
        let mut seen_s2 = None;
        for _ in 0..3 {
            match events.recv().await.unwrap() {
                LinkEvent::PairRequest { session, .. } => seen_s2 = Some(session),
                LinkEvent::PairFailed { session, .. } => assert_eq!(session, s1),
                other => panic!("unexpected {other:?}"),
            }
            if seen_s2.is_some() { break; }
        }
        let s2 = seen_s2.unwrap();
        assert_ne!(s1, s2);
        // Approvals on the new session complete it.
        send(&mut ws2, PhoneMessage::PairApprove).await;
        link.handle.approve(s2, true);
        let mut paired = false;
        for _ in 0..6 {
            if let DesktopMessage::Paired { .. } = recv(&mut ws2).await { paired = true; break; }
        }
        assert!(paired);
    }

    #[tokio::test]
    async fn desktop_denial_ends_pairing_and_stores_nothing() {
        let (link, mut events, _dir) = start(Duration::from_secs(30)).await;
        let mut ws = connect(&link).await;
        let _ = recv(&mut ws).await;
        let phone = PhoneCrypto::new();
        send(&mut ws, PhoneMessage::PairRequest { device_name: "Pixel".into(), public_key_b64: phone.public_key_b64() }).await;
        let _ = recv(&mut ws).await;
        let LinkEvent::PairRequest { session, .. } = ev(&mut events).await else { panic!() };
        send(&mut ws, PhoneMessage::PairApprove).await;
        link.handle.approve(session, false);
        assert!(matches!(recv(&mut ws).await, DesktopMessage::PairDenied));
        assert!(matches!(ev(&mut events).await, LinkEvent::PairFailed { .. }));
        assert!(link.handle.devices().is_empty());
    }

    #[tokio::test]
    async fn missed_pings_close_the_session_and_pongs_keep_it_alive() {
        let (link, mut events, _dir) = start(Duration::from_millis(60)).await;
        let mut ws = connect(&link).await;
        let _ = recv(&mut ws).await;
        let phone = PhoneCrypto::new();
        send(&mut ws, PhoneMessage::PairRequest { device_name: "P".into(), public_key_b64: phone.public_key_b64() }).await;
        let _ = recv(&mut ws).await;
        let LinkEvent::PairRequest { session, .. } = ev(&mut events).await else { panic!() };
        send(&mut ws, PhoneMessage::PairApprove).await;
        link.handle.approve(session, true);
        let _ = recv(&mut ws).await; // paired
        let _ = ev(&mut events).await; // paired
        let _ = ev(&mut events).await; // connected

        // Answer pings for a while: still connected.
        let t0 = std::time::Instant::now();
        while t0.elapsed() < Duration::from_millis(400) {
            if let Ok(Some(Ok(Message::Text(t)))) = tokio::time::timeout(Duration::from_millis(100), ws.next()).await {
                if t.contains("\"ping\"") { send(&mut ws, PhoneMessage::Pong).await; }
            }
        }
        assert!(!matches!(events.try_recv(), Ok(LinkEvent::Disconnected { .. })), "still connected while ponging");

        // Stop answering: closed after 3 missed pings (~4 intervals).
        let closed = tokio::time::timeout(Duration::from_secs(2), async {
            loop { if let Some(LinkEvent::Disconnected { .. }) = events.recv().await { break; } }
        }).await;
        assert!(closed.is_ok(), "session closed after missed pings");
    }
}
