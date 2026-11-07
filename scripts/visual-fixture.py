#!/usr/bin/env python3
"""A workbench full of sample sessions, for screenshots that need real-looking data.

Private HOME and runtime, synthetic Codex logs (40 sessions over 5 spaces, with tests,
patches, questions and finished turns), three todos, a split tab. No model calls,
credentials or user data. Needs a debug build with visual tests:
`cargo build -p mooring-host -p mooring-app --features mooring-app/visual-test`.

  python3 scripts/visual-fixture.py prepare          # start an isolated host and seed it
  python3 scripts/visual-fixture.py capture DIR      # render MOORING_PAGE_LIST into DIR
  python3 scripts/visual-fixture.py stop

Pages are the harness's names, e.g. MOORING_PAGE_LIST=Dense-first-standard-light,Main.
MOORING_VISUAL_BIN picks the build directory (default target/debug).
MOORING_FIXTURE_BOARD=1 at prepare fills the board's worst cases: a live running turn, three
done sessions, and a long branch on every project.
MOORING_FIXTURE_UNLINKED=1 at prepare adds a tab of Agents no conversation is tied to, for the
SidebarUnlinked-* pages.
"""
import json, os, pathlib, shlex, socket, struct, subprocess, sys, tempfile, time

BASE = pathlib.Path(__file__).resolve().parents[1]
STATE = pathlib.Path.home() / 'Library/Caches/MooringVisual/state.json'
BIN = pathlib.Path(os.environ.get('MOORING_VISUAL_BIN', BASE / 'target/debug'))


def read(s, n):
    b = b''
    while len(b) < n:
        chunk = s.recv(n - len(b))
        assert chunk, 'host closed the connection'
        b += chunk
    return b


def send(s, obj):
    b = b'\1' + json.dumps(obj).encode()
    s.sendall(struct.pack('>I', len(b)) + b)


def receive(s):
    return json.loads(read(s, struct.unpack('>I', read(s, 4))[0])[1:])


def rpc(root, obj):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(10)
        s.connect(str(pathlib.Path(root) / 'runtime/workbench.sock'))
        send(s, {'Hello': {'major': 3}})
        receive(s)
        send(s, obj)
        return receive(s)


def edit(root, obj):
    r = rpc(root, {'Edit': {'edit': obj}})
    assert 'Error' not in r, r
    return r


names = ['工作空间', 'sys', '模型上线 work', 'mooring', 'codex-mall']
LONG_TITLES = {12: 'Opus 5.5 dev环境 run_8746132f59319f2d27c0a1b2c3d4e5f6a7b8c9d0', 14: '文档里还有这些内容： - 产品为什么可能这么理解： 官网和定价页写的是另一回事，需要逐条核对后再回复'}
titles = ['修标签命名', '查 tab_name 从哪来', '核对标签测试', '审查反馈接口改动', '实现缓存结果补传', '核查指纹变更', '评估视频队列方案', '整理项目说明']
TASKS = [
    ('标签命名依旧有问题', '新开的终端标签还是叫 zsh。启动 Agent 时应该用待办标题命名标签；Agent 退出后保留这个名字，不要退回进程名。',
     ['新开 Codex 标签显示待办标题，不是 zsh', 'Agent 退出后标签名保留', 'cargo test -p mooring-app 全部通过']),
    ('健身', '', []),
    ('睡觉', '', []),
]


def call(cid, n, cmd, output, at):
    args = {'cmd': cmd} if cmd else {'input': output[1]}
    return [(at, 'response_item', {'type': 'function_call', 'name': 'exec_command' if cmd else 'apply_patch', 'call_id': f'{cid}-{n}', 'arguments': json.dumps(args)}),
            (at, 'response_item', {'type': 'function_call_output', 'call_id': f'{cid}-{n}', 'output': output if cmd else output[0]})]


def patch(path):
    body = ''.join(f'+line {n}\n' for n in range(12)) + '-old\n-older\n'
    return ('Success. Updated the following files:\nM ' + path, f'*** Begin Patch\n*** Update File: {path}\n@@\n{body}*** End Patch')


if sys.argv[1] == 'prepare':
    parent = STATE.parent; parent.mkdir(parents=True, exist_ok=True)
    root = pathlib.Path(tempfile.mkdtemp(prefix='todo-', dir=parent)); home = pathlib.Path(tempfile.mkdtemp(prefix='kltodo-home-', dir='/tmp'))
    logs = home / '.codex/sessions'; logs.mkdir(parents=True); (home / '.claude/projects').mkdir(parents=True)
    now = int(time.time())
    for n in range(40):
        project = root / 'projects' / names[n // 8]; project.mkdir(parents=True, exist_ok=True)
        stamp = now - (6 * 86400 if n % 8 == 5 else 60 * (n + 1)); cid = f'visual-{n}'
        records = [(stamp - 3600, 'session_meta', {'id': cid, 'cwd': str(project), 'source': 'cli', 'originator': 'codex-tui', 'thread_source': 'user'}),
                   (stamp - 3600, 'event_msg', {'type': 'user_message', 'message': (LONG_TITLES.get(n) if os.environ.get('MOORING_FIXTURE_BOARD') else None) or titles[n % 8] + ('' if n < 8 else ' · ' + names[n // 8])}),
                   (stamp - 3600, 'event_msg', {'type': 'task_started', 'turn_id': cid + '-turn'})]
        if n == 0:
            records += call(cid, 1, None, patch('crates/mooring-app/src/workbench/tabs.rs'), stamp - 1500)
            records += call(cid, 2, 'cargo test -p mooring-app tab_title', 'Process exited with code 0\ntest result: ok. 4 passed; 0 failed', stamp - 1260)
            records += call(cid, 3, 'cargo test -p mooring-app', 'Process exited with code 101\ntest result: FAILED. 211 passed; 2 failed', stamp - 720)
            # The account's usage windows, so the status bar has a quota cell to draw.
            records.append((stamp - 700, 'event_msg', {'type': 'token_count', 'rate_limits': {
                'primary': {'used_percent': 42.0, 'window_minutes': 300, 'resets_at': now + 7200},
                'secondary': {'used_percent': 31.0, 'window_minutes': 10080, 'resets_at': now + 4 * 86400}}}))
        if n == 1:
            records += call(cid, 1, None, patch('crates/mooring-core/src/model.rs'), stamp - 400)
            records += call(cid, 2, 'cargo test -p mooring-app tab_title_survives_exit', 'Process exited with code 0\ntest result: ok. 1 passed; 0 failed', stamp - 300)
            records += call(cid, 3, 'cargo test -p mooring-app', 'Process exited with code 0\ntest result: ok. 213 passed; 0 failed', stamp - 200)
        group = n % 8
        if group == 0: records.append((stamp, 'event_msg', {'type': 'request_user_input', 'message': '标签名在退出后要不要保留？我倾向保留，等你确认。', 'questions': [{'question': '标签名在退出后要不要保留？', 'options': [{'label': '保留，下次打开还叫这个名字'}, {'label': '不保留，按目录重新起名'}]}]}))
        elif group == 1: records.append((stamp, 'event_msg', {'type': 'task_complete', 'last_agent_message': '已找到 tab_name 的来源。'}))
        elif group == 2: records.extend([(stamp, 'response_item', {'type': 'function_call', 'name': 'exec_command', 'call_id': 'test', 'arguments': json.dumps({'cmd': 'bun test'})}), (stamp, 'response_item', {'type': 'function_call_output', 'call_id': 'test', 'output': 'Process exited with code 0\n31 pass 0 fail'}), (stamp, 'event_msg', {'type': 'task_complete', 'last_agent_message': '已完成，请核对。'})])
        elif group != 4: records.append((stamp, 'event_msg', {'type': 'turn_aborted'}))
        (logs / (cid + '.jsonl')).write_text(''.join(json.dumps({'timestamp': at, 'type': t, 'payload': p}, ensure_ascii=False) + '\n' for at, t, p in records))
        if os.environ.get('MOORING_FIXTURE_UNLINKED'): os.utime(logs / (cid + '.jsonl'), (stamp, stamp))
    if os.environ.get('MOORING_FIXTURE_CODEX_CHILDREN'):
        # Real child logs, parsed by the host in both builds; no UI-injected list.
        parent = logs / 'visual-0.jsonl'
        with parent.open('a') as f:
            f.write(json.dumps({'timestamp': now - 200, 'type': 'event_msg', 'payload': {'type': 'task_started', 'turn_id': 'children'}}) + '\n')
        for n, (name, end) in enumerate([('review_small_prs', None), ('loom_ci', 'task_complete'), ('rui_quota_acceptance', 'turn_aborted')]):
            cid = f'visual-child-{n}'
            spawn = {'parent_thread_id': 'visual-0', 'depth': 1, 'agent_path': '/root/' + name}
            records = [(now - 180 + n * 10, 'session_meta', {'id': cid, 'cwd': str(root / 'projects/工作空间'), 'source': {'subagent': {'thread_spawn': spawn}}, 'agent_nickname': name}),
                       (now - 180 + n * 10, 'event_msg', {'type': 'task_started', 'turn_id': cid})]
            if end: records.append((now - 50 + n * 10, 'event_msg', {'type': end}))
            (logs / (cid + '.jsonl')).write_text(''.join(json.dumps({'timestamp': at, 'type': kind, 'payload': payload}) + '\n' for at, kind, payload in records))
    child_sources = None
    if os.environ.get('MOORING_FIXTURE_CHILD_ADAPTERS'):
        import visual_child_fixture
        child_sources = (visual_child_fixture.prepare_names(root, home, now)
                         if os.environ['MOORING_FIXTURE_CHILD_ADAPTERS'] == 'names'
                         else visual_child_fixture.prepare_kimi_gemini(root, home, now)
                         if os.environ['MOORING_FIXTURE_CHILD_ADAPTERS'] == 'kimi-gemini'
                         else visual_child_fixture.prepare_more(root, home, now)
                         if os.environ['MOORING_FIXTURE_CHILD_ADAPTERS'] == 'more'
                         else visual_child_fixture.prepare(root, home, BIN, now))
    env = {'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'HOME': str(home), 'CODEX_HOME': str(home / '.codex'), 'CLAUDE_CONFIG_DIR': str(home / '.claude'), 'LANG': 'en_US.UTF-8'}
    host = subprocess.Popen([str(BIN / 'mooring'), 'serve', '--runtime', str(root / 'runtime')], env=env, stdout=subprocess.DEVNULL, stderr=open(root / 'host.log', 'w'), start_new_session=True)
    deadline = time.monotonic() + 30
    while not (root / 'runtime/workbench.sock').exists():
        assert host.poll() is None and time.monotonic() < deadline; time.sleep(.05)
    for n, name in enumerate(names): edit(root, {'AddSpace': {'id': f's{n}', 'name': name, 'cwd': str(root / 'projects' / name)}})
    # A space whose folder is gone, as after deleting a worktree.
    edit(root, {'AddSpace': {'id': 's5', 'name': '新空间', 'cwd': str(root / 'projects/gone')}})
    for n, (title, description, criteria) in enumerate(TASKS):
        edit(root, {'AddTask': {'id': f'task{n}', 'space': 's0', 'title': title, 'description': description, 'acceptance': [{'id': f't{n}-c{i}', 'description': text, 'evidence': []} for i, text in enumerate(criteria)], 'agent': 'Codex'}})
    (root / 'codex').symlink_to('/bin/sh')
    for n in range(3):
        lines = ['› ' + titles[n], '', '• 已读取项目上下文', '• 修改标签命名', '', '此处为隔离视觉测试输出，未调用模型。']
        script = 'exec 3<' + shlex.quote(str(logs / f'visual-{n}.jsonl')) + '; printf "%s\\n" ' + ' '.join(shlex.quote(s) for s in lines) + '; while IFS= read -r line; do printf "%s\\n" "$line"; done'
        r = rpc(root, {'Launch': {'id': f'p{n}', 'space': 's0', 'argv': [str(root / 'codex'), '-c', script], 'cwd': str(root / 'projects/工作空间'), 'cols': 90, 'rows': 35}}); assert 'Error' not in r, r
    if child_sources:
        visual_child_fixture.launch(root, child_sources, rpc)
    start = ['/bin/zsh', '-f']
    if os.environ.get('MOORING_FIXTURE_UNBOUND_AGENT'):
        start = [str(root / 'codex'), '-c', 'printf "OpenAI Codex\\nWaiting for your first message\\n"; while IFS= read -r line; do :; done', '--no-daemon']
        # 'shared': a Codex on the shared daemon, the one that gets the unbound note.
        if os.environ['MOORING_FIXTURE_UNBOUND_AGENT'] == 'shared': start.pop()
    rpc(root, {'Launch': {'id': 'pstart', 'space': 's0', 'argv': start, 'cwd': str(root / 'projects/工作空间'), 'cols': 100, 'rows': 35}})
    def leaf(p): return {'Leaf': p}
    def split(a, b, axis): return {'Split': {'axis': axis, 'ratio': 5000, 'first': a, 'second': b}}
    for id, title, task, layout in [('feedback', '标签命名依旧有问题', 'task0', split(leaf('p0'), split(leaf('p1'), leaf('p2'), 'Down'), 'Right')), ('start', '健身', 'task1', leaf('pstart'))]:
        if child_sources and id == 'feedback':
            layout = split(leaf(child_sources[0][0]), leaf(child_sources[1][0]), 'Right')
            title, task = '子 Agent 对照', None
        edit(root, {'CreateTab': {'tab': {'id': id, 'space': 's0', 'title': title, 'default_task': task, 'closed': False, 'layout': layout}}})
    if os.environ.get('MOORING_FIXTURE_UNLINKED'):
        # The placeholder rows from the host's own recognition, not typed in here: a Claude that
        # believes it is a child session, one yet to say anything, a Codex on the shared daemon
        # (a title, no log), and a program reporting its own status. The child session is a small
        # compiled program: macOS hides the environment of Apple's own /bin/sh from the host, which
        # would then never see the variable. It starts from a script, so the shell before its
        # `exec` does not already read as a Claude.
        (root / 'claude').symlink_to('/bin/sh')
        (root / 'child').mkdir()
        (root / 'child/hold.c').write_text('#include <unistd.h>\nint main(void) { for (;;) pause(); }\n')
        subprocess.run(['cc', '-o', str(root / 'child/claude'), str(root / 'child/hold.c')], check=True)
        hold = 'while IFS= read -r line; do :; done'
        report = 'printf "\\033]7501;state=working:app=deploy:progress=30:msg=6YOo572y5Lit\\033\\\\"; ' + hold
        child = root / 'child/start.sh'
        child.write_text(f'export CLAUDE_CODE_CHILD_SESSION=1\nexec {shlex.quote(str(root / "child/claude"))}\n')
        terminals = [('punlinked-claude', ['/bin/sh', str(child)]),
                     ('punlinked-first', [str(root / 'claude'), '-c', hold]),
                     ('punlinked-codex', [str(root / 'codex'), '-c', 'printf "\\033]0;fixture\\007"; ' + hold]),
                     ('punlinked-program', ['/bin/sh', '-c', report])]
        for id, argv in terminals:
            r = rpc(root, {'Launch': {'id': id, 'space': 's0', 'argv': argv, 'cwd': str(root / 'projects/工作空间'), 'cols': 90, 'rows': 35}}); assert 'Error' not in r, r
            time.sleep(1)
        layout = split(split(leaf('punlinked-claude'), leaf('punlinked-first'), 'Down'), split(leaf('punlinked-codex'), leaf('punlinked-program'), 'Down'), 'Right')
        # A space with such a terminal and no session at all.
        r = rpc(root, {'Launch': {'id': 'punlinked-only', 'space': 's5', 'argv': [str(root / 'codex'), '-c', 'printf "\\033]0;fixture\\007"; ' + hold], 'cwd': str(root / 'projects/工作空间'), 'cols': 90, 'rows': 35}}); assert 'Error' not in r, r
        edit(root, {'CreateTab': {'tab': {'id': 'unlinked-only', 'space': 's5', 'title': '未关联', 'default_task': None, 'closed': False, 'layout': leaf('punlinked-only')}}})
        edit(root, {'CreateTab': {'tab': {'id': 'unlinked', 'space': 's0', 'title': '未关联', 'default_task': None, 'closed': False, 'layout': layout}}})
    while True:
        w = rpc(root, 'Workspace')['Workspace']
        if len(w['agent_sessions']) >= 40: break
        assert time.monotonic() < deadline, 'index timeout'; time.sleep(.1)
    for n in range(3):
        t = next(t for t in w['terminals'] if t['id'] == f'p{n}')
        r = rpc(root, {'Hook': {'runtime': 'Codex', 'pid': t['pid'], 'cli_session_id': f'visual-{n}', 'transcript': str(logs / f'visual-{n}.jsonl'), 'ended': False, 'event': None}}); assert 'Error' not in r, r
        session = next(s for s in rpc(root, 'Workspace')['Workspace']['agent_sessions'] if s['cli_session_id'] == f'visual-{n}')
        edit(root, {'AssignTask': {'id': session['id'], 'task': 'task0'}})
        if n == 0:
            edit(root, {'AddTaskNote': {'task': 'task0', 'session': session['id'], 'text': '查清了：标签名取自第一句话，恢复会话后第一句话变了。下一步改成只取一次。'}})
        if n == 1:
            edit(root, {'AddTaskNote': {'task': 'task0', 'session': session['id'], 'text': '改好并补了测试，恢复后名字不再变。要你定：已经改乱的旧标签要不要批量修？'}})
    if os.environ.get('MOORING_FIXTURE_BOARD'):
        # What a narrow column has to hold: a long agent name beside 「开始」, a title too long for two lines.
        edit(root, {'AddTask': {'id': 'long0', 'space': 's0', 'title': 'Eval 建立 agent的评估 质量、费用、效率 每个方面都需要单独打分并写回结果', 'description': '', 'acceptance': [], 'agent': 'Claude'}})
        edit(root, {'AddTask': {'id': 'long1', 'space': 's0', 'title': '周报 + cli 的消费统计等', 'description': '', 'acceptance': [{'id': f'long1-c{i}', 'description': '验收', 'evidence': []} for i in range(5)], 'agent': 'Claude'}})
        for project in (root / 'projects').iterdir():
            subprocess.run(['git', '-C', str(project), 'init', '-q', '-b', 'feat/consolidate-local-sessions-and-more'], check=True)
        # A turn with no end, held by a live terminal: only that reaches the running column.
        script = 'exec 3<' + shlex.quote(str(logs / 'visual-4.jsonl')) + '; while IFS= read -r line; do printf "%s\\n" "$line"; done'
        r = rpc(root, {'Launch': {'id': 'p4', 'space': 's0', 'argv': [str(root / 'codex'), '-c', script], 'cwd': str(root / 'projects/工作空间'), 'cols': 90, 'rows': 35}}); assert 'Error' not in r, r
        while True:
            t = next(t for t in rpc(root, 'Workspace')['Workspace']['terminals'] if t['id'] == 'p4')
            if t.get('pid'): break
            assert time.monotonic() < deadline + 30, 'terminal timeout'; time.sleep(.1)
        time.sleep(1)
        r = rpc(root, {'Hook': {'runtime': 'Codex', 'pid': t['pid'], 'cli_session_id': 'visual-4', 'transcript': str(logs / 'visual-4.jsonl'), 'ended': False, 'event': None}}); assert 'Error' not in r, r
        if os.environ.get('MOORING_FIXTURE_UNLINKED'):
            # 今天 and 更早 hold conversations whose terminal has ended: run each in one, stop it.
            closed = [3, 5, 6, 7, 8, 10, 11, 13, 14, 15]
            for n in closed:
                r = rpc(root, {'Launch': {'id': f'pc{n}', 'space': f's{n // 8}', 'argv': [str(root / 'codex'), '-c', 'while IFS= read -r line; do :; done'], 'cwd': str(root / 'projects' / names[n // 8]), 'cols': 90, 'rows': 35}}); assert 'Error' not in r, r
            time.sleep(1)
            for n in closed:
                t = next(t for t in rpc(root, 'Workspace')['Workspace']['terminals'] if t['id'] == f'pc{n}')
                r = rpc(root, {'Hook': {'runtime': 'Codex', 'pid': t['pid'], 'cli_session_id': f'visual-{n}', 'transcript': str(logs / f'visual-{n}.jsonl'), 'ended': False, 'event': None}}); assert 'Error' not in r, r
            time.sleep(1)
            for n in closed: rpc(root, {'Stop': {'id': f'pc{n}'}})
            time.sleep(1)
        for s in rpc(root, 'Workspace')['Workspace']['agent_sessions']:
            if s['cli_session_id'] in ('visual-9', 'visual-17', 'visual-25'):
                edit(root, {'SetOutcome': {'id': s['id'], 'outcome': 'Completed', 'seen': None}})
    state = {'root': str(root), 'home': str(home), 'host': host.pid}; STATE.write_text(json.dumps(state)); print(state)
else:
    s = json.loads(STATE.read_text()); root = pathlib.Path(s['root']); home = pathlib.Path(s['home'])
    if sys.argv[1] == 'capture':
        directory = pathlib.Path(sys.argv[2]).resolve(); directory.mkdir(parents=True, exist_ok=True)
        env = {'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'HOME': str(home), 'CODEX_HOME': str(home / '.codex'), 'CLAUDE_CONFIG_DIR': str(home / '.claude'), 'LANG': 'en_US.UTF-8', 'GPUI_PAUSE_IDLE_DISPLAY_LINK': '1', 'MOORING_PAGE_EVIDENCE': '1',
               'MOORING_PAGE_LIST': os.environ.get('MOORING_PAGE_LIST', 'TodoNew,TodoNewBare,TodoNewChecks,TodoDetail,TodoDetailAttach,TodoDetailFull,TodoDetailDone')}
        result = subprocess.run([str(BIN / 'mooring-app'), '--runtime', str(root / 'runtime'), '--visual-evidence', str(directory)], env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=120)
        (directory / 'native-render.log').write_bytes(result.stdout); assert result.returncode == 0, result.stdout.decode()[-4000:]
        print('native captures', sorted(p.name for p in directory.glob('*-native.png')))
    elif sys.argv[1] == 'readback':
        w = rpc(root, 'Workspace')['Workspace']
        print(json.dumps([t for t in w['tasks'] if t['id'] == 'task0'], ensure_ascii=False, indent=1)[:3000])
        print(json.dumps({k: v['evidence'] for k, v in w['content'].items()}, ensure_ascii=False)[:3000])
    elif sys.argv[1] == 'stop':
        # The capture run stops the host it used; then there is nothing left to stop.
        try:
            for t in rpc(root, 'Workspace')['Workspace']['terminals']: rpc(root, {'Stop': {'id': t['id']}})
            os.kill(s['host'], 15)
        except (ConnectionRefusedError, FileNotFoundError, ProcessLookupError):
            pass
