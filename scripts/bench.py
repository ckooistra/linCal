#!/usr/bin/env python3
"""Run the Universal Calendar UI benchmark against the release build.

Phases:
  1. Scripted UI benchmark inside the real Tauri/WebKitGTK window
     (UC_BENCH=1; see apps/desktop/src/bench/runner.ts).
  2. Idle measurement: launch normally, let it settle, then sample CPU time
     and RSS of the app and its WebKit child processes.

Results and machine metadata are written as JSON (default
docs/benchmarks/<timestamp>-<label>.json) and summarised on stdout. The host
name is deliberately not recorded; pass --label to describe the machine.

Without a display (headless VM/CI) the script re-runs itself under Xvfb with
a private D-Bus session; numbers are then software-rendered and pessimistic.

Usage: scripts/bench.py [--sizes 100,1000,5000] [--idle-seconds 30] [--label NAME] [--out FILE]
Only the Python standard library is used.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "release" / "universal-calendar-desktop"
CLK_TCK = os.sysconf("SC_CLK_TCK")


def process_tree(root: int) -> list[int]:
    parents: dict[int, int] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text()
        except OSError:
            continue
        fields = stat[stat.rfind(")") + 1 :].split()
        parents[int(entry.name)] = int(fields[1])
    tree, frontier = [root], [root]
    while frontier:
        pid = frontier.pop()
        for child, parent in parents.items():
            if parent == pid:
                tree.append(child)
                frontier.append(child)
    return tree


def cpu_ticks(pid: int) -> int:
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
    except OSError:
        return 0
    fields = stat[stat.rfind(")") + 1 :].split()
    return int(fields[11]) + int(fields[12])  # utime + stime


def rss_kb(pid: int) -> int:
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    except OSError:
        pass
    return 0


def command_output(cmd: list[str]) -> str:
    try:
        return subprocess.run(cmd, capture_output=True, text=True, check=False).stdout.strip()
    except OSError:
        return ""


def machine_info(label: str) -> dict:
    cpu = ""
    for line in command_output(["lscpu"]).splitlines():
        if line.startswith("Model name:"):
            cpu = line.split(":", 1)[1].strip()
    mem_kb = 0
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemTotal:"):
            mem_kb = int(line.split()[1])
    return {
        "label": label,
        "kernel": platform.release(),
        "os": command_output(["sh", "-c", ". /etc/os-release && echo $PRETTY_NAME"]),
        "cpu": cpu,
        "logicalCpus": os.cpu_count(),
        "memoryGiB": round(mem_kb / 1048576, 1),
        "webkit2gtk": command_output(["pkg-config", "--modversion", "webkit2gtk-4.1"]),
        "display": "Xvfb (software rendering)" if os.environ.get("UC_BENCH_XVFB") else
        os.environ.get("XDG_SESSION_TYPE", "x11") + " " + os.environ.get("XDG_CURRENT_DESKTOP", ""),
    }


def run_ui_benchmark(sizes: str, timeout: int) -> dict:
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "report.json"
        env = dict(os.environ, UC_BENCH="1", UC_BENCH_SIZES=sizes, UC_BENCH_OUT=str(out))
        proc = subprocess.Popen([str(BIN)], env=env, stderr=subprocess.DEVNULL)
        try:
            proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            proc.kill()
            raise SystemExit(f"benchmark did not finish within {timeout}s")
        if not out.exists():
            raise SystemExit("benchmark exited without writing a report")
        return json.loads(out.read_text())


def run_idle(settle: float, seconds: float) -> dict:
    proc = subprocess.Popen([str(BIN)], stderr=subprocess.DEVNULL)
    try:
        time.sleep(settle)
        tree = process_tree(proc.pid)
        before = sum(cpu_ticks(p) for p in tree)
        start = time.monotonic()
        time.sleep(seconds)
        elapsed = time.monotonic() - start
        tree = process_tree(proc.pid)
        after = sum(cpu_ticks(p) for p in tree)
        cpu_seconds = (after - before) / CLK_TCK
        return {
            "settleSeconds": settle,
            "sampleSeconds": round(elapsed, 2),
            "processes": len(tree),
            "cpuSeconds": round(cpu_seconds, 3),
            # Percent of one core, averaged over the sample.
            "cpuPercentOfOneCore": round(100 * cpu_seconds / elapsed, 2),
            "rssKb": {str(p): rss_kb(p) for p in tree},
            "totalRssKb": sum(rss_kb(p) for p in tree),
        }
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()


def summarize(result: dict) -> str:
    lines = []
    for scenario in result["ui"]["report"]["results"]:
        lines.append(f"\n## {scenario['size']} events")
        lines.append("| interaction | n | p50 ms | p95 ms | max ms | JS p50 ms | JS+layout p50 ms |")
        lines.append("|---|---:|---:|---:|---:|---:|---:|")
        for s in scenario["interactions"]:
            lines.append(
                f"| {s['name']} | {s['count']} | {s['p50']:.0f} | {s['p95']:.0f} | {s['max']:.0f} | {s['scriptP50']:.0f} | {s.get('layoutP50', 0):.0f} |"
            )
        d = scenario["dragProxy"]
        lines.append(
            f"\ndrag proxy: {d['fps']:.1f} fps, p95 frame {d['p95FrameMs']:.1f} ms, "
            f"slow frames {d['slowFrames']}/{d['frames']}"
        )
        if scenario.get("domNodes"):
            lines.append(f"DOM elements: {scenario['domNodes']}")
        q = scenario["queryMs"]
        lines.append(f"range query p50 {q['p50']:.3f} ms, p95 {q['p95']:.3f} ms")
        if scenario.get("memoryAfter"):
            lines.append(f"memory after: {scenario['memoryAfter']['totalKb'] / 1024:.0f} MiB total")
    idle = result["idle"]
    lines.append(
        f"\nidle: {idle['cpuPercentOfOneCore']}% of one core over {idle['sampleSeconds']} s, "
        f"RSS {idle['totalRssKb'] / 1024:.0f} MiB across {idle['processes']} processes"
    )
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--sizes", default="100,1000,5000")
    parser.add_argument("--idle-seconds", type=float, default=30)
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("--label", default="local", help="machine description, e.g. plasma-laptop")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()

    if not BIN.exists():
        raise SystemExit("Release binary missing. Build first: cd apps/desktop && npx tauri build")

    if not os.environ.get("DISPLAY") and not os.environ.get("WAYLAND_DISPLAY"):
        for tool in ("xvfb-run", "dbus-run-session"):
            if not shutil.which(tool):
                raise SystemExit(f"No display and {tool} is not installed")
        env = dict(os.environ, UC_BENCH_XVFB="1", WEBKIT_DISABLE_DMABUF_RENDERER="1", LIBGL_ALWAYS_SOFTWARE="1")
        cmd = ["xvfb-run", "-a", "-s", "-screen 0 1440x900x24", "dbus-run-session", "--",
               sys.executable, *sys.argv]
        raise SystemExit(subprocess.call(cmd, env=env))

    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    out = args.out or ROOT / "docs" / "benchmarks" / f"{stamp}-{args.label}.json"
    print("Running UI benchmark…", file=sys.stderr)
    ui = run_ui_benchmark(args.sizes, args.timeout)
    print("Measuring idle CPU…", file=sys.stderr)
    idle = run_idle(settle=8, seconds=args.idle_seconds)
    result = {"machine": machine_info(args.label), "ui": ui, "idle": idle}
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, indent=2))
    print(summarize(result))
    print(f"\nSaved {out}")


if __name__ == "__main__":
    main()
