//! Virtio-blk device implementation.

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::vec;

use log::{error, info};
use spin::Mutex;

use axaddrspace::{GuestPhysAddr, GuestPhysAddrRange};
use axdevice_base::{BaseDeviceOps, EmuDeviceType};
use axerrno::{AxResult, ax_err_type, AxErrorKind, ax_err};
use axaddrspace::device::AccessWidth;

use axvmconfig::VirtioBlkMmioDeviceConfig;

use crate::backend::BlockBackend;
use crate::virtio::{
    blk, blk_status, desc_flags,
    features, mmio, VIRTIO_ID_BLOCK,
};

#[cfg(feature = "fs")]
use crate::backend::FileBackend;
use crate::backend::MemoryBackend;

/// Virtio descriptor structure (16 bytes)
#[derive(Clone, Copy)]
#[repr(C, packed)]
struct VirtqDescriptor {
    addr: u64,      // Guest physical address
    len: u32,       // Length
    flags: u16,     // Flags
    next: u16,      // Next descriptor index
}

/// Virtio available ring structure
#[repr(C, packed)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; 0], // Variable length array
}

/// Virtio used ring entry
#[repr(C, packed)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

/// Virtio used ring structure
#[repr(C, packed)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; 0], // Variable length array
}

/// Virtio-blk request header
#[repr(C, packed)]
struct VirtioBlkReq {
    req_type: u32,
    reserved: u32,
    sector: u64,
}

/// Virtio-blk request with status
#[repr(C, packed)]
struct VirtioBlkReqWithStatus {
    req_type: u32,
    reserved: u32,
    sector: u64,
    status: u8,
}

/// Virtio queue state
struct VirtQueue {
    size: u16,
    desc: GuestPhysAddr,  // Descriptor table address
    avail: GuestPhysAddr, // Available ring address
    used: GuestPhysAddr,  // Used ring address
    ready: bool,
}

impl VirtQueue {
    fn new() -> Self {
        Self {
            size: 0,
            desc: GuestPhysAddr::from(0),
            avail: GuestPhysAddr::from(0),
            used: GuestPhysAddr::from(0),
            ready: false,
        }
    }
}

/// Virtio-blk device state (mutable part)
struct VirtioBlkState {
    // MMIO registers
    device_features_sel: u32,
    driver_features_sel: u32,
    queue_sel: u16,
    queue_num: u16,
    device_status: u32,
    interrupt_status: u32,
    
    // Queue state
    queue: VirtQueue,
    // Track last processed avail index
    last_avail_idx: u16,
}

/// Virtio-blk device
pub struct VirtioBlkDevice {
    base_gpa: GuestPhysAddr,
    size: usize,
    irq_id: usize,
    
    // Mutable state protected by Mutex
    state: Mutex<VirtioBlkState>,
    
    // Backend storage
    backend: Arc<dyn BlockBackend>,
    
    // Helper for reading guest memory
    read_guest_mem: Arc<dyn Fn(GuestPhysAddr, usize) -> AxResult<Vec<u8>> + Send + Sync>,
    write_guest_mem: Arc<dyn Fn(GuestPhysAddr, &[u8]) -> AxResult<()> + Send + Sync>,
    
    // Interrupt injection callback
    inject_irq: Arc<dyn Fn(usize) -> AxResult + Send + Sync>,
}

impl VirtioBlkDevice {
    /// Create a new virtio-blk device
    /// 
    /// # Arguments
    /// * `config` - The configuration for the virtio-blk device
    /// * `read_guest_mem` - Function to read guest memory
    /// * `write_guest_mem` - Function to write guest memory
    /// * `inject_irq` - Function to inject interrupt
    pub fn new(
        config: &VirtioBlkMmioDeviceConfig,
        read_guest_mem: Arc<dyn Fn(GuestPhysAddr, usize) -> AxResult<Vec<u8>> + Send + Sync>,
        write_guest_mem: Arc<dyn Fn(GuestPhysAddr, &[u8]) -> AxResult<()> + Send + Sync>,
        inject_irq: Arc<dyn Fn(usize) -> AxResult + Send + Sync>,
    ) -> AxResult<Self> {
        let base_gpa = GuestPhysAddr::from(
            usize::from_str_radix(config.mmio_base.trim_start_matches("0x"), 16)
                .map_err(|_| ax_err_type!(InvalidInput, "Invalid MMIO base address"))?
        );
        let size = usize::from_str_radix(config.mmio_size.trim_start_matches("0x"), 16)
            .map_err(|_| ax_err_type!(InvalidInput, "Invalid MMIO size"))?;
        let irq_id = config.interrupt_number;

        let backend: Arc<dyn BlockBackend> = if !config.backend_path.is_empty() {
            #[cfg(feature = "fs")]
            {
                Arc::new(FileBackend::new(&config.backend_path)?)
            }
            #[cfg(not(feature = "fs"))]
            {
                // If backend path is specified but fs feature is disabled, we can't support it
                // unless it's a special "memory" backend path or similar logic.
                // For now, let's assume if path is not empty and no fs, it's an error or fallback.
                // But to be safe and modular, let's warn and fallback to memory or error.
                // Given the user requirement, let's error if fs is missing but path is provided.
                return ax_err!(
                    Unsupported,
                    "File backend requires 'fs' feature to be enabled"
                );
            }
        } else {
            // Default to memory backend with 1GB
            Arc::new(MemoryBackend::new(1024 * 1024 * 1024))
        };
        
        log::info!("VirtioBlkDevice initialized with memory backend");
        
        Ok(Self {
            base_gpa,
            size,
            irq_id,
            state: Mutex::new(VirtioBlkState {
                device_features_sel: 0,
                driver_features_sel: 0,
                queue_sel: 0,
                queue_num: 0,
                device_status: 0,
                interrupt_status: 0,
                queue: VirtQueue::new(),
                last_avail_idx: 0,
            }),
            backend,
            read_guest_mem,
            write_guest_mem,
            inject_irq,
        })
    }
    
    /// Read from guest memory
    fn read_guest(&self, addr: GuestPhysAddr, len: usize) -> AxResult<Vec<u8>> {
        (self.read_guest_mem)(addr, len)
    }
    
    /// Write to guest memory
    fn write_guest(&self, addr: GuestPhysAddr, data: &[u8]) -> AxResult<()> {
        (self.write_guest_mem)(addr, data)
    }
    
    /// Get device features
    fn get_device_features(&self) -> u64 {
        // Return supported features
        features::VIRTIO_F_VERSION_1
            | features::VIRTIO_BLK_F_SIZE_MAX
            | features::VIRTIO_BLK_F_SEG_MAX
            | features::VIRTIO_BLK_F_GEOMETRY
            | features::VIRTIO_BLK_F_BLK_SIZE
            | features::VIRTIO_BLK_F_FLUSH
    }
    
    /// Process virtio queue notification
    fn process_queue(&self) -> AxResult {
        let mut state = self.state.lock();
        if !state.queue.ready {
            info!("process_queue: queue not ready");
            return Ok(());
        }
        
        // Read available ring
        let avail_ring = self.read_guest(state.queue.avail, 
            (4 + state.queue.size as usize * 2) as usize)?;
        
        let avail_idx = u16::from_le_bytes([
            avail_ring[2],
            avail_ring[3],
        ]);
        
        // Only process new requests since last_avail_idx
        let last_avail_idx = state.last_avail_idx;
        let queue_size = state.queue.size;
        
        info!("process_queue: avail_idx={}, last_avail_idx={}, queue_size={}", 
              avail_idx, last_avail_idx, queue_size);
        
        // Calculate number of new requests (handle wrap-around)
        let num_new = avail_idx.wrapping_sub(last_avail_idx);
        if num_new == 0 {
            info!("process_queue: no new requests");
            return Ok(());
        }
        
        info!("process_queue: processing {} new requests", num_new);
        
        // Process each new available descriptor
        let mut processed = 0u16;
        while processed < num_new {
            // Calculate ring index (with wrap-around)
            let ring_slot = (last_avail_idx.wrapping_add(processed)) % queue_size;
            let ring_offset = 4 + ring_slot as usize * 2;
            
            if ring_offset + 1 >= avail_ring.len() {
                break;
            }
            
            let desc_idx = u16::from_le_bytes([
                avail_ring[ring_offset],
                avail_ring[ring_offset + 1],
            ]);
            
            // Process the request
            drop(state); // Release lock before processing
            self.process_request(desc_idx)?;
            
            // Update used ring for this request
            self.update_used_ring(1, desc_idx)?;
            
            state = self.state.lock();
            processed += 1;
        }
        
        // Update last_avail_idx
        state.last_avail_idx = avail_idx;
        
        Ok(())
    }
    
    /// Process a single virtio-blk request
    fn process_request(&self, head_desc_idx: u16) -> AxResult {
        info!("process_request: head_desc_idx={}", head_desc_idx);
        
        // Get queue info while holding lock, then release immediately
        let (queue_desc, queue_size) = {
            let state = self.state.lock();
            (state.queue.desc, state.queue.size)
        };
        
        // Read descriptor table (lock released)
        let desc_size = core::mem::size_of::<VirtqDescriptor>();
        let desc_table_size = desc_size * queue_size as usize;
        info!("process_request: reading desc table at {:?}, size={}", queue_desc, desc_table_size);
        let desc_table = self.read_guest(queue_desc, desc_table_size)?;
        info!("process_request: desc table read ok");
        
        // Read the head descriptor
        let head_offset = head_desc_idx as usize * desc_size;
        if head_offset + desc_size > desc_table.len() {
            return ax_err!(InvalidInput, "Invalid descriptor index");
        }
        
        let head_desc = unsafe {
            core::ptr::read_unaligned(
                desc_table.as_ptr().add(head_offset) as *const VirtqDescriptor
            )
        };
        let desc_addr = head_desc.addr;
        let desc_len = head_desc.len;
        let desc_flags = head_desc.flags;
        info!("process_request: head_desc addr={:#x}, len={}, flags={:#x}", 
              desc_addr, desc_len, desc_flags);
        
        // Read request header (first descriptor)
        info!("process_request: reading req header from GPA {:#x}", desc_addr);
        let req_buf = self.read_guest(
            GuestPhysAddr::from(desc_addr as usize),
            desc_len as usize
        )?;
        info!("process_request: req header read ok, len={}", req_buf.len());
        
        if req_buf.len() < core::mem::size_of::<VirtioBlkReq>() {
            return ax_err!(InvalidInput, "Request too short");
        }
        
        let req = unsafe {
            core::ptr::read_unaligned(req_buf.as_ptr() as *const VirtioBlkReq)
        };
        
        let req_type = req.req_type;
        let req_sector = req.sector;
        info!("process_request: req_type={}, sector={}", req_type, req_sector);
        
        // Process based on request type (no lock held here)
        let status = match req_type {
            blk::VIRTIO_BLK_T_IN => {
                info!("process_request: handling READ request");
                // Read request - find data descriptor
                self.handle_read_request(&head_desc, &desc_table, req_sector)?
            }
            blk::VIRTIO_BLK_T_OUT => {
                info!("process_request: handling WRITE request");
                // Write request
                self.handle_write_request(&head_desc, &desc_table, req_sector)?
            }
            blk::VIRTIO_BLK_T_FLUSH => {
                info!("process_request: handling FLUSH request");
                self.backend.flush()?;
                blk_status::VIRTIO_BLK_S_OK
            }
            blk::VIRTIO_BLK_T_GET_ID => {
                info!("process_request: handling GET_ID request (unsupported)");
                // Return device ID (not implemented)
                blk_status::VIRTIO_BLK_S_UNSUPP
            }
            _ => {
                info!("process_request: unknown request type {}", req_type);
                blk_status::VIRTIO_BLK_S_UNSUPP
            }
        };
        
        info!("process_request: status={}", status);
        
        // Find the status descriptor (last in chain, has WRITE flag)
        // VirtIO blk request: header -> data -> status(1 byte, WRITE)
        let desc_size = core::mem::size_of::<VirtqDescriptor>();
        let mut current = head_desc;
        let mut status_desc_addr = None;
        
        loop {
            let cur_flags = current.flags;
            let cur_next = current.next;
            let cur_addr = current.addr;
            
            // Check if this is the last descriptor or has WRITE flag with small len (status)
            if (cur_flags & desc_flags::VIRTQ_DESC_F_NEXT) == 0 {
                // Last descriptor is the status
                status_desc_addr = Some(cur_addr);
                break;
            }
            
            let next_offset = cur_next as usize * desc_size;
            if next_offset + desc_size > desc_table.len() {
                break;
            }
            
            current = unsafe {
                core::ptr::read_unaligned(
                    desc_table.as_ptr().add(next_offset) as *const VirtqDescriptor
                )
            };
        }
        
        if let Some(status_addr) = status_desc_addr {
            info!("process_request: writing status {} to GPA {:#x}", status, status_addr);
            let status_byte = [status];
            self.write_guest(
                GuestPhysAddr::from(status_addr as usize),
                &status_byte
            )?;
        } else {
            error!("process_request: no status descriptor found");
        }
        
        Ok(())
    }
    
    /// Handle read request
    fn handle_read_request(&self, head_desc: &VirtqDescriptor, desc_table: &[u8], sector: u64) -> AxResult<u8> {
        let desc_size = core::mem::size_of::<VirtqDescriptor>();
        
        let mut current = *head_desc;
        let mut data_desc = None;
        
        // Traverse descriptor chain
        loop {
            let cur_flags = current.flags;
            let cur_next = current.next;
            
            if (cur_flags & desc_flags::VIRTQ_DESC_F_WRITE) != 0 {
                data_desc = Some(current);
                break;
            }
            
            if (cur_flags & desc_flags::VIRTQ_DESC_F_NEXT) == 0 {
                break;
            }
            
            let next_offset = cur_next as usize * desc_size;
            if next_offset + desc_size > desc_table.len() {
                break;
            }
            
            current = unsafe {
                core::ptr::read_unaligned(
                    desc_table.as_ptr().add(next_offset) as *const VirtqDescriptor
                )
            };
        }
        
        if let Some(desc) = data_desc {
            let desc_addr = desc.addr;
            let desc_len = desc.len;
            let offset = sector * self.backend.block_size();
            let mut data = vec![0u8; desc_len as usize];
            let bytes_read = self.backend.read(offset, &mut data)?;
            
            // Write data to guest memory
            self.write_guest(
                GuestPhysAddr::from(desc_addr as usize),
                &data[..bytes_read]
            )?;
            
            Ok(blk_status::VIRTIO_BLK_S_OK)
        } else {
            Ok(blk_status::VIRTIO_BLK_S_IOERR)
        }
    }
    
    /// Handle write request
    fn handle_write_request(&self, head_desc: &VirtqDescriptor, desc_table: &[u8], sector: u64) -> AxResult<u8> {
        info!("handle_write_request: sector={}", sector);
        
        // Find the data descriptor (no WRITE flag)
        let desc_size = core::mem::size_of::<VirtqDescriptor>();
        
        let mut current = *head_desc;
        let mut data_desc = None;
        
        // Traverse descriptor chain
        loop {
            let cur_flags = current.flags;
            let cur_addr = current.addr;
            let cur_next = current.next;
            let head_addr = head_desc.addr;
            
            if (cur_flags & desc_flags::VIRTQ_DESC_F_WRITE) == 0 && 
               cur_addr != head_addr {
                data_desc = Some(current);
                break;
            }
            
            if (cur_flags & desc_flags::VIRTQ_DESC_F_NEXT) == 0 {
                break;
            }
            
            let next_offset = cur_next as usize * desc_size;
            if next_offset + desc_size > desc_table.len() {
                break;
            }
            
            current = unsafe {
                core::ptr::read_unaligned(
                    desc_table.as_ptr().add(next_offset) as *const VirtqDescriptor
                )
            };
        }
        
        if let Some(desc) = data_desc {
            let desc_addr = desc.addr;
            let desc_len = desc.len;
            info!("handle_write_request: found data desc addr={:#x}, len={}", desc_addr, desc_len);
            // Read data from guest memory
            let data = self.read_guest(
                GuestPhysAddr::from(desc_addr as usize),
                desc_len as usize
            )?;
            info!("handle_write_request: read {} bytes from guest", data.len());
            
            // Write to backend
            let offset = sector * self.backend.block_size();
            info!("handle_write_request: writing to backend offset={}", offset);
            self.backend.write(offset, &data)?;
            info!("handle_write_request: write complete");
            
            Ok(blk_status::VIRTIO_BLK_S_OK)
        } else {
            error!("handle_write_request: no data descriptor found");
            Ok(blk_status::VIRTIO_BLK_S_IOERR)
        }
    }
    
    /// Update used ring
    fn update_used_ring(&self, num_processed: u16, head_desc_idx: u16) -> AxResult {
        if num_processed == 0 {
            return Ok(());
        }
        
        info!("update_used_ring: num_processed={}, head_desc_idx={}", num_processed, head_desc_idx);
        
        // Get queue info without holding lock during I/O
        let (queue_used, queue_size) = {
            let state = self.state.lock();
            (state.queue.used, state.queue.size)
        };
        
        // Read current used ring header (flags + idx = 4 bytes)
        let used_header = self.read_guest(queue_used, 4)?;
        
        // Get current used index
        let used_idx = u16::from_le_bytes([used_header[2], used_header[3]]);
        info!("update_used_ring: current used_idx={}", used_idx);
        
        // Calculate position in ring for new entry
        let ring_pos = (used_idx % queue_size) as usize;
        
        // Write the used element (id=4bytes, len=4bytes = 8 bytes)
        // Offset: header(4) + ring_pos * sizeof(VirtqUsedElem)
        let elem_offset = 4 + ring_pos * core::mem::size_of::<VirtqUsedElem>();
        let mut used_elem = [0u8; 8];
        used_elem[0..4].copy_from_slice(&(head_desc_idx as u32).to_le_bytes());
        used_elem[4..8].copy_from_slice(&(512u32).to_le_bytes()); // len written
        
        self.write_guest(
            GuestPhysAddr::from(queue_used.as_usize() + elem_offset),
            &used_elem
        )?;
        
        // Update used index
        let new_used_idx = used_idx.wrapping_add(num_processed);
        let new_idx_bytes = new_used_idx.to_le_bytes();
        self.write_guest(
            GuestPhysAddr::from(queue_used.as_usize() + 2),
            &new_idx_bytes
        )?;
        info!("update_used_ring: new used_idx={}", new_used_idx);
        
        // Set interrupt status and inject
        {
            let mut state = self.state.lock();
            state.interrupt_status |= 1;
        }
        
        // Inject interrupt
        info!("update_used_ring: injecting IRQ {}", self.irq_id);
        (self.inject_irq)(self.irq_id)?;
        info!("update_used_ring: IRQ injected");
        
        Ok(())
    }
}

impl BaseDeviceOps<GuestPhysAddrRange> for VirtioBlkDevice {
    fn emu_type(&self) -> EmuDeviceType {
        axdevice_base::EmuDeviceType::VirtioBlk
    }
    
    fn address_range(&self) -> GuestPhysAddrRange {
        GuestPhysAddrRange::new(self.base_gpa, self.base_gpa + self.size)
    }
    
    fn handle_read(&self, addr: GuestPhysAddr, width: AccessWidth) -> Result<usize, AxErrorKind> {
        let offset = (addr.as_usize() - self.base_gpa.as_usize()) as usize;
        
        // Handle config space reads (offset >= 0x100)
        if offset >= mmio::CONFIG {
            let capacity = self.backend.size() / 512;
            let mut config_space = Vec::new();
            config_space.extend_from_slice(&(capacity as u64).to_le_bytes()); // capacity (8 bytes)
            config_space.extend_from_slice(&[0u8; 4]); // size_max (not set)
            config_space.extend_from_slice(&[0u8; 4]); // seg_max (not set)
            
            // Geometry
            config_space.extend_from_slice(&[0u8; 2]); // cylinders
            config_space.extend_from_slice(&[0u8; 1]); // heads
            config_space.extend_from_slice(&[0u8; 1]); // sectors
            
            config_space.extend_from_slice(&[0u8; 4]); // blk_size (not set)
            config_space.extend_from_slice(&[0u8; 1]); // physical_block_exp
            config_space.extend_from_slice(&[0u8; 1]); // alignment_offset
            config_space.extend_from_slice(&[0u8; 2]); // min_io_size
            config_space.extend_from_slice(&[0u8; 4]); // opt_io_size
            
            let config_offset = offset - mmio::CONFIG;
            let val = match width {
                AccessWidth::Byte => {
                    if config_offset < config_space.len() {
                        config_space[config_offset] as usize
                    } else {
                        0
                    }
                }
                AccessWidth::Word => {
                    if config_offset + 1 < config_space.len() {
                        u16::from_le_bytes([config_space[config_offset], config_space[config_offset + 1]]) as usize
                    } else {
                        0
                    }
                }
                AccessWidth::Dword => {
                    if config_offset + 3 < config_space.len() {
                        u32::from_le_bytes([
                            config_space[config_offset],
                            config_space[config_offset + 1],
                            config_space[config_offset + 2],
                            config_space[config_offset + 3],
                        ]) as usize
                    } else {
                        0
                    }
                }
                AccessWidth::Qword => {
                    if config_offset + 7 < config_space.len() {
                        u64::from_le_bytes([
                            config_space[config_offset],
                            config_space[config_offset + 1],
                            config_space[config_offset + 2],
                            config_space[config_offset + 3],
                            config_space[config_offset + 4],
                            config_space[config_offset + 5],
                            config_space[config_offset + 6],
                            config_space[config_offset + 7],
                        ]) as usize
                    } else {
                        0
                    }
                }
            };
            return Ok(val);
        }
        
        let val = match offset {
            mmio::MAGIC_VALUE => 0x74726976, // "virt" in little-endian
            mmio::VERSION => 2, // Virtio 1.0
            mmio::DEVICE_ID => VIRTIO_ID_BLOCK as usize,
            mmio::VENDOR_ID => 0,
            mmio::DEVICE_FEATURES => {
                let state = self.state.lock();
                let features = self.get_device_features();
                if state.device_features_sel == 0 {
                    (features & 0xffff_ffff) as usize
                } else {
                    ((features >> 32) & 0xffff_ffff) as usize
                }
            }
            mmio::QUEUE_NUM_MAX => {
                // Return maximum queue size supported by the device (typically 256 for VirtIO)
                256_usize
            }
            mmio::QUEUE_READY => {
                let state = self.state.lock();
                state.queue.ready as usize
            }
            mmio::INTERRUPT_STATUS => {
                let state = self.state.lock();
                state.interrupt_status as usize
            }
            mmio::STATUS => {
                let state = self.state.lock();
                state.device_status as usize
            }
            _ => {
                error!("handle_read: unknown offset {:#x}", offset);
                return Err(AxErrorKind::InvalidInput);
            }
        };
        
        // Extract value based on width
        match width {
            AccessWidth::Byte => Ok((val & 0xff) as usize),
            AccessWidth::Word => Ok((val & 0xffff) as usize),
            AccessWidth::Dword => Ok((val & 0xffff_ffff) as usize),
            AccessWidth::Qword => Ok(val),
        }
    }
    
    fn handle_write(&self, addr: GuestPhysAddr, _width: AccessWidth, val: usize) -> Result<(), AxErrorKind> {
        let offset = (addr.as_usize() - self.base_gpa.as_usize()) as usize;
        let val = val as u32;
        
        let mut state = self.state.lock();
        match offset {
            mmio::DEVICE_FEATURES_SEL => {
                state.device_features_sel = val;
            }
            mmio::DRIVER_FEATURES => {
                // Driver features write (we don't need to store this)
            }
            mmio::DRIVER_FEATURES_SEL => {
                state.driver_features_sel = val;
            }
            mmio::QUEUE_SEL => {
                state.queue_sel = val as u16;
            }
            mmio::QUEUE_NUM => {
                state.queue_num = val as u16;
                state.queue.size = val as u16;
            }
            mmio::QUEUE_READY => {
                state.queue.ready = val != 0;
            }
            mmio::QUEUE_DESC_LOW => {
                state.queue.desc = GuestPhysAddr::from(val as usize);
            }
            mmio::QUEUE_DESC_HIGH => {
                // Handle 64-bit address (combine with low)
                let low = state.queue.desc.as_usize() as u64;
                let high = (val as u64) << 32;
                state.queue.desc = GuestPhysAddr::from((low | high) as usize);
            }
            mmio::QUEUE_AVAIL_LOW => {
                state.queue.avail = GuestPhysAddr::from(val as usize);
            }
            mmio::QUEUE_AVAIL_HIGH => {
                let low = state.queue.avail.as_usize() as u64;
                let high = (val as u64) << 32;
                state.queue.avail = GuestPhysAddr::from((low | high) as usize);
            }
            mmio::QUEUE_USED_LOW => {
                state.queue.used = GuestPhysAddr::from(val as usize);
            }
            mmio::QUEUE_USED_HIGH => {
                let low = state.queue.used.as_usize() as u64;
                let high = (val as u64) << 32;
                state.queue.used = GuestPhysAddr::from((low | high) as usize);
            }
            mmio::QUEUE_NOTIFY => {
                info!("QUEUE_NOTIFY received, queue_sel={}", state.queue_sel);
                drop(state); // Release lock before processing
                if let Err(e) = self.process_queue() {
                    error!("process_queue failed: {:?}", e);
                    return Err(AxErrorKind::InvalidInput);
                }
                return Ok(());
            }
            mmio::INTERRUPT_ACK => {
                state.interrupt_status &= !val;
            }
            mmio::STATUS => {
                state.device_status = val;
            }
            _ => {
                return Err(AxErrorKind::InvalidInput);
            }
        }
        
        Ok(())
    }
}

