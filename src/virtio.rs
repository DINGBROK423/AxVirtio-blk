//! Virtio constants and definitions.

pub const VIRTIO_ID_BLOCK: u32 = 2;

pub mod features {
    pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;
    pub const VIRTIO_BLK_F_SIZE_MAX: u64 = 1 << 1;
    pub const VIRTIO_BLK_F_SEG_MAX: u64 = 1 << 2;
    pub const VIRTIO_BLK_F_GEOMETRY: u64 = 1 << 4;
    pub const VIRTIO_BLK_F_BLK_SIZE: u64 = 1 << 6;
    pub const VIRTIO_BLK_F_FLUSH: u64 = 1 << 9;
}

pub mod mmio {
    pub const MAGIC_VALUE: usize = 0x000;
    pub const VERSION: usize = 0x004;
    pub const DEVICE_ID: usize = 0x008;
    pub const VENDOR_ID: usize = 0x00c;
    pub const DEVICE_FEATURES: usize = 0x010;
    pub const DEVICE_FEATURES_SEL: usize = 0x014;
    pub const DRIVER_FEATURES: usize = 0x020;
    pub const DRIVER_FEATURES_SEL: usize = 0x024;
    pub const QUEUE_SEL: usize = 0x030;
    pub const QUEUE_NUM_MAX: usize = 0x034;
    pub const QUEUE_NUM: usize = 0x038;
    pub const QUEUE_READY: usize = 0x044;
    pub const QUEUE_NOTIFY: usize = 0x050;
    pub const INTERRUPT_STATUS: usize = 0x060;
    pub const INTERRUPT_ACK: usize = 0x064;
    pub const STATUS: usize = 0x070;
    pub const QUEUE_DESC_LOW: usize = 0x080;
    pub const QUEUE_DESC_HIGH: usize = 0x084;
    pub const QUEUE_AVAIL_LOW: usize = 0x090;
    pub const QUEUE_AVAIL_HIGH: usize = 0x094;
    pub const QUEUE_USED_LOW: usize = 0x0a0;
    pub const QUEUE_USED_HIGH: usize = 0x0a4;
    pub const CONFIG: usize = 0x100;
}

pub mod status {
    pub const ACKNOWLEDGE: u32 = 1;
    pub const DRIVER: u32 = 2;
    pub const DRIVER_OK: u32 = 4;
    pub const FEATURES_OK: u32 = 8;
    pub const DEVICE_NEEDS_RESET: u32 = 64;
    pub const FAILED: u32 = 128;
}

pub mod blk {
    pub const VIRTIO_BLK_T_IN: u32 = 0;
    pub const VIRTIO_BLK_T_OUT: u32 = 1;
    pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
    pub const VIRTIO_BLK_T_GET_ID: u32 = 8;
}

pub mod blk_status {
    pub const VIRTIO_BLK_S_OK: u8 = 0;
    pub const VIRTIO_BLK_S_IOERR: u8 = 1;
    pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;
}

pub mod desc_flags {
    pub const VIRTQ_DESC_F_NEXT: u16 = 1;
    pub const VIRTQ_DESC_F_WRITE: u16 = 2;
    pub const VIRTQ_DESC_F_INDIRECT: u16 = 4;
}
