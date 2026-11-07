#!/usr/bin/env python3
"""Focused regressions + account-free probes of installed CLIs. No installation or model calls.

Usage: python3 scripts/check-agent-clis.py /absolute/path/to/evidence
Ordinary Rust tests also run in CI. Local CLI absence is reported as unavailable,
never passed. Help checks establish the command surface, not an online task outcome.
"""
import datetime
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile


def probe(command, env, cwd, log):
    with log.open("w") as stream:
        process = subprocess.Popen(command, cwd=cwd, env=env, stdout=stream,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            return {"status": "failed", "reason": "20 second timeout", "log": log.name}
    output = log.read_text(errors="replace")
    if code != 0:
        return {"status": "failed", "reason": f"exit {code}", "log": log.name}
    if not output.strip():
        return {"status": "failed", "reason": "empty output", "log": log.name}
    return {"status": "passed", "log": log.name, "text": output}


def main():
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    repo = Path(__file__).resolve().parent.parent
    out = Path(sys.argv[1]).resolve()
    out.mkdir(parents=True, exist_ok=True)
    manifest = out / "commands.json"
    # Avoid reading a stale manifest if compilation or the exporter fails.
    manifest.unlink(missing_ok=True)
    env = os.environ.copy()
    env["MOORING_CLI_MANIFEST"] = str(manifest)
    suites = [
        ["cargo", "test", "-p", "mooring-host", "--lib"],
        ["cargo", "test", "-p", "mooring-host", "--test", "agent_cli_contracts",
         "--test", "subagent_adapters", "--test", "more_subagent_adapters",
         "--test", "kimi_gemini_adapters", "--test", "more_agent_adapters",
         "--test", "architecture_boundary"],
        ["cargo", "test", "-p", "mooring-core", "--lib", "agent::tests"],
        ["cargo", "test", "-p", "mooring-core", "--test", "guard", "--test", "file_size"],
        ["cargo", "test", "-p", "mooring-remote"],
        ["bun", "test"],
        ["bun", "run", "typecheck"],
    ]
    report = {"time_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip(),
              "dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=repo, text=True).strip()),
              "automated": [], "installed": [], "online_tasks": "not_run",
              "native_window": "not_run"}
    failed = False
    for n, command in enumerate(suites):
        print("Running " + " ".join(command), flush=True)
        log = out / f"suite-{n + 1}.log"
        with log.open("w") as stream:
            result = subprocess.run(command, cwd=repo / "mobile" if command[0] == "bun" else repo,
                                    env=env, stdout=stream, stderr=subprocess.STDOUT)
        status = "passed" if result.returncode == 0 else "failed"
        report["automated"].append({"command": command, "status": status, "log": log.name})
        print(f"  {status}: {log}", flush=True)
        failed |= result.returncode != 0
    if not manifest.exists():
        report["probe_error"] = "No command manifest exported; installed CLI probes not run"
        failed = True
    else:
        for spec in json.loads(manifest.read_text()):
            exe = shutil.which(spec["executable"])
            item = {"agent": spec["key"], "executable": exe}
            if not exe:
                item["status"] = "unavailable"
                report["installed"].append(item)
                print(f"{spec['key']}: unavailable (not passed)", flush=True)
                continue
            checks = {}
            with tempfile.TemporaryDirectory(prefix="mooring-cli-help-") as temp:
                # No credentials, user configuration, or provider-specific home overrides.
                isolated = {"PATH": os.environ["PATH"], "HOME": temp, "TMPDIR": temp,
                            "TERM": "dumb", "NO_COLOR": "1", "CI": "true",
                            "NO_UPDATE_NOTIFIER": "1", "DISABLE_AUTOUPDATER": "1"}
                commands = {"version": [exe, "--version"],
                            "start_help": ["/bin/sh", "-c", spec["start"] + " --help"],
                            "resume_help": ["/bin/sh", "-c", spec["resume"] + " --help"]}
                for kind, command in commands.items():
                    check = probe(command, isolated, temp, out / f"{spec['key']}-{kind}.log")
                    text = check.pop("text", "")
                    if check["status"] == "passed":
                        if kind == "version":
                            item["version_output"] = text.strip()[:400]
                        elif not re.search(r"usage|options|commands", text, re.I):
                            check.update(status="failed", reason="no help surface in output")
                    checks[kind] = check
            item["checks"] = checks
            item["status"] = "passed" if all(c["status"] == "passed" for c in checks.values()) else "failed"
            failed |= item["status"] == "failed"
            report["installed"].append(item)
            print(f"{spec['key']}: {item['status']}", flush=True)
    report["unavailable"] = [a["agent"] for a in report["installed"] if a["status"] == "unavailable"]
    report["result"] = "failed" if failed else "partial" if report["unavailable"] else "offline_checks_passed"
    (out / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(f"Result: {report['result']}; online tasks and native window NOT tested. {out / 'report.json'}")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
