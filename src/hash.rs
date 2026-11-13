use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use digest::Digest;

use crate::cli::Algorithm;

pub const PREFIX_BYTES: u64 = 64 * 1024;
const BUFFER_BYTES: usize = 1024 * 1024;

enum Hasher {
    Blake3(Box<blake3::Hasher>),
    Xxh3(Box<xxhash_rust::xxh3::Xxh3>),
    Crc32(crc32fast::Hasher),
    Md5(md5::Md5),
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
}

pub fn hash_file(path: &Path, algorithm: Algorithm, limit: Option<u64>) -> Result<String> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let length = file.metadata()?.len().min(limit.unwrap_or(u64::MAX));
    hash_reader_sized(
        file.take(length),
        algorithm,
        length.min(BUFFER_BYTES as u64).max(1) as usize,
    )
    .with_context(|| format!("hashing {}", path.display()))
}

#[cfg(test)]
fn hash_reader(mut reader: impl Read, algorithm: Algorithm) -> Result<String> {
    hash_reader_sized(&mut reader, algorithm, BUFFER_BYTES)
}

fn hash_reader_sized(
    mut reader: impl Read,
    algorithm: Algorithm,
    buffer_size: usize,
) -> Result<String> {
    let mut hasher = match algorithm {
        Algorithm::Blake3 => Hasher::Blake3(Box::new(blake3::Hasher::new())),
        Algorithm::Xxh3 => Hasher::Xxh3(Box::new(xxhash_rust::xxh3::Xxh3::new())),
        Algorithm::Crc32 => Hasher::Crc32(crc32fast::Hasher::new()),
        Algorithm::Md5 => Hasher::Md5(md5::Md5::new()),
        Algorithm::Sha1 => Hasher::Sha1(sha1::Sha1::new()),
        Algorithm::Sha256 => Hasher::Sha256(sha2::Sha256::new()),
    };
    let mut buffer = vec![0; buffer_size];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let bytes = &buffer[..count];
        match &mut hasher {
            Hasher::Blake3(h) => {
                h.update(bytes);
            }
            Hasher::Xxh3(h) => h.update(bytes),
            Hasher::Crc32(h) => h.update(bytes),
            Hasher::Md5(h) => Digest::update(h, bytes),
            Hasher::Sha1(h) => Digest::update(h, bytes),
            Hasher::Sha256(h) => Digest::update(h, bytes),
        }
    }
    Ok(match hasher {
        Hasher::Blake3(h) => h.finalize().to_hex().to_string(),
        Hasher::Xxh3(h) => format!("{:032x}", h.digest128()),
        Hasher::Crc32(h) => format!("{:08x}", h.finalize()),
        Hasher::Md5(h) => format!("{:x}", h.finalize()),
        Hasher::Sha1(h) => format!("{:x}", h.finalize()),
        Hasher::Sha256(h) => format!("{:x}", h.finalize()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        let cases = [
            (
                Algorithm::Blake3,
                "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
            ),
            (Algorithm::Xxh3, "06b05ab6733a618578af5f94892f3950"),
            (Algorithm::Crc32, "352441c2"),
            (Algorithm::Md5, "900150983cd24fb0d6963f7d28e17f72"),
            (Algorithm::Sha1, "a9993e364706816aba3e25717850c26c9cd0d89d"),
            (
                Algorithm::Sha256,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
        ];
        for (algorithm, expected) in cases {
            assert_eq!(hash_reader(&b"abc"[..], algorithm).unwrap(), expected);
        }
    }
}
