# AxVirtio-blk 技术文档

> Virtio-blk 虚拟块设备实现技术参考手册

## 目录

- [1. 项目概述](#1-项目概述)
- [2. 项目结构](#2-项目结构)
- [3. 核心模块详解](#3-核心模块详解)
  - [3.1 virtio.rs - 协议常量定义](#31-virtioirs---协议常量定义)
  - [3.2 backend.rs - 后端存储接口](#32-backendrs---后端存储接口)
  - [3.3 device.rs - 核心设备实现](#33-devicers---核心设备实现)
- [4. 数据结构](#4-数据结构)
- [5. 工作流程](#5-工作流程)
- [6. API 参考](#6-api-参考)
- [7. 配置说明](#7-配置说明)

---

## 1. 项目概述

AxVirtio-blk 是为 Axvisor Hypervisor 实现的 Virtio-blk 虚拟块设备驱动。它遵循 Virtio 1.0 规范，通过 MMIO 接口向 Guest VM 提供虚拟块存储服务。

### 1.1 核心特性

| 特性 | 描述 |
|------|------|
| **Virtio 1.0 兼容** | 完整实现 Virtio MMIO 传输层 |
| **多后端支持** | 支持文件后端和内存后端 |
| **no_std 环境** | 可在裸机/内核环境运行 |
| **可配置** | 通过 TOML 配置文件灵活配置 |

### 1.2 支持的操作

- `VIRTIO_BLK_T_IN` (0) - 读操作
- `VIRTIO_BLK_T_OUT` (1) - 写操作
- `VIRTIO_BLK_T_FLUSH` (4) - 刷新缓存

---

## 2. 项目结构

```
AxVirtio-blk/
├── Cargo.toml              # 项目配置与依赖
├── src/
│   ├── lib.rs              # 库入口，模块组织
│   ├── virtio.rs           # Virtio 协议常量定义
│   ├── backend.rs          # 后端存储接口与实现
│   └── device.rs           # 核心设备实现（789行）
└── doc/
    └── blkdoc.md  # 本文档
```

### 2.1 依赖关系

```
┌─────────────────────────────────────────────────────────┐
│                    VirtioBlkDevice                      │
│                      (device.rs)                        │
└─────────────────────────────────────────────────────────┘
         │                    │                    │
         ▼                    ▼                    ▼
┌─────────────┐      ┌─────────────┐      ┌─────────────┐
│  virtio.rs  │      │ backend.rs  │      │ 外部依赖     │
│  协议常量    |      │  存储后端   │      │ axaddrspace │
└─────────────┘      └─────────────┘      │ axdevice_base│
                                          │ axerrno     │
                            │             |   ...       |
                            ▼             └─────────────┘
                   ┌─────────────────┐
                   │  FileBackend    │
                   │  MemoryBackend  │
                   └─────────────────┘
```

---

## 3. 核心模块详解

### 3.1 virtio.rs - 协议常量定义

此模块定义了 Virtio 规范中的所有常量，包括：

#### 3.1.1 设备类型

```rust
pub const VIRTIO_ID_BLOCK: u32 = 2;  // 块设备类型 ID
```

#### 3.1.2 Feature 位（设备能力协商）

| 常量 | 值 | 描述 |
|------|-----|------|
| `VIRTIO_F_VERSION_1` | 1 << 32 | Virtio 1.0 标准 |
| `VIRTIO_BLK_F_SIZE_MAX` | 1 << 1 | 支持报告最大段大小 |
| `VIRTIO_BLK_F_SEG_MAX` | 1 << 2 | 支持报告最大段数 |
| `VIRTIO_BLK_F_GEOMETRY` | 1 << 4 | 支持报告磁盘几何信息 |
| `VIRTIO_BLK_F_BLK_SIZE` | 1 << 6 | 支持报告块大小 |
| `VIRTIO_BLK_F_FLUSH` | 1 << 9 | 支持 FLUSH 命令 |

#### 3.1.3 MMIO 寄存器偏移量

| 寄存器 | 偏移 | 读/写 | 描述 |
|--------|------|-------|------|
| `MAGIC_VALUE` | 0x000 | R | 魔数 "virt" (0x74726976) |
| `VERSION` | 0x004 | R | Virtio 版本 (2 = v1.0) |
| `DEVICE_ID` | 0x008 | R | 设备类型 ID |
| `VENDOR_ID` | 0x00c | R | 厂商 ID |
| `DEVICE_FEATURES` | 0x010 | R | 设备支持的特性 |
| `DEVICE_FEATURES_SEL` | 0x014 | W | 特性选择器 (0=低32位, 1=高32位) |
| `DRIVER_FEATURES` | 0x020 | W | 驱动接受的特性 |
| `DRIVER_FEATURES_SEL` | 0x024 | W | 驱动特性选择器 |
| `QUEUE_SEL` | 0x030 | W | 队列选择 |
| `QUEUE_NUM_MAX` | 0x034 | R | 队列最大容量 |
| `QUEUE_NUM` | 0x038 | W | 队列实际大小 |
| `QUEUE_READY` | 0x044 | RW | 队列就绪标志 |
| `QUEUE_NOTIFY` | 0x050 | W | 通知寄存器（触发处理） |
| `INTERRUPT_STATUS` | 0x060 | R | 中断状态 |
| `INTERRUPT_ACK` | 0x064 | W | 中断确认 |
| `STATUS` | 0x070 | RW | 设备状态 |
| `QUEUE_DESC_LOW/HIGH` | 0x080/0x084 | W | 描述符表地址 |
| `QUEUE_AVAIL_LOW/HIGH` | 0x090/0x094 | W | Available ring 地址 |
| `QUEUE_USED_LOW/HIGH` | 0x0a0/0x0a4 | W | Used ring 地址 |
| `CONFIG` | 0x100+ | RW | 设备特定配置空间 |

#### 3.1.4 设备状态位

| 常量 | 值 | 描述 |
|------|-----|------|
| `ACKNOWLEDGE` | 1 | Guest 发现了设备 |
| `DRIVER` | 2 | Guest 知道如何驱动 |
| `DRIVER_OK` | 4 | 驱动初始化完成 |
| `FEATURES_OK` | 8 | 特性协商完成 |
| `DEVICE_NEEDS_RESET` | 64 | 设备需要重置 |
| `FAILED` | 128 | 设备出错 |

#### 3.1.5 请求类型与状态

**请求类型 (blk module):**

| 常量 | 值 | 描述 |
|------|-----|------|
| `VIRTIO_BLK_T_IN` | 0 | 读请求 |
| `VIRTIO_BLK_T_OUT` | 1 | 写请求 |
| `VIRTIO_BLK_T_FLUSH` | 4 | 刷新缓存 |
| `VIRTIO_BLK_T_GET_ID` | 8 | 获取设备 ID |

**状态码 (blk_status module):**

| 常量 | 值 | 描述 |
|------|-----|------|
| `VIRTIO_BLK_S_OK` | 0 | 成功 |
| `VIRTIO_BLK_S_IOERR` | 1 | I/O 错误 |
| `VIRTIO_BLK_S_UNSUPP` | 2 | 不支持的操作 |

#### 3.1.6 描述符标志

| 常量 | 值 | 描述 |
|------|-----|------|
| `VIRTQ_DESC_F_NEXT` | 1 | 链表有下一个描述符 |
| `VIRTQ_DESC_F_WRITE` | 2 | 设备写入（Guest 读取） |
| `VIRTQ_DESC_F_INDIRECT` | 4 | 间接描述符 |

---

### 3.2 backend.rs - 后端存储接口

此模块定义了存储后端的抽象接口和具体实现。

#### 3.2.1 BlockBackend Trait

```rust
pub trait BlockBackend: Send + Sync {
    /// 从后端存储读取数据
    /// @param offset: 字节偏移量
    /// @param data: 读取缓冲区
    /// @return: 实际读取的字节数
    fn read(&self, offset: u64, data: &mut [u8]) -> AxResult<usize>;
    
    /// 向后端存储写入数据
    fn write(&self, offset: u64, data: &[u8]) -> AxResult<usize>;
    
    /// 获取存储总大小（字节）
    fn size(&self) -> u64;
    
    /// 刷新待写入数据到持久存储
    fn flush(&self) -> AxResult;
    
    /// 获取块大小（扇区大小），默认 512 字节
    fn block_size(&self) -> u64 { 512 }
}
```

#### 3.2.2 FileBackend（文件后端）

**启用条件**: `feature = "fs"`

```rust
pub struct FileBackend {
    file: Arc<Mutex<File>>,  // 文件句柄
    size: u64,               // 文件大小
    block_size: u64,         // 块大小 (512)
}
```

架构层次

```
┌─────────────────────────────────────────────────────────────────┐
│  宿主机 Linux                                                    │
│  └── blktest-disk.img (64MB FAT32 文件)                         │
│       └── guest-disk.img (16MB raw 文件，存储在 FAT32 内)        │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼ QEMU virtio-blk-device
┌─────────────────────────────────────────────────────────────────┐
│  Axvisor (Hypervisor)  运行在 QEMU 中                            │
│  ├── axdriver: 初始化 virtio-blk 驱动                           │
│  ├── axfs: 挂载 FAT32 文件系统 (根目录 /)                        │
│  │    └── /guest-disk.img  ← FileBackend 打开这个文件            │
│  └── axdevice/axvirtio-blk: 创建虚拟块设备给 Guest VM            │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼ AxVirtio-blk MMIO 设备
┌─────────────────────────────────────────────────────────────────┐
│  Guest VM (ArceOS blktest)                                       │
│  └── 通过 virtio-blk 驱动访问虚拟磁盘                            │
└─────────────────────────────────────────────────────────────────┘
```

#### 使用方式

**1. Axvisor 启动时初始化文件系统：**

```rust
// axruntime 初始化流程中
axfs::init()  // 挂载 FAT32 到根目录 /
```

**2. AxVirtio-blk 的 FileBackend 通过 axstd 访问文件：**

```rust
// virt-blk/AxVirtio-blk/src/backend.rs
use axstd::fs::{File, OpenOptions};
use axstd::io::{Read, Write, Seek, SeekFrom};

impl FileBackend {
    pub fn new(path: &str) -> AxResult<Self> {
        // 通过 axstd 的文件系统 API 打开文件
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;  // path = "/guest-disk.img"
        // ...
    }
}
```

**3. 依赖链：**

```
axvirtio-blk
    └── axstd (with fs feature)
            └── axfs (ArceOS 文件系统模块)
                    └── fatfs (FAT32 实现)
                            └── virtio-blk driver
                                    └── QEMU 提供的 blktest-disk.img
```

#### 关键配置

**VM 配置文件 (arceos-blktest-aarch64-qemu-smp1-fs.toml)：**
```toml
[[devices.virtio_blk_mmio]]
backend_type = "file"
backend_path = "/guest-disk.img"  # Axvisor 文件系统内的路径
```

**QEMU 配置 (qemu-aarch64-info-fs.toml)：**
```toml
args = [
  "-device", "virtio-blk-device,drive=disk0",
  "-drive", "id=disk0,if=none,format=raw,file=.../blktest-disk.img",
]
```

**特点**:
- Axvisor (Hypervisor) 提供文件系统，具体来说是 ArceOS 的 axfs 模块。
- FileBackend 依赖 **axstd** 的文件操作 API，而 axstd 底层使用 ArceOS 的 **axfs** 模块，axfs 使用 **fatfs** crate 实现 FAT32 文件系统。

#### 3.2.3 MemoryBackend（内存后端）

**始终可用**

```rust
pub struct MemoryBackend {
    data: spin::Mutex<Vec<u8>>,  // 内存缓冲区
    block_size: u64,             // 块大小 (512)
}
```

**特点**:
- 使用内存模拟磁盘
- 适用于测试环境
- 数据非持久化
- 支持自动扩展

---

### 3.3 device.rs - 核心设备实现

这是项目的核心，实现了完整的 Virtio-blk MMIO 设备。

#### 3.3.1 主要数据结构

**VirtqDescriptor - 描述符（16字节）**

```rust
#[repr(C, packed)]
struct VirtqDescriptor {
    addr: u64,      // Guest 物理地址
    len: u32,       // 数据长度
    flags: u16,     // 标志位
    next: u16,      // 下一个描述符索引
}
```

**VirtioBlkReq - 请求头（16字节）**

```rust
#[repr(C, packed)]
struct VirtioBlkReq {
    req_type: u32,  // 请求类型
    reserved: u32,  // 保留
    sector: u64,    // 起始扇区号
}
```

**VirtQueue - 队列状态**

```rust
struct VirtQueue {
    size: u16,              // 队列大小
    desc: GuestPhysAddr,    // 描述符表地址
    avail: GuestPhysAddr,   // Available ring 地址
    used: GuestPhysAddr,    // Used ring 地址
    ready: bool,            // 就绪标志
}
```

**VirtioBlkDevice - 主设备结构**

```rust
pub struct VirtioBlkDevice {
    base_gpa: GuestPhysAddr,    // MMIO 基地址
    size: usize,                // MMIO 区域大小
    irq_id: usize,              // 中断号
    
    state: Mutex<VirtioBlkState>,           // 可变状态
    backend: Arc<dyn BlockBackend>,          // 后端存储
    
    // 回调函数
    read_guest_mem: Arc<dyn Fn(GuestPhysAddr, usize) -> AxResult<Vec<u8>> + Send + Sync>,
    write_guest_mem: Arc<dyn Fn(GuestPhysAddr, &[u8]) -> AxResult<()> + Send + Sync>,
    inject_irq: Arc<dyn Fn(usize) -> AxResult + Send + Sync>,
}
```

---

## 4. 数据结构

### 4.1 Virtio 队列内存布局

```
Guest 物理内存布局：

┌─────────────────────────────────────────────────────────┐
│                    Descriptor Table                      │
│  ┌─────────────────────────────────────────────────┐    │
│  │ Desc[0]: addr=0x1000, len=16, flags=NEXT, next=1│    │
│  │ Desc[1]: addr=0x2000, len=512, flags=WRITE|NEXT │    │
│  │ Desc[2]: addr=0x3000, len=1, flags=WRITE        │    │
│  │ ...                                              │    │
│  └─────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────┤
│                    Available Ring                        │
│  ┌─────────────────────────────────────────────────┐    │
│  │ flags: 0                                         │    │
│  │ idx: 3  (下一个可用槽位)                          │    │
│  │ ring[0]: 0  (描述符链头索引)                      │    │
│  │ ring[1]: 3                                       │    │
│  │ ring[2]: 6                                       │    │
│  └─────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────┤
│                      Used Ring                           │
│  ┌─────────────────────────────────────────────────┐    │
│  │ flags: 0                                         │    │
│  │ idx: 2  (下一个空闲槽位)                          │    │
│  │ ring[0]: {id: 0, len: 512}                       │    │
│  │ ring[1]: {id: 3, len: 512}                       │    │
│  └─────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
```

### 4.2 块请求描述符链

```
读请求 (VIRTIO_BLK_T_IN):
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│ Header Desc  │────►│  Data Desc   │────►│ Status Desc  │
│ flags: NEXT  │     │ flags: WRITE │     │ flags: WRITE │
│ len: 16      │     │ |NEXT        │     │ len: 1       │
│              │     │ len: 512     │     │              │
└──────────────┘     └──────────────┘     └──────────────┘
       │                    │                    │
       ▼                    ▼                    ▼
  VirtioBlkReq        数据缓冲区              状态字节
  (type, sector)      (设备写入)            (设备写入)

写请求 (VIRTIO_BLK_T_OUT):
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│ Header Desc  │────►│  Data Desc   │────►│ Status Desc  │
│ flags: NEXT  │     │ flags: NEXT  │     │ flags: WRITE │
│ len: 16      │     │ (无 WRITE)   │     │ len: 1       │
│              │     │ len: 512     │     │              │
└──────────────┘     └──────────────┘     └──────────────┘
       │                    │                    │
       ▼                    ▼                    ▼
  VirtioBlkReq        数据缓冲区              状态字节
  (type, sector)      (Guest 提供)          (设备写入)
```

---

## 5. 工作流程

### 5.1 设备初始化流程

```
┌─────────────────────────────────────────────────────────┐
│                    Guest Driver                          │
└─────────────────────────────────────────────────────────┘
                          │
    1. 读取 MAGIC_VALUE ──┼──► 返回 0x74726976 ("virt")
    2. 读取 VERSION ──────┼──► 返回 2 (Virtio 1.0)
    3. 读取 DEVICE_ID ────┼──► 返回 2 (Block Device)
    4. 写入 STATUS ───────┼──► ACKNOWLEDGE
    5. 写入 STATUS ───────┼──► ACKNOWLEDGE | DRIVER
    6. 读取 DEVICE_FEATURES ─► 返回支持的特性
    7. 写入 DRIVER_FEATURES ─► 驱动协商特性
    8. 写入 STATUS ───────┼──► ... | FEATURES_OK
    9. 配置队列:           │
       - QUEUE_SEL = 0    │
       - QUEUE_NUM = 256  │
       - QUEUE_DESC_*     │
       - QUEUE_AVAIL_*    │
       - QUEUE_USED_*     │
       - QUEUE_READY = 1  │
   10. 写入 STATUS ───────┼──► ... | DRIVER_OK
                          │
                          ▼
              ┌───────────────────────┐
              │   设备初始化完成       │
              │   可以处理 I/O 请求    │
              └───────────────────────┘
```

### 5.2 I/O 请求处理流程

```
┌─────────────────────────────────────────────────────────────────┐
│                         Guest VM (Linux)                         │
│                                                                  │
│   1. 构造请求：分配描述符链 [Header][Data][Status]                 │
│   2. 填充 Header: {type, sector}                                 │
│   3. 写入描述符到 desc table                                      │
│   4. 更新 available ring (idx++)                                 │
│   5. 写 QUEUE_NOTIFY = 0  ◄────────────── 触发设备处理           │
│   6. 等待中断                                                    │
│   7. 读取 used ring 获取完成状态                                  │
│   8. 读取 INTERRUPT_ACK 清除中断                                  │
└─────────────────────────────────────────────────────────────────┘
                               │
                               │ MMIO 访问陷入
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                      VirtioBlkDevice                             │
│                                                                  │
│   handle_write(QUEUE_NOTIFY):                                    │
│     │                                                            │
│     ▼                                                            │
│   process_queue():                                               │
│     │                                                            │
│     ├─► 读取 available ring                                      │
│     ├─► 计算新请求数: num_new = avail_idx - last_avail_idx       │
│     │                                                            │
│     └─► for each new request:                                    │
│           │                                                      │
│           ├─► process_request(desc_idx):                         │
│           │     ├─► 读取描述符表                                  │
│           │     ├─► 解析请求头 (type, sector)                     │
│           │     ├─► 根据类型调用:                                 │
│           │     │     - handle_read_request()                    │
│           │     │     - handle_write_request()                   │
│           │     │     - backend.flush()                          │
│           │     └─► 写状态到 status 描述符                        │
│           │                                                      │
│           └─► update_used_ring():                                │
│                 ├─► 写入 UsedElem {id, len}                      │
│                 ├─► 更新 used_idx                                │
│                 ├─► 设置 interrupt_status                        │
│                 └─► inject_irq() ──────────► 通知 Guest          │
└─────────────────────────────────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                        BlockBackend                              │
│                                                                  │
│   ┌────────────────┐              ┌────────────────┐            │
│   │  FileBackend   │      OR      │ MemoryBackend  │            │
│   │                │              │                │            │
│   │  read():       │              │  read():       │            │
│   │    seek()      │              │    copy_from() │            │
│   │    file.read() │              │                │            │
│   │                │              │  write():      │            │
│   │  write():      │              │    copy_to()   │            │
│   │    seek()      │              │    resize()    │            │
│   │    file.write()│              │                │            │
│   └────────────────┘              └────────────────┘            │
└─────────────────────────────────────────────────────────────────┘
```

### 5.3 读请求详细流程

```rust
fn handle_read_request(&self, head_desc, desc_table, sector) -> AxResult<u8> {
    // 1. 遍历描述符链，找到带 WRITE 标志的数据描述符
    //    WRITE 标志表示设备可写入 = Guest 要读取的缓冲区
    
    // 2. 计算后端偏移量
    let offset = sector * 512;  // 扇区号 × 扇区大小
    
    // 3. 从后端读取数据
    let mut data = vec![0u8; data_desc.len];
    self.backend.read(offset, &mut data)?;
    
    // 4. 将数据写入 Guest 内存
    self.write_guest(GuestPhysAddr::from(data_desc.addr), &data)?;
    
    Ok(VIRTIO_BLK_S_OK)
}
```

### 5.4 写请求详细流程

```rust
fn handle_write_request(&self, head_desc, desc_table, sector) -> AxResult<u8> {
    // 1. 遍历描述符链，找到不带 WRITE 标志的数据描述符
    //    无 WRITE 标志表示设备只读 = Guest 提供的写入数据
    
    // 2. 从 Guest 内存读取数据
    let data = self.read_guest(GuestPhysAddr::from(data_desc.addr), len)?;
    
    // 3. 计算后端偏移量并写入
    let offset = sector * 512;
    self.backend.write(offset, &data)?;
    
    Ok(VIRTIO_BLK_S_OK)
}
```

---

## 6. API 参考

### 6.1 VirtioBlkDevice

#### 构造函数

```rust
pub fn new(
    config: &VirtioBlkMmioDeviceConfig,
    read_guest_mem: Arc<dyn Fn(GuestPhysAddr, usize) -> AxResult<Vec<u8>> + Send + Sync>,
    write_guest_mem: Arc<dyn Fn(GuestPhysAddr, &[u8]) -> AxResult<()> + Send + Sync>,
    inject_irq: Arc<dyn Fn(usize) -> AxResult + Send + Sync>,
) -> AxResult<Self>
```

**参数说明**:

| 参数 | 类型 | 描述 |
|------|------|------|
| `config` | `&VirtioBlkMmioDeviceConfig` | 设备配置 |
| `read_guest_mem` | 回调函数 | 读取 Guest 物理内存 |
| `write_guest_mem` | 回调函数 | 写入 Guest 物理内存 |
| `inject_irq` | 回调函数 | 向 Guest 注入中断 |

#### 实现的 Trait

**BaseDeviceOps\<GuestPhysAddrRange\>**:

```rust
fn emu_type(&self) -> EmuDeviceType;
fn address_range(&self) -> GuestPhysAddrRange;
fn handle_read(&self, addr: GuestPhysAddr, width: AccessWidth) -> Result<usize, AxErrorKind>;
fn handle_write(&self, addr: GuestPhysAddr, width: AccessWidth, val: usize) -> Result<(), AxErrorKind>;
```

### 6.2 BlockBackend Trait

```rust
pub trait BlockBackend: Send + Sync {
    fn read(&self, offset: u64, data: &mut [u8]) -> AxResult<usize>;
    fn write(&self, offset: u64, data: &[u8]) -> AxResult<usize>;
    fn size(&self) -> u64;
    fn flush(&self) -> AxResult;
    fn block_size(&self) -> u64 { 512 }
}
```

---

## 7. 配置说明

### 7.1 Cargo.toml Features

| Feature | 描述 | 依赖 |
|---------|------|------|
| `default` | 无默认特性 | - |
| `fs` | 启用文件后端支持 | `axstd/fs` |

### 7.2 设备配置结构

通过 `axvmconfig::VirtioBlkMmioDeviceConfig` 配置：

```toml
[[virtio_blk_devices]]
mmio_base = "0x0a000000"        # MMIO 基地址
mmio_size = "0x200"             # MMIO 区域大小 (512 字节)
interrupt_number = 48           # 中断号
backend_path = "memory"  # 后端文件路径（可选 fs or memory）
```

**说明**:
- 若 `backend_path` 为空，使用 1GB 内存后端
- 若指定路径，需启用 `fs` feature

### 7.3 典型配置示例

**fs后端**:

```toml
[[devices.virtio_blk_mmio]]
device_id = "blk0"
mmio_base = "0x0a000000"
mmio_size = "0x200"
interrupt_type = "spi"
interrupt_number = 48
guest_device_path = "/dev/vda"
backend_type = "file"
backend_path = "/guest-disk.img"
size = "0x1000000"           # 16MB (matches the guest-disk.img size)
readonly = false
serial = "blktest-fs-disk0"
```


**内存后端**:

```toml
[[devices.virtio_blk_mmio]]
device_id = "blk0"
mmio_base = "0x0a000000"
mmio_size = "0x200"
interrupt_type = "spi"
interrupt_number = 48
guest_device_path = "/dev/vda"
backend_type = "memory"
backend_path = ""
size = "0x4000000"
readonly = false
serial = "blktest-disk0"
```

