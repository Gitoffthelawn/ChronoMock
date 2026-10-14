#!/usr/bin/env python3
"""Fetch OSV-Scanner, and decide from its JSON report.

    python .github/scripts/osv_gate.py install "$RUNNER_TEMP/osv"
    osv-scanner scan source --no-resolve --all-packages --format json --output-file new.json -r ./
    python .github/scripts/osv_gate.py verdict new.json --old old.json    (a pull request)
    python .github/scripts/osv_gate.py verdict new.json                   (the weekly run)

What blocks
-----------
On a pull request, a vulnerability the pull request brings in: one reported
against a package in its merge commit and not against that package on the base
the merge was built on. A vulnerability already on the base does not block
there, because a pull request that did not add it cannot be the one to remove
it, and a gate that fails every pull request until somebody else fixes
something is a gate that gets bypassed. That one belongs to the weekly run in
audit.yml, which blocks on everything it finds and files it under Security.

A vulnerability is the same one on both sides when any of its identifiers
(the OSV id or an alias, CVE and GHSA included) is reported against the same
package on the base. A version bump that keeps a package vulnerable is not a
new vulnerability, and one that makes it vulnerable to something else is.

A report that did not read this repository blocks too, and that is the second
half of the gate rather than an afterthought. A clean scan writes
``"results": []``, and so does a scan that found nothing to read - measured on
2026-10-14, osv-scanner 2.7.0 - so the empty report says nothing on its own.
The scans run with ``--all-packages``, which lists every source and package
that was read, and this script checks that each ecosystem this repository has
was read with at least ``FLOORS`` packages. The counts are printed every run,
so a slow collapse is visible before it crosses a floor.

Why ``--no-resolve``
--------------------
``.github/semgrep/requirements.txt`` pins Semgrep alone, and by default the
scanner guesses what sits underneath it. Measured on 2026-10-14: the guess was
anyio 4.9.0, idna 3.9.0 and python-multipart 0.0.9, and it reported eleven
vulnerabilities against them, while the Semgrep job's own log shows pip
installing anyio 4.15.1, idna 3.20 and python-multipart 0.0.32, which the same
scanner reports clean. Every one of those findings was about a version nobody
installs. With ``--no-resolve`` the scanner reads what the files declare and
nothing it invented. What that leaves out has to be said rather than implied:
the packages pip resolves under Semgrep on the runner are not scanned by this
gate. Covering them means pinning them, a fully resolved requirements file,
which is its own decision. Cargo.lock and the NuGet lock files are complete
already, so the flag changes nothing for them.

The scanner itself
------------------
The published binary, pinned to a version AND to its checksum, for the reason
.github/scripts/cargo-deny.ps1 gives: a swapped asset fails loudly rather than
running as us. Google's own actions pull a container by a tag, which pins
nothing. Dependabot cannot see a version written here, so a newer release is
taken by hand - the version, and the checksum from that release's
``osv-scanner_SHA256SUMS`` - and measured again against this tree.
"""
import argparse
import hashlib
import json
import os
import sys
import urllib.request

VERSION = "2.7.0"
ASSET = "osv-scanner_linux_amd64"
SHA256 = "160c6253e2922e9e0a6a9771c5c4c40a4b3c6090b34a3cdd5e80c96cc2e4218b"
URL = "https://github.com/google/osv-scanner/releases/download/v%s/%s" % (VERSION, ASSET)

# The least number of packages each ecosystem must have been read with. Measured
# on 2026-10-14: crates.io 51 (Cargo.lock), NuGet 59 (three lock files and three
# project files), PyPI 1 (the Semgrep pin). A floor at about half of today
# catches a lock file the scan stopped reading without firing on a smaller
# dependency tree. Lowering one is a decision, written down here with a reason.
FLOORS = {
    "crates.io": 25,
    "NuGet": 25,
    "PyPI": 1,
}


def install(directory):
    """Fetch the pinned scanner into ``directory`` and return its path."""
    os.makedirs(directory, exist_ok=True)
    target = os.path.join(directory, "osv-scanner")
    print("fetching osv-scanner %s" % VERSION, file=sys.stderr)
    with urllib.request.urlopen(URL, timeout=300) as response:
        data = response.read()
    actual = hashlib.sha256(data).hexdigest()
    if actual != SHA256:
        raise SystemExit("osv-scanner checksum mismatch: expected %s, got %s" % (SHA256, actual))
    with open(target, "wb") as handle:
        handle.write(data)
    os.chmod(target, 0o755)
    return target


def load(path):
    """A report as the scanner wrote it, refused when it does not have the scanner's shape.

    The file is read whole and without a size limit on purpose. It is not input from outside: the
    pinned scanner writes it in the same job, a step earlier, and the largest one this repository
    produces is tens of kilobytes. The shape is checked because a report that parses but is not
    shaped like one would otherwise end in a traceback, red but unexplained.
    """
    with open(path, encoding="utf-8") as handle:
        report = json.load(handle)
    check_shape(report)
    return report


def check_shape(report):
    """Raise ValueError naming the first part of a report that is not shaped as the scanner writes it."""
    def need(value, kind, where):
        if not isinstance(value, kind):
            raise ValueError("%s is %s, not %s" % (where, type(value).__name__, kind.__name__))
        return value

    def field(owner, key, kind, where):
        # Absent or null means "none of these", which the scanner writes for a clean package. Any
        # other value of the wrong type is a report this gate does not understand - and must not be
        # read as empty, because an empty dict and an empty list are both falsy.
        value = owner.get(key)
        return kind() if value is None else need(value, kind, where)

    need(report, dict, "the report")
    for r, result in enumerate(field(report, "results", list, "results")):
        need(result, dict, "results[%d]" % r)
        field(result, "source", dict, "results[%d].source" % r)
        for p, entry in enumerate(field(result, "packages", list, "results[%d].packages" % r)):
            where = "results[%d].packages[%d]" % (r, p)
            need(entry, dict, where)
            field(entry, "package", dict, where + ".package")
            for v, vulnerability in enumerate(field(entry, "vulnerabilities", list, where + ".vulnerabilities")):
                need(vulnerability, dict, "%s.vulnerabilities[%d]" % (where, v))
                field(vulnerability, "aliases", list, "%s.vulnerabilities[%d].aliases" % (where, v))


def packages(report):
    """Every package entry in a report, with the source it was read from."""
    for result in report.get("results") or []:
        source = (result.get("source") or {}).get("path", "?")
        for entry in result.get("packages") or []:
            yield source, entry


def key(entry):
    """A package as both sides of a comparison name it, version left out."""
    package = entry.get("package") or {}
    return str(package.get("ecosystem", "?")), str(package.get("name", "?")).lower()


def identifiers(vulnerability):
    """The OSV id with every alias, so a CVE and its GHSA are one vulnerability."""
    ids = {str(vulnerability.get("id", ""))}
    ids.update(str(alias) for alias in vulnerability.get("aliases") or [])
    ids.discard("")
    return ids


def findings(report):
    """``[(source, package entry, vulnerability)]``, one row per vulnerability."""
    return [(source, entry, vulnerability)
            for source, entry in packages(report)
            for vulnerability in entry.get("vulnerabilities") or []]


def known_on(report):
    """Every identifier reported against each package, keyed by package."""
    known = {}
    for _, entry, vulnerability in findings(report):
        known.setdefault(key(entry), set()).update(identifiers(vulnerability))
    return known


def read_counts(report):
    """How many packages the scan read, per ecosystem."""
    counts = {}
    for _, entry in packages(report):
        ecosystem = key(entry)[0]
        counts[ecosystem] = counts.get(ecosystem, 0) + 1
    return counts


def describe(source, entry, vulnerability):
    package = entry.get("package") or {}
    aliases = ", ".join(sorted(identifiers(vulnerability) - {vulnerability.get("id")}))
    summary = " ".join(str(vulnerability.get("summary", "")).split())[:200]
    return "%s %s %s  %s%s\n      %s\n      in %s" % (
        package.get("ecosystem", "?"), package.get("name", "?"), package.get("version", "?"),
        vulnerability.get("id", "?"), " (%s)" % aliases if aliases else "",
        summary or "(no summary)", source)


def verdict(new, old=None):
    """``(lines, blocked)`` for one report, compared against ``old`` when given."""
    lines = []
    counts = read_counts(new)
    short = sorted(e for e, floor in FLOORS.items() if counts.get(e, 0) < floor)
    if short:
        lines.append("the scan did not read this repository, so its report says nothing about it:")
        lines += ["  %s: %d package(s) read, at least %d expected"
                  % (e, counts.get(e, 0), FLOORS[e]) for e in short]
        lines.append("  A lock file that moved, a scan pointed at the wrong folder, or an")
        lines.append("  ecosystem the scanner stopped recognising all look exactly like this.")

    known = known_on(old) if old is not None else {}
    blocking, inherited = [], []
    for row in findings(new):
        seen = identifiers(row[2]) & known.get(key(row[1]), set())
        (inherited if seen else blocking).append(row)
    if blocking:
        lines.append("vulnerabilities this %s brings in:"
                     % ("pull request" if old is not None else "tree"))
        lines += ["  " + describe(*r) for r in blocking]
    if inherited:
        lines.append("already on the base branch (not blocking here, the weekly run reports them):")
        lines += ["  " + describe(*r) for r in inherited]

    lines.append("osv-scanner %s: %d blocking, %d inherited, read %s"
                 % (VERSION, len(blocking), len(inherited),
                    ", ".join("%s %d" % (e, n) for e, n in sorted(counts.items())) or "nothing"))
    return lines, bool(short or blocking)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    fetch = sub.add_parser("install", help="fetch the pinned scanner, checksum checked")
    fetch.add_argument("directory")
    judge = sub.add_parser("verdict", help="decide from a JSON report")
    judge.add_argument("new", help="the report for the tree being judged")
    judge.add_argument("--old", help="the report for the base, on a pull request")
    args = parser.parse_args(argv)

    if args.command == "install":
        print(install(args.directory))
        return 0

    # A missing, unparsable or misshapen report is a failed gate, never a pass: it
    # means the scan step did not produce what this one was promised. Each report
    # is read on its own, so the message names the one that failed.
    reports = {}
    for role, path in (("new", args.new), ("old", args.old)):
        if path is None:
            reports[role] = None
            continue
        try:
            reports[role] = load(path)
        except (OSError, ValueError) as exc:
            if role == "old":
                which = "base's"
            else:
                which = "pull request's" if args.old else "tree's"
            print("osv gate: cannot read the %s report %s (%s). The scan step above did not leave"
                  " a JSON report this gate can read - look at that step's log first, then run"
                  " the scan again." % (which, path, exc), file=sys.stderr)
            return 2
    new, old = reports["new"], reports["old"]
    lines, blocked = verdict(new, old)
    for line in lines:
        print(line)
    return 1 if blocked else 0


if __name__ == "__main__":
    raise SystemExit(main())
