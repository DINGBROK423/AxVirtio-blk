#![no_std]

//! Virtio-blk virtual block device implementation for Axvisor.
//!
//! This crate provides a complete implementation of the Virtio-blk protocol,
//! allowing guest VMs to access host filesystem files or block devices as
//! virtual block storage.

extern crate alloc;

mod device;
mod virtio;
mod backend;

pub use device::VirtioBlkDevice;

