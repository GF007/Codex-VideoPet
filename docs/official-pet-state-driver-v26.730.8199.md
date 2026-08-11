# Codex Pet 状态驱动兼容说明

本实现以本机 Codex Desktop `26.730.8199` 的内部 Pet 行为为参照，驱动版本写入 `/v1/state` 诊断字段 `petDriver`：`codex-desktop-26.730.8199`。这是版本化兼容实现，不是 OpenAI 对外承诺的稳定 API。

## 基础状态

同一 Session 的状态按以下顺序判断，前者优先：

| 优先级 | 状态 | 条件 | 过期时间 |
| --- | --- | --- | --- |
| 1 | `waiting` | 等待审批、等待用户输入或计划尚待执行 | 24 小时 |
| 2 | `failed` | 系统错误或任务失败 | 1 小时 |
| 3 | `review` | 有未读完成结果 | 7 天 |
| 4 | `running` | Session 活跃或正在处理 | 不过期 |
| 5 | `idle` | 无以上信号 | 不过期 |

多个 Session 聚合时沿用同一优先级。过期任务使用下一 deadline 唤醒；信息层没有动画刷新循环，只由状态、播放器、窗口和 Session 事件触发重绘。

Hook 只负责提供最小、无内容的活动信号：

| Hook | 状态 |
| --- | --- |
| `SessionStart`、`SessionEnd` | `idle` |
| `UserPromptSubmit`、`PreToolUse`、`PostToolUse`、`PreCompact`、`PostCompact`、`SubagentStart`、`SubagentStop` | `running` |
| `PermissionRequest` | `waiting` |
| `Stop` | `review` |

Hook 不发送 prompt、transcript、cwd、工具名、工具参数或工具结果。Session/App Server metadata 是基础状态的权威来源；事件 ID、新鲜度、revision、结束状态与注意状态保护用于防止延迟事件倒灌。

## 瞬时交互状态

瞬时交互只覆盖可交互的普通基础状态，结束后恢复当时的当前基础状态：

| 触发 | 状态 | 结束条件 |
| --- | --- | --- |
| 进程首次显示 | `waving`，8 秒 | 到时恢复基础状态 |
| 鼠标进入 | `jumping` | 鼠标离开 |
| 拖动横向累计位移不少于 4 px | `running-left` 或 `running-right` | 拖动结束 |

本项目为了保持只读状态语义，额外规定 `waiting`、`failed`、`review` 不被交互动画遮盖；当前官方 spritesheet 组件的 hover 实际可以临时显示 `jumping`，这里属于有意强化而非逐行复制。鼠标穿透窗口接收不到真实指针语义，因此穿透开启时明确禁用悬停和拖动动画；完全点击穿透和真实 hover/drag 无法同时成立。

## 与官方 Pet 的边界

- 已对齐：九状态名称、基础状态优先级、注意状态过期、首次唤醒、悬停、拖动方向，以及状态变更重置动作的语义。
- 保留本项目行为：单进程、单 libmpv、视频素材、窗口点击穿透、Direct2D/DirectWrite 信息层和现有 Hook 隐私封装。
- 未声称还原：官方 PNG/WebP spritesheet 渲染器、方向注视、语音/音量交互、逐帧像素表现、官方私有通知存储的实时推送，以及未来 Desktop 版本的内部实现。本项目通过最小 Hook 事件和按需 `thread/read(includeTurns=false)` 快照推导状态；未暴露在这些信号中的动态状态可能延迟或不可见。
- 升级 Codex Desktop 后，应先重新核对状态优先级、过期时间、事件名称和交互阈值，再更新 `DRIVER_VERSION`。

## 验收要点

1. `SubagentStart/Stop` 必须是 `running`，不能伪造左右移动。
2. `waiting > failed > review > running > idle`，延迟活动事件不能覆盖注意状态。
3. 同状态外部事件不得重新加载视频；未设置 A-B Loop 的整片自然结束时只轮换同一状态资源池，不改变逻辑状态。真正状态变化仍只用 `loadfile replace` 切换资源池。
4. 瞬时状态结束后同时校验展示 revision 与 Session revision；期间基础状态变化时不得恢复到旧状态。
5. 过期调度不得增加帧循环或 UI 高频刷新。
