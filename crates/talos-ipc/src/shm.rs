//! Shared memory region abstraction (backed by memmap2)
//!
//! Uses memory-mapped files instead of POSIX shared memory (`shm_open`),
//! providing a pure Rust implementation that works on any platform.
//! Files are stored under `/tmp/` with the given logical name.

use memmap2::MmapMut;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;

/// Errors that can occur during shared memory operations
#[derive(Debug)]
pub enum ShmError {
    /// An underlying I/O error occurred (e.g., file creation, write)
    IoError(io::Error),
    /// Memory mapping the file failed
    MapFailed,
    /// The existing shared memory region is too small for the expected layout
    InvalidSize,
}

impl std::fmt::Display for ShmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShmError::IoError(e) => write!(f, "IO error: {}", e),
            ShmError::MapFailed => write!(f, "mmap failed"),
            ShmError::InvalidSize => write!(f, "invalid size"),
        }
    }
}

impl std::error::Error for ShmError {}

impl From<io::Error> for ShmError {
    fn from(e: io::Error) -> Self {
        ShmError::IoError(e)
    }
}

/// Get the filesystem path for a shared memory region
///
/// Strips any leading `/` from the name and places the file under `/tmp/`.
fn shm_path(name: &str) -> PathBuf {
    // Use /tmp directory, strip leading '/'
    let clean_name = name.trim_start_matches('/');
    PathBuf::from("/tmp").join(clean_name)
}

/// RAII wrapper around a memory-mapped shared memory region
///
/// On creation (via [`create`](ShmRegion::create)), the file is created,
/// zero-filled, and memory-mapped. On drop, the file is deleted if this
/// region is the owner (i.e., the producer).
///
/// # Safety
///
/// This type is `Send` and `Sync` because the underlying `MmapMut` provides
/// exclusive mutable access, and the caller is responsible for ensuring safe
/// concurrent access patterns.
pub struct ShmRegion {
    /// The memory-mapped file
    mmap: MmapMut,
    /// Path to the backing file (for cleanup on drop)
    path: PathBuf,
    /// Whether this region owns the file (producer deletes on drop)
    is_owner: bool,
}

unsafe impl Send for ShmRegion {}
unsafe impl Sync for ShmRegion {}

impl ShmRegion {
    /// Create a new shared memory region (producer side)
    ///
    /// Creates (or truncates) the backing file, writes `size` zero bytes,
    /// syncs the file to disk, then memory-maps it for read/write access.
    ///
    /// # Arguments
    ///
    /// * `name` - Logical name of the shared memory region (used as filename)
    /// * `size` - Size of the region in bytes
    ///
    /// # Errors
    ///
    /// Returns `ShmError` if file creation, writing, or mmap fails.
    pub fn create(name: &str, size: usize) -> Result<Self, ShmError> {
        let path = shm_path(name);

        // Create or truncate the backing file
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;

        // Set file size
        file.set_len(size as u64)?;

        // Write zero fill to ensure the file is backed by real disk space
        file.write_all(&vec![0u8; size])?;
        file.sync_all()?;

        // Re-open and memory-map
        let file = OpenOptions::new().read(true).write(true).open(&path)?;

        let mmap = unsafe { MmapMut::map_mut(&file)? };

        Ok(Self {
            mmap,
            path,
            is_owner: true,
        })
    }

    /// Open an existing shared memory region (consumer side)
    ///
    /// Opens the backing file for read/write and memory-maps it. Verifies that
    /// the file is at least `size` bytes long.
    ///
    /// # Arguments
    ///
    /// * `name` - Logical name of the shared memory region
    /// * `size` - Expected minimum size of the region
    ///
    /// # Errors
    ///
    /// Returns `ShmError::InvalidSize` if the file is smaller than `size`.
    pub fn open(name: &str, size: usize) -> Result<Self, ShmError> {
        let path = shm_path(name);

        let file = OpenOptions::new().read(true).write(true).open(&path)?;

        // Verify size matches expected layout
        let metadata = file.metadata()?;
        if metadata.len() < size as u64 {
            return Err(ShmError::InvalidSize);
        }

        let mmap = unsafe { MmapMut::map_mut(&file)? };

        Ok(Self {
            mmap,
            path,
            is_owner: false,
        })
    }

    /// Get a raw pointer to the shared memory
    pub fn as_ptr(&self) -> *mut u8 {
        self.mmap.as_ptr() as *mut u8
    }

    /// Get the size of the shared memory region
    pub fn size(&self) -> usize {
        self.mmap.len()
    }

    /// Interpret the shared memory as a reference to type `T`
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The memory layout of `T` matches the data in shared memory
    /// - `T` is `#[repr(C)]` and properly aligned
    /// - No mutable aliases exist concurrently
    pub unsafe fn as_ref<T>(&self) -> &T {
        unsafe { &*(self.mmap.as_ptr() as *const T) }
    }

    /// Interpret the shared memory as a mutable reference to type `T`
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The memory layout of `T` matches the data in shared memory
    /// - `T` is `#[repr(C)]` and properly aligned
    /// - No other references (mutable or immutable) exist concurrently
    pub unsafe fn as_mut<T>(&mut self) -> &mut T {
        unsafe { &mut *(self.mmap.as_ptr() as *mut T) }
    }

    /// Flush the memory mapping to the backing file
    pub fn flush(&self) -> Result<(), ShmError> {
        self.mmap.flush()?;
        Ok(())
    }
}

impl Drop for ShmRegion {
    /// On drop, delete the backing file if this region is the owner
    ///
    /// Only the producer (the creator) deletes the file. The consumer
    /// opens the file without ownership, so it does not delete it.
    fn drop(&mut self) {
        if self.is_owner {
            // Delete the backing file from /tmp
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
