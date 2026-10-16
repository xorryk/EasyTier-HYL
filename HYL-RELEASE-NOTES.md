基于官方 nightly `99db92a3`，合入 PR #2256 的 FakeTCP IPv6 套接字修复（`6649c35`）。

- 保留 nightly 的其他更新。
- Core/CLI 版本包含 `-hyl.1`；GUI 窗口标题含 `EasyTier HYL`。
- Windows x64：Core/CLI ZIP（含驱动）及 GUI 安装程序。
- Mac Apple Silicon / Intel：分别提供 GUI DMG 及 Core/CLI ZIP。
- 三个平台的构建、IPv4/IPv6 套接字测试和包完整性检查通过后，才发布本版本。

Mac 应用使用 ad-hoc 签名，未经 Apple 公证；Windows 程序未添加代码签名。安装前导出配置并退出旧程序，按系统提示允许个人构建。程序标识及配置存储位置保持兼容。

此修复不强制选择 FakeTCP，也不代表已经完成你家网络的端到端连接或性能测试。安装后需确认真实隧道协议；虚拟 IPv4 地址可以继续保留。

`BUILD-INFO-*.json` 记录源码、运行时版本及哈希；`SHA256SUMS.txt` 用于校验下载。对应源码可在本 Release 标签下载。
