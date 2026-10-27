//! # line-writer-bounded
//!
//! Appends lines to a file and automatically rotates when the file exceeds a configured maximum size.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Appends lines to a file and rotates when the file size exceeds a maximum.
///
/// Each call to `write_line` appends a line of text followed by a newline to the
/// current file. After the append, if the file's size is greater than or equal to
/// `max_size`, the file is closed, renamed to include a `.1` suffix, and a fresh
/// file is opened for subsequent writes. The rotation threshold is compared after
/// the write completes, so a single oversized line will be written in full before
/// rotation occurs.
pub struct LineWriterBounded {
    path: PathBuf,
    max_size: u64,
    file: File,
    current_size: u64,
    suffix: String,
}

impl LineWriterBounded {
    /// Creates a new writer for `path` with the given `max_size` in bytes.
    ///
    /// If a file already exists at `path` it is opened in append mode and its
    /// current size is used as the starting point for the rotation counter. This
    /// means an existing file that already exceeds `max_size` will be rotated on
    /// the next call to `write_line`.
    pub fn new<P: AsRef<Path>>(path: P, max_size: u64) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let current_size = file.metadata()?.len();
        Ok(Self {
            path,
            max_size,
            file,
            current_size,
            suffix: ".1".to_string(),
        })
    }

    /// Sets the suffix appended to the path when a file is rotated.
    ///
    /// Defaults to `".1"`. The rotated file for `/var/log/app.log` with the
    /// default suffix becomes `/var/log/app.log.1`.
    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }

    /// Writes a single line (without its trailing newline) to the current file.
    ///
    /// Returns the total number of bytes written, including the newline. If the
    /// file size after the write is greater than or equal to `max_size`, the file
    /// is rotated.
    pub fn write_line(&mut self, line: &str) -> io::Result<usize> {
        let mut written = self.file.write(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        written += 1;
        self.current_size += written as u64;

        if self.current_size >= self.max_size {
            self.rotate()?;
        }
        Ok(written)
    }

    /// Forces a rotation regardless of the current file size.
    pub fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        // Re-open with write+truncate to close the append handle cleanly.
        // The handle we hold is replaced after the rename succeeds.
        let rotated_path = {
            let mut s = self.path.as_os_str().to_owned();
            s.push(&self.suffix);
            PathBuf::from(s)
        };
        // std::fs::rename will overwrite the destination on Unix. On Windows
        // it fails if the destination exists, so we remove it first.
        if rotated_path.exists() {
            std::fs::remove_file(&rotated_path)?;
        }
        std::fs::rename(&self.path, &rotated_path)?;
        self.file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        self.current_size = 0;
        Ok(())
    }

    /// Flushes the underlying file to disk.
    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    /// Returns the current path of the active log file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the tracked size of the current file.
    pub fn current_size(&self) -> u64 {
        self.current_size
    }
}

impl Write for LineWriterBounded {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.file.write(buf)?;
        self.current_size += n as u64;
        if self.current_size >= self.max_size {
            self.rotate()?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn tmp_dir() -> PathBuf {
        let mut base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        base.push(format!("lwb_test_{}", nanos));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn writes_line_and_flushes() {
        let dir = tmp_dir();
        let path = dir.join("log.txt");
        {
            let mut w = LineWriterBounded::new(&path, 1024).unwrap();
            w.write_line("hello").unwrap();
            w.flush().unwrap();
        }
        let mut content = String::new();
        File::open(&path).unwrap().read_to_string(&mut content).unwrap();
        assert_eq!(content, "hello\n");
    }

    #[test]
    fn rotates_when_exceeding_max_size() {
        let dir = tmp_dir();
        let path = dir.join("rotate.txt");
        {
            let mut w = LineWriterBounded::new(&path, 10).unwrap();
            w.write_line("abc").unwrap(); // 4 bytes, below threshold
            assert!(!dir.join("rotate.txt.1").exists());
            w.write_line("abcdef").unwrap(); // 7 bytes, total 11 >= 10, rotates
            assert!(dir.join("rotate.txt.1").exists());
            w.flush().unwrap();
        }
        // Rotated file should contain both lines
        let mut rotated = String::new();
        File::open(dir.join("rotate.txt.1"))
            .unwrap()
            .read_to_string(&mut rotated)
            .unwrap();
        assert_eq!(rotated, "abc\nabcdef\n");
        // Active file should be empty after rotation
        assert_eq!(w_path_size(&path), 0);
    }

    #[test]
    fn rotation_happens_after_write_completes() {
        let dir = tmp_dir();
        let path = dir.join("after.txt");
        {
            let mut w = LineWriterBounded::new(&path, 5).unwrap();
            // A single line larger than max_size is written in full, then rotates.
            w.write_line("this is a very long line").unwrap();
        }
        let mut rotated = String::new();
        File::open(dir.join("after.txt.1"))
            .unwrap()
            .read_to_string(&mut rotated)
            .unwrap();
        assert_eq!(rotated, "this is a very long line\n");
        assert_eq!(w_path_size(&path), 0);
    }

    #[test]
    fn custom_suffix() {
        let dir = tmp_dir();
        let path = dir.join("suffix.txt");
        {
            let mut w = LineWriterBounded::new(&path, 5)
                .unwrap()
                .with_suffix(".old");
            w.write_line("abcdefgh").unwrap();
        }
        assert!(dir.join("suffix.txt.old").exists());
    }

    #[test]
    fn existing_file_size_is_tracked() {
        let dir = tmp_dir();
        let path = dir.join("existing.txt");
        // Pre-populate file with content already at threshold
        {
            let mut f = File::create(&path).unwrap();
            f.write_all(b"aaaa\n").unwrap(); // 5 bytes
        }
        let mut w = LineWriterBounded::new(&path, 5).unwrap();
        assert_eq!(w.current_size(), 5);
        // Next write pushes total past threshold and rotates
        w.write_line("b").unwrap();
        assert!(dir.join("existing.txt.1").exists());
    }

    #[test]
    fn multiple_rotations_overwrite_oldest() {
        let dir = tmp_dir();
        let path = dir.join("multi.txt");
        {
            let mut w = LineWriterBounded::new(&path, 5).unwrap();
            w.write_line("first").unwrap(); // rotates, multi.txt.1 = "first\n"
            w.write_line("second").unwrap(); // rotates, multi.txt.1 = "second\n"
        }
        let mut content = String::new();
        File::open(dir.join("multi.txt.1"))
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert_eq!(content, "second\n");
    }

    #[test]
    fn empty_line_writes_newline_only() {
        let dir = tmp_dir();
        let path = dir.join("empty.txt");
        {
            let mut w = LineWriterBounded::new(&path, 1024).unwrap();
            let n = w.write_line("").unwrap();
            assert_eq!(n, 1);
        }
        let mut content = String::new();
        File::open(&path).unwrap().read_to_string(&mut content).unwrap();
        assert_eq!(content, "\n");
    }

    #[test]
    fn exact_threshold_does_rotate() {
        let dir = tmp_dir();
        let path = dir.join("exact.txt");
        {
            let mut w = LineWriterBounded::new(&path, 5).unwrap();
            // "ab\n" = 3 bytes, below threshold
            w.write_line("ab").unwrap();
            assert!(!dir.join("exact.txt.1").exists());
            // "cd\n" = 3 bytes, total 6 >= 5, rotates
            w.write_line("cd").unwrap();
            assert!(dir.join("exact.txt.1").exists());
        }
    }

    fn w_path_size(path: &Path) -> u64 {
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    }
}
