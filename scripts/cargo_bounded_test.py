# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
from cargo_bounded import cargo_lock, inventory, main, prune


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.cache = self.root / "debug" / "incremental"
        self.cache.mkdir(parents=True)

    def entry(self, name, timestamp, data=b"x" * 65536):
        directory = self.cache / name
        directory.mkdir()
        file = directory / "object.o"
        file.write_bytes(data)
        for path in (file, directory):
            os.utime(path, (timestamp, timestamp))
        return directory

    def size(self, path):
        return sum(inventory(path)[0].values())

    def test_under_budget_is_unchanged_and_oldest_is_removed_first(self):
        old = self.entry("desktop-old", 100)
        recent = self.entry("desktop-recent", 200)
        before = self.size(self.cache)
        self.assertEqual(prune(self.cache, before), (before, before, 0))
        limit = before - self.size(old)
        _, after, removed = prune(self.cache, limit)
        self.assertEqual(removed, 1)
        self.assertFalse(old.exists())
        self.assertTrue(recent.exists())
        self.assertLessEqual(after, limit)
        self.assertEqual(after, self.size(self.cache))

    def test_shared_inodes_are_counted_once_and_external_links_survive(self):
        old = self.entry("desktop-old", 100)
        recent = self.cache / "desktop-recent"
        recent.mkdir()
        os.link(old / "object.o", recent / "object.o")
        os.utime(recent, (200, 200))
        external = self.root / "deps.o"
        os.link(old / "object.o", external)
        before = self.size(self.cache)
        self.assertEqual(prune(self.cache, before), (before, before, 0))
        limit = self.cache.stat().st_size if os.name == "nt" else self.cache.stat().st_blocks * 512
        _, after, removed = prune(self.cache, limit)
        self.assertEqual(removed, 2)
        self.assertLessEqual(after, limit)
        self.assertEqual(external.read_bytes(), b"x" * 65536)

    def test_symlinks_and_unrecognized_entries_are_not_deleted(self):
        protected = self.root / "protected"
        protected.mkdir()
        (protected / "data").write_bytes(b"keep")
        link = self.cache / "desktop-linked"
        try:
            link.symlink_to(protected, target_is_directory=True)
        except OSError:
            self.skipTest("Directory symlinks are unavailable")
        unknown = self.entry("notes", 100)
        with self.assertRaises(ValueError):
            prune(self.cache, 0)
        self.assertTrue(link.is_symlink())
        self.assertTrue(unknown.exists())
        self.assertEqual((protected / "data").read_bytes(), b"keep")
        with self.assertRaises(ValueError):
            prune(link, 0)

    def test_cleanup_waits_for_cargo_build_lock(self):
        old = self.entry("desktop-old", 100)
        program = (
            "import sys; sys.dont_write_bytecode = True; "
            f"sys.path.insert(0, {str(Path(__file__).parent)!r}); "
            "from pathlib import Path; from cargo_bounded import prune; "
            "print('ready', flush=True); "
            f"prune(Path({str(self.cache)!r}), 16384)"
        )
        with cargo_lock(self.cache.parent):
            child = subprocess.Popen([sys.executable, "-c", program], stdout=subprocess.PIPE, text=True)
            self.addCleanup(lambda: child.poll() is None and child.kill())
            self.assertEqual(child.stdout.readline().strip(), "ready")
            with self.assertRaises(subprocess.TimeoutExpired):
                child.wait(timeout=0.2)
            self.assertTrue(old.exists())
        self.assertEqual(child.wait(timeout=10), 0)
        child.stdout.close()
        self.assertFalse(old.exists())

    def test_cargo_failure_still_prunes_and_preserves_exit_status(self):
        with patch("cargo_bounded.subprocess.run", return_value=subprocess.CompletedProcess([], 101)) as run, \
                patch("cargo_bounded.prune", return_value=(0, 0, 0)) as cleanup:
            self.assertEqual(main(["test", "--workspace", "--locked"]), 101)
            self.assertEqual(run.call_args.args[0], ["cargo", "test", "--workspace", "--locked"])
            cleanup.assert_called_once_with()
        with patch("cargo_bounded.subprocess.run", return_value=subprocess.CompletedProcess([], 0)), \
                patch("cargo_bounded.prune", side_effect=OSError("locked")):
            self.assertEqual(main(["build"]), 1)


if __name__ == "__main__":
    unittest.main()
