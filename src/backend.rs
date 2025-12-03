//! Backend storage interface for virtio-blk device.

use alloc::vec::Vec;
use alloc::vec;
use axerrno::{AxResult, ax_err, ax_err_type};
use memory_addr::PhysAddr;

/// Trait for backend storage implementations
pub trait BlockBackend: Send + Sync {
    /// Read data from the backend storage
    fn read(&self, offset: u64, data: &mut [u8]) -> AxResult<usize>;
    
    /// Write data to the backend storage
    fn write(&self, offset: u64, data: &[u8]) -> AxResult<usize>;
    
    /// Get the size of the backend storage in bytes
    fn size(&self) -> u64;
    
    /// Flush any pending writes
    fn flush(&self) -> AxResult;
    
    /// Get the block size (sector size)
    fn block_size(&self) -> u64 {
        512 // Default to 512 bytes
    }
}

#[cfg(feature = "fs")]
mod fs_backend {
    use super::*;
    use alloc::sync::Arc;
    use spin::Mutex;
    use axstd::fs::File;
    use axstd::io::{Read, Write, Seek, SeekFrom};

    /// File-based backend storage
    pub struct FileBackend {
        file: Arc<Mutex<File>>,
        size: u64,
        block_size: u64,
    }

    impl FileBackend {
        /// Create a new file backend from a file path
        pub fn new(path: &str) -> AxResult<Self> {
            let file = File::open(path).map_err(|e| {
                ax_err_type!(
                    NotFound,
                    format!("Failed to open backend file {}: {:?}", path, e)
                )
            })?;
            
            let metadata = file.metadata().map_err(|e| {
                ax_err_type!(
                    Io,
                    format!("Failed to get file metadata: {:?}", e)
                )
            })?;
            
            let size = metadata.len();
            let block_size = 512; // Default sector size
            
            Ok(Self {
                file: Arc::new(Mutex::new(file)),
                size,
                block_size,
            })
        }
    }

    impl BlockBackend for FileBackend {
        fn read(&self, offset: u64, data: &mut [u8]) -> AxResult<usize> {
            let mut file = self.file.lock();
            file.seek(SeekFrom::Start(offset)).map_err(|e| {
                ax_err_type!(Io, format!("Failed to seek in file: {:?}", e))
            })?;
            
            let bytes_read = file.read(data).map_err(|e| {
                ax_err_type!(Io, format!("Failed to read from file: {:?}", e))
            })?;
            
            Ok(bytes_read)
        }
        
        fn write(&self, offset: u64, data: &[u8]) -> AxResult<usize> {
            let mut file = self.file.lock();
            file.seek(SeekFrom::Start(offset)).map_err(|e| {
                ax_err_type!(Io, format!("Failed to seek in file: {:?}", e))
            })?;
            
            let bytes_written = file.write(data).map_err(|e| {
                ax_err_type!(Io, format!("Failed to write to file: {:?}", e))
            })?;
            
            Ok(bytes_written)
        }
        
        fn size(&self) -> u64 {
            self.size
        }
        
        fn flush(&self) -> AxResult {
            let mut file = self.file.lock();
            file.flush().map_err(|e| {
                ax_err_type!(Io, format!("Failed to flush file: {:?}", e))
            })?;
            Ok(())
        }
        
        fn block_size(&self) -> u64 {
            self.block_size
        }
    }
}

#[cfg(feature = "fs")]
pub use fs_backend::FileBackend;

/// Memory-based backend for testing (always available)
pub struct MemoryBackend {
    data: spin::Mutex<Vec<u8>>,
    block_size: u64,
}

impl MemoryBackend {
    /// Create a new memory backend with specified size
    pub fn new(size: u64) -> Self {
        Self {
            data: spin::Mutex::new(vec![0u8; size as usize]),
            block_size: 512,
        }
    }
}

impl BlockBackend for MemoryBackend {
    fn read(&self, offset: u64, data: &mut [u8]) -> AxResult<usize> {
        let backend = self.data.lock();
        let offset = offset as usize;
        
        if offset >= backend.len() {
            return Ok(0);
        }
        
        let available = backend.len() - offset;
        let to_read = available.min(data.len());
        
        data[..to_read].copy_from_slice(&backend[offset..offset + to_read]);
        Ok(to_read)
    }
    
    fn write(&self, offset: u64, data: &[u8]) -> AxResult<usize> {
        let mut backend = self.data.lock();
        let offset = offset as usize;
        
        // Extend if necessary
        if offset + data.len() > backend.len() {
            backend.resize(offset + data.len(), 0);
        }
        
        backend[offset..offset + data.len()].copy_from_slice(data);
        Ok(data.len())
    }
    
    fn size(&self) -> u64 {
        self.data.lock().len() as u64
    }
    
    fn flush(&self) -> AxResult {
        Ok(())
    }
    
    fn block_size(&self) -> u64 {
        self.block_size
    }
}

