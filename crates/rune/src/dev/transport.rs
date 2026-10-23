//! Blocking socket I/O stays off the app thread. Queues and peer counts are bounded.
use std::collections::VecDeque;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};
use crate::Waker;

pub(crate) struct Request {
    pub message: Value,
    pub reply: mpsc::Sender<Value>,
}

#[derive(Default)]
pub(crate) struct Bridge {
    pub incoming: VecDeque<Request>,
    pub revision: u64,
    pub latest: Value,
}

pub(crate) struct Server {
    pub address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn bind(address: SocketAddr, bridge: Arc<Mutex<Bridge>>, wake: Waker) -> io::Result<Self> {
        let allowed = address.ip().is_loopback() || match address.ip() {
            std::net::IpAddr::V4(ip) => ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]),
            std::net::IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
        };
        if !allowed { return Err(io::Error::new(io::ErrorKind::InvalidInput, "dev panel must bind loopback or a Tailscale address")); }
        let listener = TcpListener::bind(address)?;
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = thread::Builder::new().name("rune-dev-listener".into()).spawn(move || {
            let mut peers: Vec<JoinHandle<()>> = Vec::new();
            for accepted in listener.incoming() {
                if stopping.load(Ordering::Acquire) { break; }
                let stream = match accepted {
                    Ok(stream) => stream,
                    Err(e) => { eprintln!("Rune dev accept: {e}"); break; }
                };
                let mut i = 0;
                while i < peers.len() {
                    if peers[i].is_finished() {
                        if peers.swap_remove(i).join().is_err() { eprintln!("Rune dev peer panicked"); }
                    } else { i += 1; }
                }
                if peers.len() >= 8 { continue; }
                let (bridge, wake, stop) = (bridge.clone(), wake.clone(), stopping.clone());
                match thread::Builder::new().name("rune-dev-peer".into()).spawn(move || {
                    if let Err(e) = peer(stream, bridge, wake, stop) {
                        // Disconnects are ordinary; malformed requests receive a protocol error.
                        if !matches!(e, tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed | tungstenite::Error::Io(_)) {
                            eprintln!("Rune dev peer: {e}");
                        }
                    }
                }) {
                    Ok(peer) => peers.push(peer),
                    Err(e) => eprintln!("Rune dev peer thread: {e}"),
                }
            }
            for peer in peers {
                if peer.join().is_err() { eprintln!("Rune dev peer panicked"); }
            }
        })?;
        Ok(Self { address, stop, thread: Some(thread) })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake blocking accept. Peer reads/writes have bounded timeouts.
        if let Err(e) = TcpStream::connect_timeout(&self.address, Duration::from_millis(250)) {
            eprintln!("Rune dev listener shutdown: {e}");
        }
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() { eprintln!("Rune dev listener panicked"); }
        }
    }
}

fn send(socket: &mut WebSocket<TcpStream>, value: &Value) -> tungstenite::Result<()> {
    socket.send(Message::Text(value.to_string().into()))
}

fn peer(stream: TcpStream, bridge: Arc<Mutex<Bridge>>, wake: Waker, stop: Arc<AtomicBool>) -> tungstenite::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
    let config = WebSocketConfig::default().max_message_size(Some(64 * 1024)).max_frame_size(Some(64 * 1024));
    let mut socket = match tungstenite::accept_with_config(stream, Some(config)) {
        Ok(socket) => socket,
        Err(_) => return Ok(()), // Failed or timed-out handshakes own no app resources.
    };
    socket.get_mut().set_read_timeout(Some(Duration::from_millis(16)))?;
    let (reply, replies) = mpsc::channel();
    let mut revision = u64::MAX;
    while !stop.load(Ordering::Acquire) {
        let snapshot = {
            let b = bridge.lock().unwrap();
            if revision == b.revision { None } else {
                revision = b.revision;
                Some(b.latest.clone())
            }
        };
        if let Some(snapshot) = snapshot { send(&mut socket, &snapshot)?; }
        for response in replies.try_iter() { send(&mut socket, &response)?; }
        match socket.read() {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<Value>(&text) {
                    Ok(message) if message["type"] == "get" => {
                        let snapshot = bridge.lock().unwrap().latest.clone();
                        send(&mut socket, &snapshot)?;
                    }
                    Ok(message) => {
                        let queued = {
                            let mut b = bridge.lock().unwrap();
                            if b.incoming.len() >= 256 { false } else {
                                b.incoming.push_back(Request { message, reply: reply.clone() });
                                true
                            }
                        };
                        if queued { wake(); } else { send(&mut socket, &json!({"type":"error","message":"command queue is full"}))?; }
                    }
                    Err(e) => send(&mut socket, &json!({"type":"error","message":e.to_string()}))?,
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => socket.flush()?,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
