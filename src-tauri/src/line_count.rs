//! Lines of a file git does not track, counted the way `git diff --numstat`
//! would count them if the file were added, without asking git to add it.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

/// git decides a file is binary when its first 8000 bytes contain a NUL.
const BINARY_SNIFF_BYTES: usize = 8000;

/// Files above git's default `core.bigFileThreshold` are diffed as binary.
const BIG_FILE_BYTES: u64 = 512 * 1024 * 1024;

const READ_BUFFER_BYTES: usize = 64 * 1024;

/// What an untracked file adds, as `git diff --numstat` would report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddedFile {
    pub lines: u32,
    /// Binary files add no lines.
    pub binary: bool,
}

/// Number of lines `path` adds. Binary files, files above the big file
/// threshold and anything that is not a regular file or a symlink count 0. A
/// symlink counts as the one line of its target, as git stores it.
pub fn added_lines(path: &Path) -> io::Result<u32> {
    added_file(path).map(|added| added.lines)
}

/// Lines `path` adds and whether git would diff it as binary: a NUL in its
/// first 8000 bytes, or a size above the big file threshold.
pub fn added_file(path: &Path) -> io::Result<AddedFile> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(AddedFile {
            lines: 1,
            binary: false,
        });
    }
    if !metadata.is_file() {
        return Ok(AddedFile {
            lines: 0,
            binary: false,
        });
    }
    if metadata.len() > BIG_FILE_BYTES {
        return Ok(AddedFile {
            lines: 0,
            binary: true,
        });
    }
    let lines = count_lines(File::open(path)?)?;
    Ok(AddedFile {
        lines: lines.unwrap_or(0),
        binary: lines.is_none(),
    })
}

/// git's binary test: a NUL anywhere in the first 8000 bytes.
pub fn looks_binary(content: &[u8]) -> bool {
    content[..content.len().min(BINARY_SNIFF_BYTES)].contains(&0)
}

/// Lines in `reader`, or `None` when it is binary.
fn count_lines(mut reader: impl Read) -> io::Result<Option<u32>> {
    let mut buffer = vec![0u8; READ_BUFFER_BYTES];
    let mut lines = 0u32;
    let mut sniffed = 0usize;
    let mut last_byte = None;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let chunk = &buffer[..read];
        if sniffed < BINARY_SNIFF_BYTES {
            let window = chunk.len().min(BINARY_SNIFF_BYTES - sniffed);
            if chunk[..window].contains(&0) {
                return Ok(None);
            }
            sniffed += window;
        }
        let newlines = chunk.iter().filter(|byte| **byte == b'\n').count();
        lines = lines.saturating_add(u32::try_from(newlines).unwrap_or(u32::MAX));
        last_byte = chunk.last().copied();
    }
    // A last line without a newline still counts.
    if last_byte.is_some_and(|byte| byte != b'\n') {
        lines = lines.saturating_add(1);
    }
    Ok(Some(lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(content: &[u8]) -> u32 {
        count_lines(content).unwrap().unwrap_or(0)
    }

    #[test]
    fn counts_lines_like_git() {
        assert_eq!(lines(b""), 0);
        assert_eq!(lines(b"one"), 1);
        assert_eq!(lines(b"one\n"), 1);
        assert_eq!(lines(b"one\ntwo"), 2);
        assert_eq!(lines(b"\n\n\n"), 3);
        assert_eq!(lines(b"crlf\r\nline\r\n"), 2);
    }

    #[test]
    fn a_nul_in_the_first_8000_bytes_makes_a_file_binary() {
        assert_eq!(count_lines(&b"text\0more\n"[..]).unwrap(), None);
        assert!(looks_binary(b"text\0more\n"));
        let mut late_nul = vec![b'a'; BINARY_SNIFF_BYTES];
        late_nul.extend_from_slice(b"\0\nlast");
        assert_eq!(lines(&late_nul), 2);
        assert!(!looks_binary(&late_nul));
        assert!(!looks_binary(b""));
    }

    #[test]
    fn counts_across_read_buffers() {
        let content = "line\n".repeat(READ_BUFFER_BYTES / 2);
        assert_eq!(
            lines(content.as_bytes()),
            u32::try_from(READ_BUFFER_BYTES / 2).unwrap()
        );
    }

    #[test]
    fn reads_files_from_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let text = dir.path().join("text.txt");
        fs::write(&text, "a\nb\n").unwrap();
        assert_eq!(added_lines(&text).unwrap(), 2);
        assert_eq!(added_lines(dir.path()).unwrap(), 0);
        let binary = dir.path().join("binary.dat");
        fs::write(&binary, b"a\n\0b\n").unwrap();
        assert_eq!(
            added_file(&binary).unwrap(),
            AddedFile {
                lines: 0,
                binary: true
            }
        );
        assert_eq!(
            added_file(&text).unwrap(),
            AddedFile {
                lines: 2,
                binary: false
            }
        );
        assert!(added_lines(&dir.path().join("missing")).is_err());
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&text, &link).unwrap();
            assert_eq!(added_lines(&link).unwrap(), 1);
        }
    }
}
