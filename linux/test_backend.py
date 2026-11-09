import base64, copy, json, os, tempfile, unittest
from pathlib import Path
from unittest.mock import patch
from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives import padding
import hashlib
import backend


class LinuxBackendTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.vault = backend.Vault(self.root / "vault", b"x" * 32)

    def tearDown(self):
        self.temp.cleanup()

    def test_vault_is_authenticated_and_private(self):
        self.vault.write_json("profiles/one.bin", {"account": "sample"})
        path = self.vault.root / "profiles/one.bin"
        self.assertNotIn(b"sample", path.read_bytes())
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        data = bytearray(path.read_bytes())
        data[-1] ^= 1
        path.write_bytes(data)
        with self.assertRaises(InvalidTag):
            self.vault.read_json("profiles/one.bin")

    def test_paths_cannot_escape_vault(self):
        for name in ("../escape", "/tmp/escape", "a/../../escape", "C:\\secret", ""):
            with self.assertRaises(ValueError):
                self.vault.write(name, b"x")

    def test_draft_not_saved_with_preferences_and_discarded(self):
        service = backend.Service(vault=self.vault, network=False)
        service.handle("add")
        service.handle("settings", {"theme": 2, "float_scale": 0.6})
        loaded = backend.Service(vault=self.vault, network=False)
        self.assertEqual(loaded.settings["claude"], [])
        self.assertEqual(loaded.settings["theme"], 2)
        service.handle("discard")
        self.assertEqual(service.settings["claude"], [])

    def test_provider_accounts_are_separate(self):
        service = backend.Service(preview=True)
        service.handle("provider", {"provider": "codex"})
        service.handle("rename", {"slot": 1, "name": "Development"})
        service.handle("provider", {"provider": "claude"})
        self.assertEqual(service.state()["accounts"][0]["name"], "Personal")
        service.handle("provider", {"provider": "codex"})
        self.assertEqual(service.state()["accounts"][0]["name"], "Development")

    def test_busy_operation_prevents_draft_index_changes(self):
        service = backend.Service(preview=True)
        service.handle("add")
        service.busy = True
        with self.assertRaises(RuntimeError):
            service.handle("discard")
        self.assertEqual(len(service.settings["claude"]), 3)

    def test_stale_provider_operations_are_rejected(self):
        service = backend.Service(preview=True)
        with self.assertRaises(ValueError):
            service.handle("save", {"slot": 1, "provider": "codex", "confirmed": True})
        self.assertEqual(service.settings["provider"], "claude")

    def test_python_310_policy_fallback_stays_fail_closed(self):
        with patch.object(backend, "tomllib", None):
            self.assertTrue(backend.file_store_policy(""))
            self.assertTrue(
                backend.file_store_policy('cli_auth_credentials_store = "file"')
            )
            self.assertFalse(
                backend.file_store_policy('cli_auth_credentials_store = "keyring"')
            )

    def test_preview_switch_and_signed_out_drafts_have_correct_active_state(self):
        service = backend.Service(preview=True)
        service.handle("switch", {"slot": 2, "confirmed": True})
        self.assertTrue(service.state()["accounts"][1]["active"])
        self.assertFalse(service.state()["accounts"][0]["active"])
        other = backend.Service(vault=self.vault, network=False)
        with patch.object(other, "current", return_value=None):
            other.handle("add")
            self.assertFalse(other.state()["accounts"][0]["active"])

    def test_confirmation_required(self):
        service = backend.Service(preview=True)
        for action in ("save", "switch", "remove"):
            with self.assertRaises(ValueError):
                service.handle(action, {"slot": 1})

    def test_codex_save_switch_and_rollback_without_touching_work(self):
        home = self.root / "codex"
        home.mkdir()
        auth = home / "auth.json"
        work = home / "sessions"
        work.mkdir()
        (work / "keep").write_text("work")
        first = {"tokens": {"account_id": "sample-one", "access_token": "sample"}}
        second = {"tokens": {"account_id": "sample-two", "access_token": "sample"}}
        with patch.dict(os.environ, {"CODEX_HOME": str(home)}):
            backend.atomic(auth, backend.encoded(first))
            service = backend.Service(vault=self.vault, network=False)
            service.handle("provider", {"provider": "codex"})
            service.handle("add")
            service.handle("save", {"slot": 1, "confirmed": True})
            self.assertFalse(service.settings["codex"][0]["draft"])
            backend.atomic(auth, backend.encoded(second))
            service.handle("switch", {"slot": 1, "confirmed": True})
            self.assertEqual(json.loads(auth.read_bytes()), first)
            service.handle("rollback", {"confirmed": True})
            self.assertEqual(json.loads(auth.read_bytes()), second)
            self.assertEqual((work / "keep").read_text(), "work")

    def test_codex_limits_missing_is_not_zero(self):
        value = backend.codex_limits(
            {
                "rateLimits": {
                    "primary": {"usedPercent": 59, "resetsAt": 2000000000},
                    "secondary": None,
                }
            }
        )
        self.assertEqual(value["session"]["used"], 59)
        self.assertIsNone(value["weekly"])
        with self.assertRaises(ValueError):
            backend.codex_limits({"rateLimits": {"primary": {"usedPercent": 500}}})

    def test_claude_cache_account_binding(self):
        cache = {
            "acct:mine|x:https://api.anthropic.com:user:profile": {"token": "sample"},
            "acct:other|x:https://api.anthropic.com:user:profile": {"token": "wrong"},
        }
        key = hashlib.pbkdf2_hmac("sha1", b"peanuts", b"saltysalt", 1, 16)
        pad = padding.PKCS7(128).padder()
        plain = pad.update(backend.encoded(cache)) + pad.finalize()
        encrypt = Cipher(algorithms.AES(key), modes.CBC(b" " * 16)).encryptor()
        value = base64.b64encode(
            b"v10" + encrypt.update(plain) + encrypt.finalize()
        ).decode()
        token, _ = backend.claude_token(
            {"lastKnownAccountUuid": "mine", "oauth:tokenCache": value}
        )
        self.assertEqual(token, "sample")
        with self.assertRaises(ValueError):
            backend.claude_token(
                {"lastKnownAccountUuid": "missing", "oauth:tokenCache": value}
            )

    def test_checkpoint_recovers_to_separate_folder(self):
        source = self.root / "claude"
        source.mkdir()
        (source / "config.json").write_text("local")
        self.vault.checkpoint(source)
        (source / "config.json").write_text("live")
        target = Path(self.vault.recover())
        self.assertEqual((target / "config.json").read_text(), "local")
        self.assertEqual((source / "config.json").read_text(), "live")

    def test_backup_refuses_symlinks(self):
        source = self.root / "claude"
        source.mkdir()
        (source / "linked").symlink_to(self.root, target_is_directory=True)
        with self.assertRaises(ValueError):
            self.vault.checkpoint(source)

    def test_claude_restore_preserves_unrelated_preferences(self):
        config = self.root / "config"
        claude = config / "Claude"
        claude.mkdir(parents=True)
        (claude / "config.json").write_text(
            json.dumps(
                {
                    "lastKnownAccountUuid": "one",
                    "theme": "dark",
                    "oauth:tokenCache": "sample",
                }
            )
        )
        (claude / "Local State").write_text("key state")
        with patch.dict(os.environ, {"XDG_CONFIG_HOME": str(config)}):
            snapshot = backend.capture_claude()
            snapshot["config"]["lastKnownAccountUuid"] = "two"
            backend.restore_claude(snapshot)
            result = json.loads((claude / "config.json").read_bytes())
            self.assertEqual(result["lastKnownAccountUuid"], "two")
            self.assertEqual(result["theme"], "dark")


if __name__ == "__main__":
    unittest.main()
