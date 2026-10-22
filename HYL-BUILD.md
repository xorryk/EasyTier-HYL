# EasyTier HYL personal build

**当前分支为独立验证版 `hyl-faketcp-validation`，内核标记 `-hyl.5-test.2`。** 本版基于 test.1 `e7f8f27ed561457c52912395f0df625377fe198d` 修复 FakeTCP 监听端在同一网卡 IPv6 地址变化后复用旧地址收包对象的问题。缓存同时区分网卡和本地 IP/端口，新地址创建独立收包对象；已有连接保留，已移除地址的空闲对象在新地址建链时清理。测试以地址过滤的模拟驱动验证实际载荷接收、重复重连、双栈/端口隔离及清理，不需要修改系统网络。真实公网 IPv6 轮换和串流仍需手动验证。

保留 test.1 的全部协议补丁：以 hyl.4 `1624c81d9f47e841fcb6828958874037194eb476` 为基线，适配 PR #2243 `4c0a313ad15d0d12c91999648c96cdb489dd7c1b` 的 ACK/SACK、握手状态及全零填充包过滤修复。上游作者为 Max-Sum，来源见 <https://github.com/EasyTier/EasyTier/pull/2243>；补丁来源和完整提交记入构建清单。自动化测试通过不代表实际串流问题已解决。

新增异常事件计数和解析错误的连接标识，验证方法及回退步骤见 [HYL-VALIDATION.md](HYL-VALIDATION.md)。GUI 标题为 `EasyTier HYL - FakeTCP Reconnect test.2`，应用标识及配置格式与 hyl.4 相同。工作流 `HYL FakeTCP Validation Build` 只为 Windows x64 和 Apple Silicon Mac 构建，发布标签前缀为 `hyl-validation-`；原 hyl.4、test.1 分支基线和发布保留。以下记录为此前版本演进，涉及 hyl.4 的版本标记仅描述基线。

本版本基于官方 nightly `99db92a3aeb3109efe1ace5a4a3e2a1bf3840ed7`，保留该版本全部代码，再合入上游 PR #2256 的提交 `6649c35aea3769a7f1704a4ae1fe564e4f2077a9`。

修复内容：FakeTCP 主动连接按远端地址选择 IPv4 或 IPv6 套接字及绑定地址，避免 Mac 连接 IPv6 对端时因错误地址族而报错 47。原作者及补丁来源保留在 Git 提交中。

`-hyl.2` 还修复 macOS 原生 BPF 收包记录的对齐错误：使用 Darwin ABI 规定的 4 字节对齐，避免 8 字节对齐跳过下一条记录并丢弃同批后续数据。此修复仅改变 Mac 收包逻辑，Windows `-hyl.1` 仍可用于对照测试。

`-hyl.3` 进一步修复 macOS BPF 接收缓冲保持系统默认 4 KiB 的问题：在绑定接口前为本程序的 BPF 描述符申请 512 KiB，读取内核实际接受的大小后分配读取缓冲；保留更大的系统默认值。内核限制或拒绝扩容时会记录警告，并按实际大小读取。继续启用立即读取模式，不等待缓冲填满，也不修改全局 sysctl 参数。

`-hyl.4` 新增每条网络的可选配置 `[flags].faketcp_bpf_recv_buffer_kib`，单位 KiB，范围 64–32768。GUI 高级设置可以填写该值；1024 为 1 MiB，2048 为 2 MiB。留空沿用 `-hyl.3` 默认逻辑；明确填写允许增大或减小，但实际大小仍由内核决定。保存并重启网络后生效，两种连接方向均使用该值，不改变全局系统参数。该参数仅用于原生 macOS FakeTCP，不属于 WASI socket ABI。

运行时内核版本包含 `-hyl.4`，GUI 窗口标题为 `EasyTier HYL - FakeTCP IPv6`。保留原应用标识、程序文件名及配置存储位置，以兼容现有配置。本构建不预置任何个人网络配置、域名、地址或密码。

## 下载及文件选择

在本 Fork 的 Releases 中下载：

- Windows 管理器 Pro：使用 `windows-x86_64-core.zip` 中的内核、CLI 和配套驱动文件。替换前停止对应网络服务，并备份旧程序；保留原配置。
- Windows 官方 GUI：使用 `windows-x86_64-setup.exe`。
- Apple Silicon Mac（M1/M2/M3/M4 等）：使用 `macos-aarch64.dmg`。

升级 Mac 前导出原配置并从菜单完全退出旧 GUI，再替换应用。本个人构建仅使用 ad-hoc 签名，没有官方 Apple Developer ID 签名及公证；macOS 可能要求在“系统设置 → 隐私与安全性”中手动允许打开。Windows GUI/可执行文件也未添加个人代码签名。

## 连接验证

本版本修复 FakeTCP IPv6 建链、macOS 批量收包对齐及接收缓冲过小的问题，不会自动将已有 UDP/QUIC 连接切换为 FakeTCP，也不保证绕过运营商 QoS。

保留虚拟 IPv4 地址。FakeTCP 对等节点仍使用可达的公网 IPv6/域名；建立连接后，应用可以继续访问原来的虚拟 IPv4。更新 Mac 后确认其运行版本带有 `-hyl.4`，再查看连接列表中的实际隧道协议和 FakeTCP 错误日志。`netstat -B` 的 EasyTier 行可检查 `Bsize`：512 KiB 对应 524288，1 MiB 对应 1048576，2 MiB 对应 2097152。连续两次采样的 `RDrop` 增量可用于判断本地 BPF 是否仍在丢包。每次调整都核对实际值，保持相同动态负载，再比较丢包与延迟。更大容量不保证更低延迟。

CI 只在 Windows x64、Apple Silicon macOS 上构建 Core/CLI/GUI，并运行补丁携带的 IPv4/IPv6 套接字测试。在 Apple Silicon Mac 上还验证系统 SDK 的 BPF ABI，运行批量记录解析回归测试，并复现旧对齐方式丢弃后续记录的情况。原生缓冲测试以管理员权限打开未绑定接口的真实 BPF 描述符，先设置 4 KiB，再验证程序扩容及内核实际大小；该测试不抓取网络数据。测试和打包成功后才发布下载。代码层面的回归测试不能替代实际串流验证，高码率黑屏是否解决仍需安装后测试。

## 复现和维护

新增配置的 protobuf 字段使用 HYL 扩展编号 50001，避免占用上游紧邻的字段编号。表单、TOML、每实例 socket context、主动连接与监听均传递同一个可选值。CI 检查这些配置往返、清空恢复默认、范围校验，并在真实 Darwin 描述符上请求 256 KiB、1 MiB、2 MiB 和 512 KiB，验证缩小与增大及内核读回值。此后调参无需改源码或重新构建。

代码分支：`hyl-nightly-faketcp`。在 Actions 中手动运行 `HYL Windows and macOS Build` 可重新构建，或推送代码修改自动触发。

各平台的 `BUILD-INFO-*.json` 记录完整源码提交、上游基线、补丁提交、工具链版本、运行时版本及文件哈希；`SHA256SUMS.txt` 可验证下载文件。

不要将自己的网络配置或密码提交到此公开 Fork。更新基线时，应先核对上游是否已合入该修复，再更新工作流和打包脚本中的固定提交值并运行测试。

上游：<https://github.com/EasyTier/EasyTier>

补丁：<https://github.com/EasyTier/EasyTier/pull/2256>

本项目及修改遵循仓库的 LGPL-3.0 许可证，见 `LICENSE`。
