#!/usr/bin/env bash
# Throwaway probe: prints what really happens on Windows. It asserts nothing, so one surprise
# can't hide the next. Fake key only; this repository's logs are public.
set +e
ROOT=$(pwd)
BIN_U="$ROOT/target/debug/mooring.exe"
export MOORING_BIN=$(cygpath -w "$BIN_U")
NODE_DIR=$(dirname "$(command -v node)")
NPM_PREFIX=$(npm prefix -g)
SYS="/c/Windows/System32:/c/Windows:/usr/bin:/mingw64/bin"
WORK=$(cygpath -u "$RUNNER_TEMP")/quota
mkdir -p "$WORK"
say() { printf '\n=== %s\n' "$*"; }
show() { for f in "$@"; do if [ -e "$f" ]; then echo "--- $f"; cat "$f"; echo; else echo "--- $f (none)"; fi; done; }
usage_case() {
  name=$1; path=$2
  d="$WORK/$name"; rm -rf "$d"; mkdir -p "$d"
  say "usage: $name"
  PATH="$path" MOORING_OPTION_USAGE=1 MOORING_PLUGIN_DATA="$(cygpath -w "$d")" "$BIN_U" claude-quota usage
  echo "exit=$?"
  show "$d/usage.json" "$d/usage.failed"
}

say "where claude is, npm"
ls -la "$NPM_PREFIX" | grep -i claude
ls -la "$NPM_PREFIX/node_modules/@anthropic-ai/claude-code/" | head -20
PATH="$NPM_PREFIX:$NODE_DIR:$SYS" claude --version

say "raw /usage through npm's cmd, no login"
cmd //c "$(cygpath -w "$NPM_PREFIX/claude.cmd")" -p /usage --output-format json --tools "" --strict-mcp-config --setting-sources "" --settings '{"disableDeepLinkRegistration":"disable"}' --no-session-persistence
echo "exit=$?"

usage_case no-claude "$NODE_DIR:$SYS"
usage_case npm-real "$NPM_PREFIX:$NODE_DIR:$SYS"

# Only the .cmd, so the real installer's other files can't decide which one is run.
say "npm with only claude.cmd visible"
ONLY="$WORK/only-cmd"; mkdir -p "$ONLY"
printf '@echo off\r\ncall "%s" %%*\r\n' "$(cygpath -w "$NPM_PREFIX/claude.cmd")" > "$ONLY/claude.cmd"
usage_case npm-cmd-only "$ONLY:$NODE_DIR:$SYS"

say "fake claude.cmd: are the arguments intact, is the text read"
FAKE="$WORK/fake"; mkdir -p "$FAKE"
cat > "$FAKE/fake.js" <<'JS'
const fs = require('fs');
fs.writeFileSync(process.env.FAKE_ARGS, JSON.stringify(process.argv.slice(2)));
const result = "You are currently using your subscription to power your Claude Code usage\n\nCurrent session: 2% used · resets Nov 7 at 8:49pm (Asia/Shanghai)\nCurrent week (all models): 19% used · resets Nov 13 at 5:59pm (Asia/Shanghai)\nCurrent week (Fable): 0% used · resets Nov 13 at 6pm (Asia/Shanghai)\n";
process.stdout.write(JSON.stringify({ type: 'result', is_error: false, result, total_cost_usd: 0, num_turns: 0 }));
JS
printf '@echo off\r\nnode "%%~dp0fake.js" %%*\r\n' > "$FAKE/claude.cmd"
export FAKE_ARGS="$(cygpath -w "$WORK/args.json")"
usage_case fake-cmd "$FAKE:$NODE_DIR:$SYS"
show "$WORK/args.json"

say "the declared command, as Mooring runs it: sh -c"
d="$WORK/declared"; mkdir -p "$d"
PATH="$FAKE:$NODE_DIR:$SYS" MOORING_OPTION_USAGE=1 MOORING_PLUGIN_DATA="$(cygpath -w "$d")" sh -c '"$MOORING_BIN" claude-quota usage'
echo "exit=$?"

say "official installer"
powershell -NoProfile -Command "irm https://claude.ai/install.ps1 | iex" 2>&1 | tail -5
ls -la "$USERPROFILE/.local/bin" 2>&1 | head
NATIVE=$(cygpath -u "$USERPROFILE")/.local/bin
PATH="$NATIVE:$NODE_DIR:$SYS" claude --version
usage_case native-on-path "$NATIVE:$NODE_DIR:$SYS"
usage_case native-off-path "$NODE_DIR:$SYS"

say "setup with a status line already there"
CLAUDE_DIR=$(cygpath -u "$USERPROFILE")/.claude
mkdir -p "$CLAUDE_DIR"
MARKER="$WORK/original-ran"
cat > "$CLAUDE_DIR/settings.json" <<JSON
{"model":"keep-me","statusLine":{"type":"command","command":"cat > '$MARKER'; echo mine"}}
JSON
SL="$WORK/statusline"; mkdir -p "$SL"
MOORING_PLUGIN_DATA="$(cygpath -w "$SL")" "$BIN_U" claude-quota setup; echo "exit=$?"
show "$CLAUDE_DIR/settings.json"
ls -la "$SL"
COMMAND=$(node -e "console.log(JSON.parse(require('fs').readFileSync(process.argv[1])).statusLine.command)" "$(cygpath -w "$CLAUDE_DIR/settings.json")")
echo "command: $COMMAND"
PAYLOAD='{"session_id":"probe-1","context_window":{"used_percentage":3},"rate_limits":{"five_hour":{"used_percentage":7,"resets_at":1794050000}}}'

say "status line through bash"
rm -rf "$SL/statusline.json" "$MARKER"
printf '%s' "$PAYLOAD" | bash -c "$COMMAND"; echo "exit=$?"
show "$SL/statusline.json" "$MARKER"

say "status line through cmd"
rm -rf "$SL/statusline.json" "$MARKER"
printf '%s' "$PAYLOAD" > "$WORK/payload.json"
printf '@echo off\r\n%s < "%s"\r\n' "$COMMAND" "$(cygpath -w "$WORK/payload.json")" > "$WORK/run.cmd"
cmd //c "$(cygpath -w "$WORK/run.cmd")"; echo "exit=$?"
show "$SL/statusline.json"

say "status line through powershell"
rm -rf "$SL/statusline.json" "$MARKER"
printf '%s' "$PAYLOAD" | powershell -NoProfile -Command "$COMMAND"; echo "exit=$?"
show "$SL/statusline.json"

say "a real interactive Claude, fake key: does it call the status line"
rm -rf "$SL/statusline.json" "$MARKER" "$SL/contexts"
python -m pip install --quiet pywinpty 2>&1 | tail -1
cat > "$WORK/tui.py" <<'PY'
import os, sys, time
from winpty import PtyProcess
env = dict(os.environ); env["ANTHROPIC_API_KEY"] = "sk-ant-api03-" + "x" * 80
exe = sys.argv[1]; marker = sys.argv[2]
p = PtyProcess.spawn([exe], env=env, dimensions=(40, 120))
seen = ""; start = time.time(); pokes = 0
while time.time() - start < 90:
    try:
        seen += p.read(65536)
    except Exception:
        pass
    if os.path.exists(marker):
        print("status line ran after %.0f s" % (time.time() - start)); break
    # Past whatever first-run question is on screen.
    if time.time() - start > 15 * (pokes + 1) and pokes < 4:
        p.write("\r"); pokes += 1
print("screen tail:", ascii(seen[-1500:]))
p.terminate(force=True)
PY
python "$WORK/tui.py" "$(cygpath -w "$NATIVE/claude.exe")" "$(cygpath -w "$MARKER")"
show "$MARKER"
ls -la "$SL" "$SL/contexts" 2>&1

say "teardown gives it back"
MOORING_PLUGIN_DATA="$(cygpath -w "$SL")" "$BIN_U" claude-quota teardown; echo "exit=$?"
show "$CLAUDE_DIR/settings.json"

say "the real path: official catalog, install, enable, set"
rm -f "$CLAUDE_DIR/settings.json"
"$BIN_U" plugins available --remote 2>&1 | grep -i -E "claude-quota|error|错" ; echo "exit=$?"
"$BIN_U" plugins install claude-quota --remote --yes 2>&1; echo "exit=$?"
"$BIN_U" plugins 2>&1 | head -30; echo "exit=$?"
"$BIN_U" plugins enable claude-quota 2>&1; echo "exit=$?"
"$BIN_U" plugins set claude-quota 2>&1; echo "exit=$?"
"$BIN_U" plugins set claude-quota usage true 2>&1; echo "exit=$?"
"$BIN_U" plugins log claude-quota 2>&1; echo "exit=$?"
show "$CLAUDE_DIR/settings.json"
