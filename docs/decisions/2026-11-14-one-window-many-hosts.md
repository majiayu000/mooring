---
status: proposed
date: 2026-11-14
decision-makers: majiayu000
---

# 窗口用本机那套协议直连别的电脑的后台，远程和本机走同一套代码

**要支撑的决定：** 另一台电脑上的会话怎么做到和本机「完全一致」：是继续给远程补一套界面，还是让窗口像连本机一样连它。

用户的话：「能不能两端都做到完全一致？……这个差异是来自于哪里呢？」「先试统一方案」「先出正式方案」「这是最好的方案吗？其他的平台是这样做的吗？」。

## Context

**差异从哪来（2026-11-14 查过代码）**

| | 本机 | 远程（现在） |
|---|---|---|
| 连接 | 本机 `workbench.sock`，原生协议 | `ssh host fibo remote --stdio`，手机那套协议 |
| 拿到的数据 | 整个工作区：空间、标签、分屏布局、会话、待办 | 每个终端一张卡片：标题、状态、目录；**没有标签和布局** |
| 能发的请求 | 85 种（分屏、移窗格、改名、调比例……） | 看、打字、审批、开 / 关终端、标完成 |
| 窗口代码 | 读 `self.workspace` 436 处、`self.store` 66 处 | 另写的偷看、主区域、开始页、分屏（PR #340） |

终端画面本身已经是同一套：`Pane::remote` 和本机窗格用同一个终端组件，只是连接不同。

后果：远程每补一样本机功能就要再写一遍，接缝处出错。PR #340 一天里收到 Codex 的 11 条评论（另有 2 条在之后的提交上），全部是接缝问题：焦点落到被盖住的本机窗格、⌘W 关掉看不见的本机 shell（P1）、⌘[ ⌘] 走本机布局、待办表单挡住、机器列表变了开始页指错电脑。`docs/multi-machine/spec.md` 的放弃条件早写过：「App 要为本机和远程维护两套终端代码，而且差异越来越大 → 改成本机也走远程协议，只留一套。」

**原型（分支 `proto/attach-remote-host`，提交 `29c917e1`）**

- 在 agentos 上另起一个临时后台（`/tmp/fibo-proto`），`ssh -L` 把它的 socket 转到本机 `/tmp/fibo-attach/workbench.sock`，窗口加 `--attach --runtime /tmp/fibo-attach` 直连。
- `MOORING_ATTACH_EVIDENCE` 在挂接窗口里走 ⌘T、⌘D、⇧⌘D、⌘W、运行 Agent：**PASS**。agentos 后台里存下了这个标签和左右分屏，关掉的终端是 Exited，Codex v0.169 在 agentos 上跑起来了。
- 原生协议本来就是一个窗格一条连接，`ssh -L` 转发 Unix socket 天然支持多条（两台都是 OpenSSH 10.2）。
- 跑通前踩到两处「默认是本机」：
  1. 版本不同的后台会被当成旧后台。窗口会去接管它的终端，或者**按那台的 pid 停本机进程**。原型加了挂接模式，这种情况一律不启动、不停止、不替换后台。
  2. 开终端前在本机查目录是否存在，不在就退回本机 home，被那台后台拒绝。挂接时改成交给那台后台判断。

**同类产品怎么做（2026-11-14 查过）**

| 产品 | 状态在哪 | 怎么连 | 版本 |
|---|---|---|---|
| VS Code Remote-SSH | 远程的 VS Code Server | 本地界面经 SSH 连过去 | 和客户端完全一致，连接时自动装进 `~/.vscode-server` |
| Zed 远程开发 | 远程的 `zed-remote-server`（终端、语言服务、文件） | 调用系统 `ssh` | 和本地一致，自动下载或上传 |
| WezTerm SSH 多路复用 | 远程的多路复用服务，标签和窗格在那边 | 经 SSH 连远程的 Unix socket；本机和远程的标签可以在同一个窗口（「域」） | 两边都装 WezTerm；往返延迟超过阈值时本地预测回显 |
| iTerm2 + `tmux -CC` | 远程 tmux | tmux 控制协议翻译成原生标签和分屏 | 依赖 tmux；常被抱怨重连卡 |

共同点：状态留在那台，本地只是界面，用的是和连本机同一套协议，没有另造一套摘要协议再在本地补界面。iTerm2 那种「翻译别人的协议」最接近 PR #340 的路，也是毛病最多的。

## Decision

窗口对每台电脑都用原生协议连它自己的后台。界面只有一套代码：标签、分屏、菜单、快捷键、看板、待办都读「当前这台」的工作区。

- **连接由窗口自己管。** 每台电脑起一个 `ssh -N -L <本机 socket>:<那台 socket> <host>`，断了按退避重连，ssh 选项沿用 `machines.rs` 现有那组（BatchMode、ConnectTimeout、ServerAlive）。本机 socket 放在 `<runtime>/machines/<host>/workbench.sock`。
- **挂接的后台不归这个窗口管。** 不启动、不停止、不替换、不当成旧后台。这一条已在原型里，正式版加测试守住。
- **那台的 socket 在哪由那台说。** 后台新增 `fibo socket`，打印自己正在用的 socket 路径（agentos 用的还是 `~/.local/share/keepline-workbench`）；没有这个命令的旧版提示「先更新那台的 Fibo」。
- **版本不同就帮着更新那台。** 协议主版本不同不连（和本机现在的规则一样，`Hello { major }`）；不只是提示，给一个「更新 <那台> 上的 Fibo」按钮，经 ssh 让那台走[自己的更新](2026-11-08-self-update.md)，后台交接，终端不断。不照 VS Code / Zed 另装一份服务端：那样它和那台自己的窗口不是同一个后台，两边就又不一致了。
- **慢了先本地预测回显。** 照 WezTerm：往返延迟超过阈值时，窗口先按本地预测把按键显示出来，再以那台回来的画面为准。第 1 步只测量，不做；测出来慢才做。

**不做的：**

- 不给手机协议补标签和布局。手机继续用它，桌面不再用它画主区域。
- 不做跨电脑的分屏（一格本机、一格 agentos 在同一个标签里）。一个标签属于一台电脑的一个空间，和 [电脑 → 空间 → 会话](2026-11-07-machines-hold-spaces.md) 一致。
- 不同步两台的空间和设置。

## 本机专属的功能怎么处理

| 功能 | 现在读哪里 | 挂接时 |
|---|---|---|
| 开终端的目录、「目录不见了」提示 | 本机文件系统 | 交给那台后台判断（原型已做） |
| 新空间的默认目录、目录选择器、最近目录 | 本机 home、本机列目录 | 用那台的 home；列目录走后台请求（手机已有 `dirs`，原生协议补一个） |
| 欢迎页和设置里装 Hook | 本机 `~/.claude` 等 | 在那台上执行 `ssh host fibo hooks install <agent>`；不写本机配置 |
| 状态栏额度 | 本机 Agent 日志 | 读那台后台算好的（手机已有 `usage`、`quota`） |
| 在访达中显示、点文件链接、用编辑器打开 | 本机路径 | 不提供，菜单项置灰并说明原因；以后再考虑 `ssh` 打开 |
| 侧栏标题「本机」、状态栏「本机 · N 个会话」 | 写死 | 用那台后台 `machine.name` |
| 主题、字号、快捷键、插件界面 | 本机配置 | 保持本机：这些是窗口的，不是那台电脑的 |

## 分步

| 步 | 做什么 | 验收 | 估计 |
|---|---|---|---|
| 1 一台一个窗口 | 侧栏机器标题菜单加「在新窗口打开」：窗口自己起 `ssh -L`、挂接、重连；名字改成那台的；上表前三行 | 假 ssh 证据脚本（照 `remote-board-evidence.sh`，再起一个本机后台当另一台）：⌘T / ⌘D / ⇧⌘D / ⌘W / 运行 Agent、断线重连、旧版提示；真 agentos 走一遍 | 1.5–2 天 |
| 2 一个窗口里，空间带着电脑走 | `store`、`workspace`、`layout.panes`、`nav` 收进按电脑分的结构。不做全局「切换电脑」：点哪台的空间，标签栏和主区域就是哪台的（像 WezTerm 的域，也合[电脑 → 空间 → 会话](2026-11-07-machines-hold-spaces.md)）。其他电脑在侧栏仍用现有卡片摘要 | 本机空间和 agentos 空间来回点，两边的标签、焦点、分屏都在；性能门（`scripts/perf-gate.py`）不退 | 3–4 天 |
| 3 侧栏和看板也直接读各台工作区 | 每台的工作区都保持订阅，侧栏、看板、⌘J 跨电脑排序用同一套规则；删掉桌面里的偷看远程分支和卡片摘要 | `crates/fibo-core/tests/guard.rs` 的读数只降；远程会话在看板里和本机一样能拖 | 2–3 天 |

第 1 步做完就能日常用（agentos 一个窗口、本机一个窗口），这正是 VS Code 和 Zed 的默认形态；第 2、3 步做的是 WezTerm 那样「一个窗口看全部」。

## PR #340 怎么办

建议**关掉不合**：它的主区域、开始页、窗口内分屏到第 3 步都会删掉，剩下 9 条接缝问题修了也会被扔。

要留下的只有设置里那句说明（「侧栏里只看，不能打字」已经不对了），放进第 1 步一起改。

## Consequences

- 远程和本机走同一套代码，以后本机加的功能远程自动就有，不用再写一遍。
- 两台电脑的 Fibo 要协议主版本相同，更新要两边都做。
- 本机窗口在那台上开的终端，那台自己的窗口也看得到、也能用，标签和分屏一致。两个窗口同时打字沿用现有的接管规则（后开的接管，另一边变「只读 · 可明确接管」）。
- 能 ssh 进去就等于完全控制那台的 Fibo，和现在 `fibo remote --stdio` 一样，不新增权限面。

## 放弃条件

- 第 1 步做完，经 Tailscale 连 agentos 打字回显 p95 > 200 ms（手机协议实测是 15–46 ms）：先做本地预测回显；做了还 > 200 ms，说明原生协议过 ssh 太慢 → 回到给手机协议补字段。
- 第 2 步发现窗口里按电脑切换要改的地方远超上面的数（`self.workspace` 436 处、`self.store` 66 处），三天收不住 → 停在第 1 步「一台一个窗口」。
- 你两周内没用第 1 步的窗口连过 agentos → 停在第 1 步，不做第 2、3 步。

## 待定问题

| 问题 | 影响 | 最晚 |
|---|---|---|
| 第 1 步先用「新窗口」，还是直接做第 2 步「空间带着电脑走」？ | 先新窗口能早两三天用上，但第 2 步要重做入口 | 开工前 |
| PR #340 关掉，还是先合进去当过渡？ | 合进去要先修 9 条接缝问题，约半天，之后再删 | 开工前 |
| 「在访达中显示」这类只能本机的功能，置灰还是隐藏？ | 只影响菜单 | 第 1 步验收前 |

## Verification

```sh
# 原型：agentos 上起临时后台，转发过来，跑挂接场景
ssh agentos 'mkdir -m 700 -p /tmp/fibo-proto && nohup /opt/homebrew/bin/fibo serve --runtime /tmp/fibo-proto >/tmp/fibo-proto/host.out 2>&1 &'
ssh -N -o StreamLocalBindUnlink=yes -L /tmp/fibo-attach/workbench.sock:/tmp/fibo-proto/workbench.sock agentos &
cargo build -p fibo-app --features fibo-app/visual-test
MOORING_ATTACH_EVIDENCE=1 target/debug/fibo-app --runtime /tmp/fibo-attach --attach --visual-evidence /tmp/fibo-attach-shots
```

## Alternatives

- **继续给远程补界面（PR #340 的路）。** 否：每个本机功能都要再写一遍，一天 11 条接缝问题，而且分屏只能存在于这个窗口里，那台自己的窗口看不到。
- **给手机协议补标签、布局和全部编辑请求。** 否：等于把原生协议再抄一遍，两份协议要一起维护。只有当原生协议过 ssh 太慢时才回到这条（见放弃条件）。
- **照 VS Code / Zed 在那台另装一份版本完全一致的服务端。** 否：版本省心，但它是另一个后台，那台自己的窗口看不到这里开的标签和分屏，又回到两边不一致。用「帮着更新那台」代替。
- **全局「切换电脑」模式。** 否：多一个入口，也和「空间属于电脑」重复；点空间就带着电脑走。
- **一条 `ssh host fibo bridge` 走 stdio，把原生协议塞进一条连接。** 否：原生协议一个窗格一条连接，塞进一条要自己做多路复用；`ssh -L` 现成支持。
- **不加挂接模式，直接把窗口指向转发过来的 socket。** 否：版本不同会被当成旧后台，去接管那台的终端，或按那台的 pid 停本机进程。

## More Information

- 原型：分支 `proto/attach-remote-host`，提交 `29c917e1`；场景 `crates/fibo-app/src/harness/attach_evidence.rs`。
- PR #340：https://github.com/majiayu000/fibo/pull/340
- 同类：[Zed 远程开发](https://zed.dev/docs/remote-development)、[WezTerm 多路复用](https://wezterm.org/multiplexing.html) 和 [SshDomain](https://wezterm.org/config/lua/SshDomain.html)、[VS Code Remote-SSH](https://code.visualstudio.com/blogs/2019/07/25/remote-ssh) 和 [远程扩展](https://code.visualstudio.com/api/advanced-topics/remote-extensions.md)、[iTerm2 tmux 集成](https://iterm2.com/documentation-tmux-integration.html)。
- 相关：`docs/multi-machine/spec.md`（第 1–4 步和放弃条件）、[电脑 → 空间 → 会话](2026-11-07-machines-hold-spaces.md)、[发布版自己更新，终端靠后台交接](2026-11-08-self-update.md)。

## 后来的发现

- 2026-11-14 第 1 步 1b：挂接窗口里两样暂时不显示，因为它们读的是本机文件，要由那台后台算好送来，需要改协议：
  - 窗格标题上的上下文圈（`fibo_core::context_fill`，读会话记录）。
  - 空间的 git 领先 / 落后（`workbench/divergence.rs`，读仓库）。
  另外，终端里的文件链接在挂接窗口里关掉了，以后要做就经 ssh 在那台用编辑器打开。第 2 步之前定做不做。
- 2026-11-14 第 1 步 1c：`scripts/attach-evidence.sh`（假 ssh，本机再起一个后台当另一台）和 agentos 真后台各跑一遍，都 PASS：⌘T、⌘D、⇧⌘D、⌘W、运行 Agent，掐掉 `ssh -L` 后自己重建并继续开得了终端，最后关掉这里开的标签。真后台前后都是 46 个标签、40 个在跑，agentos 自己的窗口和后台进程没换过。agentos 的 Fibo 还没有 `fibo socket`，走的是按旧版运行目录找（`keepline-workbench`）。没做：窗口崩掉时 `ssh -L` 会留下（正常退出会收掉），下次挂接用 `StreamLocalBindUnlink` 顶掉旧 socket，不影响使用。
