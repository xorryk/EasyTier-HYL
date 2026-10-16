修复 macOS 原生 FakeTCP 收包模块的 BPF 记录对齐：Darwin 要求按 4 字节对齐，旧实现按 64 位 `long` 的 8 字节对齐，可能跳过下一条记录的开头，随后丢弃同一次读取中的剩余数据。批量收包更容易触发这个缺陷。

本版仍基于官方 nightly `99db92a3`，保留 PR #2256 的 FakeTCP IPv6 套接字修复（`6649c35`）。

- 保留 nightly 的其他更新。
- Core/CLI 版本包含 `-hyl.2`；GUI 窗口标题含 `EasyTier HYL`。
- Windows x64：Core/CLI ZIP（含驱动）及 GUI 安装程序。
- Mac Apple Silicon / Intel：分别提供 GUI DMG 及 Core/CLI ZIP。
- 三个平台的构建、IPv4/IPv6 套接字测试和包完整性检查通过后，才发布本版本。
- 两种 Mac 架构额外检查系统 SDK 的 BPF ABI，并运行批量记录、混合包长、末尾无填充及截断数据测试；测试中复现旧 8 字节对齐造成后续记录无法解析的问题。
- 本次运行逻辑修改仅涉及 macOS；现有 `-hyl.1` Windows 内核可继续用于对照测试。

Mac 应用使用 ad-hoc 签名，未经 Apple 公证；Windows 程序未添加代码签名。安装前导出配置并退出旧程序，按系统提示允许个人构建。程序标识及配置存储位置保持兼容。

该代码缺陷已加入回归测试，但是否解决实际网络中的高码率黑屏，仍需更新 Mac 后进行端到端对照测试。本版不强制选择 FakeTCP，也不保证特定码率或绕过运营商 QoS；虚拟 IPv4 地址可以继续保留。

ABI 依据：[Apple XNU 的 BPF_ALIGNMENT / BPF_WORDALIGN 定义](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/net/bpf.h)。

`BUILD-INFO-*.json` 记录源码、运行时版本及哈希；`SHA256SUMS.txt` 用于校验下载。对应源码可在本 Release 标签下载。
