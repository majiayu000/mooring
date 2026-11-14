#!/bin/bash
# A window attached to another machine's host (`fibo-app --attach <host>`), end to end
# without a second Mac: a second host here with its own HOME plays that machine, reached
# through a fake ssh whose `-L` is a link to its socket. The window asks it where its host
# listens (`fibo socket`), keeps the forward, works it with ⌘T / ⌘D / ⇧⌘D / ⌘W and an
# Agent, survives a dropped link, and closes what it opened.
# Needs a debug build with visual tests:
#   cargo build -p fibo-host -p fibo-app --features fibo-app/visual-test
# Prints PASS and what the other machine holds afterwards; saves shots to $R/shots.
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
R=${MOORING_EVIDENCE_TMP:-/private/tmp/mooring-at}
pkill -f "$R/far" 2>/dev/null
rm -rf "$R"; mkdir -p "$R/home/.config/fibo" "$R/bin" "$R/shots" "$R/farhome/code"
mkdir -m 700 "$R/far"
cat > "$R/bin/ssh" <<SH
#!/bin/bash
# Fake ssh: drops options and the host. \`-N -L here:there\` links here to there and stays
# up like a forward; anything else runs on "the other machine": HOME farhome, bash its shell.
forward=""
while [ \$# -gt 0 ]; do
  case "\$1" in
    -L) forward="\$2"; shift 2 ;;
    -o) shift 2 ;;
    -*) shift ;;
    *) shift; break ;;
  esac
done
if [ -n "\$forward" ]; then
  ln -sf "\${forward#*:}" "\${forward%%:*}"
  exec -a "fake-forward-$R" sleep 86400
fi
HOME=$R/farhome SHELL=/bin/bash exec /bin/sh -c "\$*"
SH
chmod +x "$R/bin/ssh"
printf '#!/bin/sh\nMOORING_RUNTIME=%s/far exec %s/target/debug/fibo "$@"\n' "$R" "$REPO" > "$R/far-fibo"
chmod +x "$R/far-fibo"
printf '[{"host":"far","path":"%s/far-fibo"}]' "$R" > "$R/home/.config/fibo/machines.json"
env -i PATH=/usr/bin:/bin HOME="$R/farhome" LANG=en_US.UTF-8 \
  "$REPO/target/debug/fibo" serve --runtime "$R/far" > "$R/far.log" 2>&1 &
FAR=$!
rpc() {
  python3 -I - "$R/far/workbench.sock" "$1" <<'PY'
import json, socket, struct, sys, time
def read(s, n):
    b = b''
    while len(b) < n:
        c = s.recv(n - len(b)); assert c; b += c
    return b
def send(s, o):
    b = b'\1' + json.dumps(o).encode(); s.sendall(struct.pack('>I', len(b)) + b)
def recv(s): return json.loads(read(s, struct.unpack('>I', read(s, 4))[0])[1:])
for _ in range(100):
    try:
        with socket.socket(socket.AF_UNIX) as s:
            s.settimeout(10); s.connect(sys.argv[1]); send(s, {'Hello': {'major': 3}}); recv(s)
            send(s, json.loads(sys.argv[2])); print(json.dumps(recv(s)))
        break
    except (FileNotFoundError, ConnectionRefusedError):
        time.sleep(0.1)
PY
}
rpc "{\"Edit\":{\"edit\":{\"AddSpace\":{\"id\":\"far\",\"name\":\"far\",\"cwd\":\"$R/farhome/code\"}}}}" > /dev/null
PATH="$R/bin:$PATH" HOME="$R/home" MOORING_ATTACH_EVIDENCE=1 \
  perl -e 'alarm 180; exec @ARGV' "$REPO/target/debug/fibo-app" --attach far --visual-evidence "$R/shots" > "$R/app.log" 2>&1
echo "app exit $?"
grep -E "^attach|panicked" -A2 "$R/app.log" | head
rpc '"Workspace"' | python3 -I -c '
import json, sys
w = json.load(sys.stdin)["Workspace"]
print("far tabs left:", [t["id"] for t in w["tabs"]])
print("far terminals:", sorted({t["state"] for t in w["terminals"]}))
shells = sorted({t["argv"][0] for t in w["terminals"]})
print("far shells:", shells, "(its own: /bin/bash)" if shells == ["/bin/bash"] else "WRONG: not its own shell")'
pgrep -f "fake-forward-$R" >/dev/null && echo "a fake forward outlived the window" || echo "forward ended with the window"
pkill -f "fake-forward-$R" 2>/dev/null
kill $FAR 2>/dev/null; sleep 1; pkill -f "$R/far" 2>/dev/null
ls "$R/shots"
