import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("export-network-config.py")
CONFIG = Path(__file__).resolve().parents[1] / "environments.toml"


class ExportNetworkConfigTests(unittest.TestCase):
    def export(self, format_name, network="mainnet", config=CONFIG):
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--format",
                format_name,
                "--config",
                str(config),
                network,
            ],
            check=False,
            capture_output=True,
            text=True,
        )

    def test_shell_exports_selected_network_settings(self):
        result = self.export("shell", "testnet")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("https://soroban-testnet.stellar.org", result.stdout)
        self.assertIn("Test SDF Network ; September 2015", result.stdout)

    def test_terraform_export_uses_region_values_from_toml(self):
        result = self.export("terraform")

        self.assertEqual(result.returncode, 0, result.stderr)
        values = json.loads(result.stdout)
        self.assertEqual(
            values["rpc_urls"]["eu-west-1"],
            "https://soroban-mainnet-eu-west.stellar.org",
        )
        self.assertEqual(values["vpc_cidrs"]["ap-southeast-1"], "10.2.0.0/16")
        self.assertEqual(
            values["network_passphrase"],
            "Public Global Stellar Network ; September 2015",
        )

    def test_rejects_unknown_network(self):
        result = self.export("shell", "unknown")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not defined", result.stderr)


if __name__ == "__main__":
    unittest.main()
