"""jsonio: whole-file JSON artifact writes."""

from __future__ import annotations

import json
import threading
import unittest
from unittest import mock

from pn import jsonio
from tests.builders import TempDirTestCase


class WriteTest(TempDirTestCase):
    def test_round_trips(self):
        path = self.tmp / "sub" / "a.json"
        jsonio.write(path, {"x": [1, "é"]})
        self.assertEqual(jsonio.read(path), {"x": [1, "é"]})
        self.assertEqual([p.name for p in path.parent.iterdir()], ["a.json"])

    def test_concurrent_writers_of_one_artifact_both_land_whole(self):
        """A writer paused mid-serialization while another writes and
        publishes the same artifact still publishes its own whole file, and
        the other's temporary file is untouched by it."""
        path = self.tmp / "a.json"
        paused, resume = threading.Event(), threading.Event()
        real_dump = json.dump
        errors: list[BaseException] = []

        def slow_dump(obj, f, **kw):
            if obj == {"run": "A"}:
                f.write('{"run": ')
                paused.set()
                resume.wait(5)
                f.write('"A"}')
                return None
            return real_dump(obj, f, **kw)

        def write_a():
            try:
                jsonio.write(path, {"run": "A"})
            except BaseException as e:  # noqa: BLE001 - surfaced below
                errors.append(e)

        with mock.patch.object(jsonio.json, "dump", slow_dump):
            a = threading.Thread(target=write_a)
            a.start()
            self.assertTrue(paused.wait(5))
            jsonio.write(path, {"run": "B"})
            self.assertEqual(jsonio.read(path), {"run": "B"})
            resume.set()
            a.join(5)
        self.assertEqual(errors, [])
        self.assertEqual(jsonio.read(path), {"run": "A"})
        self.assertEqual([p.name for p in self.tmp.iterdir()], ["a.json"])


class WriteFailureTest(TempDirTestCase):
    def test_a_failure_before_the_file_opens_closes_its_descriptor(self):
        path = self.tmp / "a.json"
        jsonio.write(path, {"old": True})
        closed: list[int] = []
        real_close = jsonio.os.close

        def track_close(fd):
            closed.append(fd)
            real_close(fd)

        with (
            mock.patch.object(jsonio.os, "fdopen", side_effect=OSError("no memory")),
            mock.patch.object(jsonio.os, "close", track_close),
            self.assertRaises(OSError),
        ):
            jsonio.write(path, {"new": True})
        self.assertEqual(len(closed), 1)
        self.assertEqual(jsonio.read(path), {"old": True})
        self.assertEqual([p.name for p in self.tmp.iterdir()], ["a.json"])


if __name__ == "__main__":
    unittest.main()
