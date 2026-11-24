use anyhow::{bail, Result};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub struct Rcon {
    stream: TcpStream,
}

impl Rcon {
    pub fn connect(addr: &str, password: &str) -> Result<Self> {
        let stream = TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let mut rcon = Self { stream };
        rcon.send(1, 3, password)?;
        let (id, _, _) = rcon.recv()?;
        if id != 1 {
            bail!("RCON authentication failed");
        }
        Ok(rcon)
    }

    fn send(&mut self, id: i32, ty: i32, text: &str) -> Result<()> {
        let body = text.as_bytes();
        let len = (8 + body.len() + 2) as i32;
        let mut pkt = Vec::with_capacity(4 + len as usize);
        pkt.extend_from_slice(&len.to_le_bytes());
        pkt.extend_from_slice(&id.to_le_bytes());
        pkt.extend_from_slice(&ty.to_le_bytes());
        pkt.extend_from_slice(body);
        pkt.push(0);
        pkt.push(0);
        self.stream.write_all(&pkt)?;
        Ok(())
    }

    fn recv_exact(&mut self, n: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; n];
        self.stream.read_exact(&mut buf)?;
        Ok(buf)
    }

    pub fn recv(&mut self) -> Result<(i32, i32, String)> {
        let hdr = self.recv_exact(4)?;
        let len = i32::from_le_bytes(hdr.try_into().unwrap()) as usize;
        if !(10..=4096).contains(&len) {
            bail!("Unexpected RCON packet length {len}");
        }
        let payload = self.recv_exact(len)?;
        let id = i32::from_le_bytes(payload[..4].try_into().unwrap());
        let ty = i32::from_le_bytes(payload[4..8].try_into().unwrap());
        let text = String::from_utf8_lossy(&payload[8..len - 2]).into_owned();
        Ok((id, ty, text))
    }

    pub fn command(&mut self, id: i32, cmd: &str) -> Result<(i32, String)> {
        self.send(id, 2, cmd)?;
        let (resp_id, _, resp) = self.recv()?;
        Ok((resp_id, resp))
    }
}
