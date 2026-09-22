"""Prevent normalization/reference comparisons from turning regressions green."""
import copy
import unittest

from run import compare_reference, observe, project


class ContractChecks(unittest.TestCase):
    def test_json_boolean_is_not_an_integer(self):
        with self.assertRaises(AssertionError):
            project({"count": True}, {"count": 1})

    def test_http_and_function_errors_are_not_hidden(self):
        success = {"status": 200, "body": {"echo": {}}, "headers": {}}
        expected = {"status": 200, "body": {"echo": {}}}
        observe(success, expected, {})
        for change in ({"status": 500}, {"error": "ServiceException"},
                       {"headers": {"x-amz-function-error": "Unhandled"}}, {"body": None}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                observe({**success, **change}, expected, {})

    def test_reference_must_match_provenance_and_observations(self):
        report = {"provider": "aws-lambda", "cases_sha256": "cases", "fixture_source_sha256": "fixture",
                  "architecture": "x86_64", "versions": dict.fromkeys(("aws_cli", "javascript", "boto3", "botocore"), "1"),
                  "results": [{"client": "python", "case": "timeout", "passed": True,
                               "observed": {"status": 200, "headers": {"x-amz-function-error": "Unhandled"}}}]}
        compare_reference(report, report)
        for key, value in (("provider", "zapcloud"), ("cases_sha256", "other"),
                           ("fixture_source_sha256", "other"),
                           ("architecture", "arm64"), ("failure", "transport error"), ("results", [])):
            with self.subTest(key=key), self.assertRaises((AssertionError, KeyError)):
                compare_reference(report, {**report, key: value})
        changed = copy.deepcopy(report)
        changed["results"][0]["observed"]["headers"] = {}
        with self.assertRaises(AssertionError):
            compare_reference(changed, report)
        changed["results"][0]["passed"] = False
        with self.assertRaises(AssertionError):
            compare_reference(changed, report)


if __name__ == "__main__":
    unittest.main()
