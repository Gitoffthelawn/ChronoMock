#!/usr/bin/env python3
"""The OSV gate checked before it judges anything.

    python -m unittest discover -s .github/scripts -p test_osv_gate.py

Both OSV jobs run this first, so a gate that stopped blocking, or started
blocking on nothing, fails as itself rather than as a verdict about the tree.

The reports here are synthetic but shaped as osv-scanner 2.7.0 writes them
with ``--all-packages`` (measured on 2026-10-14): ``results`` holds one entry
per file read, with ``source.path`` and a ``packages`` list, and each package
carries ``package.{name, version, ecosystem}`` and, when it has any,
``vulnerabilities`` with ``id``, ``aliases`` and ``summary``.
"""
import copy
import io
import json
import os
import sys
import tempfile
import unittest
import urllib.request
from contextlib import redirect_stderr, redirect_stdout

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import osv_gate  # noqa: E402  (the module under test sits beside this file)


def package(ecosystem, name, version, *vulnerabilities):
    entry = {"package": {"name": name, "version": version, "ecosystem": ecosystem}}
    if vulnerabilities:
        entry["vulnerabilities"] = [
            {"id": vid, "aliases": list(aliases), "summary": "synthetic"} for vid, aliases in vulnerabilities]
    return entry


def report(*sources):
    return {"results": [{"source": {"path": path, "type": "lockfile"}, "packages": list(entries)}
                        for path, entries in sources]}


def tree(crates=30, nuget=30, pypi=1, extra=None):
    """A report that read about what this repository has, with ``extra`` packages added to Cargo.lock."""
    cargo = [package("crates.io", "crate%d" % i, "1.0.%d" % i) for i in range(crates)] + list(extra or [])
    return report(
        ("/w/Cargo.lock", cargo),
        ("/w/gui/App/packages.lock.json", [package("NuGet", "Pkg.%d" % i, "2.0.0") for i in range(nuget)]),
        ("/w/.github/semgrep/requirements.txt", [package("PyPI", "semgrep", "1.179.0")] * pypi),
    )


def run(argv):
    """``(exit code, stdout, stderr)`` of the gate's command line."""
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        code = osv_gate.main(argv)
    return code, out.getvalue(), err.getvalue()


class Verdict(unittest.TestCase):
    def test_a_clean_tree_that_was_read_passes_and_says_what_it_read(self):
        lines, blocked = osv_gate.verdict(tree())
        self.assertFalse(blocked)
        self.assertIn("crates.io 30", lines[-1])
        self.assertIn("NuGet 30", lines[-1])
        self.assertIn("PyPI 1", lines[-1])

    def test_a_vulnerability_the_pull_request_brings_in_blocks(self):
        new = tree(extra=[package("crates.io", "bad", "1.0.0", ("RUSTSEC-2099-0001", ["CVE-2099-1"]))])
        lines, blocked = osv_gate.verdict(new, tree())
        self.assertTrue(blocked)
        self.assertIn("1 blocking, 0 inherited", lines[-1])

    def test_any_vulnerability_blocks_the_weekly_run(self):
        new = tree(extra=[package("crates.io", "bad", "1.0.0", ("RUSTSEC-2099-0001", []))])
        self.assertTrue(osv_gate.verdict(new)[1])

    def test_the_same_vulnerability_on_the_base_is_inherited_by_any_of_its_identifiers(self):
        new = tree(extra=[package("crates.io", "bad", "1.0.1", ("RUSTSEC-2099-0001", ["CVE-2099-1"]))])
        old = tree(extra=[package("crates.io", "bad", "1.0.0", ("GHSA-xxxx-yyyy-zzzz", ["CVE-2099-1"]))])
        lines, blocked = osv_gate.verdict(new, old)
        self.assertFalse(blocked)
        self.assertIn("0 blocking, 1 inherited", lines[-1])

    def test_a_package_name_differing_only_in_case_is_the_same_package(self):
        new = tree(extra=[package("crates.io", "bad", "1.0.1", ("RUSTSEC-2099-0001", []))])
        old = tree(extra=[package("crates.io", "BAD", "1.0.0", ("RUSTSEC-2099-0001", []))])
        self.assertFalse(osv_gate.verdict(new, old)[1])

    def test_the_same_identifier_on_another_package_does_not_excuse_this_one(self):
        new = tree(extra=[package("crates.io", "bad", "1.0.0", ("CVE-2099-1", []))])
        old = tree(extra=[package("NuGet", "bad", "1.0.0", ("CVE-2099-1", []))])
        self.assertTrue(osv_gate.verdict(new, old)[1])

    def test_a_new_vulnerability_on_an_old_package_is_still_new(self):
        new = tree(extra=[package("crates.io", "bad", "1.1.0", ("RUSTSEC-2099-0002", []))])
        old = tree(extra=[package("crates.io", "bad", "1.0.0", ("RUSTSEC-2099-0001", []))])
        self.assertTrue(osv_gate.verdict(new, old)[1])


class ReadWhatTheRepositoryHas(unittest.TestCase):
    def test_an_empty_report_blocks(self):
        lines, blocked = osv_gate.verdict({"results": []})
        self.assertTrue(blocked)
        self.assertIn("read nothing", lines[-1])

    def test_each_ecosystem_has_its_own_floor(self):
        for short in ({"crates": 0}, {"nuget": 3}, {"pypi": 0}):
            with self.subTest(short=short):
                lines, blocked = osv_gate.verdict(tree(**short))
                self.assertTrue(blocked)
                self.assertTrue(any("at least" in line for line in lines))

    def test_the_floors_are_the_ones_measured_and_written_down(self):
        self.assertEqual({"crates.io": 25, "NuGet": 25, "PyPI": 1}, osv_gate.FLOORS)


class Reports(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)

    def write(self, name, content):
        path = os.path.join(self.folder.name, name)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(content if isinstance(content, str) else json.dumps(content))
        return path

    def test_a_clean_report_exits_zero_and_a_new_vulnerability_exits_one(self):
        clean = self.write("clean.json", tree())
        bad = self.write("bad.json", tree(extra=[package("crates.io", "bad", "1.0.0", ("X-1", []))]))
        self.assertEqual(0, run(["verdict", clean])[0])
        self.assertEqual(1, run(["verdict", bad, "--old", clean])[0])

    def test_a_missing_report_exits_two_and_names_it(self):
        missing = os.path.join(self.folder.name, "missing.json")
        code, _, err = run(["verdict", missing])
        self.assertEqual(2, code)
        self.assertIn("missing.json", err)

    def test_an_unreadable_base_report_is_named_as_the_base(self):
        clean = self.write("clean.json", tree())
        code, _, err = run(["verdict", clean, "--old", self.write("old.json", "{not json")])
        self.assertEqual(2, code)
        self.assertIn("base's report", err)

    def test_a_report_of_the_wrong_shape_exits_two_rather_than_a_traceback(self):
        shapes = {
            "a list": [],
            "results not a list": {"results": {}},
            "a package not an object": {"results": [{"source": {}, "packages": ["serde"]}]},
            "vulnerabilities not a list": report(("/w/Cargo.lock", [{"package": {}, "vulnerabilities": "X-1"}])),
            "aliases not a list": report(("/w/Cargo.lock", [{"package": {}, "vulnerabilities": [{"id": "X", "aliases": "Y"}]}])),
        }
        for name, shape in shapes.items():
            with self.subTest(shape=name):
                self.assertEqual(2, run(["verdict", self.write("shape.json", shape)])[0])


class Install(unittest.TestCase):
    def test_a_swapped_asset_is_refused_and_leaves_nothing_behind(self):
        class Response(io.BytesIO):
            def __enter__(self):
                return self

            def __exit__(self, *_):
                return False

        real = urllib.request.urlopen
        urllib.request.urlopen = lambda url, timeout=None: Response(b"not the scanner")
        try:
            with tempfile.TemporaryDirectory() as folder, redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as refused:
                    osv_gate.install(folder)
                self.assertIn("checksum mismatch", str(refused.exception))
                self.assertFalse(os.path.exists(os.path.join(folder, "osv-scanner")))
        finally:
            urllib.request.urlopen = real

    def test_the_pin_names_one_version_and_its_asset(self):
        self.assertEqual(
            "https://github.com/google/osv-scanner/releases/download/v%s/osv-scanner_linux_amd64" % osv_gate.VERSION,
            osv_gate.URL)
        self.assertRegex(osv_gate.SHA256, "^[0-9a-f]{64}$")


if __name__ == "__main__":
    unittest.main()
