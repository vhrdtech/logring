//! [ImageFile]: a ring image, a ring file copied off a card, or a split `.wlr` as a read-only [LogMedium] (`std`),
//! for the [Reader](crate::reader::Reader) on a PC. No raw device access, no root.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use ww_stream::store::LogMedium;

/// A file read as a medium; writes and erases fail.
pub struct ImageFile {
    file: File,
    len: u64,
}

impl ImageFile {
    /// Open `path` read-only.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(ImageFile { file, len })
    }
}

fn read_only() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "the image is opened read-only")
}

impl LogMedium for ImageFile {
    type Error = io::Error;
    const WRITE_SIZE: usize = 512;
    const ERASE_SIZE: usize = 512;
    const NEEDS_ERASE: bool = false;

    fn capacity(&self) -> u64 {
        self.len
    }

    async fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), io::Error> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buf)
    }

    async fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), io::Error> {
        Err(read_only())
    }

    async fn erase(&mut self, _from: u64, _to: u64) -> Result<(), io::Error> {
        Err(read_only())
    }
}
