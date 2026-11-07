"""Isolated child sources; never calls a model or reads user data."""
import json
import shlex
import sqlite3
import time

PARENT = 'c19a5748-897d-4836-a555-fce8a7797222'
CHILDREN = ['19817791-63b7-441c-ad06-25fe5e5293ba', '43c5c185-4a65-45bb-a8d7-fc45cb8a4907', 'f6d889ad-8931-4baf-8476-a000f368f3b3']
TASKS = ['检查事件读取', '核对索引更新', '验证侧栏状态']


def prepare(root, home, binary_dir, now):
    cwd = root / 'projects/工作空间'
    database = home / '.local/share/opencode/opencode.db'
    database.parent.mkdir(parents=True)
    db = sqlite3.connect(database)
    db.executescript('CREATE TABLE session(id TEXT PRIMARY KEY,parent_id TEXT,directory TEXT,title TEXT,time_updated INTEGER);'
                     'CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_updated INTEGER,data TEXT);'
                     'CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_updated INTEGER,data TEXT);')
    for n, cid in enumerate([PARENT] + CHILDREN):
        at = now - 200 + n * 10
        title = 'OpenCode 并行任务' if n == 0 else TASKS[n - 1]
        db.execute('INSERT INTO session VALUES(?,?,?,?,?)', (cid, None if n == 0 else PARENT, str(cwd), title, at * 1000))
        db.execute('INSERT INTO message VALUES(?,?,?,?)', ('ask-' + cid, cid, at * 1000, json.dumps({'role': 'user'})))
        db.execute('INSERT INTO part VALUES(?,?,?,?,?)', ('ask-' + cid, 'ask-' + cid, cid, at * 1000, json.dumps({'type': 'text', 'text': title})))
        if n >= 2:
            end = now - 60 + n * 10
            data = {'role': 'assistant'}
            if n == 3:
                data['error'] = {'name': 'MessageAbortedError'}
            db.execute('INSERT INTO message VALUES(?,?,?,?)', ('end-' + cid, cid, end * 1000, json.dumps(data)))
            if n == 2:
                db.execute('INSERT INTO part VALUES(?,?,?,?,?)', ('end-' + cid, 'end-' + cid, cid, end * 1000, json.dumps({'type': 'step-finish', 'reason': 'stop'})))
            db.execute('UPDATE session SET time_updated=? WHERE id=?', (end * 1000, cid))
    db.commit()
    db.close()
    # Each build uses its own declaration, against the same source database shapes.
    declaration = binary_dir.parent.parent / 'plugins/opencode/agent.json'
    config = home / '.config/mooring/agents'
    config.mkdir(parents=True, exist_ok=True)
    (config / 'opencode.json').write_bytes(declaration.read_bytes())
    chat = home / f'.cursor/chats/hash/{PARENT}'
    chat.mkdir(parents=True)
    (chat / 'meta.json').write_text(json.dumps({'cwd': str(cwd)}))
    (chat / 'prompt_history.json').write_text('[]')
    logs = home / f'.cursor/projects/fixture/agent-transcripts/{PARENT}'
    (logs / 'subagents').mkdir(parents=True)
    for n, cid in enumerate([PARENT] + CHILDREN):
        title = 'Cursor 并行任务' if n == 0 else TASKS[n - 1]
        records = [{'timestamp': now - 200 + n * 10, 'role': 'user', 'message': {'content': [{'type': 'text', 'text': title}]}}]
        if n >= 2:
            records.append({'timestamp': now - 60 + n * 10, 'type': 'turn_ended', 'status': 'success' if n == 2 else 'aborted'})
        path = logs / ('' if n == 0 else 'subagents') / (cid + '.jsonl')
        path.write_text(''.join(json.dumps(r, ensure_ascii=False) + '\n' for r in records))
    return [('poc', 'opencode', 'opencode', home / f'.local/share/mooring/sources/opencode/{PARENT}.jsonl'),
            ('pcursor', 'cursor-agent', 'Cursor', logs / (PARENT + '.jsonl'))]


def launch(root, sources, rpc):
    deadline = time.monotonic() + 30
    for pane, executable, runtime, transcript in sources:
        cli = root / executable
        cli.symlink_to('/bin/sh')
        while not transcript.exists():
            assert time.monotonic() < deadline, f'missing mirror {transcript}'
            time.sleep(.1)
        script = f'exec 3<{shlex.quote(str(transcript))}; printf "%s\\n" "隔离子 Agent 验收 · 未调用模型"; while IFS= read -r line; do :; done'
        r = rpc(root, {'Launch': {'id': pane, 'space': 's0', 'argv': [str(cli), '-c', script], 'cwd': str(root / 'projects/工作空间'), 'cols': 80, 'rows': 30}})
        assert 'Error' not in r, r
        while True:
            w = rpc(root, 'Workspace')['Workspace']
            t = next(t for t in w['terminals'] if t['id'] == pane)
            if t.get('pid'): break
            assert time.monotonic() < deadline, 'terminal timeout'
            time.sleep(.1)
        r = rpc(root, {'Hook': {'runtime': runtime, 'pid': t['pid'], 'cli_session_id': ('session_' + PARENT if runtime == 'Kimi' else PARENT), 'transcript': str(transcript), 'ended': False, 'event': None}})
        # The comparison's old binary has no adapter for either new CLI. It still
        # launches the identical mock terminal, but cannot bind its transcript.
        assert 'Error' not in r or (runtime in ('Kimi', 'Gemini') and 'transcript has no CLI identity' in str(r)), r


def prepare_more(root, home, now):
    from datetime import datetime, timezone
    from urllib.parse import quote
    stamp = lambda at: datetime.fromtimestamp(at, timezone.utc).isoformat()
    cwd = root / 'projects/工作空间'
    qp = home / f'.qoder/projects/fixture/{PARENT}.jsonl'
    gp = home / '.grok/sessions' / quote(str(cwd), safe='') / PARENT / 'updates.jsonl'
    qp.parent.mkdir(parents=True)
    gp.parent.mkdir(parents=True)
    def qrecord(at, role, text, child=False):
        return {'type': role, 'sessionId': PARENT, 'cwd': str(cwd), 'timestamp': stamp(at),
                'entrypoint': 'cli', 'userType': 'external', 'isSidechain': child,
                'message': {'role': role, 'content': text, 'stop_reason': 'end_turn' if role == 'assistant' else None}}
    qp.write_text(json.dumps(qrecord(now - 200, 'user', 'Qoder 并行任务'), ensure_ascii=False) + '\n')
    gp.write_text(json.dumps({'timestamp': now - 200, 'method': 'session/update', 'params': {'sessionId': PARENT,
                  'update': {'sessionUpdate': 'user_message_chunk', 'content': {'type': 'text', 'text': 'Grok 并行任务'}}}}, ensure_ascii=False) + '\n')
    gp.with_name('summary.json').write_text(json.dumps({'session_summary': 'Grok 并行任务'}, ensure_ascii=False))
    for n, cid in enumerate(CHILDREN):
        start, end = now - 180 + n * 10, now - 50 + n * 10
        qchild = qp.with_suffix('') / 'subagents' / f'agent-{cid}.jsonl'
        qchild.parent.mkdir(parents=True, exist_ok=True)
        records = [qrecord(start, 'user', TASKS[n], True)]
        if n == 1: records.append(qrecord(end, 'assistant', '已核对', True))
        if n == 2: records.append(qrecord(end, 'user', '[Request interrupted by user]', True))
        qchild.write_text(''.join(json.dumps(r, ensure_ascii=False) + '\n' for r in records))
        qchild.with_suffix('.meta.json').write_text(json.dumps({'description': TASKS[n], 'agentType': 'general-purpose'}, ensure_ascii=False))
        gmeta = gp.parent / 'subagents' / cid / 'meta.json'
        gmeta.parent.mkdir(parents=True)
        gmeta.write_text(json.dumps({'subagent_id': cid, 'child_session_id': cid, 'parent_session_id': PARENT,
                         'description': TASKS[n], 'subagent_type': 'explore', 'started_at': stamp(start),
                         'completed_at': None if n == 0 else stamp(end),
                         'status': ['running', 'completed', 'cancelled'][n]}, ensure_ascii=False))
    return [('pqoder', 'qodercli', 'Qoder', qp), ('pgrok', 'grok', 'Grok', gp)]


def prepare_kimi_gemini(root, home, now):
    from datetime import datetime, timezone
    stamp = lambda at: datetime.fromtimestamp(at, timezone.utc).isoformat()
    cwd = str(root / 'projects/工作空间')
    session = home / f'.kimi-code/sessions/wd_fixture/session_{PARENT}'
    session.mkdir(parents=True)
    (session / 'state.json').write_text(json.dumps({'workDir': cwd, 'title': 'Kimi 并行任务',
        'agents': {'main': {'type': 'main', 'parentAgentId': None},
                   **{f'agent-{n}': {'type': 'sub', 'parentAgentId': 'main'} for n in range(3)}}}, ensure_ascii=False))
    project = home / '.gemini/tmp/fixture'
    project.mkdir(parents=True)
    (project / '.project_root').write_text(cwd)
    gp = project / f'chats/session-2026-11-07T09-00-{PARENT[:8]}.jsonl'
    kp = session / 'agents/main/wire.jsonl'
    for n in range(4):
        start, end = now - 200 + n * 10, now - 50 + n * 10
        kpath = kp if n == 0 else session / f'agents/agent-{n - 1}/wire.jsonl'
        gpath = gp if n == 0 else project / 'chats' / PARENT / (CHILDREN[n - 1] + '.jsonl')
        krecords = [{'type': 'config.update', 'cwd': cwd, 'time': start * 1000},
                    {'type': 'turn.prompt', 'input': [{'type': 'text', 'text': 'Kimi 并行任务' if n == 0 else TASKS[n - 1]}], 'time': start * 1000}]
        grecords = [{'sessionId': PARENT if n == 0 else CHILDREN[n - 1], 'projectHash': 'fixture', 'kind': 'main' if n == 0 else 'subagent'},
                    {'type': 'user', 'id': f'ask-{n}', 'content': 'Gemini 并行任务' if n == 0 else TASKS[n - 1], 'timestamp': stamp(start)}]
        if n >= 2:
            krecords.append({'type': 'turn.ended', 'reason': 'completed' if n == 2 else 'cancelled', 'time': end * 1000})
            grecords.append({'type': 'gemini', 'id': 'finish', 'content': '', 'timestamp': stamp(end),
                             'toolCalls': [{'id': 'finish-call', 'name': 'complete_task', 'status': 'success', 'timestamp': stamp(end)}]}
                            if n == 2 else {'type': 'error', 'id': 'error', 'content': 'Agent failed', 'timestamp': stamp(end)})
        for path, records in [(kpath, krecords), (gpath, grecords)]:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(''.join(json.dumps(r, ensure_ascii=False) + '\n' for r in records))
    return [('pkimi', 'kimi', 'Kimi', kp), ('pgemini', 'gemini', 'Gemini', gp)]
