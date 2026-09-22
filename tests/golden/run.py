#!/usr/bin/env python3
"""Shared Lambda contract cases through real CLI/v3 JS SDK/Boto3 clients."""
import argparse
import ast
import base64
import contextlib
import configparser
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid
import zipfile

import boto3
import botocore
from botocore.config import Config
from botocore.exceptions import ClientError

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
OPERATIONS = {
    "CreateFunction": "create_function", "GetFunction": "get_function",
    "ListFunctions": "list_functions", "UpdateFunctionCode": "update_function_code",
    "Invoke": "invoke", "DeleteFunction": "delete_function",
}
HEADERS = ("content-type", "x-amzn-errortype", "x-amz-function-error", "x-amz-executed-version")


def command(args, **kwargs):
    return subprocess.run(args, check=True, text=True, capture_output=True, timeout=90, **kwargs)


def python_call(client, operation, params):
    params = dict(params)
    if "Code" in params:
        params["Code"] = {"ZipFile": base64.b64decode(params["Code"]["ZipFile"])}
    for key in ("Payload", "ZipFile"):
        if key in params:
            params[key] = base64.b64decode(params[key])
    try:
        result = getattr(client, OPERATIONS[operation])(**params)
        meta = result.pop("ResponseMetadata")
        body = json.loads(result["Payload"].read()) if operation == "Invoke" else result
        return {"status": meta["HTTPStatusCode"], "headers": meta["HTTPHeaders"], "body": body}
    except ClientError as error:
        result = error.response
        meta = result["ResponseMetadata"]
        return {"status": meta["HTTPStatusCode"], "headers": meta["HTTPHeaders"],
                "error": result["Error"]["Code"], "body": {"message": result["Error"]["Message"]}}


def cli_call(operation, params, endpoint, region, work):
    params = dict(params)
    args = ["aws", "--debug", "--region", region, "--no-cli-pager", "--output", "json", "--cli-binary-format", "base64"]
    if endpoint:
        args += ["--endpoint-url", endpoint]
    args += ["lambda", OPERATIONS[operation].replace("_", "-")]
    # fileb bypasses shell/argv size limits and CLI binary-format ambiguity.
    if operation == "Invoke":
        args += ["--function-name", params.pop("FunctionName")]
        payload = work / "payload.bin"
        payload.write_bytes(base64.b64decode(params.pop("Payload")))
        args += ["--payload", f"fileb://{payload}"]
        assert not params, f"unsupported CLI Invoke parameters: {list(params)}"
    else:
        source = work / "request.json"
        source.write_text(json.dumps(params))
        args += ["--cli-input-json", f"file://{source}"]
    output = work / "response.json"
    if operation == "Invoke":
        output.unlink(missing_ok=True)
        args.append(str(output))
    result = subprocess.run(args, text=True, capture_output=True, timeout=90)
    # The pinned CLI exposes wire metadata only in debug logs. Never persist those
    # logs (they include signing data); extract only status and response headers.
    statuses = re.findall(r'HTTP/1\.[01]" (\d{3}) ', result.stderr)
    header_lines = re.findall(r'Response headers: (\{[^\n]*\})', result.stderr)
    if not statuses or not header_lines:
        raise RuntimeError(f"CLI {operation}: no HTTP response (exit {result.returncode})")
    headers = ast.literal_eval(header_lines[-1])
    wire = {"status": int(statuses[-1]), "headers": headers}
    if result.returncode:
        error = re.search(r'An error occurred \(([^)]+)\)[^\n]*?: ([^\n]+)', result.stderr)
        if not error:
            raise RuntimeError(f"CLI {operation}: unrecognized error (exit {result.returncode})")
        return {**wire, "error": error[1], "body": {"message": error[2]}}
    body = json.loads(output.read_text()) if operation == "Invoke" else json.loads(result.stdout or "{}")
    return {**wire, "body": body}


def package(bootstrap, revision):
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, data, mode in [("bootstrap", bootstrap.read_bytes(), 0o755), ("revision.txt", revision.encode(), 0o644)]:
            entry = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            entry.external_attr = (0o100000 | mode) << 16
            entry.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(entry, data)
    return base64.b64encode(stream.getvalue()).decode()


def substitute(value, variables):
    if isinstance(value, dict):
        return {key: substitute(item, variables) for key, item in value.items()}
    if isinstance(value, list):
        return [substitute(item, variables) for item in value]
    return variables.get(value, value) if isinstance(value, str) else value


def project(actual, expected, path="body"):
    """Assert the documented subset, keep stable values for reference comparison."""
    if isinstance(expected, dict):
        if not isinstance(actual, dict):
            raise AssertionError(f"{path}: expected object, got {type(actual).__name__}")
        return {key: project(actual.get(key), value, f"{path}.{key}") for key, value in expected.items()}
    if expected == "$nonempty":
        assert isinstance(actual, str) and actual, f"{path}: expected nonempty string"
        return actual
    if expected == "$array":
        assert isinstance(actual, list), f"{path}: expected array"
        return "$array"
    if expected == "$message":
        assert isinstance(actual, str) and actual, f"{path}: expected an error message"
        return "$message"
    if expected == "$timeout":
        assert isinstance(actual, str) and re.search(r'Task timed out after \d+\.\d+ seconds', actual), f"{path}: {actual!r}"
        return "$timeout"
    assert type(actual) is type(expected) and actual == expected, f"{path}: expected {expected!r}, got {actual!r}"
    return actual


def observe(result, expected, variables):
    assert result["status"] == expected["status"], f"HTTP {result['status']} != {expected['status']}"
    assert result.get("error") == expected.get("error"), f"error: {result.get('error')!r} != {expected.get('error')!r}"
    headers = {key.lower(): value for key, value in result["headers"].items()}
    if "content-type" in headers:
        headers["content-type"] = headers["content-type"].split(";", 1)[0].lower()
    stable = {"status": result["status"], "error": result.get("error"),
              "headers": {key: headers[key] for key in HEADERS if key in headers},
              "body": project(result["body"], substitute(expected.get("body", {}), variables))}
    project(stable["headers"], expected.get("headers", {}), "headers")
    if expected["status"] == 200 and "error" not in expected and "x-amz-function-error" not in expected.get("headers", {}):
        assert "x-amz-function-error" not in headers, "unexpected FunctionError"
    # Names/ARNs are unique per run; preserve all other selected body values.
    reverse = {value: key for key, value in variables.items() if key in ("$name", "$limits", "$arn")}
    return substitute(stable, reverse)


def compare_reference(report, reference):
    assert reference["provider"] == "aws-lambda", "reference must be an AWS capture"
    for field in ("cases_sha256", "fixture_source_sha256", "architecture"):
        assert reference[field] == report[field], f"reference differs: {field}"
    for client in ("aws_cli", "javascript", "boto3", "botocore"):
        assert reference["versions"][client] == report["versions"][client], f"reference uses a different {client} version"
    assert not reference.get("failure"), "reference capture failed"
    assert reference["results"] and all(r["passed"] for r in reference["results"]), "reference contains failed or missing cases"
    assert report["results"], "no local observations"
    expected = {(r["client"], r["case"]): r["observed"] for r in reference["results"] if r["passed"]}
    for record in report["results"]:
        assert record["passed"], f"local contract failed: {record['client']}/{record['case']}"
        assert record["observed"] == expected[(record["client"], record["case"])], f"reference mismatch: {record['client']}/{record['case']}"


@contextlib.contextmanager
def local_daemon(binary, work, region):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    endpoint = f"http://127.0.0.1:{port}"
    config = work / "zapcloud.toml"
    config.write_text(f'''[server]
listen = "127.0.0.1:{port}"
region = "{region}"
[storage]
metadata = "sqlite://{work}/metadata.db"
artifacts = "{work}/artifacts"
runtimes = "{work}/runtimes"
[security]
tenant_trust = "trusted"
[auth]
mode = "sigv4"
''')
    with (work / "daemon.log").open("w") as log:
        process = subprocess.Popen([str(binary), "serve", "--config", str(config)], stdout=log, stderr=log)
        try:
            until = time.monotonic() + 20
            while True:
                if process.poll() is not None:
                    raise RuntimeError(f"daemon exited: {(work / 'daemon.log').read_text()}")
                try:
                    with urllib.request.urlopen(endpoint + "/health/ready", timeout=1) as response:
                        if response.status == 200:
                            break
                except OSError:
                    if time.monotonic() >= until:
                        raise RuntimeError("daemon readiness timeout")
                    time.sleep(0.1)
            yield endpoint
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def matrix(args, work, endpoint, report):
    options = {"region_name": args.region, "config": Config(retries={"total_max_attempts": 1}, parameter_validation=False, read_timeout=30)}
    if endpoint:
        options["endpoint_url"] = endpoint
    sdk = boto3.client("lambda", **options)
    suite = json.loads((HERE / "cases.json").read_text())
    for client in args.clients.split(","):
        name = f"zc-golden-{client}-{uuid.uuid4().hex[:10]}"
        variables = {"$name": name, "$limits": name + "-limits", "$role": args.role,
                     "$architecture": args.architecture,
                     "$arn": f"arn:aws:lambda:{args.region}:{args.role.split(':')[4]}:function:{name}",
                     "$zip": package(args.bootstrap, "v1"), "$zip2": package(args.bootstrap, "v2")}
        created = set()
        def call(operation, params):
            if client == "python":
                return python_call(sdk, operation, params)
            if client == "cli":
                return cli_call(operation, params, endpoint, args.region, work)
            message = {"operation": operation, "params": params, "endpoint": endpoint, "region": args.region}
            return json.loads(command(["node", str(HERE / "javascript.mjs")], input=json.dumps(message)).stdout)
        try:
            for case in suite["cases"]:
                params = substitute(case.get("params", {}), variables)
                if case["operation"] == "CreateFunction":
                    params = {"FunctionName": name, "Role": args.role, "Runtime": "provided.al2023",
                              "Handler": "bootstrap", "MemorySize": 128, "Timeout": 1,
                              "Architectures": [args.architecture], "Code": {"ZipFile": variables["$zip"]}, **params}
                if case["operation"] == "Invoke":
                    payload = case.get("payload", "{}")
                    if "payload_bytes" in case:
                        payload = "{}" + " " * (case["payload_bytes"] - 2)
                    params["Payload"] = base64.b64encode(payload.encode()).decode()
                result = call(case["operation"], params)
                if case["operation"] == "CreateFunction" and result["status"] == 201:
                    created.add(params["FunctionName"])
                if case["operation"] == "DeleteFunction" and result["status"] == 204:
                    created.discard(params["FunctionName"])
                # AWS creates/updates asynchronously; this wait is outside the observation.
                if args.aws and case["operation"] in ("CreateFunction", "UpdateFunctionCode") and result["status"] < 300:
                    waiter = "function_active_v2" if case["operation"] == "CreateFunction" else "function_updated_v2"
                    sdk.get_waiter(waiter).wait(FunctionName=params["FunctionName"], WaiterConfig={"Delay": 2, "MaxAttempts": 60})
                record = {"client": client, "case": case["id"]}
                try:
                    record["observed"] = observe(result, case["expect"], variables)
                    record["passed"] = True
                except AssertionError as error:
                    record.update(passed=False, mismatch=str(error), status=result["status"], error=result.get("error"))
                report["results"].append(record)
                print(f"{'PASS' if record['passed'] else 'FAIL'} {client}/{case['id']}" + (": " + record["mismatch"] if not record["passed"] else ""), flush=True)
        finally:
            for function in created:
                result = python_call(sdk, "DeleteFunction", {"FunctionName": function})
                if result["status"] not in (204, 404):
                    raise RuntimeError(f"cleanup failed for {function}: HTTP {result['status']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clients", default="cli,javascript,python")
    parser.add_argument("--daemon", type=Path, default=ROOT / "target/debug/zapcloud")
    parser.add_argument("--bootstrap", type=Path)
    parser.add_argument("--report", type=Path, default=HERE / "results/local.json")
    parser.add_argument("--reference", type=Path, help="Compare against a reviewed report captured with --aws")
    parser.add_argument("--aws", action="store_true", help="Explicit opt-in: creates/invokes/deletes temporary functions in AWS (billable)")
    parser.add_argument("--role", default="arn:aws:iam::000000000000:role/golden")
    parser.add_argument("--region", default="us-east-1")
    parser.add_argument("--architecture", choices=["arm64", "x86_64"], default="arm64" if platform.machine() in ("arm64", "aarch64") else "x86_64")
    args = parser.parse_args()
    if any(client not in ("cli", "javascript", "python") for client in args.clients.split(",")):
        parser.error("unknown client")
    if args.aws and (args.role == "arn:aws:iam::000000000000:role/golden" or args.bootstrap is None):
        parser.error("--aws requires an explicit --role and --bootstrap compatible with AL2023")
    args.bootstrap = args.bootstrap or ROOT / "target/debug/api_test_bootstrap"
    if not re.fullmatch(r"[a-z0-9-]+", args.region) or not re.fullmatch(r"arn:aws:iam::\d{12}:role/[\w+=,.@/-]+", args.role):
        parser.error("invalid region or role ARN")
    if not args.aws and args.role != "arn:aws:iam::000000000000:role/golden":
        parser.error("local tests use only the dummy role")
    report = {"schema": 1, "provider": "aws-lambda" if args.aws else "zapcloud",
              "reference_status": "capture-requires-review" if args.aws else "not-captured",
              "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "cases_sha256": hashlib.sha256((HERE / "cases.json").read_bytes()).hexdigest(),
              "fixture_sha256": hashlib.sha256(args.bootstrap.read_bytes()).hexdigest(),
              "fixture_source_sha256": hashlib.sha256((ROOT / "zapcloud-functions/api-lambda/src/bin/api_test_bootstrap.rs").read_bytes()).hexdigest(),
              "region": args.region, "architecture": args.architecture,
              "versions": {"boto3": boto3.__version__, "botocore": botocore.__version__, "node": command(["node", "--version"]).stdout.strip(),
                           "aws_cli": command(["aws", "--version"]).stdout.strip().split()[0],
                           "javascript": json.loads((HERE / "node_modules/@aws-sdk/client-lambda/package.json").read_text())["version"]},
              "results": []}
    failure = None
    with tempfile.TemporaryDirectory(prefix="zc-golden-") as temporary:
        work = Path(temporary)
        # Ignore ambient profiles/endpoints in local mode. No real credentials or
        # network AWS access are needed by either the daemon or the three clients.
        if not args.aws:
            for key in list(os.environ):
                if key.startswith("AWS_") or key.upper().endswith("_PROXY"):
                    del os.environ[key]
            os.environ.update(AWS_ACCESS_KEY_ID="golden", AWS_SECRET_ACCESS_KEY="golden-secret",
                              AWS_CONFIG_FILE=str(work / "aws-config"), AWS_SHARED_CREDENTIALS_FILE=str(work / "credentials"),
                              AWS_EC2_METADATA_DISABLED="true", AWS_MAX_ATTEMPTS="1")
            (work / "aws-config").write_text("[default]\nparameter_validation = false\ncli_binary_format = base64\n")
        if args.aws:
            # Preserve profiles/SSO without modifying the user's config. Disable
            # client-side range validation so boundary cases reach the service.
            config = configparser.RawConfigParser()
            config.read(os.environ.get("AWS_CONFIG_FILE", str(Path.home() / ".aws/config")))
            if not config.has_section("default"):
                config.add_section("default")
            for section in config.sections():
                config.set(section, "parameter_validation", "false")
            with (work / "aws-config").open("w") as destination:
                config.write(destination)
            os.environ["AWS_CONFIG_FILE"] = str(work / "aws-config")
        os.environ.update(AWS_PAGER="", AWS_MAX_ATTEMPTS="1")
        try:
            target = contextlib.nullcontext(f"https://lambda.{args.region}.amazonaws.com") if args.aws else local_daemon(args.daemon, work, args.region)
            with target as endpoint:
                matrix(args, work, endpoint, report)
            if args.reference:
                reference = json.loads(args.reference.read_text())
                compare_reference(report, reference)
                report["reference_status"] = "matched"
        except Exception as error:
            failure = f"{type(error).__name__}: {error}"
            report["failure"] = failure
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    failed = sum(not result["passed"] for result in report["results"])
    print(f"Report: {args.report}; {len(report['results']) - failed} passed, {failed} failed; AWS reference: {report['reference_status']}")
    if failure:
        print(failure, file=sys.stderr)
    return 1 if failure or failed else 0


if __name__ == "__main__":
    sys.exit(main())
