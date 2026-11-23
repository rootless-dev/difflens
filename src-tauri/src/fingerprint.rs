//! Fingerprints of reviewed content, stored with a mark so a later look can
//! tell whether the content changed since. They are compared for equality
//! only, so each kind of content carries a prefix: a git object id and a
//! hash DiffLens computed can never be mistaken for each other.

use std::io::{self, Read};

use sha2::{Digest, Sha256};

/// What a file or function that does not exist on the side looked at gets.
pub const ABSENT: &str = "absent";

/// A git object id, as the tree records it: a blob, or a submodule's commit.
pub fn of_object(oid: &str) -> String {
    format!("git:{oid}")
}

pub fn of_bytes(bytes: &[u8]) -> String {
    hex(Sha256::digest(bytes).as_slice())
}

/// Hashes everything `reader` yields, without holding it in memory.
pub fn of_reader(mut reader: impl Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(hasher.finalize().as_slice()))
}

/// Lines `start` to `end` (1-based, both included) of `text`, the source of
/// a function as the analyzer located it. Line ends are left out (`lines`
/// drops `\r\n` too), so a file whose line endings alone changed still reads
/// the same.
pub fn of_lines(text: &str, start: u32, end: u32) -> String {
    let skip = start.saturating_sub(1) as usize;
    let take = end.saturating_sub(start).saturating_add(1) as usize;
    let mut hasher = Sha256::new();
    for line in text.lines().skip(skip).take(take) {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    hex(hasher.finalize().as_slice())
}

fn hex(digest: &[u8]) -> String {
    let mut text = String::with_capacity(7 + digest.len() * 2);
    text.push_str("sha256:");
    for byte in digest {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_and_readers_agree_and_are_stable() {
        // The well known SHA-256 of "abc", so stored fingerprints stay valid.
        let expected = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(of_bytes(b"abc"), expected);
        assert_eq!(of_reader(&b"abc"[..]).unwrap(), expected);
        let large = vec![7u8; 200_000];
        assert_eq!(of_reader(large.as_slice()).unwrap(), of_bytes(&large));
    }

    #[test]
    fn object_ids_never_collide_with_hashes() {
        assert_eq!(of_object("abc"), "git:abc");
        assert_ne!(of_object("abc"), of_bytes(b"abc"));
    }

    #[test]
    fn lines_hash_only_the_function_they_span() {
        let before = "package a\n\nfunc A() {\n\treturn\n}\n\nfunc B() {}\n";
        let after = "package a\n// moved down\n\nfunc A() {\n\treturn\n}\n\nfunc B() { x() }\n";
        assert_eq!(of_lines(before, 3, 5), of_lines(after, 4, 6));
        assert_ne!(of_lines(before, 7, 7), of_lines(after, 8, 8));
        assert_eq!(of_lines("a\r\nb\r\n", 1, 2), of_lines("a\nb\n", 1, 2));
        // A span past the end hashes what is there.
        assert_eq!(of_lines("a\nb\n", 2, 9), of_lines("b\n", 1, 1));
        // The SHA-256 of "abc\n", so stored function fingerprints stay valid.
        assert_eq!(
            of_lines("abc\r\ndef\n", 1, 1),
            "sha256:edeaaff3f1774ad2888673770c6d64097e391bc362d7d6fb34982ddf0efd18cb"
        );
    }
}
