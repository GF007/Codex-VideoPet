# Codex Pet 视频状态机资源与动画制作规范

## 结论

当前九个名称是**逻辑状态**，不是九个永久循环视频：

`idle`、`running-right`、`running-left`、`waving`、`jumping`、`failed`、`waiting`、`running`、`review`。

视频层需要另外维护 `enter / loop / exit / once / transition` 等**媒体阶段**。持续状态不能把“进入、循环、退出”烘焙成一条再无限重播，否则角色会周期性地重新开始和结束同一状态。

推荐的制作基线是：

- 7 张必备视觉参考图；
- 17 条源视频；
- 1 份动作 manifest；
- 逐片首尾帧、循环和组合切换 QA 证据。

本规范借用了 `hatch-pet` 的九状态语义、角色身份锁定、轻量特效和视觉 QA 原则，但产物属于本项目的独立视频覆盖层。它不是 Codex 官方自定义 Pet 的 `1536×1872 spritesheet.webp + pet.json` 包，也不能直接安装到官方 Pet 目录。

其中 15 条可以完成附件原稿的最精简 MVP；但 `failed` 在当前驱动中最长可持续 1 小时，推荐用 `failed_enter + failed_loop + failed_exit` 取代原稿的单条 `failed_once`，因此正式基线有意升级为 17 条。这是对原稿语义的加强，不是原稿本身的数量结论。

## 两层状态机

逻辑状态由 Session、Hook 和交互事件决定；媒体阶段只决定当前应该播放哪一段动画。媒体 EOF 不得擅自修改逻辑状态。

```text
logical_state = running
media_phase   = running_enter
                 ↓ EOF
media_phase   = running_loop × 无限
                 ↓ logical_state 变为 review
media_phase   = running_exit
                 ↓ EOF
media_phase   = review_enter
                 ↓ EOF
media_phase   = review_loop × 无限
```

一次性动作同样不能用 EOF 提前结束逻辑状态。例如首次问候逻辑上持续 8 秒、hover 可能持续任意时间、拖动持续到鼠标释放：

```text
logical_state = waving / jumping / running-left / running-right
media_phase   = *_once
                 ↓ EOF
media_phase   = idle_loop（展示尾段）或 hold_last
                 ↓ 外部 TTL / mouseleave / drag-end
logical_state = 当前 Session 基础状态
```

这与官方 spritesheet 的“短动作后进入低动作循环”思路相同，但结束条件仍由当前视频覆盖层的外部事件与 revision 保护决定。

精简 MVP 的 `failed_once` 是特例：动作结束后必须 `hold_last` 保持失败姿势，直到恢复事件或 1 小时通知到期；不能转入普通 `idle_loop`。推荐基线用完整的 `failed_enter/loop/exit` 取代这一折中。

## 资源数量分级

| 等级 | 数量 | 内容 | 适用范围 |
| --- | ---: | --- | --- |
| 现状兼容 | 9 个逻辑 slot | 每状态一条完整无缝循环；不能表达真正的 one-shot 或 enter/exit | 现有 V1 播放器 |
| 精简 MVP | 15 条源视频 | `idle_loop`；三种持续态各 3 条；5 个一次性动作 | 允许 `failed_once` 播完后静止保持 |
| 推荐基线 | 17 条源视频 | MVP 基础上把 `failed_once` 换成 `failed_enter/loop/exit` | 保持长期失败语义 |
| 增强版 | 24 条源视频 | 推荐 17 条 + 4 个常用 `_loop_B` + 3 个直接过渡 | 长时间运行、降低重复感 |

增强版的四个 B 循环建议为 `idle`、`waiting`、`running`、`review`；原来的基线 loop 视为 A 版，可保留原文件名，也可统一重命名为 `_loop_A`，重命名不增加数量。三个高频直接过渡建议为：

- `waiting_to_running`
- `running_to_review`
- `running_to_failed`

第一版不要先做任意状态两两直达。先让每个 exit 都回到 `idle_anchor`，每个 enter 再从 `idle_anchor` 出发；确认真实高频状态链后再增加直达资源。

## 必备参考图资源

### 最少 7 张

```text
references/
  00_character_identity_board.png
  01_frame_camera_light_lock.png
  anchors/
    idle_anchor.png
    waiting_anchor.png
    running_anchor.png
    review_anchor.png
    failed_anchor.png
```

`00_character_identity_board.png` 至少包含全身三分之二正侧面、正面/左右侧面提示、脸部近景、色板、服装和固定道具。它是所有生成任务的 canonical identity source。

`01_frame_camera_light_lock.png` 固定画布比例、角色中心、脚底基线、眼睛高度、角色占屏比例、焦距、相机高度、光向、背景色和安全边距。所有锚点必须使用同一份锁定参数。

五个锚点不是五个不同角色，也不是五个不同镜头：它们只改变身体姿势、表情、视线和手部任务位置。

### 推荐补充关键姿势板

```text
keyposes/
  waving_keyposes.png
  jumping_keyposes.png
  running-left_keyposes.png
  running-right_keyposes.png
  failed_reaction_keyposes.png
```

关键姿势板用于约束预备、动作峰值和落定，不进入运行时包。生成式视频不能只依靠一张角色参考图，否则角色外观可能相似，但肩线、手位和重心仍会在切换时漂移。

## 推荐的 17 条视频

```text
clips/
  idle/
    idle_loop.mp4

  waiting/
    waiting_enter.mp4
    waiting_loop.mp4
    waiting_exit.mp4

  running/
    running_enter.mp4
    running_loop.mp4
    running_exit.mp4

  review/
    review_enter.mp4
    review_loop.mp4
    review_complete.mp4

  failed/
    failed_enter.mp4
    failed_loop.mp4
    failed_exit.mp4

  waving/
    waving_once.mp4
  jumping/
    jumping_once.mp4
  running-left/
    running-left_once.mp4
  running-right/
    running-right_once.mp4
```

命名必须使用现有逻辑词汇：工作态叫 `running`，不能在运行时资源里另起 `processing`；左右拖动态叫 `running-left/right`，不能另起 `move-left/right`。制作说明里可以把 `running` 注释为 processing/work。

## 逐状态姿势设计

| 逻辑状态 | 状态锚点与动作动词 | Enter | Loop / Once | Exit / Complete | 必须避免 |
| --- | --- | --- | --- | --- | --- |
| `idle` | `idle_anchor`；呼吸、眨眼、轻微软组织延迟 | 无 | 4–6 秒低幅循环，首尾回到 `idle_anchor` | 无 | 工作、审阅、挥手、跳跃、大幅点头、新道具；六秒完全不动也不合格 |
| `running` | `running_anchor`；专注、扫描、操作、思考 | `idle_anchor → running_anchor`，身体轻微前倾，手和视线进入工作区域 | 4–6 秒小幅手眼动作、呼吸和重心变化，回到 `running_anchor`；建议 A/B 两版 | `running_anchor → idle_anchor` | 字面奔跑、抬膝、摆臂冲刺、方向位移、速度线、灰尘；不可复用左右拖动素材 |
| `waiting` | `waiting_anchor`；期待、探询、请求 | `idle_anchor → waiting_anchor`，轻探身、抬眼或克制的询问手势 | 4–6 秒低幅目光、手、耳朵变化，回到 `waiting_anchor` | `waiting_anchor → idle_anchor` | 与 idle 无差别；工作动作、奔跑、大手势、循环中反复“发现需要输入” |
| `review` | `review_anchor`；注意、检查、确认 | 基线从 `idle_anchor` 进入；增强版可用 `running_to_review` 从工作锚点直达 | 3–5 秒小幅前倾、眨眼、头部或手部调整，回到 `review_anchor` | 轻点头/确认后回到 `idle_anchor` | 临时新增放大镜、纸张、代码、UI、标点或漂浮图标 |
| `failed` | `failed_anchor`；受挫、低头、肩部下沉 | `idle_anchor → failed_anchor`，一次短促反应后落定 | 4–6 秒极低幅呼吸/眨眼，持续保持失败可读性 | 收到恢复信号或 1 小时通知到期后，`failed_anchor → idle_anchor` | 结尾自动弹回开心中性姿势；红叉、漂浮符号、分离泪滴或烟雾 |
| `waving` | 公共 `idle_anchor`；抬手、挥动、放下 | 合并在 once 内 | 预备抬手 → 挥 2–3 次 → 手完全放下 → `idle_anchor` | 合并在 once 内 | 波纹、动作弧线、星光、漂浮特效；停在举手峰值 |
| `jumping` | 公共 `idle_anchor`；蹲、起、顶点、落地、settle | 合并在 once 内 | 蹲下预备 → 离地 → 顶点 → 下落 → 着地 → 至少两个 settle 姿势 → `idle_anchor` | 合并在 once 内 | 相机上移、缩放、地面阴影、灰尘、落地爆点、弹跳垫；脚底基线漂移 |
| `running-right` | 公共 `idle_anchor`；向右发力、侧身、拖动反馈 | 合并在 once 内 | 右向重心转移和肢体交替 → 回正 → `idle_anchor` | 合并在 once 内 | 工作语义、静态单一倾斜、速度线、灰尘；方向必须明确向右 |
| `running-left` | 公共 `idle_anchor`；向左发力、侧身、拖动反馈 | 合并在 once 内 | 左向重心转移和肢体交替 → 回正 → `idle_anchor` | 合并在 once 内 | 同上；只有在非对称服装、道具、标记和光照翻转仍合理时，才可由右向镜像 |

## 姿势与切换原则

### 1. 锚点一致不等于姿势相同

所有锚点必须共享角色身份、相机、尺度、脚底线、灯光和背景；但不同状态必须有自己的可读姿势。持续循环的首尾对齐对象是状态锚点，不是 `idle_anchor`。

```text
running_enter: running from idle_anchor to running_anchor
running_loop:  running_anchor to running_anchor
running_exit:  running_anchor to idle_anchor
```

### 2. 每段只表达一个视觉动词

动作采用 `预备 → 主动作 → 落定`。等待是期待，工作是专注操作，审阅是检查确认，失败是受挫保持。不要在一条短片中同时转身、挥手、跳跃、变焦和释放特效。

### 3. 用姿势停留而不是堆补间帧

按 8–12 fps 设计关键姿势，再编码为 24 fps。Enter/Exit 建议 0.6–1.2 秒、4–8 个关键姿势；关键动作末帧保留约 0.2–0.4 秒。Loop 的主要动作幅度要小，不能每 4 秒重新开始工作或重新发现一次任务。

### 4. 只在低速度点切换

允许切换的姿势包括：手完全放下、脚落地并 settle、身体回正、呼吸峰谷、视线落定。腾空、强倾斜、举手峰值或生成特效爆发点不能作为可中断出口。

### 5. 角色身份漂移是一票否决

跨片检查脸型、眼距、肩宽、胸廓、服装边缘、耳朵长度、手指/爪形、固定道具、材质、色板和轮廓。角色“基本相似”不够；硬切时任何比例变化都会表现为变形 pop。

### 6. 相机和画面必须完全锁定

禁止相机推拉、平移、摇移、焦距变化、自动重新构图和角色整体缩放。方向状态只能通过角色身体与肢体表达，不能靠镜头移动。

### 7. 沿用 hatch-pet 的轻量特效约束

优先使用姿势、表情和轮廓表达状态。默认不使用文本、UI、可读 Logo、速度线、拖影、灰尘、落地印记、地面阴影、光晕、漂浮标点或分离粒子。必须使用的效果应贴附角色、边缘清晰、体积很小，并且跨帧不漂移。

## 时长建议

| 片段 | 建议时长 |
| --- | ---: |
| `idle_loop` | 4–6 秒；可参考官方 6.6 秒低频节奏 |
| `waiting_loop` | 4–6 秒 |
| `running_loop` | 4–6 秒 |
| `review_loop` | 3–5 秒 |
| `failed_loop` | 4–6 秒，幅度最低 |
| 普通 `enter/exit` | 0.6–1.2 秒 |
| `waving_once` | 1.2–1.8 秒 |
| `jumping_once` | 0.8–1.2 秒 |
| 左右方向 once | 0.6–1.0 秒 |

Loop A/B 必须从同一个状态锚点开始并回到同一个状态锚点，程序才可以随机交替而不产生跳变。

## 技术交付约束

第一批保持当前媒体基线，避免引入新的解码变量：

- `720×1280`，24 fps；
- H.264、`yuv420p`、MP4；
- 无音轨；
- 每个文件恒定帧率、固定画布、固定背景和完全一致的边缘处理；
- 禁止白边、白色 matte、压缩前后背景颜色漂移；
- 文件首尾必须导出精确帧，不依赖播放器 crossfade。

如果未来确实需要透明视频，应另做 libmpv、硬件解码和窗口合成验收，不能在本批素材中直接更换编码格式。

## 动作 manifest 最小字段

运行时动作清单应与包内 SHA/bytes `manifest.json` 分开，建议命名 `motion-manifest.json`：

```json
{
  "schema_version": 1,
  "logical_vocabulary": "codex-desktop-26.730.8199",
  "canvas": { "width": 720, "height": 1280, "fps": 24, "audio": false },
  "assets": [
    {
      "id": "running_loop",
      "state": "running",
      "phase": "loop",
      "file": "clips/running/running_loop.mp4",
      "from_anchor": "running_anchor",
      "to_anchor": "running_anchor",
      "on_complete": "loop",
      "interruptible": "always"
    },
    {
      "id": "waving_once",
      "state": "waving",
      "phase": "once",
      "file": "clips/waving/waving_once.mp4",
      "from_anchor": "idle_anchor",
      "to_anchor": "idle_anchor",
      "on_complete": "play_asset:idle_loop",
      "interruptible": "attention_only"
    }
  ]
}
```

最小播放字段是 `state`、`phase`、`file`、`on_complete`。`on_complete` 至少支持：

- `loop`：循环当前片段；
- `hold_last`：保持最后一帧；
- `next_phase:<asset_id>`：进入同一逻辑状态的下一阶段；
- `play_asset:<asset_id>`：播放指定展示资源，但不改变逻辑状态；
- `return_base`：只在外部 TTL、mouseleave、drag-end、Session 状态变化或注意状态过期时恢复当前基础状态，不能由媒体 EOF 自行触发。

`interruptible` 建议使用 `always / attention_only / never`，不要用含义模糊的布尔值。当前项目的注意状态优先级始终高于普通展示过渡；任何延迟 EOF 或旧 phase 回调还必须校验 logical-state revision 和 media generation，不能覆盖更新后的 `waiting/failed/review`。

例如 `waving_once` 播完后可以复用 `idle_loop` 作为低动作展示尾段，但逻辑状态仍是 `waving`，直到 8 秒 TTL 结束。这里不需要额外的 `idle_tail.mp4`。制作和 QA 还应保存 `from_anchor`、`to_anchor`、`variant`、`interruptible`、时长、尺寸、fps 和 SHA-256。

如果为了减少 phase 间的 `loadfile replace`，把一个状态的 `enter/action + loop tail` 合并进同一 MP4，则 manifest 必须额外提供 `loop_start_ms` 和 `loop_end_ms`；播放器先播前段，再只循环尾段。逻辑状态保持不变。

## 当前项目的落地边界

当前 V1 native 播放器是 `state → 选择完整视频 → 自然 EOF 后在同一状态重新选择`，支持可选 A-B Loop，以及状态级 `transition_policy=finish_current_clip` 的最小完整片段保护，但仍没有 phase controller 或动作 manifest：

- 直接把 `*_enter`、`*_exit`、`*_once` 放进 `VideoSource/V1`，它们仍会被目录扫描器当作普通状态候选；自然 EOF 只会继续从同一状态池抽取，不能表达一次性 phase；
- `running_to_review.mp4` 等直达文件也可能按最长状态前缀被错误归类；
- 当前 V1 没有独立 `running.mp4`，通用工作态会回退到 `running-left/right` 候选。这是现状兼容机制，但与本规范“工作态不能复用方向拖动素材”冲突，因此 V2 正式资源必须补齐独立 `running` 三阶段素材；
- 显式 A-B Loop 会在状态保持期间循环，不触发自然 EOF 轮换；普通整片每次结束只改变媒体 generation，不改变 logical-state revision；
- `finish_current_clip` 只保证已开始的 V1 整片在普通切换时进入片尾安全窗口；latest-wins 队列合并快速切换，注意态仍可抢占。它不是 `enter/loop/exit/once` phase controller 的替代品；
- 因此本规范的 V2 phase 源素材必须先放在当前扫描目录之外，等 phase controller 和 manifest loader 完成后再启用；
- 现有 `dist-native/CodexPet/manifest.json` 是文件 hash/bytes 清单，不能承载动作阶段信息。

建议工作路径：

```text
VideoSourceAuthoring/V2/
  references/
  keyposes/
  clips/
  motion-manifest.json
  qa/
```

## 动画验收

1. 对每个锚点做叠帧检查：角色尺度、脚底线、眼睛高度、镜头和背景必须一致。
2. 对每个 loop 做首尾差分；只允许状态设计中声明的软组织与细微像素变化。
3. 逐帧检查预备、峰值、落定和关键姿势停留，拒绝裁切、肢体融合、角色漂移与闪烁。
4. 组合播放 `enter → loop×3 → exit`，不能出现第二次“进入状态”。
5. 组合播放 `once → idle_loop/hold_last`，逻辑状态未结束前不得从头重复 once；MVP `failed_once` 只能 hold，不能回普通 idle 展示。
6. 测试 `idle ↔ 所有状态`、`waiting → running`、`running → review/failed` 和 `running-left ↔ running-right`。
7. 连续随机切换至少 10 分钟；有明显 pop 时优先修正锚点和关键姿势，不先用 crossfade 掩盖。
8. Contact sheet 和抽帧只能定位问题；最终必须以逐帧视频、组合状态链和真实窗口播放为准。
