# AxVirtio-blk

为 Axvisor 实现的基于 Virtio-blk 协议的虚拟块设备。

## 功能特性

- ✅ 完整的 Virtio MMIO 寄存器实现
- ✅ Virtio 队列机制（descriptor ring, available ring, used ring）
- ✅ 支持读写请求处理
- ✅ 支持 Flush 请求
- ✅ 文件后端存储（需要 `fs` feature）
- ✅ 内存后端存储（用于测试）
- ✅ 符合 `BaseMmioDeviceOps` trait 规范

## 项目结构

```
AxVirtio-blk/
├── src/
│   ├── lib.rs          # 库入口
│   ├── device.rs       # Virtio-blk 设备主实现
│   ├── virtio.rs       # Virtio 协议常量和定义
│   └── backend.rs       # 后端存储接口实现
├── Cargo.toml          # 项目配置
└── README.md           # 本文档
```

## 使用方法

### 1. 在 axdevice 中启用

在 `Axdevice/axdevice/Cargo.toml` 中启用 feature：

```toml
[features]
virtio-blk = ["axvirtio-blk"]
```

### 2. 在 VM 配置中添加设备

`axvmconfig` 的 `VMDevicesConfig` 已包含 `virtio_blk_mmio` 字段。示例：

```toml
[devices]
emu_devices = []
passthrough_devices = []

[[devices.virtio_blk_mmio]]
device_id = "virtio-blk@0"
mmio_base = "0xa000000"
mmio_size = "0x1000"
interrupt_type = "spi"
interrupt_number = 32
guest_device_path = "/dev/vblk0"
backend_type = "file"
backend_path = "/data/disk.img"
size = "2G"
readonly = false
serial = "axvblk0001"
```

### 3. 构建和运行

```bash
# 在 Axvisor 目录下
cargo build --features virtio-blk
```

## 待完成的工作

1. **实现 Guest 内存访问**
   - 当前实现中，guest 内存访问函数是占位符
   - 需要修改 `AxVmDevices` 以支持传递 VM 引用或 guest 内存访问函数
   - 可以通过 `AxVM::read_from_guest_of` 和 `AxVM::write_to_guest_of` 来实现

2. **中断处理**
   - 当前实现了中断状态寄存器，但需要与 GIC 集成以实际触发中断

3. **完整测试**
   - 需要在实际 VM 环境中测试设备功能
   - 验证读写操作的正确性

## 实现细节

### Virtio MMIO 寄存器

设备实现了完整的 Virtio 1.0 MMIO 寄存器集：
- Magic Value, Version, Device ID, Vendor ID
- Device/Driver Features
- Queue 配置（selector, size, addresses）
- Interrupt Status
- Device Status
- Configuration Space（容量信息）

### 队列处理

设备支持标准的 Virtio 队列机制：
- Descriptor Ring：存储请求描述符链
- Available Ring：Guest 通知 Host 有新请求
- Used Ring：Host 通知 Guest 请求完成

### 后端存储

支持两种后端：
- **FileBackend**：使用 Host 文件系统文件（需要 `fs` feature）
- **MemoryBackend**：内存后端，用于测试

## 开发指南

详细的集成指南请参考 `INTEGRATION.md`。

## 许可证

GPL-3.0-or-later OR Apache-2.0 OR MulanPSL-2.0
