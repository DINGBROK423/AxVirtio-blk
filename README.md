# AxVirtio-blk

为 Axvisor 实现的基于 Virtio-blk 协议的虚拟块设备。

## 功能特性

- ✅ 完整的 Virtio 1.0 MMIO 寄存器实现
- ✅ 标准 Virtio 队列机制（Descriptor Ring, Available Ring, Used Ring）
- ✅ 支持 Guest 物理内存访问
- ✅ 支持中断注入
- ✅ 支持读写 (`VIRTIO_BLK_T_IN`/`OUT`) 及 Flush (`VIRTIO_BLK_T_FLUSH`) 请求
- ✅ 文件后端存储（需启用 `fs` feature，支持宿主机文件作为磁盘）
- ✅ 内存后端存储（默认启用，适用于测试）


## 项目结构

```
AxVirtio-blk/
├── src/
│   ├── lib.rs          # 库入口
│   ├── device.rs       # Virtio-blk 设备核心逻辑（MMIO处理、队列调度）
│   ├── virtio.rs       # Virtio 协议标准常量定义
│   └── backend.rs      # 后端存储抽象（MemoryBackend/FileBackend）
├── Cargo.toml          # 项目依赖配置
└── README.md           # 项目说明文档
```

## 使用方法

### 1. 依赖配置

在 `Axdevice` 中引入：

```toml
[dependencies]
axvirtio-blk = { git = "https://github.com/DINGBROK423/AxVirtio-blk.git", branch = "p1", default-features = false }

[features]
# 如需支持文件后端，需启用 fs 特性
default = []
fs = ["axvirtio-blk/fs"]
```

### 2. VM 配置文件

在 Axvisor 的 VM 配置文件（如 `tmp/configs/arceos-aarch64-qemu-smp1.toml`）中添加 `virtio_blk_mmio` 配置段。

**配置示例及字段说明：**

```toml
# Virtio-blk 设备配置列表
[[devices.virtio_blk_mmio]]
# 设备唯一标识符，用于日志和调试
device_id = "virtio-blk0"

# MMIO 基地址 (Guest Physical Address)，需与 Guest 设备树/驱动匹配
mmio_base = "0x0a000000"

# MMIO 区域大小，Virtio MMIO 标准通常为 0x200 字节
mmio_size = "0x200"

# 中断类型，目前支持 "spi" (Shared Peripheral Interrupt)
interrupt_type = "spi"

# 中断号，需与 Guest 设备树/驱动匹配 (例如 48)
interrupt_number = 48

# Guest 内部设备路径标识（仅作元数据记录，不影响虚拟化逻辑）
guest_device_path = "/dev/vda"

# 后端类型："file" (文件) 或 "memory" (内存)
# 注意：使用 "file" 类型需要编译时开启 `fs` feature
backend_type = "file"

# 后端文件路径。如果 backend_type="file"，此处指定宿主机镜像路径；如果是 "memory"，可留空
backend_path = ""

# 磁盘容量大小，支持 hex 字符串 (如 "0x4000000") 或带单位字符串 (如 "1G")
size = "0x4000000"

# 是否只读
readonly = false

# 设备序列号，Guest 可通过通过相应命令读取
serial = "vblk0"
```

### 3. 构建与运行

参照以下命令构建并启动带有 Virtio-blk 支持的 Axvisor：

**1：运行 ArceOS SMP 示例**

```bash
cargo xtask qemu \
--build-config tmp/configs/qemu-aarch64.toml \
--qemu-config tmp/configs/qemu-aarch64-info.toml \
--vmconfigs tmp/configs/arceos-aarch64-qemu-smp1.toml
```

**2：运行 Block R/W Test 示例**

```bash
cargo xtask qemu \
--build-config tmp/configs/qemu-aarch64.toml \
--qemu-config tmp/configs/qemu-aarch64-info.toml --vmconfigs tmp/configs/arceos-blktest-aarch64-qemu-smp1.toml
```

## 实现原理

`AxVirtio-blk` 采用模块化的设计模式，实现了与 Axvisor的解耦：

1.  **设备创建**：由 `Axdevice` 管理器根据 TOML 配置实例化 `VirtioBlkDevice`。
2.  **函数注入**：实例化时传入 `read_guest_mem`、`write_guest_mem` 和 `inject_irq` 三个闭包函数。
3.  **运行时**：
    -   **MMIO 截获**：处理 Guest 对寄存器（如 Queue Notify）的读写。
    -   **数据搬运**：利用注入的闭包在 Guest 物理内存和 Backend 存储之间拷贝数据。
    -   **中断通知**：请求完成后，利用注入的闭包向 Guest 发送中断。


