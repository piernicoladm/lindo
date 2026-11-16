#!/usr/bin/env python3
"""Repeat filesystem safety and cancellation tests with the official Swift toolchain."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time


DEFAULT_FILTER = "|".join(
    (
        "scannerCancellation",
        "duplicateCancellation",
        "cancelledReader",
        "sequentialReads",
        "supersededBlockedScan",
        "concurrentTrash",
        "queuedScanChildren",
        "queuedApplicationChildren",
        "folderReaderCancellation",
        "folderReaderRejectsReplacement",
        "duplicateTrash",
        "duplicateSelection",
        "duplicateRescan",
        "duplicateKeeperLoss",
        "duplicateBatchStops",
        "coldSafety",
    )
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--timeout", type=float, default=300, help="Seconds per iteration, including the first build")
    parser.add_argument("--scratch-path", type=Path, default=Path.home() / "Library/Developer/Lindo/stability-build")
    parser.add_argument("--output", type=Path, help="Directory for the JSON report and per-iteration logs")
    parser.add_argument("--filter", default=DEFAULT_FILTER, help="Swift test name regular expression")
    args = parser.parse_args()
    if args.iterations < 1 or args.timeout <= 0:
        parser.error("--iterations and --timeout must be positive")

    package = Path(__file__).resolve().parent.parent
    scratch = args.scratch_path.expanduser().resolve()
    if scratch == package or package in scratch.parents:
        parser.error("Build products must stay outside the source repository")
    try:
        version_run = subprocess.run(["swift", "--version"], capture_output=True, text=True, check=True, timeout=20)
    except (OSError, subprocess.SubprocessError) as error:
        print(f"Cannot inspect the Swift toolchain: {error}", file=sys.stderr)
        return 2
    version = version_run.stdout.strip()
    match = re.search(r"Swift version (\d+)\.(\d+)", version)
    if not match or tuple(map(int, match.groups())) < (6, 2):
        print(f"Swift 6.2 or newer is required. Found: {version}", file=sys.stderr)
        return 2

    output = args.output.expanduser().resolve() if args.output else Path(tempfile.mkdtemp(prefix="lindo-stability-"))
    output.mkdir(parents=True, exist_ok=True)
    scratch.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    # This driver never enables OS Trash or scale fixtures, even when inherited.
    environment["LINDO_REAL_TRASH_TEST"] = "0"
    environment["LINDO_SCALE_TEST"] = "0"
    command = ["swift", "test", "--disable-xctest", "--package-path", str(package),
               "--scratch-path", str(scratch), "--filter", args.filter]
    frameworks = Path("/Library/Developer/CommandLineTools/Library/Developer/Frameworks")
    libraries = Path("/Library/Developer/CommandLineTools/Library/Developer/usr/lib")
    if (frameworks / "Testing.framework").exists():
        command += ["-Xswiftc", "-F", "-Xswiftc", str(frameworks),
                    "-Xlinker", "-F", "-Xlinker", str(frameworks),
                    "-Xlinker", "-rpath", "-Xlinker", str(frameworks),
                    "-Xlinker", "-rpath", "-Xlinker", str(libraries)]
    report = {
        "started_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "toolchain": version,
        "iterations_requested": args.iterations,
        "filter": args.filter,
        "real_trash_enabled": False,
        "scale_enabled": False,
        "command": command,
        "iterations": [],
        "passed": False,
    }
    started = time.monotonic()
    expected_count = None
    for index in range(1, args.iterations + 1):
        iteration_started = time.monotonic()
        log_path = output / f"iteration-{index:02d}.log"
        iteration_command = command + (["--skip-build"] if index > 1 else [])
        reason = None
        try:
            with log_path.open("w") as log:
                process = subprocess.Popen(iteration_command, stdout=log, stderr=subprocess.STDOUT,
                                           env=environment, start_new_session=True)
                try:
                    exit_code = process.wait(timeout=args.timeout)
                except subprocess.TimeoutExpired:
                    # Stop compiler/test children as well as the Swift driver.
                    try:
                        os.killpg(process.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        try:
                            os.killpg(process.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        process.wait()
                    exit_code, reason = 124, "Iteration timed out"
        except OSError as error:
            exit_code, reason = 2, str(error)
        text = log_path.read_text(errors="replace")
        counts = re.findall(r"Test run with (\d+) tests?[^\n]* passed", text)
        count = int(counts[-1]) if counts else 0
        if exit_code != 0:
            reason = reason or f"Swift test exited with status {exit_code}"
        elif count == 0:
            reason = "No successful nonempty Swift Testing run was reported"
        elif expected_count is not None and count != expected_count:
            reason = f"Test count changed from {expected_count} to {count}"
        if expected_count is None and count > 0:
            expected_count = count
        duration = time.monotonic() - iteration_started
        report["iterations"].append({"iteration": index, "passed": reason is None, "tests": count,
                                     "duration_seconds": duration, "exit_code": exit_code,
                                     "log": str(log_path), "failure": reason})
        report["duration_seconds"] = time.monotonic() - started
        report["test_executions"] = sum(item["tests"] for item in report["iterations"])
        report["passed"] = reason is None and index == args.iterations
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(f"{'PASS' if reason is None else 'FAIL'} {index}/{args.iterations}: {count} tests, {duration:.2f}s", flush=True)
        if reason:
            print(f"{reason}. See {log_path}", file=sys.stderr)
            return 1
    print(f"Passed {args.iterations} iterations, {report['test_executions']} test executions in {report['duration_seconds']:.2f}s")
    print(f"Report: {output / 'report.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
