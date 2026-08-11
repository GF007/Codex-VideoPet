# Codex Pet for Windows：安装与首次配置

> 当前公开 GitHub 仓库只提供源码，不提供可公开分发的 MSI。本文描述的是完成依赖、媒体权利、签名和真实 Windows 验收之后的安装合同；本地 `unsigned-qa` 产物仅限内部测试。

## 安装

正式交付使用当前用户范围的 `CodexPet-<version>-x64.msi`。安装位置为 `%LOCALAPPDATA%\Programs\CodexPet`，不需要管理员权限，也不会安装 Windows 服务。便携 ZIP 仅用于开发和内部排障。

首次从开始菜单启动 `Codex Pet` 时会打开两页向导：

1. **安装并连接**：向当前用户的 `%USERPROFILE%\.codex\hooks.json` 合并 11 个 Codex Pet Hook。写入前备份原文件；只替换本产品拥有的 Hook，保留其它 Hook 和未知字段。可同时选择“随 Windows 登录启动”。
2. **授权并验证**：在 Codex 中打开 `/hooks`，逐项信任全部 11 项，然后重启 Codex Desktop 或新建任务并发送一句消息。向导检测到安装后的新 Hook 事件后会确认传输链可用；该检测不声称自动读回 11 项信任，最终信任状态仍以 `/hooks` 的逐项显示为准。

安装器和向导不会修改 Codex 的信任记录，也不会替用户自动授权。`hooks.json` 中存在或 enabled 不等于 trusted。

## 启动方式

- 正常启动：从开始菜单运行 `Codex Pet`。
- Codex 触发启动：每个已信任 Hook 先把最小事件落到本机 outbox；若 Pet 未运行，同一个 `CodexPet.exe --hook` 入口会发起一次后台启动。单实例锁会合并并发启动。
- 登录启动：向导中的选项只写当前用户 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\CodexPet`。登录后程序保持隐藏，收到新的 Codex Hook 后显示。关闭该选项会删除本产品自己的 Run 值。
- 窗口大小：托盘菜单可选择小（288×512）、中（360×640，默认）或大（450×800）。三档均为 9:16，切换时播放器合成分辨率与窗口同步更新；选择保存在当前用户 `settings.json` 中。

Hook 只发送 `schema_version`、`hook_event_name`、`session_id`、可选 `turn_id`、随机事件 ID、状态、动作类别和时间，不发送 prompt、transcript、工作目录、工具输入或工具输出。待投递事件保存在 `%LOCALAPPDATA%\com.local.codex-pet-overlay\session-data\events`；HTTP 确认并持久化 Session 快照后才删除。outbox 最多保留 2048 个文件或 32 MiB，达到上限时优先淘汰最旧事件；损坏文件会移动到 `events\quarantine`，不阻塞后续合法事件。

## 修复与验证

重新打开向导：

```powershell
& "$env:LOCALAPPDATA\Programs\CodexPet\CodexPet.exe" --setup
```

检查本机进程：

```powershell
Invoke-RestMethod http://127.0.0.1:48193/health
Invoke-RestMethod http://127.0.0.1:48193/v1/state
Invoke-RestMethod http://127.0.0.1:48193/v1/sessions
```

如果向导一直停在第二页，优先检查 Codex `/hooks` 是否全部 trusted；配置存在但未信任时，Pet 不会收到实时任务状态。端口冲突、Hook 配置损坏或启动失败会保留原配置/事件，不会把失败当成安装完成。

## 升级、卸载与数据

- 使用同一发布者、同一用户范围的新 MSI 直接覆盖升级。升级保留窗口大小等设置、Session 快照和 outbox，幂等迁移 owned Hook 到新 EXE，并把首次配置标记恢复为待验证；命令变化后需要重新在 `/hooks` 确认 11 项信任。
- MSI 卸载会移除程序文件、开始菜单入口、本产品拥有的 11 个 Hook 和本产品的登录启动项，并把保留设置标记为“重装需重新配置”；不会删除第三方 Hook。
- 默认保留 `%LOCALAPPDATA%\com.local.codex-pet-overlay`，便于重装后恢复。需要彻底清除时，应先退出 Pet、卸载 MSI，再由用户手动删除该目录。

## 包与签名

正式公开发布必须同时满足：MSI 和 `CodexPet.exe` 的 Authenticode 状态为 `Valid`，签名链为预期发布者且带 RFC 3161 时间戳。带 `unsigned-qa` 标记的产物只用于内部测试，不是正式发行包。
