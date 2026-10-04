"""Exercise the real Linux resource guard without game data or large allocations."""

import json
import os
import re
import shutil
import signal
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "run-heavy.sh"


class HeavyRunnerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not all(shutil.which(name) for name in ["systemd-run", "systemctl", "findmnt"]):
            raise unittest.SkipTest("Requires Linux systemd user scopes and util-linux")
        probe = subprocess.run(
            ["systemd-run", "--user", "--scope", "-q", "true"], capture_output=True
        )
        if probe.returncode:
            raise unittest.SkipTest("Requires a running systemd user manager")

    def setUp(self):
        parent = Path.home() / ".cache" / "fo76-tools"
        parent.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=parent)
        self.root = Path(self.directory.name)
        self.script = self.root / "run-heavy.sh"
        source = SCRIPT.read_text()
        self.assertEqual(source.count("unit=fo76-heavy.scope"), 1)
        self.unit = f"fo76-heavy-test-{self.root.name}.scope"
        self.script.write_text(source.replace("unit=fo76-heavy.scope", f"unit={self.unit}"))
        self.env = os.environ | {
            "FO76_SCRATCH_DIR": str(self.root / "scratch"),
            "XDG_STATE_HOME": str(self.root / "state"),
        }

    def tearDown(self):
        subprocess.run(
            ["systemctl", "--user", "kill", "--kill-whom=all", "--signal=KILL", self.unit],
            capture_output=True, timeout=10,
        )
        self.directory.cleanup()

    def run_command(self, profile, *command):
        return subprocess.run(
            ["bash", str(self.script), profile, *command],
            env=self.env,
            cwd=self.root,
            text=True,
            capture_output=True,
            timeout=15,
        )

    def log_path(self, result):
        match = re.search(r"full log=(.+)", result.stdout)
        assert match is not None
        return Path(match[1])

    def test_limits_descendants_arguments_and_disk_tmp(self):
        program = """
import json,os,pathlib,subprocess,sys
group=next(line.split(':',2)[2].strip() for line in pathlib.Path('/proc/self/cgroup').read_text().splitlines() if line.startswith('0:'))
base=pathlib.Path('/sys/fs/cgroup')/group.lstrip('/')
child=subprocess.check_output([sys.executable,'-c',"import pathlib;print(pathlib.Path('/proc/self/cgroup').read_text().strip())"],text=True)
print(json.dumps({'max':(base/'memory.max').read_text().strip(),'swap':(base/'memory.swap.max').read_text().strip(),'oom':(base/'memory.oom.group').read_text().strip(),'group':group,'child':child,'args':sys.argv[1:],'cwd':os.getcwd(),'tmp':os.environ['TMPDIR']}))
"""
        for profile, limit in [("esm", 5 * 1024**3), ("cargo", 10 * 1024**3)]:
            with self.subTest(profile=profile):
                result = self.run_command(profile, "python3", "-c", program, "a b", "$HOME", "--flag")
                self.assertEqual(result.returncode, 0, result.stderr)
                lines = self.log_path(result).read_text().splitlines()
                info = json.loads(next(line for line in lines if line.startswith("{")))
                self.assertEqual(int(info["max"]), limit)
                self.assertEqual(info["swap"], "0")
                self.assertEqual(info["oom"], "1")
                self.assertIn(info["group"], info["child"])
                self.assertEqual(info["args"], ["a b", "$HOME", "--flag"])
                self.assertEqual(info["cwd"], str(self.root))
                self.assertTrue(info["tmp"].startswith(str(self.root / "scratch")))
                self.assertFalse(Path(info["tmp"]).exists())

    def test_failure_status_and_bounded_excerpt(self):
        result = self.run_command(
            "esm", "python3", "-c", "import sys;print('x'*20000);sys.exit(23)"
        )
        self.assertEqual(result.returncode, 23)
        self.assertLess(len(result.stderr), 4200)
        self.assertGreater(self.log_path(result).stat().st_size, 20000)

    def test_caller_stdin_is_preserved(self):
        result = subprocess.run(
            ["bash", str(self.script), "esm", "python3", "-c", "import sys;print(repr(sys.stdin.read()))"],
            input="caller input\n", env=self.env, text=True, capture_output=True, timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("'caller input\\n'", self.log_path(result).read_text())

    def test_relative_scratch_survives_child_directory_change(self):
        (self.root / "child").mkdir()
        env = self.env | {"FO76_SCRATCH_DIR": "relative scratch"}
        result = subprocess.run(
            ["bash", str(self.script), "esm", "python3", "-c",
             "import json,os,tempfile;os.chdir('child');"
             "print(json.dumps({'tmp':os.environ['TMPDIR'],'chosen':tempfile.gettempdir()}))"],
            cwd=self.root, env=env, text=True, capture_output=True, timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        info = json.loads(self.log_path(result).read_text())
        self.assertTrue(Path(info["tmp"]).is_absolute())
        self.assertTrue(Path(info["tmp"]).is_relative_to(self.root / "relative scratch"))
        self.assertEqual(info["chosen"], info["tmp"])

    def test_descendant_keeps_scope_exclusive_and_scratch_alive(self):
        ready = self.root / "descendant.json"
        program = """
import os,pathlib,subprocess,sys
child="import json,os,pathlib,time;pathlib.Path('descendant.json').write_text(json.dumps({'tmp':os.environ['TMPDIR']}));time.sleep(2)"
subprocess.Popen([sys.executable,'-c',child],close_fds=True,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
sys.exit(23)
"""
        process = subprocess.Popen(
            ["bash", str(self.script), "esm", "python3", "-c", program],
            cwd=self.root, env=self.env, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        try:
            deadline = time.monotonic() + 8
            while not ready.exists() and process.poll() is None and time.monotonic() < deadline:
                time.sleep(0.05)
            self.assertTrue(ready.exists())
            self.assertIsNone(process.poll())
            temporary = Path(json.loads(ready.read_text())["tmp"])
            self.assertTrue(temporary.exists())
            self.assertEqual(self.run_command("cargo", "true").returncode, 75)
            _, error = process.communicate(timeout=10)
            self.assertEqual(process.returncode, 23, error)
            self.assertFalse(temporary.exists())
        finally:
            if process.poll() is None:
                process.terminate()
                process.communicate(timeout=10)

    def test_cancellation_during_delayed_start_never_runs_command(self):
        shim_dir = self.root / "bin"
        shim_dir.mkdir()
        real_runner = shutil.which("systemd-run")
        assert real_runner is not None
        shim = shim_dir / "systemd-run"
        shim.write_text(
            "#!/usr/bin/env python3\nimport os,pathlib,time\n"
            "pathlib.Path('startup-ready').touch()\ntime.sleep(0.6)\n"
            f"os.execv({real_runner!r},[{real_runner!r}]+__import__('sys').argv[1:])\n"
        )
        shim.chmod(0o700)
        process = subprocess.Popen(
            ["bash", str(self.script), "esm", "touch", "must-not-start"],
            cwd=self.root, env=self.env | {"PATH": f"{shim_dir}:{os.environ['PATH']}"},
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        try:
            deadline = time.monotonic() + 8
            while not (self.root / "startup-ready").exists() and process.poll() is None and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue((self.root / "startup-ready").exists())
            process.send_signal(signal.SIGTERM)
            output, error = process.communicate(timeout=5)
            self.assertEqual(process.returncode, 143, error)
            time.sleep(0.7)
            self.assertFalse((self.root / "must-not-start").exists())
            match = re.search(r"unit=([^;]+)", output)
            assert match is not None
            self.assertNotEqual(
                subprocess.run(["systemctl", "--user", "is-active", "--quiet", match[1]]).returncode, 0
            )
        finally:
            if process.poll() is None:
                process.terminate()
                process.communicate(timeout=10)

    def test_relative_xdg_settings_use_shared_home_defaults(self):
        fake_home = self.root / "home"
        fake_home.mkdir()
        self.env = self.env | {
            "HOME": str(fake_home), "XDG_STATE_HOME": "relative-state", "XDG_CACHE_HOME": "relative-cache",
        }
        del self.env["FO76_SCRATCH_DIR"]
        process = self.start_waiting_command()
        try:
            other = self.root / "another directory"
            other.mkdir()
            result = subprocess.run(
                ["bash", str(self.script), "cargo", "true"], cwd=other, env=self.env,
                capture_output=True, text=True, timeout=10,
            )
            self.assertEqual(result.returncode, 75, result.stderr)
            self.assertTrue((fake_home / ".cache" / "fo76-tools" / "heavy").is_dir())
            self.assertFalse((self.root / "relative-state").exists())
            self.assertFalse((other / "relative-state").exists())
            self.assertFalse((self.root / "relative-cache").exists())
        finally:
            process.terminate()
            process.communicate(timeout=10)

    def test_disabled_memory_controller_rejects_command(self):
        if not shutil.which("busctl"):
            self.skipTest("Requires busctl for an isolated disabled-controller slice")
        unit = f"fo76guardtest{os.getpid()}{time.monotonic_ns()}.slice"
        subprocess.run(
            ["busctl", "--user", "call", "org.freedesktop.systemd1", "/org/freedesktop/systemd1",
             "org.freedesktop.systemd1.Manager", "StartTransientUnit", "ssa(sv)a(sa(sv))",
             unit, "fail", "1", "DisableControllers", "as", "1", "memory", "0"],
            check=True, capture_output=True, timeout=10,
        )
        try:
            # Select only our transient test slice; run the production guard unchanged.
            copy = self.root / "disabled-controller.sh"
            source = self.script.read_text()
            marker = "--quiet --collect --unit="
            self.assertEqual(source.count(marker), 1)
            copy.write_text(source.replace(marker, f"--quiet --collect --slice={unit} --unit="))
            result = subprocess.run(
                ["bash", str(copy), "esm", "touch", "must-not-exist"],
                cwd=self.root, env=self.env, capture_output=True, text=True, timeout=15,
            )
            self.assertEqual(result.returncode, 78, result.stderr)
            self.assertIn("Memory controller is unavailable", result.stderr)
            self.assertFalse((self.root / "must-not-exist").exists())
        finally:
            subprocess.run(["systemctl", "--user", "stop", unit], check=True, timeout=10)

    def start_waiting_command(self, program=None):
        ready = self.root / "ready"
        if program is None:
            program = "import pathlib,time;pathlib.Path('ready').touch();time.sleep(30)"
        process = subprocess.Popen(
            ["bash", str(self.script), "esm", "python3", "-c",
             program],
            cwd=self.root, env=self.env, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        deadline = time.monotonic() + 8
        while not ready.exists() and process.poll() is None and time.monotonic() < deadline:
            time.sleep(0.05)
        if not ready.exists():
            process.terminate()
            output = process.communicate(timeout=5)
            self.fail(f"Waiting command did not start: {output}")
        return process

    def test_cancellation_kills_term_resistant_command(self):
        process = self.start_waiting_command(
            "import pathlib,signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);"
            "pathlib.Path('ready').touch();time.sleep(30)"
        )
        try:
            process.send_signal(signal.SIGTERM)
            _, error = process.communicate(timeout=3)
            self.assertEqual(process.returncode, 143, error)
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=5)

    def test_absolute_xdg_state_variations_still_exclude_overlap(self):
        process = self.start_waiting_command()
        try:
            result = subprocess.run(
                ["bash", str(self.script), "cargo", "true"],
                env=self.env | {"XDG_STATE_HOME": str(self.root / "different valid state")},
                text=True, capture_output=True, timeout=10,
            )
            self.assertEqual(result.returncode, 75, result.stderr)
        finally:
            process.terminate()
            process.communicate(timeout=10)

    def test_tracking_error_retains_live_descendant_scratch(self):
        shim_dir = self.root / "bin"
        shim_dir.mkdir()
        real_stat = shutil.which("stat")
        assert real_stat is not None
        shim = shim_dir / "stat"
        shim.write_text(
            "#!/usr/bin/env python3\nimport os,sys\n"
            "if any(x.startswith('/proc/') and '/fd/' in x for x in sys.argv[1:]):sys.exit(1)\n"
            f"os.execv({real_stat!r},[{real_stat!r}]+sys.argv[1:])\n"
        )
        shim.chmod(0o700)
        program = """
import os,pathlib,subprocess,sys
pathlib.Path('temporary').write_text(os.environ['TMPDIR'])
subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'],close_fds=True,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
sys.exit(23)
"""
        result = subprocess.run(
            ["bash", str(self.script), "esm", "python3", "-c", program],
            cwd=self.root, env=self.env | {"PATH": f"{shim_dir}:{os.environ['PATH']}"},
            text=True, capture_output=True, timeout=10,
        )
        self.assertEqual(result.returncode, 78, result.stderr)
        self.assertIn("Cannot verify scope completion", result.stderr)
        self.assertTrue(Path((self.root / "temporary").read_text()).exists())
        self.assertEqual(self.run_command("cargo", "true").returncode, 75)

    def test_cancellation_cannot_kill_replacement_scope(self):
        shim_dir = self.root / "bin"
        shim_dir.mkdir()
        shim = shim_dir / "sleep"
        shim.write_text(
            "#!/usr/bin/env python3\nimport pathlib,time\n"
            "pathlib.Path('scope-handle-ready').touch()\n"
            "while not pathlib.Path('release-wait').exists():time.sleep(0.01)\n"
        )
        shim.chmod(0o700)
        first_program = """
import subprocess,sys
subprocess.Popen([sys.executable,'-c','import time;time.sleep(0.8)'],close_fds=True,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
"""
        first = subprocess.Popen(
            ["bash", str(self.script), "esm", "python3", "-c", first_program],
            cwd=self.root, env=self.env | {"PATH": f"{shim_dir}:{os.environ['PATH']}"},
            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        second = None
        try:
            deadline = time.monotonic() + 5
            while not (self.root / "scope-handle-ready").exists() and first.poll() is None and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue((self.root / "scope-handle-ready").exists())
            while time.monotonic() < deadline:
                if subprocess.run(["systemctl", "--user", "is-active", "--quiet", self.unit]).returncode:
                    break
                time.sleep(0.02)
            self.assertNotEqual(subprocess.run(["systemctl", "--user", "is-active", "--quiet", self.unit]).returncode, 0)
            second = self.start_waiting_command()
            first.terminate()
            (self.root / "release-wait").touch()
            _, error = first.communicate(timeout=5)
            self.assertEqual(first.returncode, 143, error)
            self.assertIsNone(second.poll())
            self.assertEqual(subprocess.run(["systemctl", "--user", "is-active", "--quiet", self.unit]).returncode, 0)
        finally:
            (self.root / "release-wait").touch()
            for process in [first, second]:
                if process is not None and process.poll() is None:
                    process.terminate()
                    process.communicate(timeout=5)

    def test_concurrency_across_working_directories_and_cancellation(self):
        process = self.start_waiting_command()
        try:
            other = self.root / "other worktree"
            other.mkdir()
            result = subprocess.run(
                ["bash", str(self.script), "cargo", "true"], cwd=other, env=self.env,
                capture_output=True, text=True, timeout=10,
            )
            self.assertEqual(result.returncode, 75, result.stderr)
        finally:
            process.send_signal(signal.SIGTERM)
            output, error = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 143, error)
        match = re.search(r"unit=([^;]+)", output)
        assert match is not None
        unit = match[1]
        self.assertNotEqual(
            subprocess.run(["systemctl", "--user", "is-active", "--quiet", unit]).returncode, 0
        )
        self.assertEqual(self.run_command("cargo", "true").returncode, 0)

    def test_ram_backed_scratch_is_rejected(self):
        filesystem = subprocess.check_output(
            ["findmnt", "-n", "-o", "FSTYPE", "-T", "/dev/shm"], text=True
        ).strip()
        if filesystem != "tmpfs":
            self.skipTest("No RAM-backed /dev/shm available")
        with tempfile.TemporaryDirectory(dir="/dev/shm") as scratch:
            env = self.env | {"FO76_SCRATCH_DIR": scratch}
            result = subprocess.run(
                ["bash", str(self.script), "esm", "touch", str(self.root / "must-not-exist")],
                env=env, capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 78)
            self.assertFalse((self.root / "must-not-exist").exists())

    def test_forced_launcher_exit_keeps_lock_until_scope_stopped(self):
        process = self.start_waiting_command()
        assert process.stdout is not None
        first_line = process.stdout.readline()
        match = re.search(r"unit=([^;]+)", first_line)
        assert match is not None
        unit = match[1]
        try:
            process.kill()
            process.communicate(timeout=5)
            self.assertEqual(self.run_command("cargo", "true").returncode, 75)
        finally:
            subprocess.run(["systemctl", "--user", "stop", unit], check=True)
        self.assertEqual(self.run_command("cargo", "true").returncode, 0)

    def test_invalid_profile_does_not_run_command(self):
        result = self.run_command("invalid", "touch", "must-not-exist")
        self.assertEqual(result.returncode, 64)
        self.assertFalse((self.root / "must-not-exist").exists())


if __name__ == "__main__":
    unittest.main()
