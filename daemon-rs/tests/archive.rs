//! The archive container: zstd compression with a header that carries the
//! original length and its SHA-256. Behaviour pinned here: round trips are byte
//! identical; damaged, truncated or mislabelled input is refused with a specific
//! error; a header cannot be used to inflate past its declared size; a failed
//! file unpack leaves nothing behind at the destination.

use harness_sync::archive::{pack, pack_file, unpack, unpack_file, ArchiveError};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

const HEADER: usize = 4 + 1 + 8 + 32;
const SUM_AT: usize = 4 + 1 + 8;

fn sandbox(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hs-archive-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn jsonl(lines: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..lines {
        writeln!(
            out,
            r#"{{"type":"message","id":"msg-{i}","role":"assistant","content":[{{"type":"text","text":"Line {i} of a synthetic session transcript."}}]}}"#
        )
        .unwrap();
    }
    out
}

fn random_bytes(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

#[test]
fn round_trip_empty() {
    let p = pack(b"", 3);
    assert_eq!(unpack(&p).unwrap(), b"");
}

#[test]
fn round_trip_small_jsonl() {
    let data = jsonl(10);
    assert_eq!(unpack(&pack(&data, 3)).unwrap(), data);
}

#[test]
fn round_trip_5mb_jsonl_compresses_well() {
    let mut data = jsonl(50_000);
    data.truncate(5 * 1024 * 1024);
    assert!(data.len() >= 5 * 1024 * 1024 - 1, "synthetic input is too short");
    let p = pack(&data, 3);
    let ratio = p.len() as f64 / data.len() as f64;
    eprintln!("5MB jsonl: {} -> {} bytes, ratio {:.4}", data.len(), p.len(), ratio);
    assert!(ratio < 0.20, "ratio {ratio}");
    assert_eq!(unpack(&p).unwrap(), data);
}

#[test]
fn round_trip_random_bytes() {
    let data = random_bytes(200_000, 0x9e3779b97f4a7c15);
    assert_eq!(unpack(&pack(&data, 3)).unwrap(), data);
}

#[test]
fn flipped_byte_in_frame_is_refused() {
    let data = jsonl(2000);
    let p = pack(&data, 3);
    for at in [HEADER + 10, HEADER + (p.len() - HEADER) / 2, p.len() - 1] {
        let mut bad = p.clone();
        bad[at] ^= 0x41;
        match unpack(&bad) {
            Err(ArchiveError::Decompress(_)) | Err(ArchiveError::ChecksumMismatch) => {}
            other => panic!("flip at {at}: expected refusal, got {:?}", other.map(|v| v.len())),
        }
    }
}

#[test]
fn flipped_byte_in_stored_checksum_is_refused() {
    let data = jsonl(100);
    let mut p = pack(&data, 3);
    p[SUM_AT + 5] ^= 0x01;
    assert!(matches!(unpack(&p), Err(ArchiveError::ChecksumMismatch)));
}

#[test]
fn truncated_input_is_refused() {
    let data = jsonl(2000);
    let p = pack(&data, 3);
    for cut in [0, 2, 4, 5, 9, 13, 20, HEADER - 1, HEADER, HEADER + 1, p.len() / 2, p.len() - 1] {
        let r = unpack(&p[..cut]);
        assert!(r.is_err(), "cut at {cut} was accepted");
        if cut < HEADER {
            assert!(
                matches!(r, Err(ArchiveError::Truncated) | Err(ArchiveError::BadMagic)),
                "cut at {cut}: {:?}",
                r.map(|v| v.len())
            );
        }
    }
}

#[test]
fn header_only_cut_is_truncated() {
    let p = pack(&jsonl(10), 3);
    assert!(matches!(unpack(&p[..HEADER - 1]), Err(ArchiveError::Truncated)));
    assert!(matches!(unpack(&p[..3]), Err(ArchiveError::Truncated)));
}

#[test]
fn wrong_magic_is_refused() {
    let mut p = pack(b"hello", 3);
    p[0] = b'X';
    assert!(matches!(unpack(&p), Err(ArchiveError::BadMagic)));
}

#[test]
fn unknown_version_is_refused() {
    let mut p = pack(b"hello", 3);
    p[4] = 2;
    assert!(matches!(unpack(&p), Err(ArchiveError::UnsupportedVersion(2))));
}

#[test]
fn smaller_declared_length_is_refused() {
    let data = jsonl(2000);
    let mut p = pack(&data, 3);
    let small = (data.len() as u64 / 2).to_le_bytes();
    p[5..13].copy_from_slice(&small);
    match unpack(&p) {
        Err(ArchiveError::TooLarge) => {}
        other => panic!("expected TooLarge, got {:?}", other.map(|v| v.len())),
    }
}

#[test]
fn decompression_bomb_stops_at_declared_length() {
    let d = sandbox("bomb");
    let arc = d.join("bomb.hsar");
    let dst = d.join("bomb.out");
    let zeros = vec![0u8; 32 * 1024 * 1024];
    let mut p = pack(&zeros, 1);
    drop(zeros);
    assert!(p.len() < 64 * 1024, "bomb frame should be tiny, got {}", p.len());
    p[5..13].copy_from_slice(&1024u64.to_le_bytes());
    assert!(matches!(unpack(&p), Err(ArchiveError::TooLarge)));
    fs::write(&arc, &p).unwrap();
    assert!(matches!(unpack_file(&arc, &dst), Err(ArchiveError::TooLarge)));
    assert_eq!(dir_names(&d), vec!["bomb.hsar"]);
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn larger_declared_length_is_refused() {
    let data = jsonl(200);
    let mut p = pack(&data, 3);
    p[5..13].copy_from_slice(&(data.len() as u64 + 7).to_le_bytes());
    assert!(matches!(unpack(&p), Err(ArchiveError::LengthMismatch)));
}

#[test]
fn errors_display() {
    for e in [
        ArchiveError::BadMagic,
        ArchiveError::UnsupportedVersion(9),
        ArchiveError::Truncated,
        ArchiveError::TooLarge,
        ArchiveError::LengthMismatch,
        ArchiveError::ChecksumMismatch,
        ArchiveError::Io("x".into()),
        ArchiveError::Decompress("y".into()),
    ] {
        assert!(!e.to_string().is_empty());
        let _: &dyn std::error::Error = &e;
    }
}

fn dir_names(d: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn unpack_file_on_damaged_archive_leaves_nothing() {
    let src_dir = sandbox("dmg-src");
    let out_dir = sandbox("dmg-out");
    let src = src_dir.join("s.jsonl");
    let arc = src_dir.join("s.hsar");
    fs::write(&src, jsonl(5000)).unwrap();
    pack_file(&src, &arc, 3).unwrap();

    let good = fs::read(&arc).unwrap();
    let cases: Vec<Vec<u8>> = vec![
        {
            let mut b = good.clone();
            b[SUM_AT] ^= 0xff;
            b
        },
        {
            let mut b = good.clone();
            let mid = HEADER + (b.len() - HEADER) / 2;
            b[mid] ^= 0x55;
            b
        },
        good[..good.len() - 3].to_vec(),
        good[..HEADER - 2].to_vec(),
        {
            let mut b = good.clone();
            b[5..13].copy_from_slice(&100u64.to_le_bytes());
            b
        },
    ];
    for (i, bad) in cases.iter().enumerate() {
        fs::write(&arc, bad).unwrap();
        let dst = out_dir.join("restored.jsonl");
        let r = unpack_file(&arc, &dst);
        assert!(r.is_err(), "case {i} accepted");
        assert!(!dst.exists(), "case {i} left dst");
        assert!(dir_names(&out_dir).is_empty(), "case {i} left {:?}", dir_names(&out_dir));
    }
}

#[test]
fn unpack_file_failure_keeps_existing_dst() {
    let d = sandbox("keep");
    let src = d.join("s.jsonl");
    let arc = d.join("s.hsar");
    let dst = d.join("existing.jsonl");
    fs::write(&src, jsonl(100)).unwrap();
    fs::write(&dst, b"previous contents").unwrap();
    pack_file(&src, &arc, 3).unwrap();
    let mut b = fs::read(&arc).unwrap();
    b[SUM_AT] ^= 1;
    fs::write(&arc, b).unwrap();
    assert!(matches!(unpack_file(&arc, &dst), Err(ArchiveError::ChecksumMismatch)));
    assert_eq!(fs::read(&dst).unwrap(), b"previous contents");
    assert_eq!(dir_names(&d), vec!["existing.jsonl", "s.hsar", "s.jsonl"]);
}

#[test]
fn pack_file_matches_pack_and_unpacks_in_memory() {
    let d = sandbox("compat");
    let src = d.join("s.jsonl");
    let arc = d.join("s.hsar");
    let data = jsonl(3000);
    fs::write(&src, &data).unwrap();
    pack_file(&src, &arc, 3).unwrap();
    assert_eq!(unpack(&fs::read(&arc).unwrap()).unwrap(), data);
    let out = d.join("o.jsonl");
    fs::write(&arc, pack(&data, 3)).unwrap();
    unpack_file(&arc, &out).unwrap();
    assert_eq!(fs::read(&out).unwrap(), data);
}

#[test]
fn file_round_trip_20mb() {
    let d = sandbox("big");
    let src = d.join("big.jsonl");
    let arc = d.join("big.hsar");
    let out = d.join("big.out.jsonl");
    let mut data = jsonl(120_000);
    data.extend_from_slice(&random_bytes(2 * 1024 * 1024, 42));
    data.resize(20 * 1024 * 1024, b'\n');
    fs::write(&src, &data).unwrap();
    pack_file(&src, &arc, 1).unwrap();
    unpack_file(&arc, &out).unwrap();
    assert_eq!(fs::metadata(&out).unwrap().len(), data.len() as u64);
    assert!(fs::read(&out).unwrap() == data, "20MB round trip differs");
    assert_eq!(dir_names(&d), vec!["big.hsar", "big.jsonl", "big.out.jsonl"]);
    fs::remove_dir_all(&d).unwrap();
}
