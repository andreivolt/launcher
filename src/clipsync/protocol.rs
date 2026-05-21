//! Wire protocol for clip-sync.
//!
//! Length-prefixed framing over a TCP stream. Every frame is a 1-byte tag
//! followed by a 4-byte big-endian payload length and the payload bytes.
//! The payload encoding is hand-rolled (no serde) — the message set is tiny
//! and fixed, so a few fixed-width integers and length-prefixed blobs keep
//! the wire format obvious and dependency-free.

use std::io::{self, Read, Write};

/// Protocol version. Bumped on any incompatible wire change; a peer that sees
/// a mismatch in [`Hello`] drops the connection rather than guessing.
pub const PROTOCOL_VERSION: u32 = 1;

/// Hard cap on a single frame payload (16 MiB). Guards against a corrupt or
/// hostile length prefix forcing an unbounded allocation. Clipboard entries
/// are images at worst — comfortably under this.
const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

const TAG_HELLO: u8 = 1;
const TAG_HAVE: u8 = 2;
const TAG_WANT: u8 = 3;
const TAG_ENTRY: u8 = 4;
const TAG_DONE: u8 = 5;

/// One clipboard entry in transit. Carries the merge key (`content_hash`) plus
/// enough metadata to reconstruct the row on the peer with its original
/// timestamps, so an entry keeps the same age on both machines.
#[derive(Debug, Clone)]
pub struct WireEntry {
    pub content_hash: i64,
    pub created_at: i64,
    pub last_used: i64,
    pub mime: String,
    pub content: Vec<u8>,
}

/// A decoded protocol message.
#[derive(Debug)]
pub enum Message {
    /// Handshake, sent once by each side immediately after connecting.
    Hello { version: u32, host: String },
    /// "These are all the content hashes I currently hold."
    Have(Vec<i64>),
    /// "Of the hashes you advertised, send me these — I lack them."
    Want(Vec<i64>),
    /// A full entry the peer asked for.
    Entry(WireEntry),
    /// End of the current `Entry` batch; the reconciliation round is complete.
    Done,
}

impl Message {
    /// Serialize and write this message as a single framed unit.
    pub fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let (tag, payload) = match self {
            Message::Hello { version, host } => {
                let mut p = Vec::new();
                p.extend_from_slice(&version.to_be_bytes());
                write_str(&mut p, host);
                (TAG_HELLO, p)
            }
            Message::Have(hashes) => (TAG_HAVE, encode_hashes(hashes)),
            Message::Want(hashes) => (TAG_WANT, encode_hashes(hashes)),
            Message::Entry(e) => {
                let mut p = Vec::new();
                p.extend_from_slice(&e.content_hash.to_be_bytes());
                p.extend_from_slice(&e.created_at.to_be_bytes());
                p.extend_from_slice(&e.last_used.to_be_bytes());
                write_str(&mut p, &e.mime);
                write_bytes(&mut p, &e.content);
                (TAG_ENTRY, p)
            }
            Message::Done => (TAG_DONE, Vec::new()),
        };

        if payload.len() as u64 > MAX_FRAME_LEN as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "outgoing frame exceeds size cap",
            ));
        }
        w.write_all(&[tag])?;
        w.write_all(&(payload.len() as u32).to_be_bytes())?;
        w.write_all(&payload)?;
        Ok(())
    }

    /// Read and decode the next framed message. Blocks until a full frame
    /// arrives; returns an `UnexpectedEof` error on a clean peer disconnect.
    pub fn read_from<R: Read>(r: &mut R) -> io::Result<Message> {
        let mut tag = [0u8; 1];
        r.read_exact(&mut tag)?;

        let mut len_buf = [0u8; 4];
        r.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf);
        if len > MAX_FRAME_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incoming frame exceeds size cap",
            ));
        }

        let mut payload = vec![0u8; len as usize];
        r.read_exact(&mut payload)?;
        let mut cur = &payload[..];

        match tag[0] {
            TAG_HELLO => {
                let version = read_u32(&mut cur)?;
                let host = read_str(&mut cur)?;
                Ok(Message::Hello { version, host })
            }
            TAG_HAVE => Ok(Message::Have(decode_hashes(&mut cur)?)),
            TAG_WANT => Ok(Message::Want(decode_hashes(&mut cur)?)),
            TAG_ENTRY => {
                let content_hash = read_i64(&mut cur)?;
                let created_at = read_i64(&mut cur)?;
                let last_used = read_i64(&mut cur)?;
                let mime = read_str(&mut cur)?;
                let content = read_bytes(&mut cur)?;
                Ok(Message::Entry(WireEntry {
                    content_hash,
                    created_at,
                    last_used,
                    mime,
                    content,
                }))
            }
            TAG_DONE => Ok(Message::Done),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown message tag {other}"),
            )),
        }
    }
}

fn encode_hashes(hashes: &[i64]) -> Vec<u8> {
    let mut p = Vec::with_capacity(4 + hashes.len() * 8);
    p.extend_from_slice(&(hashes.len() as u32).to_be_bytes());
    for h in hashes {
        p.extend_from_slice(&h.to_be_bytes());
    }
    p
}

fn decode_hashes(cur: &mut &[u8]) -> io::Result<Vec<i64>> {
    let count = read_u32(cur)? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_i64(cur)?);
    }
    Ok(out)
}

fn write_str(p: &mut Vec<u8>, s: &str) {
    write_bytes(p, s.as_bytes());
}

fn write_bytes(p: &mut Vec<u8>, b: &[u8]) {
    p.extend_from_slice(&(b.len() as u32).to_be_bytes());
    p.extend_from_slice(b);
}

fn read_str(cur: &mut &[u8]) -> io::Result<String> {
    let bytes = read_bytes(cur)?;
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid utf-8 string field"))
}

fn read_bytes(cur: &mut &[u8]) -> io::Result<Vec<u8>> {
    let len = read_u32(cur)? as usize;
    if cur.len() < len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame truncated: byte field longer than payload",
        ));
    }
    let (head, rest) = cur.split_at(len);
    *cur = rest;
    Ok(head.to_vec())
}

fn read_u32(cur: &mut &[u8]) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    cur.read_exact(&mut buf)?;
    Ok(u32::from_be_bytes(buf))
}

fn read_i64(cur: &mut &[u8]) -> io::Result<i64> {
    let mut buf = [0u8; 8];
    cur.read_exact(&mut buf)?;
    Ok(i64::from_be_bytes(buf))
}
