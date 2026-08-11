# Codex Pet Overlay

Codex Pet 是一个 Windows 原生桌面视频悬浮层。当前生产入口使用单进程 Win32 + 单控制器 libmpv A/B 视频槽 + Direct2D/DirectWrite，不再使用 WebView2 播放或承载界面，也不启动终端窗口。

> [!IMPORTANT]
> 这是非官方、与 OpenAI 无隶属或背书关系的兼容性项目。当前 GitHub 仓库是 **source-only public preview**：不跟踪角色视频、libmpv 二进制、安装包、QA 证据或机器本地 authoring 输出，也暂未授予开源许可证。公开二进制发布必须先完成第三方依赖与媒体权利审计。

## 仓库边界与快速验证

- `native/` 是唯一生产程序；`src-tauri/` 与 `src/` 只保留旧实现和 Video Studio 的回归/编辑用途。
- `VideoSource/V1/video-config.json` 与 `ui-palette.json` 保留资源契约；完整媒体通过独立、经授权审查的资源包提供。
- `native/third_party/mpv/` 只跟踪许可证、来源和固定哈希说明；本地开发 DLL 不进入普通 Git 历史。
- `dist-native/`、`artifacts/`、各类 `target/`/`obj/` 目录和用户 Session/Hook 快照均不属于源码提交。
- 许可状态、贡献规则、安全报告和本地资源准备分别见 [`LICENSE`](LICENSE)、[`CONTRIBUTING.md`](CONTRIBUTING.md)、[`SECURITY.md`](SECURITY.md) 与 [`docs/source-development.md`](docs/source-development.md)。

不安装媒体和 libmpv 也可以验证源码层：

```powershell
npm ci
npm run source:check
```

该门禁包含 TypeScript/前端构建、无媒体 native 测试、Hook 隐私自检和 Rust release 编译；它不等同于播放器、安装器、真实 Codex Hook、签名或媒体视觉验收。

## 当前架构

- 一个进程、一个播放器控制线程、两个 Win32/libmpv 视频槽。稳定播放时只保留一个 handle；状态切换、整片轮换和 A-B Loop 接缝前短暂预载第二个 handle，并在约 5 帧（24 fps 素材约 208 ms）内交叉淡化，结束后立即回到一个 handle。
- libmpv 使用 `vo=gpu-next`、D3D11 上下文和 `hwdec=auto`；诊断接口通过 `hwdec-current` 返回实际硬解路径。
- Win32 无边框工具窗口置顶；信息层是同进程拥有窗口，支持透明、置顶、非激活和点击穿透。
- Direct2D/DirectWrite 在视频框外绘制信息层：活动或未读 Session 以独立卡片悬浮在视频上方，当前状态面板悬浮在视频下方，角色视频区域不再被信息遮挡。上方最多显示三张卡片；超过三张时可在卡片上滚轮浏览，空闲且已读的 Session 不占用界面。
- Session 卡片与状态面板共用从 canonical idle 视频首帧提取的三色主题：深色基底、主题强调色和浅色文字。背景只使用低对比渐变；`failed`、`waiting`、`review` 继续使用固定红、黄、绿语义色。
- UI 只由状态、播放器、窗口和尺寸事件触发重绘；视频透明度时间线留在播放器线程中，Session 卡片与状态面板不参与淡化。
- 隐藏后立即暂停；默认连续隐藏 5 分钟销毁两个视频槽中的全部 libmpv handle，再显示时重建当前视频并恢复播放。
- 关闭按钮仅隐藏。托盘菜单提供显示/隐藏、鼠标穿透、始终置顶、退出，以及严格 9:16 的小（288×512）、中（360×640）、大（450×800）三档窗口。切档会同步调整 Win32 窗口与 libmpv D3D11 composition size，默认中档；选择写入当前用户设置并在重启、升级后保留。没有播放控制。

旧的 Tauri/WebView2 实现仍保留在 `src-tauri` 与 `src` 中，只作为 A/B 基线和回归参考；默认 `dev`、`build` 与正式打包均只走 `native`。

## 状态与 Session 契约

Hook/Session 数据封装保持不变；状态语义对齐本机 Codex Desktop `26.730.8199` 的 Pet 驱动：九个状态、最多 32 个 Session、每 Session 64 个事件 ID 去重、事件新鲜度保护、注意状态防延迟降级、App Server metadata revision/end/review guard 均由原生核心实现。

九个状态：`idle`、`running-right`、`running-left`、`waving`、`jumping`、`failed`、`waiting`、`running`、`review`。

Hook 仍只发送 `session_id`、可选 `turn_id`、随机事件 ID、状态、动作类别和时间；不发送 prompt、transcript、工作目录、工具输入或工具输出。Hook `session_id` 只作为活动关联键，不再直接视为可导航的 Desktop Thread。恢复出来的 Session 默认没有路由资格；原生进程先通过完整分页的非归档 `thread/list` 活动集合做正向确认，再按需用 `thread/read(includeTurns=false)` 补全标题并验证持久化 `thread.id`，最多重试三次。只有仍位于非归档活动集合、且返回的非临时 Thread ID 与候选 ID 精确一致时才生成跳转目标。活动集合与路由验证结果都只保留在运行时，重启后重新协调，不会沿用快照中的旧链接；直接在 Codex 归档、已经删除或不再出现在活动集合中的旧记录只在内部保留，不参与卡片和聚合状态。标题优先使用用户可见名称；名称为空时只在内存中显示截断后的本地预览，预览不会写入 Hook、事件 outbox 或 Session 快照；仍不可用时显示不含 ID 的“当前任务”。

Hook 在投递前把同一份最小事件写入 `%LOCALAPPDATA%\com.local.codex-pet-overlay\session-data\events`；原生进程确认事件并持久化 Session 快照后，Hook 才删除待投递文件。原生进程重启时优先恢复 `%LOCALAPPDATA%\com.local.codex-pet-overlay\session-data\session-state.json`，再幂等重放残留事件。该恢复只覆盖已经被 Hook 观察到的 Session，不扫描或声称还原 Hook 安装/信任前的全部 Codex 历史。

基础状态由 Session Hook 的实时事件驱动，App Server 只提供明确的 `waiting/failed/running/review` 正向状态信号和 Thread metadata；`notLoaded`、`idle`、`closed` 或未知中性状态不会把仍在 Thinking 的 Hook `running` 降成 `idle`。聚合优先级为 `waiting > failed > review > running > idle`。`waiting`、`failed`、`review` 分别在 24 小时、1 小时、7 天后过期；过期由单次 deadline 事件处理，不增加 UI 高频刷新。首次唤醒显示 8 秒 `waving`；关闭鼠标穿透后，悬停显示 `jumping`，拖动窗口按横向位移显示 `running-left/right`，交互结束恢复当前 Session 基础状态。注意状态不会被交互覆盖；启用鼠标穿透时不提供悬停或拖动互动。

只有已验证主体 Thread 的 Session 卡片可以点击，跳转目标固定为 `codex://threads/<verified-thread-id>`。后台每 30 秒协调一次已有记录；点击时强制刷新非归档活动集合，再执行 `thread/read(includeTurns=false)` 预检。目标已归档、已删除、集合查询不完整、验证失败、ID 不一致、临时 Thread 或系统提交失败时都不会发送 URI，也不会清除未读；新的实时 Hook 会重新激活此前被抑制的本地记录。深链成功提交后即把本次点击视为“已查看”，原子地清除对应 `review`/未读并回到当前聚合状态；路由 revision 已变化时不会误清后续的新提醒。未验证或被抑制的完成态不会进入 REVIEW 卡片或聚合优先级，刚由实时 Hook 激活的活动态可暂时显示“正在确认主体任务”但不可点击。内部仍保存最多 32 条有界记录供恢复和事件去重；`SubagentStart/Stop` 与工具事件都映射为通用 `running`，左右方向只表示窗口拖动方向。完整版本化映射与兼容边界见 [`docs/official-pet-state-driver-v26.730.8199.md`](docs/official-pet-state-driver-v26.730.8199.md)。

## 视频目录

运行时优先读取 `VideoSource/V1/video-config.json`。该文件描述当前 native 状态源中的状态到视频资源池的映射、选择模式、权重、状态切换策略和可选单段 A-B Loop；编辑器与运行时共同使用 `native/src/video_config.rs` 的模型和校验器，不维护第二份状态枚举。配置不存在时才回退到递归扫描：支持 `.mp4`、`.webm`、`.m4v`；文件名使用最长状态前缀匹配，也支持 `<state>/任意文件名` 子目录。通用 `running` 在没有独立素材时使用 `running-left*` 与 `running-right*` 联合集合。

目录查找顺序：

1. `CODEX_PET_VIDEO_DIR` 指向的绝对目录；
2. 程序旁边的 `VideoSource/V1`；
3. 源码目录的 `VideoSource/V1`；
4. `%LOCALAPPDATA%\com.local.codex-pet-overlay\VideoSource\V1`。

逻辑状态首次生效、真正进入另一状态，或一条未设置 A-B Loop 的整片接近自然结束时，都会从当前状态资源池选择并预载下一条。`weighted_random` 按权重随机并在多候选时避免立即重复；单候选从尾部交叉淡化回头部，`fixed` 固定选择第一条启用资源。同一状态的重复外部事件仍不重新选片，也不改变逻辑状态。每个状态的 `transition_policy` 默认为 `immediate`；设置为 `finish_current_clip` 后，普通状态变化只更新播放器的 latest-wins 下一目标，当前片段会播放到片尾安全窗口再开始 5 帧 Blend，`waiting/failed/review` 注意态仍可立即抢占。当前 `waving` 和 `jumping` 使用完整片段策略。资源设置 `loop.start_ms/end_ms` 时，视频先从 0 播放到 B，随后以 A/B 双槽在 B 尾部到 A 头部做约 5 帧接缝淡化；未设置时整片尾部与下一片头部使用相同策略。

运行中的 Pet 每 250ms 检查一次原子保存的配置。新配置通过同一校验器后增加配置 revision、保留仍有效的当前资源并重新应用 Loop；非法配置不会覆盖最后一份有效配置。`GET /v1/video-config` 同时返回配置指纹和已就绪播放器指纹，供编辑器确认“文件已保存”和“实际播放器已应用”不是同一件事。

## Idle 色盘与 UI 主题

`scripts/generate-ui-palette.mjs` 固定选择 `video-config.json` 中第一条启用的 idle 资源作为 canonical 来源，只解码第 0 帧并缩小到 96×96。脚本使用确定性的 OKLab K-Means 聚类，内部候选最终只收敛为 `base`、`accent`、`light` 三个 UI 核心颜色，并将结果写入 `VideoSource/V1/ui-palette.json`。Sidecar 同时记录源文件相对路径、SHA-256、帧号和算法版本；源文件没有变化时直接跳过，不重复解码。

`npm run dev`、`npm run build`、`npm run native:build` 与 `npm run native:package` 都会先生成或校验色盘。运行时不读取播放器像素，也不会随状态或视频帧持续取色；它只监听 sidecar，合法新色盘到达时增加一次主题 revision 并触发一次 Direct2D 重绘。Sidecar 缺失或非法时保留最后一个有效主题，启动阶段则使用安全的暗紫色 fallback。

```powershell
npm run palette:generate
npm run palette:test
```

## 视频配置器

视频配置器是 editor-only Tauri 工具，不进入 native 便携包。状态列表启动时直接从 native `VALID_STATES`/driver schema 派生；前端没有状态 union、数组或中文映射副本。若 Pet 正在运行，编辑器以 `GET /v1/video-config` 返回的 `configRoot/configPath` 为权威，直接编辑该进程正在监听的资源目录；Pet 未运行时才回退到本地开发资源目录。

```powershell
npm run editor:dev
```

使用方式：左侧选择固定状态；把 MP4/WebM/M4V 拖到窗口中，文件会复制到该状态的受管目录；右侧调整启用、权重和选择模式；中间通过单播放器预览并在时间轴设置一个 A-B Loop；最后点击“保存并应用”。底部同步状态只有在运行时配置指纹和实际播放指纹一致时才显示已同步。`Ctrl+S` 保存，空格播放/暂停。

只构建编辑器可执行文件：

```powershell
npm run editor:build
```

编辑器前端输出到 `dist-editor`，Tauri 二进制输出到 `src-tauri/target`；`scripts/package-native.ps1` 的 allowlist 不复制这些目录。存在 `video-config.json` 时，native 包只复制配置引用的视频以及配置本身；包级 `manifest.json` 仍只负责文件 bytes/SHA-256，不承载播放语义。

## 构建、测试与打包

需要 Windows 10/11、Rust stable MSVC；MSI 构建另外需要 .NET 8，由 `installer/CodexPet.wixproj` 固定并恢复 WiX SDK。正式程序和 Hook 入口都由同一个 `CodexPet.exe` 提供，运行时不需要 Node、Rust、WebView2、Tauri、ffmpeg 或终端，但需要程序旁的 `libmpv-2.dll` 与完整视频目录。

```powershell
npm run native:test
npm run native:build
npm run native:package
npm run native:msi
```

正式 Windows 安装包输出为 `dist-native/CodexPet-<version>-x64.msi`，安装到当前用户 `%LOCALAPPDATA%\Programs\CodexPet`，不安装服务、不要求管理员权限。受控 payload 输出到 `dist-native/CodexPet`，版本化便携 ZIP 输出为 `dist-native/CodexPet-<version>-win64.zip`。包内包括 `CodexPet.exe`、固定版本 libmpv、视频、安装说明、第三方声明、GPL 文本和 SHA-256 manifest。`native:msi` 默认只生成明确标记的 unsigned QA 包；正式发布必须向 `scripts/package-msi.ps1` 提供证书 thumbprint，使 EXE 与 MSI 均通过 Authenticode/时间戳门禁。完整流程见 [`docs/windows-release.md`](docs/windows-release.md)。

本地运行：

```powershell
npm run dev
```

`#![windows_subsystem = "windows"]` 保证 release/debug GUI 入口均不弹终端。

## Hook 安装与本机接口

```powershell
npm run hook:self-test
npm run hooks:install
npm run hooks:uninstall
npm run state -- running
Invoke-RestMethod http://127.0.0.1:48193/v1/state
Invoke-RestMethod http://127.0.0.1:48193/v1/sessions
Invoke-RestMethod http://127.0.0.1:48193/v1/video-config
```

首次从 MSI 的开始菜单入口运行时会打开两页原生向导：第一页备份并幂等合并 11 个由本产品拥有的 Hook，可选择当前用户登录启动；第二页要求用户在 Codex `/hooks` 中逐项授权，并以安装后收到的新 Hook 事件验证传输链。该检测只能证明至少一条新事件已抵达，11 项是否全部 trusted 仍以 Codex `/hooks` 的逐项显示为准。安装器不会自动改信任记录，配置存在或 enabled 不代表 Hook 已获准运行。详细安装、修复和卸载边界见 [`docs/windows-installation.md`](docs/windows-installation.md)。

Hook 命令为同一签名 EXE 的 `CodexPet.exe --hook --owner codex-pet-overlay` 子命令；它只写入 schema 版本、Hook 事件名、Session ID、可选 Turn ID、随机事件 ID、状态、动作类别和时间，先持久化最小 outbox 事件，再以 250ms 预算投递。如果 Pet 未运行，Hook 会发起一次后台启动，运行中的 outbox worker 幂等补投。并发 Hook 仍由单实例锁收敛为一个 Pet 进程。outbox 采用 2048 文件 / 32 MiB 的有界保留策略，长时离线达到上限时优先淘汰最旧事件；损坏事件会隔离到 `events/quarantine`，不会阻塞后续合法事件。选择“随 Windows 登录启动”时只写当前用户 HKCU Run；登录后的 Pet 保持隐藏，收到新的 Hook 后显示。

Hook endpoint 固定为 `http://127.0.0.1:48193/v1/codex-event`。接口只绑定 loopback；如设置 `CODEX_PET_TOKEN`，桌宠和 Hook 必须使用同一值，写接口启用 Bearer 校验。

诊断返回当前视频、视频目录、`source_switch_count`、`decoder_start_count`、`crossfade_count`、`crossfade_active`、淡化时长、活动播放器实例数、隐藏释放次数、libmpv/API 版本、构建配置、`hwdec_current`、窗口标志和 D2D/DWrite 错误。

窗口自动化：

```powershell
Invoke-RestMethod -Method Post http://127.0.0.1:48193/v1/window -ContentType application/json -Body '{"click_through":true}'
Invoke-RestMethod -Method Post http://127.0.0.1:48193/v1/window -ContentType application/json -Body '{"visible":false}'
```

## 性能验收

先执行 `npm run native:package` 生成最新便携包，并退出所有 CodexPet 实例，再运行：

```powershell
npm run native:perf
```

默认执行 25 次预热、1000 次跨状态替换、200 次同状态洪泛、隐藏暂停、测试阈值下的长期隐藏释放及重新显示重建。报告写入 `artifacts/perf-*/result.json` 和 `samples.json`，检查：

- 隐藏释放并重新显示后的稳定进程树 private bytes 相对改造前 WebView2 基线降低至少 15%；该预算包含 A/B 交叉阶段所需的第二个 D3D11 解码槽与 DirectComposition 资源；
- 1000 次切换压力期内存增长保持在 64 MiB 或预热值 25% 的较大者以内，增长斜率不超过每 100 次切换 8 MiB；长期隐藏释放后 private bytes 必须回落到压力前平台以内；
- 同状态事件不增加视频替换计数；
- 稳定播放为 1 个活动播放器，交叉淡化窗口允许短暂达到 2 个，但不得出现第 3 个；
- 至少完成一次交叉淡化，且压力阶段结束后 `crossfade_active=false`；
- `hwdec_current` 为 D3D11VA/DXVA2；
- 隐藏暂停、长期隐藏释放、显示重建均成立；
- 播放器和 Direct2D/DirectWrite 无诊断错误。

该脚本是短时压力门禁，不替代 8 小时 soak。正式验收还需独立运行长时 Session 与频繁切换测试，比较首尾稳定平台并确认没有持续单调增长。

## 第三方边界

当前固定的 community libmpv build 未声明 `-Dgpl=false`，因此按 GPL build 对待。内部 QA 包带 GPL 文本与来源/hash 记录。公开或商业分发前必须完成精确依赖许可证审计和对应源码义务，或替换为按目标许可配置自行构建并审计的 libmpv。
