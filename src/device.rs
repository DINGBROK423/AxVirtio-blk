//! Virtio-blk device implementation.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use spin::Mutex;

use axaddrspace::{GuestPhysAddr, GuestPhysAddrRange};
use axdevice_base::{BaseDeviceOps, BaseMmioDeviceOps, EmuDeviceType};
use axerrno::{AxResult, ax_err, ax_err_type};
use axaddrspace::device::AccessWidth;
use memory_addr::PhysAddr;

use crate::backend::BlockBackend;
use crate::virtio::{
    blk, blk_status, desc_flags, features, mmio, status, VIRTIO_ID_BLOCK,
};

#[cfg(feature = "fs")]
use crate::backend::FileBackend;
use crate::backend::MemoryBackend;

/// Virtio descriptor structure (16 bytes)
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
    /// * `base_gpa` - Base guest physical address of the device
    /// * `size` - Size of the MMIO region
    /// * `irq_id` - Interrupt ID for the device
    /// * `backend_path` - Optional path to backend file/device. If None, uses memory backend.
    /// * `read_guest_mem` - Function to read guest memory
    /// * `write_guest_mem` - Function to write guest memory
    /// * `inject_irq` - Function to inject interrupt
    pub fn new(
        base_gpa: GuestPhysAddr,
        size: usize,
        irq_id: usize,
        backend_path: Option<&str>,
        read_guest_mem: Arc<dyn Fn(GuestPhysAddr, usize) -> AxResult<Vec<u8>> + Send + Sync>,
        write_guest_mem: Arc<dyn Fn(GuestPhysAddr, &[u8]) -> AxResult<()> + Send + Sync>,
        inject_irq: Arc<dyn Fn(usize) -> AxResult + Send + Sync>,
    ) -> AxResult<Self> {
        let backend: Arc<dyn BlockBackend> = if let Some(path) = backend_path {
            #[cfg(feature = "fs")]
            {
                Arc::new(FileBackend::new(path)?)
            }
            #[cfg(not(feature = "fs"))]
            {
                return ax_err!(
                    Unsupported,
                    "File backend requires 'fs' feature to be enabled"
                );
            }
        } else {
            // Default to memory backend with 1GB
            Arc::new(MemoryBackend::new(1024 * 1024 * 1024))
        };
        
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
            return Ok(());
        }
        
        // Read available ring
        let avail_ring = self.read_guest(state.queue.avail, 
            (4 + state.queue.size as usize * 2) as usize)?;
        
        let avail_idx = u16::from_le_bytes([
            avail_ring[2],
            avail_ring[3],
        ]);
        
        // Process each available descriptor
        let mut processed = 0;
        while processed < avail_idx {
            let ring_idx = (4 + processed as usize * 2) % avail_ring.len();
            let desc_idx = u16::from_le_bytes([
                avail_ring[ring_idx],
                avail_ring[ring_idx + 1],
            ]);
            
            // Process the request
            drop(state); // Release lock before processing
            self.process_request(desc_idx)?;
            state = self.state.lock();
            processed += 1;
        }
        
        // Update used ring
        drop(state);
        self.update_used_ring(processed)?;
        
        Ok(())
    }
    
    /// Process a single virtio-blk request
    fn process_request(&self, head_desc_idx: u16) -> AxResult {
        let state = self.state.lock();
        // Read descriptor table
        let desc_size = core::mem::size_of::<VirtqDescriptor>();
        let desc_table_size = desc_size * state.queue.size as usize;
        let desc_table = self.read_guest(state.queue.desc, desc_table_size)?;
        
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
        
        // Read request header (first descriptor)
        let req_buf = self.read_guest(
            GuestPhysAddr::from(head_desc.addr as usize),
            head_desc.len as usize
        )?;
        
        if req_buf.len() < core::mem::size_of::<VirtioBlkReq>() {
            return ax_err!(InvalidInput, "Request too short");
        }
        
        let req = unsafe {
            core::ptr::read_unaligned(req_buf.as_ptr() as *const VirtioBlkReq)
        };
        
        // Process based on request type
        let status = match req.req_type {
            blk::VIRTIO_BLK_T_IN => {
                // Read request - find data descriptor
                self.handle_read_request(&head_desc, req.sector)?
            }
            blk::VIRTIO_BLK_T_OUT => {
                // Write request
                self.handle_write_request(&head_desc, req.sector)?
            }
            blk::VIRTIO_BLK_T_FLUSH => {
                self.backend.flush()?;
                blk_status::VIRTIO_BLK_S_OK
            }
            blk::VIRTIO_BLK_T_GET_ID => {
                // Return device ID (not implemented)
                blk_status::VIRTIO_BLK_S_UNSUPP
            }
            _ => blk_status::VIRTIO_BLK_S_UNSUPP,
        };
        
        // Write status back (usually in the last descriptor)
        // For simplicity, we'll write it after the request header
        let status_byte = [status];
        self.write_guest(
            GuestPhysAddr::from(head_desc.addr as usize + core::mem::size_of::<VirtioBlkReq>()),
            &status_byte
        )?;
        
        Ok(())
    }
    
    /// Handle read request
    fn handle_read_request(&self, head_desc: &VirtqDescriptor, sector: u64) -> AxResult<u8> {
        let state = self.state.lock();
        // Find the data descriptor (WRITE flag set)
        let desc_size = core::mem::size_of::<VirtqDescriptor>();
        let desc_table = self.read_guest(state.queue.desc, 
            desc_size * state.queue.size as usize)?;
        
        let mut current = head_desc;
        let mut data_desc = None;
        
        // Traverse descriptor chain
        loop {
            if (current.flags & desc_flags::VIRTQ_DESC_F_WRITE) != 0 {
                data_desc = Some(current);
                break;
            }
            
            if (current.flags & desc_flags::VIRTQ_DESC_F_NEXT) == 0 {
                break;
            }
            
            let next_offset = current.next as usize * desc_size;
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
            let offset = sector * self.backend.block_size();
            let mut data = vec![0u8; desc.len as usize];
            let bytes_read = self.backend.read(offset, &mut data)?;
            
            // Write data to guest memory
            self.write_guest(
                GuestPhysAddr::from(desc.addr as usize),
                &data[..bytes_read]
            )?;
            
            Ok(blk_status::VIRTIO_BLK_S_OK)
        } else {
            Ok(blk_status::VIRTIO_BLK_S_IOERR)
        }
    }
    
    /// Handle write request
    fn handle_write_request(&self, head_desc: &VirtqDescriptor, sector: u64) -> AxResult<u8> {
        let state = self.state.lock();
        // Find the data descriptor (no WRITE flag)
        let desc_size = core::mem::size_of::<VirtqDescriptor>();
        let desc_table = self.read_guest(state.queue.desc, 
            desc_size * state.queue.size as usize)?;
        
        let mut current = head_desc;
        let mut data_desc = None;
        
        // Traverse descriptor chain
        loop {
            if (current.flags & desc_flags::VIRTQ_DESC_F_WRITE) == 0 && 
               current.addr != head_desc.addr {
                data_desc = Some(current);
                break;
            }
            
            if (current.flags & desc_flags::VIRTQ_DESC_F_NEXT) == 0 {
                break;
            }
            
            let next_offset = current.next as usize * desc_size;
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
            // Read data from guest memory
            let data = self.read_guest(
                GuestPhysAddr::from(desc.addr as usize),
                desc.len as usize
            )?;
            
            // Write to backend
            let offset = sector * self.backend.block_size();
            self.backend.write(offset, &data)?;
            
            Ok(blk_status::VIRTIO_BLK_S_OK)
        } else {
            Ok(blk_status::VIRTIO_BLK_S_IOERR)
        }
    }
    
    /// Update used ring
    fn update_used_ring(&self, num_processed: u16) -> AxResult {
        if num_processed == 0 {
            return Ok(());
        }
        
        let mut state = self.state.lock();
        // Read current used ring
        let used_ring_size = 4 + core::mem::size_of::<VirtqUsedElem>() * state.queue.size as usize;
        let mut used_ring = self.read_guest(state.queue.used, used_ring_size)?;
        
        // Update used ring index
        let used_idx = u16::from_le_bytes([used_ring[2], used_ring[3]]);
        let new_used_idx = used_idx.wrapping_add(num_processed);
        used_ring[2] = (new_used_idx & 0xff) as u8;
        used_ring[3] = ((new_used_idx >> 8) & 0xff) as u8;
        
        // Write back
        self.write_guest(state.queue.used, &used_ring)?;
        
        // Set interrupt status
        state.interrupt_status |= 1;
        
        // Inject interrupt
        (self.inject_irq)(self.irq_id)?;
        
        Ok(())
    }
}

impl BaseDeviceOps<GuestPhysAddrRange> for VirtioBlkDevice {
    fn emu_type(&self) -> EmuDeviceType {
        axdevice_base::EmuDeviceType::VirtioBlk
    }
    
    fn address_range(&self) -> GuestPhysAddrRange {
        GuestPhysAddrRange::new(self.base_gpa, self.size)
    }
    
    fn handle_read(&self, addr: GuestPhysAddr, width: AccessWidth) -> AxResult<usize> {
        let offset = (addr.as_usize() - self.base_gpa.as_usize()) as usize;
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
                let state = self.state.lock();
                state.queue.size as usize
            }
            mmio::INTERRUPT_STATUS => {
                let state = self.state.lock();
                state.interrupt_status as usize
            }
            mmio::STATUS => {
                let state = self.state.lock();
                state.device_status as usize
            }
            mmio::CONFIG => {
                let capacity = self.backend.size() / 512;
                let mut config_space = Vec::new();
                config_space.extend_from_slice(&(capacity as u64).to_le_bytes()); // capacity
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
                
                // Return byte at offset relative to config space
                let config_offset = offset - mmio::CONFIG;
                if config_offset < config_space.len() {
                    config_space[config_offset] as usize
                } else {
                    0
                }
            }
            _ => {
                return ax_err!(InvalidInput, format!("Unhandled MMIO read at offset {:#x}", offset));
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
    
    fn handle_write(&self, addr: GuestPhysAddr, width: AccessWidth, val: usize) -> AxResult {
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
                drop(state); // Release lock before processing
                self.process_queue()?;
                return Ok(());
            }
            mmio::INTERRUPT_ACK => {
                state.interrupt_status &= !val;
            }
            mmio::STATUS => {
                state.device_status = val;
            }
            _ => {
                return ax_err!(InvalidInput, format!("Unhandled MMIO write at offset {:#x}", offset));
            }
        }
        
        Ok(())
    }
}

