# 虚拟块设备开发文档
## Day 1
选题，让agent分析一下项目架构。

之前只接触过传统内核，unikernel，libos。头一次做Hypervisor的项目，难度还很大的，自己只是一介本科生，工程能力没那么强，但还是要挑战一下自己。

### 第一类和第二类Hypervisor:
```
第 1 类虚拟机监控器：

第 1 类虚拟机监控器或裸机虚拟机监控器直接与底层计算机硬件交互。裸机虚拟机监控器直接安装在主机的物理硬件上，而不是通过操作系统安装。某些情况下，在计算机的固件中嵌入第 1 类虚拟机监控器。

第 1 类虚拟机监控器直接与服务器硬件协商，为虚拟机分配专用资源。此类虚拟机监控器还可以根据各种虚拟机请求灵活共享资源。

第 2 类虚拟机监控器：

第 2 类虚拟机监控器或托管虚拟机管理监控器通过主机的操作系统与底层主机硬件进行交互。第 2 类虚拟机监控器安装在计算机上，在其中作为应用程序运行。

第 2 类虚拟机监控器与操作系统协商以获取底层系统资源。但是，主机操作系统优先为自己的功能和应用程序分配资源，而不是为虚拟工作负载分配资源。

```
**Axvisor属于第一类**

根据项目说明书：

在 Axvisor 的整体架构中，ArceOS 处于最底层，负责提供内存管理、任务调度、设备驱动、同步原语等多种基础功能；这些功能会被 Axvisor 的各个组件所使用。然而，从软件工程的角度上，我们不能让 Axvisor 的各个组件直接依赖于 ArceOS （的 axstd 等接口组件）；这一方面是因为我们希望 ArceOS 与 Axvisor 之间的耦合度尽可能的低，这样可以提高系统的可移植性、可扩展性和可维护性，另一方面则是因为，将对 ArceOS 的依赖分散在各个组件中，会使得依赖和 feature 管理变得极度混乱，容易出现各种错误。 

其底层 OS（ArceOS）是为 hypervisor/unikernel 场景设计的极简内核，不属于第二类 Hypervisor 所依赖的“宿主 OS”。


![alt text](image.png)

## Day 2 
看了下项目说明文档（https://arceos-hypervisor.github.io/doc/development/docs/docs.html）和virtio-blk文档（https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter9/2device-driver-3.html）
了解Axvisor设计逻辑，学习虚拟化块设备以及驱动的架构

axdevice_base 是设备抽象的基础库,做了一系列定义。axdevice 中实现新设备需要符合 axdevice_base 设定的接口规范

## Day 3

根据：
![alt text](1a0168726f5d55febf788a61b905008f.png)
并自己fork了对应子仓库查看项目结构，确定virt-blk应该就是在axdevice 内实现。不过axdevice里当前实现的虚拟设备功能很少，网络模块也没有实现，支持平台也只实现了aarch64 。

## Day 4

接着看项目架构，虽然大体知道了要在axdevice中实现这个块设备，但是很多接口对接细节还不清晰（感觉像在一堆齿轮中找到那个空隙嵌入新齿轮，并且还要保证和其他齿轮准确咬合）
于是我借助现代科技的力量，问了一些问题：

### 1. Axvisor中实现的虚拟设备模块如何在axvm注册实例，并最终集成到Axdevice的，具体到代码层面

Axvisor 将 configs/vms/*.toml 中的设备描述转换为 AxVMCrateConfig，再转成 AxVMConfig 并创建 VM。

AxVMConfig 把 EmulatedDeviceConfig 与直通设备等配置传递给 AxVM::new。

AxVM::new 用这些配置实例化 AxVmDevices，它会按类型创建/登记具体的虚拟设备。

AxVmDevices 内部基于 Axdevice_base 提供的 BaseDeviceOps 接口，完成 MMIO、端口、系统寄存器虚拟设备的查找与访问。

### Axvisor：配置生成与入口

**入口函数：**
init_guest_vms 在 Axvisor/axvisor/src/vmm/config.rs。它调用 config::static_vm_configs() 读取各个 configs/vms/*.toml，解析为 AxVMCrateConfig，再转换成 AxVMConfig 并创建 VM。

**配置来源：**
build.rs（同根目录）负责把指定的 TOML 文件内容嵌入 OUT_DIR/vm_configs.rs，运行期通过 include! 载入。可在 configs/vms/ 目录看到示例，如 nimbos-aarch64.toml 中 [devices] 段的 emu_devices、passthrough_devices。

### axvm：配置结构与 VM 内部挂接

**配置结构：**
AxVMConfig 定义于 axvm/axvm/src/config.rs，字段包含 emu_devices: Vec<EmulatedDeviceConfig>、pass_through_devices、中断模式等；这些数据直接来自 Axvisor 的 AxVMCrateConfig。

**实例化流程：**
AxVM::new 位于 axvm/axvm/src/vm.rs。它做三件关键事：
1. 建立地址空间、内存映射及直通设备映射（pass_through_devices()）。
2. 构造 AxVmDevices：let mut devices = AxVmDevices::new(AxVmDeviceConfig { emu_configs: config.emu_devices().to_vec(), });
3. 根据 VMInterruptMode 做额外设置，如 Passthrough 模式下查找 VGicD（用 axdevice_base::map_device_of_type）分配 SPI，非 Passthrough 模式下通过 get_sysreg_device() 注入系统寄存器设备。

### Axdevice：虚拟设备容器与类型初始化

**导出接口：**
Axdevice/axdevice/src/lib.rs 暴露 AxVmDeviceConfig（配置封装）和 AxVmDevices（设备集合管理）。

**配置封装：**
Axdevice/axdevice/src/config.rs，AxVmDeviceConfig::new(Vec<EmulatedDeviceConfig>) 只是简单持有配置。

**设备初始化与调度：**

核心在 Axdevice/axdevice/src/device.rs。AxVmDevices::new 读取 AxVmDeviceConfig，逐个匹配 EmulatedDeviceType。目前实现了 InterruptController、GPPTRedistributor、GPPTDistributor、GPPTITS、IVCChannel 等类型，并调用 arm_vgic 系列设备或 RangeAllocator 来创建实例。

AxVmDevices 同时维护 MMIO/Port/SysReg 三类集合，并提供 handle_mmio_read/write 等接口给 AxVM::run_vcpu 调用，一旦 VM 退出到 hypervisor，就通过这些函数按地址分发到对应设备。

### Axdevice_base：接口定义
位置：Axdevice_base/axdevice_base/src/lib.rs。

**接口内容：**
BaseDeviceOps trait 统一了 handle_read/handle_write、address_range 等方法，并通过 BaseMmioDeviceOps、BaseSysRegDeviceOps、BasePortDeviceOps 这三个 alias 细分不同地址空间。map_device_of_type 则允许上层（如 axvm）把 trait object 下转到具体类型进行额外操作。

### 2. MMIO/Port/SysReg 这三类有什么区别

**MMIO（Memory-Mapped I/O）：**
把设备寄存器映射到访客物理地址空间里，Guest 通过普通的内存读写指令（load/store）访问。AxVmDevices::handle_mmio_* 会根据访问的 GPA 匹配某个 BaseMmioDeviceOps 设备，然后执行设备逻辑。适合 SoC 风格的外设（GIC、UART 等），因为这些硬件本来就是内存映射寄存器。

**Port I/O：**
x86 等体系的传统端口地址空间（in/out 指令）；地址宽度通常较小（16 位），跟普通内存空间分离。AxVmDevices::handle_port_* 会拿到端口号 Port，匹配 BasePortDeviceOps。Port I/O 主要用于兼容 PC 风格设备，如老式串口、键盘控制器、PCI 配置空间等。

**SysReg I/O：**
系统寄存器访问（例如 AArch64 的 mrs/msr）。这些寄存器不在普通物理地址范围内，而是由 CPU 特权指令直接读写。Guest 读写某些虚拟化相关系统寄存器时会陷入，AxVmDevices::handle_sys_reg_* 依据 SysRegAddr 查找 BaseSysRegDeviceOps 设备并返回模拟值。常见用途是虚拟化控制寄存器、定时器等需要特殊拦截的寄存器。

简言之：

- MMIO = 像访问内存一样操作的设备寄存器。
- Port = x86 风格的独立端口 I/O 空间。
- SysReg = CPU 专用寄存器，通过特权指令访问，需要 hypervisor 拦截模拟。

### 3. 现有虚拟设备框架

目前 VM 配置里出现的 “virtio_mmio” 都被放在 passthrough_devices，即把宿主已有的 Virtio-MMIO 设备（例如 QEMU 提供的 virtio-blk）直接映射给 Guest，Axvisor 自身并不模拟块设备寄存器。

在这种配置下，块存储由宿主（QEMU 或物理板子的真实控制器）负责，Axvisor 仅做地址映射和中断透传，不需要在 AxVmDevices 里挂载虚拟块设备。

## Day 5
设计思路：

```
Guest OS
  ↓ (访问 virtio_mmio@0xa00_0000)
AxVM::run_vcpu → AxVmDevices::handle_mmio_read/write
  ↓
VirtioBlkDevice (实现 BaseMmioDeviceOps)
  ↓ (处理 virtio 队列、描述符等)
Host 文件系统/块设备 (/dev/sdb 或普通文件)
```

参考资料：https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter9/2device-driver-2.html

## Day 6

大概写了个框架 
backend.rs --> 后端存储接口
device.rs --> 块设备核心
lib.rs -->入口
virtio.rs --> mmio参数设置

## Day 7
做对接，修改其他子仓库的文件，引入块设备依赖。

## Day 8
解决Virtio-blk 设备目前仍使用占位的 guest 内存读写闭包问题。 调用AxVM 对外暴露地址空间访问接口，真正处理队列数据。
又发现了一个新问题，目前已经将VirtioBlkDevice集成到Axdevice中，但是VirtioBlkDevice相和EmulatedDeviceType结构体定义在axvmconfig俩分支里(main,debin/add_virtio_mmio)。依赖问题还得处理。另外，中断联通还未实现：需要与 GIC/IO-APIC 等中断控制器集成，才能真正向 Guest 注入中断。

## Day 9

### 解决了device.rs的依赖冲突问题(引入axvmconfig的两个分支)

```rust
axvmconfig = { version = "0.1", default-features = false }

# Virtio-blk dependent modules provided by another branch (debin/add_virtio_mmio).
axvmconfig_virtio = { package = "axvmconfig", path = "../../axvmconfig/axvmconfig", default-features = false }
```

### 解决中断注入问题

1. AxVirtio-blk 修改：

VirtioBlkDevice 结构体新增 inject_irq 回调字段。
new 方法增加 inject_irq 参数。
在 update_used_ring 中，当设置中断状态位后，调用 self.inject_irq(self.irq_id) 触发中断。
修正 emu_type 返回 VirtioBlk (0xE1)。
完善 mmio::CONFIG 读取，返回包含容量信息的配置空间数据。

2. Axdevice 修改：

在 create_virtio_blk_device 中，创建了一个闭包作为 inject_irq 回调。
该闭包内部将 vm 转换为 AxVMRef，并调用 vm.inject_interrupt_to_vcpu 向 vCPU 0 注入中断。
现在，当 Virtio-blk 设备完成 I/O 请求时，应该能正确地向 Guest OS 注入中断了。

## Day 10-12
跑测试没跑通，11号更新分支后依旧没跑通。

![alt text](image-1.png)
后来去群里询问了一下其他训练营同学，是需要根据镜像文件地址来修改toml配置文件，但是现在还没成功，卡三天了。