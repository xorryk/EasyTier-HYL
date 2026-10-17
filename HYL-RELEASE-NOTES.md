macOS 原生 FakeTCP 之前沿用 BPF 的 4 KiB 接收缓冲，突发流量可能在交给 EasyTier 之前溢出。本版在绑定接口前为本程序申请 512 KiB，并以内核实际接受的大小读取数据，避免被系统限制时使用错误的读取长度。保留立即读取模式和更大的已有默认值；不修改全局系统参数。

本版仍基于官方 nightly `99db92a3`，保留 PR #2256 的 FakeTCP IPv6 套接字修复（`6649c35`）及 `-hyl.2` 的 Darwin BPF 4 字节对齐修复。

- Core/CLI 版本包含 `-hyl.3`；GUI 窗口标题含 `EasyTier HYL`。
- Windows x64：Core/CLI ZIP（含驱动）及 GUI 安装程序。
- Apple Silicon Mac：GUI DMG 及 Core/CLI ZIP；不构建 Intel Mac。
- 两个平台的构建、IPv4/IPv6 套接字测试和包完整性检查通过后，才发布本版本。
- Mac 验证系统 SDK 的 BPF ABI、批量记录解析，并在真实 BPF 描述符上验证从 4 KiB 扩容及内核返回的大小。
- 本次运行逻辑修改仅涉及 macOS；现有 `-hyl.1` Windows 内核可继续用于对照测试。

更新 Mac 后彻底退出旧应用，再打开新版并启动网络。使用 `netstat -B` 查看 EasyTier 行：默认情况下 `Bsize` 应为 `524288`；连续两次记录的 `RDrop` 增量可用于检查是否仍有本地 BPF 丢包。旧连接不会自动更换缓冲大小。

Mac 应用使用 ad-hoc 签名，未经 Apple 公证；Windows 程序未添加代码签名。安装前导出配置并退出旧程序。程序标识及配置存储位置保持兼容。

原生缓冲测试验证代码与 macOS 内核的交互；实际高码率串流仍需端到端测试。本版不强制选择 FakeTCP，也不保证特定码率或绕过运营商 QoS；虚拟 IPv4 地址可以继续保留。

依据：[Apple XNU 的 BPF 缓冲及 ioctl 实现](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/net/bpf.c)。

`BUILD-INFO-*.json` 记录源码、运行时版本及哈希；`SHA256SUMS.txt` 用于校验下载。对应源码可在本 Release 标签下载。
