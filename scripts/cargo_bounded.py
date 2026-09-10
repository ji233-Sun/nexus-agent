# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Run Cargo, then keep this workspace's debug incremental cache below 5 GiB."""

import argparse
from collections import Counter
from contextlib import ExitStack, contextmanager
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / "target" / "debug" / "incremental"
LIMIT = 5 * 1024**3
CACHE_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*-[a-z0-9]+")


@contextmanager
def cargo_lock(directory):
    # Cargo 1.98 takes the build lock before the legacy compatibility lock.
    with ExitStack() as locks:
        for name in (".cargo-build-lock", ".cargo-lock"):
            path = directory / name
            if path.is_symlink():
                raise ValueError(f"Refusing a symlinked Cargo lock: {path}")
            handle = locks.enter_context(path.open("a+b"))
            handle.seek(0)
            if os.name == "nt":
                import msvcrt

                while True:
                    try:
                        msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
                        break
                    except OSError as error:
                        if error.errno not in (13, 36):
                            raise
                        time.sleep(0.1)
            else:
                import fcntl

                fcntl.flock(handle, fcntl.LOCK_EX)
        yield


def inventory(path):
    """Measure allocated bytes once per inode without following symlinks."""
    sizes = {}
    newest = 0
    pending = [path]
    while pending:
        entry = pending.pop()
        info = entry.lstat()
        key = (info.st_dev, info.st_ino)
        sizes[key] = info.st_blocks * 512 if hasattr(info, "st_blocks") else info.st_size
        newest = max(newest, info.st_mtime_ns)
        if stat.S_ISDIR(info.st_mode):
            pending.extend(entry.iterdir())
    return sizes, newest


def prune(cache=CACHE, limit=LIMIT):
    for directory in (cache.parent.parent, cache.parent, cache):
        if directory.is_symlink():
            raise ValueError(f"Refusing a symlinked cache directory: {directory}")
    if not cache.exists():
        return 0, 0, 0

    with cargo_lock(cache.parent):
        sizes = {}
        references = Counter()
        candidates = []
        for entry in cache.iterdir():
            files, newest = inventory(entry)
            sizes.update(files)
            references.update(files.keys())
            if not entry.is_symlink() and entry.is_dir() and CACHE_NAME.fullmatch(entry.name):
                candidates.append((newest, entry, files.keys()))

        root_info = cache.stat()
        root_size = root_info.st_blocks * 512 if hasattr(root_info, "st_blocks") else root_info.st_size
        before = total = root_size + sum(sizes.values())
        removed = 0
        for _, entry, files in sorted(candidates):
            if total <= limit:
                break
            shutil.rmtree(entry)
            removed += 1
            for key in files:
                references[key] -= 1
                if references[key] == 0:
                    total -= sizes[key]

        after = sum(inventory(cache)[0].values())
        if after > limit:
            raise ValueError("Cache still exceeds its limit; unrecognized entries were preserved")
        return before, after, removed


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("build", "check", "test", "clippy", "run", "prune"))
    parser.add_argument("cargo_args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if args.command == "prune" and args.cargo_args:
        parser.error("prune does not accept Cargo arguments")

    code = 0
    if args.command != "prune":
        try:
            code = subprocess.run(["cargo", args.command, *args.cargo_args], cwd=ROOT).returncode
        except KeyboardInterrupt:
            code = 130
        except OSError as error:
            print(f"Cargo failed to start: {error}", file=sys.stderr)
            return 1
    if code < 0:
        code = 128 - code

    try:
        print("Waiting for Cargo locks before checking the incremental cache…", flush=True)
        before, after, removed = prune()
        print(f"Incremental cache: {before / 1024**3:.2f} → {after / 1024**3:.2f} GiB; "
              f"removed {removed} old cache directories (limit: 5 GiB).")
    except (OSError, ValueError) as error:
        print(f"Incremental cache cleanup failed: {error}", file=sys.stderr)
        return code or 1
    return code


if __name__ == "__main__":
    sys.exit(main())
