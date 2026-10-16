//! Lossless compressed container for session files. A container is the magic
//! b"HSAR", a version byte, the original length (u64 LE), the SHA-256 of the
//! original bytes, then one zstd frame. Unpacking checks all of it: a damaged,
//! truncated or mislabelled copy is refused, never returned.

use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAGIC: &[u8; 4] = b"HSAR";
const VERSION: u8 = 1;
const HEADER_LEN: usize = 4 + 1 + 8 + 32;
const CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    /// The input does not start with b"HSAR".
    BadMagic,
    /// The version byte is not one this build reads. Holds the byte.
    UnsupportedVersion(u8),
    /// The input ends inside the header or inside the zstd frame.
    Truncated,
    /// The frame inflates past the declared original length.
    TooLarge,
    /// The frame inflates to fewer bytes than the declared original length.
    LengthMismatch,
    /// The inflated bytes do not hash to the stored SHA-256.
    ChecksumMismatch,
    /// Reading or writing a file failed.
    Io(String),
    /// The zstd frame is corrupt.
    Decompress(String),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArchiveError::BadMagic => write!(f, "not an archive (bad magic)"),
            ArchiveError::UnsupportedVersion(v) => write!(f, "unsupported archive version {v}"),
            ArchiveError::Truncated => write!(f, "archive is truncated"),
            ArchiveError::TooLarge => write!(f, "archive inflates past its declared length"),
            ArchiveError::LengthMismatch => write!(f, "archive length does not match its header"),
            ArchiveError::ChecksumMismatch => write!(f, "archive checksum does not match"),
            ArchiveError::Io(e) => write!(f, "archive io error: {e}"),
            ArchiveError::Decompress(e) => write!(f, "archive decompress error: {e}"),
        }
    }
}

impl std::error::Error for ArchiveError {}

impl From<io::Error> for ArchiveError {
    fn from(e: io::Error) -> Self {
        ArchiveError::Io(e.to_string())
    }
}

/// A decoder error. zstd reports a frame cut short as UnexpectedEof.
fn decode_error(e: io::Error) -> ArchiveError {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        ArchiveError::Truncated
    } else {
        ArchiveError::Decompress(e.to_string())
    }
}

struct Header {
    len: u64,
    sum: [u8; 32],
}

fn header_bytes(len: u64, sum: &[u8; 32]) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..4].copy_from_slice(MAGIC);
    h[4] = VERSION;
    h[5..13].copy_from_slice(&len.to_le_bytes());
    h[13..].copy_from_slice(sum);
    h
}

/// Parses the header from the first bytes of a container. Magic and version are
/// checked on whatever bytes are present, so a short non-archive reads as BadMagic.
fn parse_header(b: &[u8]) -> Result<Header, ArchiveError> {
    let m = b.len().min(4);
    if b[..m] != MAGIC[..m] {
        return Err(ArchiveError::BadMagic);
    }
    if b.len() > 4 && b[4] != VERSION {
        return Err(ArchiveError::UnsupportedVersion(b[4]));
    }
    if b.len() < HEADER_LEN {
        return Err(ArchiveError::Truncated);
    }
    let mut len = [0u8; 8];
    len.copy_from_slice(&b[5..13]);
    let mut sum = [0u8; 32];
    sum.copy_from_slice(&b[13..HEADER_LEN]);
    Ok(Header { len: u64::from_le_bytes(len), sum })
}

fn encoder<W: Write>(w: W, level: i32) -> io::Result<zstd::stream::write::Encoder<'static, W>> {
    let mut e = zstd::stream::write::Encoder::new(w, level)?;
    e.include_checksum(true)?;
    Ok(e)
}

/// Compresses `original` into a container at zstd `level`.
pub fn pack(original: &[u8], level: i32) -> Vec<u8> {
    let sum: [u8; 32] = Sha256::digest(original).into();
    let mut out = header_bytes(original.len() as u64, &sum).to_vec();
    let mut e = encoder(&mut out, level).expect("zstd encoder");
    e.write_all(original).expect("zstd write to memory");
    e.finish().expect("zstd finish to memory");
    out
}

/// Inflates the frame from `r` into `sink`, refusing to produce more than
/// `h.len` bytes, then checks the length and checksum.
fn inflate<R: Read>(
    r: R,
    h: &Header,
    mut sink: impl FnMut(&[u8]) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    let mut d = zstd::stream::read::Decoder::new(r).map_err(decode_error)?;
    let mut hasher = Sha256::new();
    let mut got: u64 = 0;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = match d.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(decode_error(e)),
        };
        got += n as u64;
        if got > h.len {
            return Err(ArchiveError::TooLarge);
        }
        hasher.update(&buf[..n]);
        sink(&buf[..n])?;
    }
    if got != h.len {
        return Err(ArchiveError::LengthMismatch);
    }
    let sum: [u8; 32] = hasher.finalize().into();
    if sum != h.sum {
        return Err(ArchiveError::ChecksumMismatch);
    }
    Ok(())
}

/// Returns the original bytes of a container, or an error if any check fails.
pub fn unpack(packed: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    let h = parse_header(packed)?;
    let mut out = Vec::with_capacity(h.len.min(64 * 1024 * 1024) as usize);
    inflate(&packed[HEADER_LEN..], &h, |b| {
        out.extend_from_slice(b);
        Ok(())
    })?;
    Ok(out)
}

/// A temp file next to the destination. Removed on drop unless committed.
struct TempOut {
    path: PathBuf,
    done: bool,
}

impl TempOut {
    fn create(dst: &Path) -> io::Result<(TempOut, File)> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = match dst.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        loop {
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!(".{name}.hsar-tmp-{}-{n}", std::process::id()));
            match OpenOptions::new().write(true).read(true).create_new(true).open(&path) {
                Ok(f) => return Ok((TempOut { path, done: false }, f)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    fn commit(mut self, dst: &Path) -> io::Result<()> {
        fs::rename(&self.path, dst)?;
        self.done = true;
        Ok(())
    }
}

impl Drop for TempOut {
    fn drop(&mut self) {
        if !self.done {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Hashes and counts everything written through it.
struct Tee<W> {
    inner: W,
    hasher: Sha256,
    len: u64,
}

impl<W: Write> Write for Tee<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(b)?;
        self.hasher.update(&b[..n]);
        self.len += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Compresses the file at `src` into a container at `dst`, streaming. The
/// header is written last, over a placeholder, so `src` is read once. `dst`
/// appears only when complete.
pub fn pack_file(src: &Path, dst: &Path, level: i32) -> io::Result<()> {
    let mut input = BufReader::with_capacity(CHUNK, File::open(src)?);
    let (tmp, file) = TempOut::create(dst)?;
    let mut w = BufWriter::with_capacity(CHUNK, file);
    w.write_all(&[0u8; HEADER_LEN])?;
    let mut tee = Tee { inner: encoder(w, level)?, hasher: Sha256::new(), len: 0 };
    io::copy(&mut input, &mut tee)?;
    let Tee { inner, hasher, len } = tee;
    let mut w = inner.finish()?;
    let sum: [u8; 32] = hasher.finalize().into();
    w.seek(SeekFrom::Start(0))?;
    w.write_all(&header_bytes(len, &sum))?;
    let file = w.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()?;
    drop(file);
    tmp.commit(dst)
}

/// Restores the original file from the container at `src` to `dst`, streaming.
/// Output goes to a temp file next to `dst` and is renamed into place only
/// after length and checksum pass, so a failure never leaves a partial `dst`
/// and never replaces an existing one.
pub fn unpack_file(src: &Path, dst: &Path) -> Result<(), ArchiveError> {
    let mut input = BufReader::with_capacity(CHUNK, File::open(src)?);
    let mut head = Vec::with_capacity(HEADER_LEN);
    (&mut input).take(HEADER_LEN as u64).read_to_end(&mut head)?;
    let h = parse_header(&head)?;
    let (tmp, file) = TempOut::create(dst)?;
    let mut w = BufWriter::with_capacity(CHUNK, file);
    inflate(input, &h, |b| w.write_all(b).map_err(ArchiveError::from))?;
    let file = w.into_inner().map_err(|e| ArchiveError::from(e.into_error()))?;
    file.sync_all()?;
    drop(file);
    tmp.commit(dst)?;
    Ok(())
}
