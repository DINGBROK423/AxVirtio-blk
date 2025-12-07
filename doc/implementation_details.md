# AxVirtio-blk 技术实现与集成文档

本文档详细描述了 `AxVirtio-blk` 虚拟块设备的实现细节、集成方案以及使用方法，旨在为技术文档编写提供素材。

## 1. 修改文件概览

本次修改涉及三个主要模块：`AxVirtio-blk`（独立设备库）、`Axdevice`（设备管理库）和 `axvm`（虚拟机核心库）。

### 1.1 AxVirtio-blk (新增/重构)
*   **`src/virtio.rs`**: [新增] 定义 Virtio 协议标准常量（Feature bits, Status bits, MMIO 寄存器偏移等）。
*   **`src/device.rs`**: [重构] `VirtioBlkDevice` 的核心逻辑。
    *   构造函数 `new` 接收配置和回调函数。
    *   实现 `BaseMmioDeviceOps` trait，处理 MMIO 读写。
    *   实现 Virtqueue 的处理逻辑（Descriptor Chain 解析）。
*   **`src/backend.rs`**: [修改] 提供 `MemoryBackend` 和 `FileBackend`（可选），用于模拟磁盘存储。
*   **`Cargo.toml`**: 添加 `axvmconfig` 依赖。

### 1.2 Axdevice (集成层)
*   **`axdevice/Cargo.toml`**: 添加 `axvirtio-blk` 依赖。
*   **`axdevice/src/config.rs`**:
    *   `AxVmDeviceConfig` 结构体新增 `virtio_blk_configs` 字段，用于传递 Virtio-blk 的配置。
*   **`axdevice/src/device.rs`**:
    *   `AxVmDevices::new` 方法签名变更，新增 `read_guest_mem`, `write_guest_mem`, `inject_irq` 三个回调参数。
    *   在初始化流程中，遍历 `virtio_blk_configs`，实例化 `VirtioBlkDevice` 并注册到 MMIO 设备列表中。

### 1.3 axvm (核心层)
*   **`axvm/Cargo.toml`**: 添加 `axvirtio-blk` 依赖，指向本地路径。
*   **`axvm/src/config.rs`**:
    *   `AxVMConfig` 结构体新增 `virtio_blk_mmio` 字段。
    *   更新 `From<AxVMCrateConfig>` 实现，从 TOML 配置中透传 Virtio-blk 配置。
*   **`axvm/src/vm.rs`**:
    *   `AxVMInnerMut` 中的 `address_space` 类型从 `Mutex<AddrSpace>` 变更为 `Arc<Mutex<AddrSpace>>`，支持多线程共享。
    *   `AxVM::new` 初始化流程调整：
        1.  创建 `Arc<Mutex<AddrSpace>>`。
        2.  构造读写 Guest 内存的闭包（捕获 `address_space`）。
        3.  构造注入中断的闭包（捕获 `vm_id`）。
        4.  调用 `AxVmDevices::new` 时传入上述闭包和配置。

## 2. 为什么这么改？（设计思路）

### 2.1 模块化与解耦
*   **目标**：保持 `AxVirtio-blk` 作为一个独立的 crate，不依赖具体的 Hypervisor 实现细节（如 `AxVM` 结构体）。
*   **实现**：通过 **回调函数 (Callbacks)** 机制切断依赖。`AxVirtio-blk` 需要访问 Guest 内存和注入中断，但它不直接调用 `AxVM` 的方法，而是要求调用者（`Axdevice` -> `AxVM`）在创建设备时传入 `Fn(GuestPhysAddr, ...)` 类型的闭包。这使得 `AxVirtio-blk` 可以被集成到任何提供这些能力的 VMM 中。

### 2.2 内存安全与并发
*   **问题**：设备模拟是在 VM 运行过程中发生的（通常在 vCPU 线程的 VM Exit 处理路径中），同时设备可能需要异步访问内存（虽然目前是同步实现）。
*   **解决**：将 `AddressSpace` 包装在 `Arc<Mutex<...>>` 中。
    *   `Arc` 保证了即使 `AxVM` 结构体发生移动或生命周期变化，设备持有的闭包仍然能安全访问地址空间。
    *   `Mutex` 保证了对页表的并发访问安全（尽管 `axaddrspace` 内部可能有锁，但在外层加锁更通用）。
    *   在 `AxVM::new` 中添加 `<H as AxVMHal>::PagingHandler: Send + 'static` 约束，确保页表操作句柄可以安全地跨线程传递，这是 Rust 异步/多线程编程的硬性要求。

### 2.3 配置驱动
*   **目标**：用户只需修改 TOML 配置文件即可添加设备，无需修改代码。
*   **实现**：从 `axvmconfig` (解析 TOML) -> `AxVMConfig` (VM 内部配置) -> `AxVmDeviceConfig` (设备管理器配置) -> `VirtioBlkDevice` (具体设备)，配置数据流贯穿始终。

## 3. 虚拟块设备实现原理

### 3.1 MMIO 接口
*   `VirtioBlkDevice` 模拟了一段 MMIO 内存区域（通常 512 字节）。
*   当 Guest OS 读写这段内存（如读取 `MagicValue`, `Version`, `DeviceID`，或写 `QueueNotify`）时，会触发 EPT 缺页或 MMIO Exit。
*   Hypervisor 捕获 Exit，调用 `handle_mmio_read/write`。
*   设备根据寄存器定义（如 `VIRTIO_MMIO_QUEUE_NOTIFY`）执行相应逻辑。

### 3.2 数据传输 (Virtqueue)
1.  **Guest 准备**：Guest OS 将磁盘读写请求放入 Descriptor Table，更新 Available Ring，然后写 `QueueNotify` 寄存器。
2.  **Host 响应**：
    *   `handle_mmio_write` 收到 Notify。
    *   设备读取 Available Ring（通过 `read_guest_mem` 回调）。
    *   解析 Descriptor Chain，找到数据缓冲区地址（GPA）。
    *   **执行 I/O**：根据请求类型（Read/Write），调用 Backend（`MemoryBackend` 或 `FileBackend`）将数据从后端复制到 Guest 缓冲区（或反之）。
    *   **完成**：更新 Used Ring，通过 `inject_irq` 回调注入中断。

## 4. 集成与使用方法

### 4.1 配置文件
在你的 VM 配置文件（如 `qemu-aarch64.toml`）中添加如下段落：

```toml
[[device.virtio_blk_mmio]]
path = "disk.img"          # 后端文件路径（若启用 fs 特性）或内存模拟标识
mmio_base = 0x0a003e00     # 设备 MMIO 基地址
mmio_size = 0x200          # 区域大小 (512B)
irq = 48                   # 中断号 (SPI)
```

### 4.2 编译与运行
确保 `axvm` 和 `axdevice` 启用了必要的特性（如果使用文件后端，需要启用 `fs` feature，目前默认是内存后端）。

运行 Hypervisor 时，`AxVM` 会自动：
1.  解析上述配置。
2.  初始化 `VirtioBlkDevice`。
3.  将其映射到 GPA `0x0a003e00`。

Guest OS 启动后，Virtio 驱动会自动发现该设备并加载。

## 5. 关键代码片段

**AxVM 初始化设备 (axvm/src/vm.rs):**
```rust
// 构造闭包
let read_guest_mem = {
    let address_space = address_space.clone();
    Arc::new(move |addr, len| { ... })
};

// 传入 AxVmDevices
let mut devices = axdevice::AxVmDevices::new(
    config,
    read_guest_mem,
    write_guest_mem,
    inject_irq,
);
```

**Axdevice 创建设备 (axdevice/src/device.rs):**
```rust
pub fn new(config, read_op, write_op, irq_op) -> Self {
    // ...
    for blk_cfg in &config.virtio_blk_configs {
        let dev = VirtioBlkDevice::new(blk_cfg, read_op.clone(), ...)?;
        this.add_mmio_dev(Arc::new(dev));
    }
    // ...
}
```
