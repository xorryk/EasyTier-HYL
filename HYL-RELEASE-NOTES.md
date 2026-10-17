这是用于验证 FakeTCP 异常断连的 **hyl.5-test.1 测试版**。自动化检查通过后发布；实际串流稳定性尚待对照测试，不替代现有 hyl.4。

以 hyl.4 源码 `1624c81d9f47e841fcb6828958874037194eb476` 为固定基线，适配上游 [PR #2243](https://github.com/EasyTier/EasyTier/pull/2243) 在 `4c0a313ad15d0d12c91999648c96cdb489dd7c1b` 的改动：

- 从握手捕获初始 TCP 状态，避免把 FakeTCP 数据接收进度写入系统 TCP 的 ACK。
- 接收时过滤全零填充包，防止填充内容进入 EasyTier 帧解析器并触发 `body too short`。
- 修正 SACK 序号回绕判断，保留必要的填充行为。
- 为握手缓存设置到期清理，并保留原来的真实断线、重置和异常帧关闭逻辑。

新增轻量验证日志：`zero_filler_dropped`、`sack_filler_sent`、`peer_reset`。计数在 1、2、4、8……次时输出，关闭连接时输出准确汇总；不保存数据包内容，不逐包写日志。收包解析错误额外带上连接 ID、对端 ID，便于与直连消失的时间对齐。请按验证说明临时启用 warning 日志，测试后恢复原级别。

只构建 Windows x64、Apple Silicon Mac。Windows 管理器 Pro 使用 `windows-x86_64-core.zip`；Windows 官方 GUI 使用 `windows-x86_64-setup.exe`；Apple Silicon Mac 使用 `macos-aarch64.dmg`。不构建 Intel Mac。

保留 hyl.4 的可调 macOS BPF 接收缓冲、IPv6 修复和原配置存储位置。GUI 标题及运行时版本明确标记验证版；应用标识相同，升级会替换原应用，不能把两个版本当成独立应用同时运行。安装前导出配置、保留旧安装包；按原配置测试，先不要同时改变 BPF 大小、码率或路由设置。

回归测试覆盖 ACK、SACK、序号回绕、缓存过期和填充过滤；另以 IPv4/IPv6 模拟实际 FakeTCP → 帧解码路径，确认填充后两帧正常数据仍可读取，真实 RST 仍关闭连接。保留此前 BPF ABI、批量读取、真实描述符大小与配置测试。这些检查不能证明所有实际网络问题已经解决。

测试步骤见测试分支的 [HYL-VALIDATION.md](https://github.com/xorryk/EasyTier-HYL/blob/hyl-faketcp-validation/HYL-VALIDATION.md)，Core ZIP 也包含该文档。此前 hyl.4 发布保留在 [hyl-nightly-20261017-r7](https://github.com/xorryk/EasyTier-HYL/releases/tag/hyl-nightly-20261017-r7)，可回退。新旧版本使用相同配置格式。

Mac 为 ad-hoc 签名，未经 Apple 公证；Windows 可执行文件未添加代码签名。`BUILD-INFO-*.json` 包含基线、补丁、源码提交及文件哈希，`SHA256SUMS.txt` 用于校验下载。本测试版不保证零丢包、不取消正常断线检测，也不强制禁用中转。
