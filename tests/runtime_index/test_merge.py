"""Exercise the exact jq filter used by runtimes.yml, without a registry."""
import copy
import itertools
import json
from pathlib import Path
import subprocess
import unittest


FILTER = Path(__file__).resolve().parents[2] / ".github/scripts/merge-runtime-index.jq"


def entry(version):
    return {"interpreter_version": version, "oci_ref": "ghcr.io/example/runtime:test",
            "oci_digest": "sha256:" + version * 64, "tree_sha256": version * 64}


def fragment(runtime, platform, version):
    return {runtime: {platform: entry(version)}}


def merge(base, *fragments):
    return subprocess.run(
        ["jq", "-e", "-S", "-s", "-f", str(FILTER)],
        input="\n".join(json.dumps(value) for value in (base, *fragments)),
        text=True, capture_output=True,
    )


class ConcurrentPublications(unittest.TestCase):
    def test_two_jobs_preserve_both_publications_and_unrelated_pins(self):
        base = {"nodejs22.x": {"linux-x86_64": entry("1"), "linux-arm64": entry("2")},
                "python3.13": {"linux-x86_64": {**entry("3"), "pbs_release": "old"}},
                "other-runtime": {"linux-x86_64": entry("4")}}
        node = fragment("nodejs22.x", "linux-x86_64", "5")
        python = fragment("python3.13", "linux-x86_64", "6")
        expected = copy.deepcopy(base)
        expected["nodejs22.x"]["linux-x86_64"] = node["nodejs22.x"]["linux-x86_64"]
        expected["python3.13"]["linux-x86_64"] = python["python3.13"]["linux-x86_64"]
        outputs = []
        for publications in itertools.permutations((node, python)):
            result = merge(base, *publications)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), expected)
            outputs.append(result.stdout)
        self.assertEqual(outputs[0], outputs[1])

    def test_two_platforms_of_one_runtime_from_empty_index(self):
        x86 = fragment("nodejs22.x", "linux-x86_64", "1")
        arm = fragment("nodejs22.x", "linux-arm64", "2")
        result = merge({}, x86, arm)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout),
                         {"nodejs22.x": {**x86["nodejs22.x"], **arm["nodejs22.x"]}})

    def test_rejects_old_full_snapshots_instead_of_overwriting_new_pins(self):
        base = {"nodejs22.x": {"linux-x86_64": entry("1")},
                "python3.13": {"linux-x86_64": entry("2")}}
        # Both jobs checked out the same base. This was the old artifact format.
        node_snapshot, python_snapshot = copy.deepcopy(base), copy.deepcopy(base)
        node_snapshot["nodejs22.x"]["linux-x86_64"] = entry("3")
        python_snapshot["python3.13"]["linux-x86_64"] = entry("4")
        for snapshots in itertools.permutations((node_snapshot, python_snapshot)):
            result = merge(base, *snapshots)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("exactly one runtime", result.stderr)
            self.assertEqual(result.stdout, "")

    def test_rejects_duplicate_publications(self):
        first = fragment("nodejs22.x", "linux-x86_64", "1")
        for second in (first, fragment("nodejs22.x", "linux-x86_64", "2")):
            result = merge({}, first, second)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("duplicate publication", result.stderr)
            self.assertEqual(result.stdout, "")

    def test_rejects_missing_or_malformed_fragments(self):
        self.assertNotEqual(merge({}).returncode, 0)
        for bad in ({}, None, [], {"nodejs22.x": {}}, {"nodejs22.x": []},
                    {"nodejs22.x": {"linux-x86_64": None}},
                    {"nodejs22.x": {"linux-x86_64": entry("1"), "linux-arm64": entry("2")}}):
            with self.subTest(fragment=bad):
                result = merge({}, bad)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
