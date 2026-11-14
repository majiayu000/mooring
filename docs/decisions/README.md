# 决定记录

一次改动的动机、做法和否掉的方案写在**提交说明**里（`Why:` `Changed:` `Rejected:` `Impact:` `Guarded-by:` `Supersedes:`，见 `AGENTS.md`）。这里只放比一次改动活得久的决定：跨好几处代码的、被推翻过一次的、同一个方案被否掉两次的、照设计稿或惯例看会觉得奇怪的。

文件名 `YYYY-MM-DD-<英文短名>.md`（日期是决定那天；不用顺序编号，并行会话不会抢号）。模板照 [adr-skill](https://github.com/skillrecordings/adr-skill) 的简版：frontmatter 的 `status`（accepted / rejected / superseded by …）、`date`、`decision-makers`，正文 Context、Decision（写清不做什么）、Consequences、Implementation Plan（受影响路径、要避免的写法）、Verification（能直接跑的命令）、Alternatives、More Information（重开条件、来源提交和会话）。推翻时新写一条，旧的改成 `superseded by`，两边互相链接；后来的发现带日期追加，不改原文。

改到某个文件时，提到它的决定会和它最近的 `Rejected:` / `Guarded-by:` 一起贴给 Agent（`~/.config/agent-trace`，只在本机）。

| 日期 | 决定 | 状态 |
|---|---|---|
| 2026-11-14 | [窗口用本机那套协议直连别的电脑的后台，远程和本机走同一套代码](2026-11-14-one-window-many-hosts.md) | 提议 |
| 2026-11-13 | [旧名字的兼容：Keepline 删掉，Mooring 分步退](2026-11-13-compat-retire.md) | 采用 |
| 2026-11-13 | [文件、crate 和程序也改名 Fibo；旧名字留成链接](2026-11-13-rename-files-fibo.md) | 采用 |
| 2026-11-11 | [中断结束统计区间；计划正文从 CLI 自己的记录定位](2026-11-11-phone-turn-boundaries.md) | 采用 |
| 2026-11-11 | [同一功能的规则由现有所有者提供](2026-11-11-shared-feature-rules.md) | 采用 |
| 2026-11-11 | [App 图标换成 Fibo 标志：f 加金点，晚霞渐变底](2026-11-11-fibo-app-icon.md) | 采用 |
| 2026-11-11 | [默认 Agent 用下拉选择](2026-11-11-default-agent-dropdown.md) | 采用 |
| 2026-11-10 | [分隔条按两侧窗格确认身份](2026-11-10-divider-identity.md) | 采用 |
| 2026-11-10 | [Hook 不静默安装：欢迎页默认勾选随启动装，设置 › 通用是它的家](2026-11-10-hook-with-first-start.md) | 采用 |
| 2026-11-10 | [性能测试各自记录并清理进程](2026-11-10-perf-fixture-cleanup.md) | 采用 |
| 2026-11-09 | [对外改名 Fibo；内部名字和 `MOORING_*` 不动](2026-11-09-rename-fibo.md) | 内部名字部分被 [2026-11-13](2026-11-13-rename-files-fibo.md) 推翻 |
| 2026-11-09 | [「后台连接断开」只说连接；识别状态不再借用它](2026-11-09-cut-off-means-the-link.md) | 采用 |
| 2026-11-08 | [发布版从 GitHub Releases 自己更新，换包靠改名，终端靠后台交接](2026-11-08-self-update.md) | 采用 |
| 2026-11-08 | [手机主页：等你验收排在在跑上面](2026-11-08-review-above-running.md) | 采用 |
| 2026-11-08 | [界面英文化：中文原文当键，英文表编进程序](2026-11-08-english-ui.md) | 采用 |
| 2026-11-08 | [官方中继的公网地址不进公开仓库，历史已重写](2026-11-08-scrub-relay-address.md) | 采用 |
| 2026-11-07 | [Claude 额度可以让 Claude Code 自己查 /usage，默认关](2026-11-07-claude-usage-read.md) | 采用，11-21 复查 |
| 2026-11-07 | [服务不算在等：只留一个服务的回合是等你验收](2026-11-07-a-server-is-not-waiting.md) | 采用 |
| 2026-11-07 | [试两周按提交复盘返工](2026-11-07-rework-review.md) | 采用，11-21 复查 |
| 2026-11-07 | [电脑下面是空间，界面按这个层级画](2026-11-07-machines-hold-spaces.md) | 采用 |
| 2026-11-07 | [Agent 接入按实际边界回归，缺项不记通过](2026-11-07-agent-regression.md) | 采用 |
| 2026-11-07 | [Hermes、Droid、Pi/omp、DSH 按来源接入现有 Adapter](2026-11-07-more-agent-inputs.md) | 采用 |
| 2026-11-07 | [子 Agent 沿现有 Adapter 接入，共用父子索引和侧栏](2026-11-07-subagents-use-existing-adapters.md) | 采用 |
| 2026-11-06 | [看改动从会话记录取，合成净改动，盖在终端上](2026-11-06-turn-changes-from-the-log.md) | 采用 |
| 2026-11-06 | [手机聊天页显示 Claude 的下一句建议，用两周再定去留](2026-11-06-phone-input-suggestion.md) | 采用 |
| 2026-11-06 | [Agent 身份先于聊天记录](2026-11-06-agent-identity-before-transcript.md) | 采用 |
| 2026-11-03 | [鸟瞰看板拖起卡片时倾斜](2026-11-03-tilt-board-cards-while-dragging.md) | 否定 |
| 2026-11-03 | [后台 shell 一律算等后台](2026-11-03-count-background-shells-as-waiting.md) | 被取代 |
| 2026-11-03 | [后台 shell 按监听端口分任务和服务](2026-11-03-split-background-shells-by-listening-port.md) | 采用 |
| 2026-11-04 | [后台 shell：脚本自己连的端口不算服务，规则和真实案例记在这一处](2026-11-04-background-shell-own-client.md) | 采用 |
| 2026-11-05 | [第一版图标用系泊环，信号黄加墨色](2026-11-05-ring-icon.md) | 被取代 |
| 2026-11-05 | [工作台主库改 synchronous=NORMAL](2026-11-05-workbench-db-synchronous-normal.md) | 采用 |
| 2026-11-05 | [本地 main 只跟随远端](2026-11-05-local-main-follows-origin.md) | 采用，推送方式已改 |
| 2026-11-06 | [main 必须走 PR 且 CI 通过](2026-11-06-main-requires-ci.md) | 采用 |
| 2026-11-07 | [在先不管的空间里开始做事，整个空间就接着管](2026-11-07-quiet-space-doing-work-returns.md) | 采用 |
| 2026-11-06 | [侧栏不设行数上限，整列滚动](2026-11-06-sidebar-rows-scroll.md) | 采用 |
