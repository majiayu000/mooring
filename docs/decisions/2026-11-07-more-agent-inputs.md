---
status: accepted
date: 2026-11-07
decision-makers: 用户、Codex
---

# Hermes、Droid、Pi/omp、DSH 按来源接入现有 Adapter

## Context

用户先要求参考 Magpie 评估更多 Agent，随后明确要求逐个接入。要支撑的决定是：不改变公共状态、索引和侧栏，能否增加有来源证据的会话输入。放弃条件：只有模型网关配置，或必须根据提示词、时间相近猜父子关系，便不宣称支持该关系。

## Decision

采用现有 `AgentSpec` + `Adapter`。Hermes、Droid、Pi、oh-my-pi、DeepSeek Harness 注册启动/恢复命令；各自解释输入，共用日志索引、父子投影和卡片。Pi/omp 共享消息格式实现，只有目录归属、工具和命令差异。没有新增策略接口、配置页面或品牌专属 UI。

| 工具 | 默认输入与覆盖 | 恢复命令 | 可确定的关系和状态 |
| --- | --- | --- | --- |
| Hermes | `~/.hermes/state.db`，支持 `HERMES_HOME` | `hermes --resume <id>` | 仅 `source=subagent` + parent_session_id 作为子代理；明确 finish_reason 判定结束，clarify 调用/结果判定等待/继续 |
| Droid | `~/.factory/sessions/**/*.jsonl`，支持 `FACTORY_HOME_OVERRIDE/.factory` | `droid --resume <id>` | callingSessionId 是调用关系（也包含 /btw 侧问），parent 只是 fork；Anthropic 形状工具/回复，不凭普通文本推断结束 |
| Pi | `~/.pi/agent/sessions`，支持 `PI_CODING_AGENT_DIR` | `pi --session <id>` | 普通会话；parentSession/parentId 均不作为子代理；stopReason 判定停止/中断/错误 |
| omp | `~/.omp/agent/sessions`，已有 XDG_DATA_HOME/omp 时读取其 sessions | `omp --resume <id>` | `<session stem>/<task>.jsonl` 对应旁边父日志 header.id；支持 ask/toolResult、stopReason |
| DSH | `$DSH_HOME/sessions`，默认 `~/.dsh/sessions` | `dsh tui --resume <id>` | v4 header 的 origin=subagent、delegationDepth>0、parentSession；turn/end 原因和 approval 事件判定状态 |

Hermes 和 DSH 的输入不是只追加的普通文本，因此在 `agents/` 内归一化为 Mooring 自己目录下的 JSONL；不改变源数据库/日志。原子替换触发现有 inode 重读逻辑，避免撤回消息后仍显示旧结果。

- Hermes：SQLite 只读事务；数据库/WAL 变化后读取近 30 天活动的 cli/tui/subagent 会话。其他来源（cron、Telegram、API 等）不伪装成本地终端。前端可选的 cwd、active、compacted 字段以数据库实际 schema 为准；字段缺失不猜测其值。保留 compacted 历史、排除撤回记录。整份有效消息比较能发现同长度修改；未变化不重写镜像。源删除/过期只删除 Mooring 自己的镜像文件。
- DSH：只读取近 30 天修改的日志，只支持核对过的当前 v4，选择目录最高代；不回退旧代、不迁移旧格式。支持原始 JSONL、连续 zstd 帧，尚未写完的行/帧保留上次有效镜像。继承的 seed 前缀不再当新任务。超过 64 MiB 解压体量或更高未知格式报错，不默默截断。一个损坏会话不阻止其他会话继续读取。

## Consequences and boundaries

- 没有明确结束记录时，普通会话保留 Unknown，由现有进程/屏幕证据处理；子代理中间文本不制造完成。DSH 未知 turn/end 原因表示结束但结果未明，不当成功。
- Hermes 部分版本将子代理 source 写成父会话的 cli/tui。仅有 parent_session_id 时还可能是 fork 或压缩，无法可靠分类，当前不把它折叠成子代理。未调用模型来人为制造该标记。
- Droid 真实日志样例没有证明所有回复都持久化停止原因。本次确认会话、调用关系与消息/工具输入，未承诺所有回合均可从磁盘确定完成。
- 列表沿用直接子任务展示。omp 可以识别嵌套日志的直接父 ID，但没有新增递归树展示或任意深度状态传播。Pi 第三方 subagent 扩展、omp profile 枚举、自定义 PI_CODING_AGENT_SESSION_DIR/--session-dir、多进程内存中的分支指针不在本次范围内；会话文件里的 lineage 不等于派生关系。
- 没有新增模型供应商配置、账号接入、额度读取、专用图标、远程会话控制或 UI 状态源。

## Implementation

`mooring-core/src/agent.rs` 只注册身份与命令；`mooring-host/src/agents/{hermes,droid,pi,dsh}.rs` 解释记录，`hermes_store.rs`、`dsh_store.rs` 读取非追加/压缩来源，由现有镜像启动入口启动。公共索引、状态规则和 UI 不增加品牌分支。

## Verification

```sh
cargo test -p mooring-host --lib
cargo test -p mooring-host --test more_agent_adapters --test subagent_adapters --test architecture_boundary
cargo test -p mooring-core --lib agent::tests
cargo test -p mooring-core --test guard --test file_size
cargo clippy -p mooring-host -p mooring-core --all-targets -- -D warnings
cargo fmt --all --check
```

Hermes 的真实回放只对私有数据库副本运行：设置 `MOORING_REPLAY_HERMES_DB`，执行 `cargo test -p mooring-host --lib replay_hermes_database_copy -- --ignored --nocapture`。本机副本在近 30 天窗口读出 2 个会话，两个都经过真实 Index 建立卡片；不输出/提交对话原文。子代理、修改、撤回、核心 schema、等待/继续由合成 SQLite 夹具验收。

另以隔离 HOME 启动真实 `mooring serve`：五个主会话经各自文件/数据库自动进入 Workspace；Hermes、Droid、omp、DSH 各有一个直接子任务，Pi 无虚构子任务。全程零模型调用，结束后停止隔离 host。

其余格式使用以下上游结构构造夹具，通过真实 Adapter → Index → Content；DSH 实际进行 zstd 多帧编码/解码，截断后保留旧镜像。相邻回归涵盖 OpenCode/Cursor、同 ID 跨 CLI、fork、压缩续接、Pi lineage、omp 嵌套直接归属与重启回放。

未验：Droid、Pi、omp、DSH 的已登录真实 CLI 在线任务，Hermes 新模型任务、原生窗口及真手机。本次不安装这些 CLI、不修改账号配置，也不将 fixture 通过当作真实模型任务验收。

## Alternatives and sources

否定第二套 Strategy/Factory；否定将 Magpie 的模型网关接入名单视为完整生命周期支持；否定按 prompt/时间相近推断父子关系。Hermes 的更新语义与 DSH 的压缩属于具体来源，不为它们改造所有插件的通用声明协议。

- [Magpie 专用读取器](https://github.com/yetone/magpie/tree/30ae0e3f9209929925fc41f03db365821d4ddf10/internal/sessions)：用于定位格式和辨别 fork/caller；没有照搬其网关或账本。
- [Hermes core schema](https://github.com/NousResearch/hermes-agent/blob/dadc8aa25580ac1ecc65d6185dfc6bd0e1d6d279/hermes_state.py)、[委派创建](https://github.com/NousResearch/hermes-agent/blob/dadc8aa25580ac1ecc65d6185dfc6bd0e1d6d279/tools/delegate_tool.py)：与本机数据库 schema/聚合计数交叉核对；source=subagent 来自本机真实记录，不声称所有版本都提供。
- [Pi 当前会话格式](https://github.com/earendil-works/pi/blob/085c84e98e7f4c950f2be606466fff4f75d8fe7c/packages/coding-agent/docs/session-format.md)、[CLI 参数](https://github.com/earendil-works/pi/blob/085c84e98e7f4c950f2be606466fff4f75d8fe7c/packages/coding-agent/src/cli/args.ts)。当前 --resume 是选择器，指定会话使用 --session。
- [omp 格式](https://github.com/can1357/oh-my-pi/blob/f2b795ba747b23d42052184d822b41e3d01357cc/docs/session.md)、[任务日志归属](https://github.com/can1357/oh-my-pi/blob/f2b795ba747b23d42052184d822b41e3d01357cc/packages/coding-agent/src/task/executor.ts)。
- [DSH v4 类型](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/core/session/src/types.ts)、[物理日志格式](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/session/session-persistence-jsonl/src/format.ts)、[当前启动命令](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/apps/cli/src/args.ts)。

重开条件：来源增加明确的子代理/生命周期事件、真实 CLI 回放发现不符、或镜像负载测量证明当前具体读取器不足；届时改对应来源，不提前增加框架。
