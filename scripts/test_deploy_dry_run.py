#!/usr/bin/env python3
"""Exercise deploy_testnet.sh offline with a mocked Soroban RPC endpoint."""

import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
from http.server import ThreadingHTTPServer


ROOT = Path(__file__).resolve().parents[1]
RPC_MODULE_PATH = ROOT / "scripts" / "mock-soroban-rpc.py"


def load_rpc_module():
    spec = importlib.util.spec_from_file_location("mock_soroban_rpc", RPC_MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Unable to load RPC mock from {RPC_MODULE_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_executable(path, content):
    path.write_text(content, encoding="utf-8")
    path.chmod(0o755)


def run_test():
    rpc_mock = load_rpc_module()
    server = ThreadingHTTPServer(
        ("127.0.0.1", 0),
        rpc_mock.make_handler(rpc_mock.DEFAULT_RESPONSES),
    )
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()

    try:
        with tempfile.TemporaryDirectory(prefix="deploy-dry-run-") as temp_dir:
            temp = Path(temp_dir)
            fake_bin = temp / "bin"
            fake_bin.mkdir()
            isolated_repo = temp / "repo"
            (isolated_repo / "scripts").mkdir(parents=True)
            deployment_script = isolated_repo / "scripts" / "deploy_testnet.sh"
            script_text = (ROOT / "scripts" / "deploy_testnet.sh").read_text(
                encoding="utf-8"
            )
            cargo_env_line = "source ~/.cargo/env 2>/dev/null || true"
            if cargo_env_line not in script_text:
                raise AssertionError("deployment script changed; cannot safely mock curl")
            script_text = script_text.replace(
                cargo_env_line,
                "curl() { python \"$CURL_MOCK_SCRIPT\" \"$@\"; }\n"
                "sleep() { :; }\n"
                + cargo_env_line,
                1,
            )
            deployment_script.write_text(script_text, encoding="utf-8")

            calls_log = temp / "stellar-calls.jsonl"
            curl_log = temp / "curl-calls.jsonl"
            balance_count = temp / "balance-count"
            rpc_url = f"http://127.0.0.1:{server.server_port}/soroban/rpc"

            write_executable(
                fake_bin / "stellar",
                """#!/usr/bin/env python
import json
import os
import sys
from urllib.request import Request, urlopen

args = sys.argv[1:]
with open(os.environ["STELLAR_CALLS_LOG"], "a", encoding="utf-8") as log:
    log.write(json.dumps(args) + "\\n")

for index, method in enumerate(("getHealth", "getLatestLedger"), start=1):
    request = Request(
        os.environ["MOCK_SOROBAN_RPC_URL"],
        data=json.dumps({"jsonrpc": "2.0", "id": index, "method": method}).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urlopen(request, timeout=3) as response:
        result = json.load(response)["result"]
    if method == "getHealth" and result.get("status") != "healthy":
        raise SystemExit("mock RPC health check failed")
    if method == "getLatestLedger" and result.get("sequence") != 1:
        raise SystemExit("unexpected mock RPC ledger")

if args[:2] == ["keys", "generate"]:
    pass
elif args[:2] == ["keys", "address"]:
    print("GTESTDEPLOYERADDRESS")
elif args[:2] == ["contract", "deploy"]:
    wasm = args[args.index("--wasm") + 1]
    ids = {
        "asset_registry.wasm": "CASSETREGISTRY",
        "engineer_registry.wasm": "CENGINEERREGISTRY",
        "lifecycle.wasm": "CLIFECYCLE",
    }
    print(ids[wasm.rsplit("/", 1)[-1]])
elif args[:2] == ["contract", "invoke"]:
    pass
else:
    raise SystemExit("unexpected stellar command: " + repr(args))
""",
            )

            curl_mock = temp / "curl_mock.py"
            curl_mock.write_text(
                """import json
import os
import sys

url = sys.argv[-1]
with open(os.environ["CURL_CALLS_LOG"], "a", encoding="utf-8") as log:
    log.write(json.dumps(url) + "\\n")

if url.startswith("https://horizon-testnet.stellar.org/accounts/"):
    count_file = os.environ["BALANCE_COUNT_FILE"]
    try:
        count = int(open(count_file, encoding="utf-8").read())
    except FileNotFoundError:
        count = 0
    count += 1
    with open(count_file, "w", encoding="utf-8") as count_output:
        count_output.write(str(count))
    balance = "0.0000000" if count == 1 else "100.0000000"
    print(json.dumps({"balances": [{"balance": balance}]}, separators=(",", ":")))
elif url.startswith("https://friendbot.stellar.org/?addr="):
    print("funded")
else:
    raise SystemExit("unexpected external HTTP request: " + url)
""",
                encoding="utf-8",
            )
            env = os.environ.copy()
            env.update(
                {
                    "PATH": str(fake_bin) + os.pathsep + env.get("PATH", ""),
                    "HOME": str(temp / "home"),
                    "MOCK_SOROBAN_RPC_URL": rpc_url,
                    "STELLAR_CALLS_LOG": str(calls_log),
                    "CURL_CALLS_LOG": str(curl_log),
                    "BALANCE_COUNT_FILE": str(balance_count),
                    "CURL_MOCK_SCRIPT": str(curl_mock),
                    "STELLAR_NETWORK": "testnet",
                }
            )
            (temp / "home").mkdir()

            result = subprocess.run(
                ["bash", str(deployment_script)],
                cwd=isolated_repo,
                env=env,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
            if result.returncode:
                raise AssertionError(
                    f"deployment script exited {result.returncode}\n"
                    f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}\n"
                    "mock HTTP calls:\n"
                    f"{curl_log.read_text(encoding='utf-8') if curl_log.exists() else '(none)'}"
                )

            stellar_calls = [
                json.loads(line)
                for line in calls_log.read_text(encoding="utf-8").splitlines()
            ]
            curl_calls = [
                json.loads(line) for line in curl_log.read_text(encoding="utf-8").splitlines()
            ]
            if stellar_calls[:2] != [
                ["keys", "generate", "deployer", "--network", "testnet"],
                ["keys", "address", "deployer"],
            ]:
                raise AssertionError(f"unexpected deployer setup calls: {stellar_calls[:2]}")
            expected_invocations = [
                "initialize_admin",
                "initialize_admin",
                "initialize",
            ]
            invocations = [
                call[call.index("--") + 1]
                for call in stellar_calls
                if call[:2] == ["contract", "invoke"]
            ]
            deploy_wasm = [
                call[call.index("--wasm") + 1].rsplit("/", 1)[-1]
                for call in stellar_calls
                if call[:2] == ["contract", "deploy"]
            ]
            if deploy_wasm != [
                "asset_registry.wasm",
                "engineer_registry.wasm",
                "lifecycle.wasm",
            ]:
                raise AssertionError(f"unexpected contract deployment order: {deploy_wasm}")
            if invocations != expected_invocations:
                raise AssertionError(f"unexpected initialization calls: {invocations}")
            initialized_ids = [
                call[call.index("--id") + 1]
                for call in stellar_calls
                if call[:2] == ["contract", "invoke"]
            ]
            if initialized_ids != ["CASSETREGISTRY", "CENGINEERREGISTRY", "CLIFECYCLE"]:
                raise AssertionError(f"unexpected initialization targets: {initialized_ids}")
            if any(
                "--network" in call and call[call.index("--network") + 1] != "testnet"
                for call in stellar_calls
            ):
                raise AssertionError("a Stellar CLI call did not target testnet")
            if len(curl_calls) != 3 or not any(
                "friendbot.stellar.org" in url for url in curl_calls
            ):
                raise AssertionError(f"unexpected mocked HTTP requests: {curl_calls}")
            deployment_env = (isolated_repo / "deployments" / "testnet.env").read_text(
                encoding="utf-8"
            )
            expected_env = (
                "CONTRACT_ASSET_REGISTRY=CASSETREGISTRY\n"
                "CONTRACT_ENGINEER_REGISTRY=CENGINEERREGISTRY\n"
                "CONTRACT_LIFECYCLE=CLIFECYCLE\n"
            )
            if deployment_env != expected_env:
                raise AssertionError(f"unexpected deployment output:\n{deployment_env}")

            print("PASS: deployment script completed offline against mocked RPC and HTTP endpoints")
    finally:
        server.shutdown()
        server.server_close()
        server_thread.join(timeout=3)


if __name__ == "__main__":
    run_test()
