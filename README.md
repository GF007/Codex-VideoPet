# Codex Pet

Codex Pet 是由 Codex Task Hook 驱动的 Windows 原生视频桌宠。<br>
Codex Pet is a native Windows video pet driven by Codex task hooks.

> 非官方项目，与 OpenAI 无隶属或背书关系。当前仓库提供源码，暂未发布已签名安装包或可公开分发的角色视频包。<br>
> Unofficial project; not affiliated with or endorsed by OpenAI. The repository currently provides source code only—no signed public installer or redistributable character pack yet.

## 安装 / Install from source

要求 Windows 10/11、Rust stable MSVC、Node.js 22 和 npm。准备一个你有权使用的完整九状态视频包。<br>
Requires Windows 10/11, Rust stable MSVC, Node.js 22, npm, and an authorized nine-state video pack.

```powershell
git clone https://github.com/GF007/Codex-VideoPet.git
cd Codex-VideoPet
npm ci
npm run libmpv:install
$env:CODEX_PET_VIDEO_DIR = 'C:\absolute\path\to\video-pack'
npm run native:build
.\native\target\release\codex-pet-native.exe --setup
```

`libmpv:install` 下载并校验固定的 LGPL 构建；DLL 不进入 Git。该构建当前用于本地与 unsigned QA，公开二进制仍需完成传递依赖 notice/license 审计。视频目录必须包含合法配置和所有引用的视频。<br>
`libmpv:install` downloads and verifies the pinned LGPL build; the DLL is not committed to Git. It is currently approved for local use and unsigned QA; public binaries still require a transitive dependency notice/license review. The media root must contain a valid config and every referenced clip.

只验证源码、不准备视频时：<br>
To verify the source without media:

```powershell
npm run source:check
```

## 首次配置 / First setup

1. 向导会备份并合并 11 个 Codex Pet Hook，可选“随 Windows 登录启动”。<br>
   The wizard backs up and merges 11 owned Hooks and optionally enables launch at sign-in.
2. 在 Codex 中打开 `/hooks`，逐项信任全部 11 项。安装器不会自动授权。<br>
   Open `/hooks` in Codex and trust all 11 entries. The installer cannot grant trust automatically.
3. 重启 Codex Desktop，或新建任务并发送一句消息，然后在向导中检测传输。<br>
   Restart Codex Desktop, or create a new task and send a message, then verify transport in the wizard.

Hook 只传递 Session/Turn ID、事件名、状态、动作类别和时间，不传 prompt、transcript、工作目录或工具输入输出。<br>
Hooks send only session/turn IDs, event name, state, activity, and time—never prompts, transcripts, working directories, or tool input/output.

## 使用 / Usage

- 已信任的 Hook 可在 Pet 未运行时自动启动单个实例。<br>
  A trusted Hook can start one Pet instance when it is not running.
- 托盘菜单：显示/隐藏、小 288×512、中 360×640（默认）、大 450×800、鼠标穿透、始终置顶、退出。<br>
  Tray menu: show/hide, small 288×512, medium 360×640 (default), large 450×800, click-through, always-on-top, and quit.
- 登录启动时程序保持隐藏，收到新的 Hook 后显示。<br>
  Launch-at-sign-in stays hidden until a new Hook arrives.
- 重新配置：`.\native\target\release\codex-pet-native.exe --setup`。<br>
  Reconfigure with `.\native\target\release\codex-pet-native.exe --setup`.

## 卸载 / Uninstall

正式 MSI 可从 Windows“已安装的应用”卸载；它只删除本项目拥有的 Hook 和登录启动项，默认保留 `%LOCALAPPDATA%\com.local.codex-pet-overlay` 用户数据。源码开发环境可运行 `npm run hooks:uninstall`。<br>
A future MSI can be removed from Windows Installed apps; it removes only product-owned Hooks and the sign-in entry, while preserving `%LOCALAPPDATA%\com.local.codex-pet-overlay` by default. For source development, run `npm run hooks:uninstall`.

## 许可证 / License

项目源码采用 [MIT](LICENSE)。libmpv/FFmpeg 采用 LGPL，详见 [依赖清单](native/third_party/mpv/DEPENDENCY_LICENSES.json) 与 [第三方声明](THIRD_PARTY_NOTICES.md)。角色视频不包含在 MIT 授权中，见 [媒体权利说明](MEDIA_LICENSES.md)。<br>
Project source is [MIT](LICENSE). libmpv/FFmpeg use LGPL; see the [dependency manifest](native/third_party/mpv/DEPENDENCY_LICENSES.json) and [third-party notices](THIRD_PARTY_NOTICES.md). Character media is outside the MIT grant; see the [media rights policy](MEDIA_LICENSES.md).

开发、安装器与发行细节见 [source development](docs/source-development.md)、[Windows installation](docs/windows-installation.md) 和 [release runbook](docs/windows-release.md)。<br>
For development, installer, and release details, see [source development](docs/source-development.md), [Windows installation](docs/windows-installation.md), and the [release runbook](docs/windows-release.md).
