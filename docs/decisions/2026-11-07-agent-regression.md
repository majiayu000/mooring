---
status: accepted
date: 2026-11-07
decision-makers: 用户、Codex
---

# Agent 接入按实际边界回归，缺项不记通过

## Context

用户指出不能只核对以前的记录，还应测试各 CLI 的选项，并要求制定、执行稳定性回归。要支撑的决定是：每次改适配器或升级 CLI，能否重现我们承诺的启动、恢复、归属、状态和展示行为，并明确缺失证据。

判定标准：恢复命令 argv 与 CLI 契约一致；只凭明确证据绑定会话；目录覆盖有效；子日志更新到达父列表；重启后结果一致；错误、未知格式和不完整输入不制造成功状态。失败时修实现或降低支持声明，不放宽断言。没有可执行 CLI、账号、明确生命周期记录时，不宣称在线验收通过。

## Decision

采用现有 Cargo 测试、隔离后台与一个本机检查脚本，没有新测试平台或配置页面。

| 层次 | 检查内容 | 运行时机与证据边界 |
|---|---|---|
| 命令 | 注册工具有 Adapter；启动/恢复经真实 shell 进入无副作用 argv 记录器；中文、空格、引号和命令替换字符；OpenCode 临时密码和端口参数 | 每个 PR；证明 Mooring 的命令传递，不代表真实 CLI 已执行任务 |
| 参数与目录 | 恢复、短选项、上游支持的等号写法、选择器/缺值/冲突、`--`、参数形状的提示词、fork、允许保留的模型/权限参数；默认与覆盖目录 | 每个 PR；环境变量测试在独立子进程中，不污染并行测试 |
| 记录与索引 | 各来源等待/继续/结束/中断、同 ID 跨 CLI、父日志不动、回合过滤、重启；公共索引半行/截断/换 inode；Hermes 改写撤回、DSH 压缩半帧和未知版本 | 每个 PR；针对所覆盖的记录格式 |
| 后台 | 同时自动发现 Kimi、Gemini、Hermes、Droid、Pi、omp、DSH 的自定义目录；六组子任务运行→完成；父日志不动；后台重启 | 每个 PR；真实 `mooring serve` 扫描/监听/镜像/RPC，不注入 Workspace，不调用模型 |
| 历史与名称 | CLI 持久化名称进入公共命名逻辑；同长度改写、撤回使聊天旧游标失效；手机分页、轮询、重连替换旧消息，普通追加保留已有消息 | Rust 后台 RPC 与手机纯逻辑测试；不能代替真机操作 |
| 本机 CLI | 从注册表导出命令，记录版本，隔离 HOME 执行启动/恢复命令的 `--help` | CLI 升级后、发布前；帮助优先退出的 CLI 不能据此证明全部参数语义 |
| 实机任务与窗口 | 新会话、工具调用、一个子任务、提问/回答、取消、恢复；窗口与源记录相符 | 各 CLI 首次发布支持前、上游格式改变后；需要单独记录版本、账号条件和未验项 |

生产代码只抽出重复的选择器读取函数，名称和取值语义仍由各 Adapter 提供。Pi 不凭空接受其解析器不支持的 `--session=id`。内置注册数据因两批接入合并超过 800 行移到 `agent_builtin.rs`，未增加抽象层。

回归和 PR 审查另外确认两项实际缺陷：Pi/omp、Hermes、DSH 的持久化名称进入了优先级低于首句的字段；Hermes 快照改写后手机仍按旧字节游标合并，留下已修改或撤回的回复。名称归入公共 `Content.name`。索引重建时在同一事务内更新历史 generation，并随 Chat 回复传给手机；手机发现变化时放弃旧分页游标，重新读取并替换消息。只追加的镜像保留 inode 和 generation。采用这一跨端最小修复，否定仅清桌面索引或只靠消息文本去重：两者都不能删除手机已有的旧回复。桌面与手机须使用同一版本协议，不新增兼容层。旧版已有索引（没有 generation 表）重开后仍可读，首次改写产生新 generation；已用保留记录、删除该新表后重启的用例验证。名称解析规则改变则递增现有 RULES 到 14，使旧投影按既有机制重算；保留规则 13 的缓存用例曾在升级后仍显示首句，修复后必须显示持久化名称。不另写迁移或回填程序。

## Coverage and limits

- Codex/Claude 模型与权限快捷切换已有菜单和状态单元测试；新增测试明确其它 CLI 不提供快捷入口。会话接入不等于替每个 CLI 实现所有配置、插件或菜单。
- 目前仅 Codex/Claude 提取 argv 中允许保留的设置用于恢复。其它 CLI 的模型/权限能否由其自身会话恢复，必须实机验证；不能称为 Mooring 保留任意启动参数。
- Grok 使用自身 PID 注册表，OpenCode 使用声明式日志/接口，不强行把 argv 当身份来源。
- 参数用例覆盖 Mooring 涉及的选项及代表性组合。第三方扩展新选项、任意短选项组合、所有自定义目录、所有 TUI 菜单不在“全部测试过”的范围内。新增差异先加入最小失败样本。
- 本机探测不读取账号配置、不安装或升级 CLI、不启动在线任务。未安装记为 `unavailable`，有缺项总结果记为 `partial`。退出 0 只说明自动回归与可用 CLI 探测没有失败，不代表全部实机通过。
- 现有 macOS `Workbench Rust checks` 自动运行新增 Rust 测试，手机 CI 运行 Bun 测试和 TypeScript 检查；本机入口也执行这些手机检查和 remote 测试。Linux 工作流现已运行 host 库测试，但不运行本次后台集成用例；Windows 及跨平台共享 crate 的通过不能作为 macOS host 适配器已验收证据。完整 workspace 和原生窗口检查继续由现有 CI 负责。

## Verification

```sh
# 本机针对性回归及已安装 CLI 的帮助探测，结果不覆盖账号和模型任务。
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
  python3 scripts/check-agent-clis.py /tmp/mooring-agent-evidence

# 迭代时只运行相关用例。
cargo test -p mooring-host --test agent_cli_contracts
cargo test -p mooring-host --test more_agent_adapters daemon_discovers
cargo clippy -p mooring-host -p mooring-core -p mooring-remote --all-targets -- -D warnings
cargo fmt --all --check
```

`report.json` 记录源码 SHA、工作树是否有改动、每组结果、CLI 路径及版本、失败日志、未安装项和未验范围。CI 来源样本为构造数据；真实历史回放沿用两份接入决定中的只读入口，私有对话不提交。

新增参数测试在修改前暴露：`--` 后误绑、把 `--model` 当 ID、等号恢复写法缺失、Claude fork 误绑定。修复后通过，并已分别撤销参数边界、名称字段、历史 generation 的修复，确认对应断言失败；旧名称缓存用例也先失败再修复，避免只测试实现自己的输出。构造样本与测试预期本身也须对照真实格式审查：`Subagent.stopped` 表示中断，正常完成用 `ended` 判断；不能把它当通用完成布尔值。

### 2026-11-07 本轮结果

- 采用：本机入口的七组自动检查全部通过；host/remote、真实后台集成、手机测试与类型检查、Clippy 和格式检查通过。它们验证上述具体边界，不构成所有选项或在线任务的保证。
- 实际 CLI：Codex、Claude、Grok、Cursor、Qoder、Hermes、Kimi、OpenCode 的版本、启动帮助、恢复帮助通过；Gemini、Droid、Pi、omp、DSH 未安装，报告总状态为 `partial`。
- 原生名称对照：同一份来源记录，`a9d084b` 与其父提交分别干净构建；修复前四项显示原始首句，修复后 Hermes、DSH、Pi、omp 均显示持久化名称。终端正文仍显示原始请求，侧栏名称由公共命名逻辑给出。
- 撤销参数边界、名称字段、历史 generation 修复均使对应测试失败。旧规则 13 的缓存用例先暴露升级后仍用首句的问题，递增已有解析规则后通过。
- 测试环境也必须隔离：不同提交复用 Cargo 产物曾造成旧模块混入，正式结果使用独立目录，原生对照分别清理项目 crate 产物；窗口运行时用短临时目录，避免 Unix socket 路径上限。这些失败日志与最终输出一起保留，没有记为通过。
- 待验证：在线模型、真实 CLI 菜单、手机真机、长时间运行；桌面和手机需配套协议版本。没有替换安装版或修改用户账号配置。

## Alternatives and follow-up

否定为每种 CLI 重写完整解析器、另建插件框架、穷举全部模型/权限排列；它们超出接入职责。保留各工具的明确差异，用真实来源和负例约束边界。

采用本回归入口。未安装 CLI 的帮助入口、全部工具在线任务、其它原生交互和长期运行仍待验证；发现差异时，把对应版本的去敏最小记录加入 CI。

参数依据：本机八种 CLI 帮助输出；[Pi 参数解析](https://github.com/earendil-works/pi/blob/085c84e98e7f4c950f2be606466fff4f75d8fe7c/packages/coding-agent/src/cli/args.ts)、[omp 参数表](https://github.com/can1357/oh-my-pi/blob/f2b795ba747b23d42052184d822b41e3d01357cc/packages/coding-agent/src/cli/flag-tables.ts)、[Factory CLI 参考](https://docs.factory.com/droid-cli/cli-reference)。日志格式和其它版本见相邻两份接入决定。
