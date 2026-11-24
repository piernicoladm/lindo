#!/usr/bin/env python3
"""Measure the native window on synthetic files below an explicitly chosen parent."""
import argparse
from pathlib import Path
import subprocess
import tempfile


def run(binary, mode, root, entries, groups, rows, timeout):
    result = subprocess.run([binary, "--ui-probe", mode, root], capture_output=True,
                            timeout=timeout, text=True, check=True)
    line = next(s for s in result.stdout.splitlines() if s.startswith("LINDO_UI_QA"))
    print(line, flush=True)
    fields = dict(token.split("=", 1) for token in line.split() if "=" in token)
    if (fields.get("ok") != "true" or int(fields["visited"]) < entries
            or int(fields["groups"]) != groups or int(fields["items"]) != rows
            or int(fields["warmup_frames"]) < 8 or int(fields["opening_ms"]) < 300
            or int(fields["result_frames"]) < 4 or int(fields["longest_gap_ms"]) > 1000
            or max(int(fields[key]) for key in ("scanning_work_ms", "results_work_ms")) > 1000):
        raise SystemExit("Unexpected result count or native window stalled for more than one second")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("parent", type=Path, help="existing, local test directory under your home, outside Library")
    parser.add_argument("--files", type=int, default=120000)
    parser.add_argument("--copies", type=int, default=3600)
    parser.add_argument("--flat-files", type=int, default=15000)
    args = parser.parse_args()
    if args.files < 1 or args.flat_files < 1 or args.copies < 2:
        parser.error("At least one file and two copies are required")
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="lindo-ui-", dir=args.parent.resolve(strict=True)) as scratch:
        base = Path(scratch)
        space, duplicates = base / "space", base / "duplicates"
        space.mkdir()
        duplicates.mkdir()
        directories = (args.files + 999) // 1000
        for offset in range(args.files):
            folder = space / f"directory-{offset // 1000:04}"
            if offset % 1000 == 0:
                folder.mkdir()
            (folder / f"{offset % 1000:04}.dat").touch()
        flat = base / "flat-map"
        flat.mkdir()
        for index in range(args.flat_files):
            (flat / f"entry-{index:05}.dat").touch()
        for index in range(args.copies):
            (duplicates / f"copy-{index:06}.bin").write_bytes(b"Lindo synthetic duplicate fixture." * 40)
        run(binary, "space", space, args.files, 0, directories, 120)
        run(binary, "space", flat, args.flat_files, 0, args.flat_files, 120)
        run(binary, "duplicates", duplicates, args.copies, 1, args.copies, 120)

    menu = subprocess.run([binary, "--menu-probe"], capture_output=True, text=True, timeout=45)
    if menu.returncode or "LINDO_MENU_QA ok=true" not in menu.stdout:
        raise SystemExit(f"Menu lifecycle failed ({menu.returncode}): {menu.stdout} {menu.stderr}")
    print("LINDO_MENU_QA ok=true", flush=True)


if __name__ == "__main__":
    main()
