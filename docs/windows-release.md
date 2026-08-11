# Windows Release Runbook

> 当前公开 GitHub 仓库是 source-only preview，尚无满足本手册全部门禁的公开二进制版本。未完成第三方依赖/媒体权利审计或没有有效发布者签名时，禁止把本地 QA MSI/ZIP 标记为正式 Release。

## 发行合同

- 唯一生产入口：`native` Win32/libmpv 程序。`src-tauri` 与旧 NSIS 只作回归参考，不进入发行。
- 版本源：`native/Cargo.toml` 与根 `package.json` 必须一致；Windows Installer 只使用前三段版本号。
- `UpgradeCode` 跨版本固定；`package-msi.ps1` 根据三段式产品版本确定性生成 MSI `ProductCode`，因此新版本必然变化、同版本重打包保持不变。
- 交付：x64、per-user MSI，安装到 `%LOCALAPPDATA%\Programs\CodexPet`；便携 ZIP 是附加的诊断产物。
- 运行时依赖：包内 `CodexPet.exe`、固定且校验过的 LGPL `libmpv-2.dll`、`DEPENDENCY_LICENSES.json`、LGPL 文本和视频目录；Hook 不依赖 Node。
- 信任边界：安装器只合并 Hook 配置，不写 Codex trust；所有 11 项仍需用户在 `/hooks` 中授权。
- 用户数据：升级/repair/uninstall 都不删除 `%LOCALAPPDATA%\com.local.codex-pet-overlay`。卸载只清本产品 owned Hook 和 HKCU Run 值。

## 构建

内部未签 QA：

```powershell
npm run libmpv:install
npm run native:test
npm run palette:test
npm run native:msi
```

正式签名发行（证书已导入当前用户或机器证书库）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/package-msi.ps1 `
  -CertificateThumbprint '<40-HEX-THUMBPRINT>'
```

流程顺序固定为：校验固定 LGPL libmpv → 生成色盘与 release EXE → 签自有 EXE → 生成受控 payload/manifest/ZIP → WiX 生成 MSI → 签 MSI → `release-check.ps1` 读回。第三方 `libmpv-2.dll` 保留上游身份、依赖清单和 LGPL 文本，不冒充本产品签名。

WiX SDK 由 `installer/CodexPet.wixproj` 固定到无需商业 EULA 的 WiX 4 LTS 兼容版本，并由 `dotnet build` 恢复。Windows SDK 需要提供 `signtool.exe`。CI 不得在日志中输出证书私钥或密码。

## 自动门禁

`scripts/release-check.ps1` 验证：

- Cargo、npm、manifest 版本一致；
- manifest 中每个文件的 bytes/SHA-256 与实际一致，且只有 `manifest.json` 自身不在清单中；
- 包内 README 与源码 README 相同；
- 必需的 EXE、libmpv、安装说明、视频配置存在；
- `DEPENDENCY_LICENSES.json`、LGPL 2.1/3.0 文本、DLL bytes/SHA-256 与固定来源一致，运行时 API 至少为 2.5，且 `mpv-configuration` 明确包含 `-Dgpl=false`；旧 GPL payload 不得残留；
- 包内 `CodexPet.exe --hook-self-test` 通过；
- 正式模式下 EXE 与 MSI 的 Authenticode 都为 `Valid`，签名证书 thumbprint 与发行输入一致，并存在 RFC 3161 时间戳；签名门禁必须同时收到 MSI 路径。
- WiX 中声明的全部 File Source 与受控 package manifest 一一对应，避免动态视频进入便携包却漏出 MSI。

`-AllowUnsignedQa` 只能把签名结论降级为 `unsigned-qa`，不能产生 `signed-release`。它与 `-CertificateThumbprint` 互斥，避免把 QA 流程误标为正式发行。

## 安装生命周期验收

在非管理员测试用户上保留 MSI 日志：

```powershell
msiexec.exe /i .\dist-native\CodexPet-0.3.1-x64.msi /l*vx .\artifacts\msi-install.log
msiexec.exe /fa {PRODUCT-CODE} /l*vx .\artifacts\msi-repair.log
msiexec.exe /x {PRODUCT-CODE} /l*vx .\artifacts\msi-uninstall.log
```

在开发者本机只验证文件生命周期、且必须保留当前真实 Hooks 时，可显式给卸载命令追加 `SKIPINTEGRATIONCLEANUP=1`；该属性只用于 QA，不用于正式卸载验收。

最低实机矩阵：

1. Fresh install 无 UAC，文件位于 LocalAppData，开始菜单可启动两页向导。
2. 现有 `hooks.json` 同组混入第三方 Hook 时，安装两次仍只有 11 个 owned Hook，第三方内容保持不变。
3. 未信任、只信任 10/11、全部信任三种状态不会被混淆；真实新 Hook 只确认传输链，11/11 信任仍需在 Codex `/hooks` 逐项读回。
4. Pet 完全退出时触发多个 Hook：outbox 先落盘、只启动一个 UI 实例、HTTP 持久化后清文件。
5. 登录启动只存在 HKCU Run，启动后隐藏；第一个新 Hook 显示窗口；关闭选项后 Run 值消失。
6. 同上下文升级保留设置/Session/outbox，替换二进制和视频；downgrade 被阻止。
7. Repair 恢复缺失程序文件，不重置用户 Hook/trust/data。
8. 卸载删除程序、快捷方式、owned Hook 与 Run 值，保留第三方 Hook和用户数据。
9. `signtool verify /pa /all` 对 EXE/MSI 成功并核对发布者、时间戳和证书链。
10. 托盘小/中/大三档分别得到 288×512、360×640、450×800 的窗口和 D3D11 composition；切档无黑帧/播放器错误，重启与升级后保留最后选择。

静态单测、HTTP loopback、自签名证书或仅能安装 MSI 都不能替代真实 Codex `/hooks` 信任、登录启动、升级/卸载和签名链验收。
