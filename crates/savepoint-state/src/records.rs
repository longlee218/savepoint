//! Checksummed, length-prefixed record files used by key/value providers.
//!
//! Layout: `MAGIC` | records… | `TRAILER` | u64 record count | sha256(all preceding bytes)
//! Each record: u16 field count, then per field u32 length + bytes (little endian).

use crate::error::{Result, StateError};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"SPSTATE1";
pub const TRAILER: &[u8; 8] = b"SPEND001";

pub struct RecordWriter {
    buf: Vec<u8>,
    count: u64,
}

impl Default for RecordWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordWriter {
    pub fn new() -> Self {
        Self {
            buf: MAGIC.to_vec(),
            count: 0,
        }
    }

    pub fn push(&mut self, fields: &[&[u8]]) {
        let n = u16::try_from(fields.len()).expect("record field count fits in u16");
        self.buf.extend_from_slice(&n.to_le_bytes());
        for f in fields {
            let len = u32::try_from(f.len()).expect("record field fits in u32");
            self.buf.extend_from_slice(&len.to_le_bytes());
            self.buf.extend_from_slice(f);
        }
        self.count += 1;
    }

    pub fn len(&self) -> u64 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn bytes(&self) -> usize {
        self.buf.len()
    }

    /// Writes atomically (temp file + rename) so a crash never leaves a half file.
    pub fn finish(mut self, path: &Path) -> Result<()> {
        self.buf.extend_from_slice(TRAILER);
        self.buf.extend_from_slice(&self.count.to_le_bytes());
        let digest = Sha256::digest(&self.buf);
        self.buf.extend_from_slice(&digest);
        write_atomic(path, &self.buf)
    }
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp-partial");
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| StateError::io(&tmp, e))?;
        f.write_all(bytes).map_err(|e| StateError::io(&tmp, e))?;
        f.sync_all().map_err(|e| StateError::io(&tmp, e))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| StateError::io(path, e))
}

/// Reads and verifies a record file. Fails on bad magic, truncation or checksum mismatch.
pub fn read_records(path: &Path) -> Result<Vec<Vec<Vec<u8>>>> {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(StateError::MissingSnapshot(path.to_path_buf()))
        }
        Err(e) => return Err(StateError::io(path, e)),
    };
    let corrupt = |msg: &str| StateError::Corrupt {
        path: path.to_path_buf(),
        msg: msg.to_string(),
    };
    let tail = TRAILER.len() + 8 + 32;
    if data.len() < MAGIC.len() + tail || &data[..MAGIC.len()] != MAGIC {
        return Err(corrupt("bad magic or truncated header"));
    }
    let (body, digest) = data.split_at(data.len() - 32);
    if Sha256::digest(body).as_slice() != digest {
        return Err(corrupt("checksum mismatch (truncated or modified)"));
    }
    let trailer_at = body.len() - 8 - TRAILER.len();
    if &body[trailer_at..trailer_at + TRAILER.len()] != TRAILER {
        return Err(corrupt("missing trailer"));
    }
    let expected = u64::from_le_bytes(body[body.len() - 8..].try_into().unwrap());

    let mut out = Vec::new();
    let mut i = MAGIC.len();
    let end = trailer_at;
    let take = |i: &mut usize, n: usize| -> Result<&[u8]> {
        if *i + n > end {
            return Err(corrupt("record overruns file"));
        }
        let s = &data[*i..*i + n];
        *i += n;
        Ok(s)
    };
    while i < end {
        let nf = u16::from_le_bytes(take(&mut i, 2)?.try_into().unwrap()) as usize;
        let mut rec = Vec::with_capacity(nf);
        for _ in 0..nf {
            let len = u32::from_le_bytes(take(&mut i, 4)?.try_into().unwrap()) as usize;
            rec.push(take(&mut i, len)?.to_vec());
        }
        out.push(rec);
    }
    if out.len() as u64 != expected {
        return Err(corrupt("record count mismatch"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_binary_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.bin");
        let all_bytes: Vec<u8> = (0u8..=255).collect();
        let mut w = RecordWriter::new();
        w.push(&[b"key\r\n\x00", &all_bytes, b""]);
        w.push(&[b"second"]);
        assert_eq!(w.len(), 2);
        w.finish(&path).unwrap();

        let recs = read_records(&path).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0][0], b"key\r\n\x00");
        assert_eq!(recs[0][1], all_bytes);
        assert_eq!(recs[0][2], b"");
        assert_eq!(recs[1], vec![b"second".to_vec()]);
    }

    #[test]
    fn empty_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.bin");
        RecordWriter::new().finish(&path).unwrap();
        assert!(read_records(&path).unwrap().is_empty());
    }

    #[test]
    fn detects_tampering_and_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.bin");
        let mut w = RecordWriter::new();
        w.push(&[b"k", b"value-value-value"]);
        w.finish(&path).unwrap();
        let good = std::fs::read(&path).unwrap();

        let mut flipped = good.clone();
        flipped[MAGIC.len() + 8] ^= 0x01;
        std::fs::write(&path, &flipped).unwrap();
        assert!(matches!(read_records(&path), Err(StateError::Corrupt { .. })));

        std::fs::write(&path, &good[..good.len() - 5]).unwrap();
        assert!(matches!(read_records(&path), Err(StateError::Corrupt { .. })));

        std::fs::write(&path, b"NOTMAGIC").unwrap();
        assert!(matches!(read_records(&path), Err(StateError::Corrupt { .. })));

        std::fs::remove_file(&path).unwrap();
        assert!(matches!(read_records(&path), Err(StateError::MissingSnapshot(_))));
    }
}
