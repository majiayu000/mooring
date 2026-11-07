---
status: accepted
date: 2026-11-07
decision-makers: 用户、Codex
---

# 子 Agent 沿现有 Adapter 接入，共用父子索引和侧栏列表

## Context

用户要求把 Codex、OpenCode、Cursor 的父子会话/子 Agent 都接入，随后要求分析并再接入几个工具，询问是否应采用策略模式以及现有架构是否混乱。

`mooring-host::agents::Adapter` 已经是按 CLI 选择的策略接口。Claude 的 `subagents()` 读取独立元数据；Codex 的 `links_children()` 使用子日志中的父线程 ID。两种数据都由 core 的 `Content::keep_subagents` 筛选并输出 `Subagent`，侧栏无需识别 CLI。

缺口在输入：OpenCode 声明只选 `parent_id IS NULL` 的根会话，Cursor 扫描跳过 `subagents/`；共享子日志查询又没有隔离 CLI 名称空间。OpenCode 原查询只返回毫秒游标 `_t`，没有解析器需要的秒级 `timestamp`。

## Decision

采用现有 Adapter；不增加第二套 Strategy/Factory、按品牌分支的 UI 或新的状态表。

- Codex 从 session metadata 读取父线程 ID，保持现有方式。
- Cursor 从 `<parent>/subagents/<child>.jsonl` 读取父子 ID，工作目录沿父会话元数据取得。子任务名取其第一条任务文本。日志的 `turn_ended` 是明确的完成/中断/失败信号。
- OpenCode 保持声明式插件；`transcripts.parent` 指向源记录中的父 ID。SQLite mirror 透传查询选出的会话元数据，并在消息前写入，避免子会话先以顶层会话出现。查询自己把毫秒转换为 `timestamp` 秒数。
- 父子查询按 `(runtime, session_id)` 区分；子线程的新日志沿共享 index 路径更新父会话。
- Claude 保留官方元数据读取策略，不强制把它的 meta 文件伪装成子会话日志。
- Qoder 读取 `<session>/subagents/agent-<id>.jsonl`，以 agent ID 区分子会话。其 meta 文件没有完成字段，所以只借用任务名称，生命周期以子日志为准；仅在 Qoder 适配器中归一化 sidechain 记录供既有事件读取器使用。
- Grok 读取父会话的 `subagents/<id>/meta.json`，使用明确的 parent_session_id、subagent_id、started_at、completed_at 和 status。summary 的 parent_session_id 只表示 fork／恢复来源，不能充当派生关系。该元数据不提供等待问题或前后台形态，不据此推断。
- 元数据变更到父日志的路径映射交由 Adapter::subagent_parent；原有 Claude 路径规则移入自己的适配器。Host 共用刷新流程。
- 八种工具共用 `Content.subagents`、回合筛选和现有侧栏折叠列表。当前列表展示直接子任务，不增加递归树或子线程终端控制。

## Consequences

新增来源应在自己的适配器或声明里归一化事实，不能要求 Host/侧栏识别 OpenCode、Cursor 等品牌。两种来源是存储方式差异，不是两套 UI 语义。

新增的父字段使用现有插件级别门槛 6；OpenCode 插件为 0.2.2，旧 App 在安装时会要求更新。目录签名和 index 仍由发布工作流生成。

Cursor 没有给每条记录写时间；用户消息时间沿用现有 timestamp 解析，其他记录沿用文件时间，不能当作精确事件遥测。关系按磁盘中明确记录的归属呈现，不根据任务文本、目录相似性或并行启动时间推断。

## Implementation Plan

- `crates/mooring-core/src/agent_transcripts.rs`：从达到文件大小上限的 agent.rs 拆出已有声明类型，并补父字段。
- `crates/mooring-host/src/agents/{cursor,declared,mirror}.rs`：各来源的归一化。
- `crates/mooring-host/src/content/children.rs`：从 db.rs 拆出已有父子投影，按 CLI 隔离。
- `crates/mooring-host/src/log_index*`：复用日志监听、持久化、父会话更新。
- `plugins/opencode/agent.json`：读取父子关系、时间转换、问题回答后的继续事件。

## Verification

- `cargo test -p mooring-host --test subagent_adapters`：OpenCode SQLite → mirror → adapter → index；Cursor 子日志 → 同一个 index；同 ID 跨 CLI 隔离、运行/等待/结束/中断、父日志不动、重启。
- `cargo test -p mooring-host --lib`：Codex、Claude、mirror 与 Cursor 相邻路径。
- `cargo test -p mooring-host --test architecture_boundary`：适配器之外不识别 CLI 品牌。
- `cargo test -p mooring-core --test guard --test file_size`：单一事实所有者、文件大小约束。
- 原生窗口同源数据前后对照；撤销父关系接入后的回归失败。

真实日志回放使用 `MOORING_REPLAY_OPENCODE_DB` / `MOORING_REPLAY_CURSOR_CHILD`，单独运行 `cargo test -p mooring-host --test subagent_adapters replay_local_child_records -- --ignored --nocapture`（Agent 注册表是进程内单例）。源数据库只读；Cursor 原文复制进临时目录后回放。旧样本越过日期窗口只发生在测试，生产查询不回填历史。

实际命令结果记录在提交说明和本地验收产物中。

## Alternatives

- 新建 SubagentStrategy 工厂：现有 Adapter 已表达来源能力，额外工厂只会增加路由和接口。
- 每种 CLI 单独写侧栏：重复状态、计数、折叠逻辑，后续必然出现差异。
- 只移除 OpenCode 顶层过滤：子会话会混进普通会话列表，未完成归属。
- 用目录或任务描述猜关系：真实日志已经提供明确父 ID 或目录归属，不采用猜测。

## More Information

仅在新的真实来源无法通过现有元数据/子日志两种入口表达时，重新评估接口。此决定不授权历史回填、兼容层或额外配置模式。


## Additional-source review

2026-11-07 决定接入 Qoder CLI 和 Grok Build：本机分别存在 85 份子日志和 207 份父会话所属的子任务 meta；只读回放抽样确认 Qoder 完成状态及 Grok 的 14 个任务结果。真实样本包含恢复来源，不能把上下文来源误当派生归属。

用户随后明确要求接入 Kimi 和 Gemini，决定继续使用现有 Adapter。Qwen 本机仅有登录数据，Amp / Pi 未发现运行记录，不据此声称已支持。

来源：[Qoder subagents](https://docs.qoder.com/cli/subagent)、[Grok subagents](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/16-subagents.md)、本机 Grok 随版本附带的 `user-guide/17-sessions.md` 与真实 meta；[Kimi data locations](https://moonshotai.github.io/kimi-cli/en/configuration/data-locations.html)、[Gemini subagents](https://geminicli.com/docs/core/subagents/)。

新增验证：`cargo test -p mooring-host --test more_subagent_adapters`；设置 MOORING_REPLAY_QODER_CHILD / MOORING_REPLAY_GROK_PARENT 后，使用 `replay_qoder_child_and_grok_metadata_read_only -- --ignored --nocapture` 只读回放。


## Kimi / Gemini integration

决定：采用两个内置 Adapter，启动／恢复入口登记在现有 AgentSpec；不增加策略工厂、UI 分支、数据库表、配置开关或历史回填。验收目标是明确归属、当前任务列表和索引重启后的同一投影；没有确定结束事件时不猜完成。

- **Kimi Code 2.3.1**：本机实际使用 `~/.kimi-code/sessions/<workspace>/session_<uuid>/agents/<agent>/wire.jsonl`，不是早期 kimi-cli 的 `.kimi` 目录。遵循 `KIMI_CODE_HOME`。`state.json` 的 `workDir`、`agents[*].parentAgentId` 提供目录和直接父关系；子 ID 加 session 前缀，避免不同会话的 `agent-0` 串联。`forkedFrom` 不作为派生关系。读取 `turn.prompt`、wire loop 的 `step.end.finishReason`、`turn.ended`、取消与 interaction 请求／解除；重复的 `agent.message.appended` 不再计为一遍用户输入。
- **Gemini CLI**：对齐上游 `62bab5b232c15b925d31e655ce844ab939c882b7` 的 JSONL 记录。`tmp/<project>/.project_root` 提供 cwd；`chats/<root-session-id>/<child>.jsonl` 提供归属。上游嵌套代理也保留根 session ID，因此这里只能展示其记录的根归属，不虚构直接父链。`GEMINI_CLI_HOME` 是用户目录覆盖值，后面仍接 `.gemini/tmp`。
- Gemini 会反复追加相同消息的工具状态快照，工具成功和单纯文本都不能证明回合结束。子代理 `complete_task` 成功才是明确完成；取消工具可能只是拒绝一次操作，不直接判整轮中断。错误记录判 Error。当前 recorder 通常只持久化完成后的工具状态，因此审批态测试只证明该状态被记录时的读取行为，不声称有实时审批遥测。
- Gemini 主会话没有持久化的明确结束标记：普通回复后的日志状态保持 Unknown，交给现有进程／屏幕状态处理；子代理中间回复保留其运行态。历史 `$patch`／`$rewindTo` 使已有回复状态失效，下一条完整事件恢复。没有新增通用历史重写引擎。仅支持当前 JSONL；旧 `.json` 快照、Seatbelt 沙箱目录和远端代理无本地日志的情形未接入。

证据：Kimi 本机实际 wire/state 只读结构与回放；Gemini 本机没有 CLI 或会话样本，使用官方源码格式构造隔离回放，不将其称为真实模型任务测试。合成数据与源文件位置记录在 `kimi_gemini_adapters.rs` 和 `visual_child_fixture.py`，不会提交私有对话原文。老 Kimi 样本的 `recent_days` 仅在回放测试中放宽。

来源：[Kimi data locations](https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/data-locations.html)、本机 `kimi --help` / `--version`；[Gemini recording](https://github.com/google-gemini/gemini-cli/blob/62bab5b232c15b925d31e655ce844ab939c882b7/packages/core/src/services/chatRecordingService.ts)、[Gemini local executor](https://github.com/google-gemini/gemini-cli/blob/62bab5b232c15b925d31e655ce844ab939c882b7/packages/core/src/agents/local-executor.ts)、[Gemini project directory marker](https://github.com/google-gemini/gemini-cli/blob/62bab5b232c15b925d31e655ce844ab939c882b7/packages/core/src/config/projectRegistry.ts)。

验证入口：`cargo test -p mooring-host --test kimi_gemini_adapters`；真实 Kimi 回放设置 `MOORING_REPLAY_KIMI_CHILD`，运行该文件的 ignored 测试；原生对照使用 `MOORING_FIXTURE_CHILD_ADAPTERS=kimi-gemini python3 scripts/visual-check.py shots HEAD /tmp/mooring-kimi-gemini --pages MainSpace@1420`。安装版、在线 Gemini 调用和真手机未验。
