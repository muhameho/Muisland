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

    def test_position_survives_restart_and_rejects_nonfinite_values(self):
        service = backend.Service(vault=self.vault, network=False)
        service.handle("position", {"position": [-1880, 320]})
        loaded = backend.Service(vault=self.vault, network=False)
        self.assertEqual(loaded.settings["position"], [-1880, 320])
        for value in ([float("nan"), 0], [float("inf"), 0], [0], [True, 0]):
            with self.assertRaises(ValueError):
                service.handle("position", {"position": value})

    def test_clamping_handles_negative_monitors_and_removed_screens(self):
        self.assertEqual(
            backend.clamped_position([-2000, -500], [76, 76], [-1920, 0, 1920, 1080]),
            (-1912, 8),
        )
        self.assertEqual(
            backend.clamped_position([10000, 10000], [76, 76], [0, 0, 1280, 720]),
            (1196, 636),
        )
        self.assertEqual(
            backend.clamped_position([-1900, 300], [76, 76], [0, 0, 1280, 720]),
            (8, 300),
        )

    def test_missing_key_cannot_replace_key_for_existing_profiles(self):
        self.vault.write_json("settings.bin", {"theme": 2})
        vault = backend.Vault(self.vault.root)
        with patch.object(
            backend.shutil, "which", return_value="secret-tool"
        ), patch.object(backend.subprocess, "run") as run:
            run.return_value.stdout = b""
            with self.assertRaisesRegex(RuntimeError, "original desktop keyring"):
                vault.cipher()
            self.assertEqual(run.call_count, 1)
        self.assertEqual(self.vault.read_json("settings.bin"), {"theme": 2})

    def test_autostart_can_be_disabled_without_touching_profiles(self):
        with patch.dict(os.environ, {"XDG_CONFIG_HOME": str(self.root)}):
            self.assertTrue(backend.set_autostart(True))
            self.assertIn("--background", backend.autostart_path().read_text())
            self.assertFalse(backend.set_autostart(False))

    def test_claude_save_reopens_after_capture_failure_without_overwriting_profile(
        self,
    ):
        service = backend.Service(vault=self.vault, network=False)
        service.handle("add")
        self.vault.write_json("claude/account-1.bin", {"previous": True})
        with patch.object(
            backend, "claude_config", return_value={"lastKnownAccountUuid": "first"}
        ), patch.object(
            backend, "claude_executable", return_value="/opt/claude/claude-desktop"
        ), patch.object(
            backend, "claude_processes", return_value=[123]
        ), patch.object(
            backend, "close_claude"
        ) as close, patch.object(
            backend, "capture_claude", side_effect=ValueError("unsupported format")
        ), patch.object(
            backend, "launch_claude"
        ) as reopen:
            with self.assertRaisesRegex(ValueError, "unsupported format"):
                service.handle("save", {"slot": 1, "confirmed": True})
            close.assert_called_once()
            reopen.assert_called_once_with("/opt/claude/claude-desktop")
        self.assertEqual(
            self.vault.read_json("claude/account-1.bin"), {"previous": True}
        )
        self.assertFalse(service.busy)
        self.assertTrue(service.settings["claude"][0]["draft"])

    def test_claude_save_never_closes_app_if_relaunch_is_unavailable(self):
        service = backend.Service(vault=self.vault, network=False)
        service.handle("add")
        with patch.object(
            backend, "claude_config", return_value={"lastKnownAccountUuid": "first"}
        ), patch.object(backend, "claude_executable", return_value=None), patch.object(
            backend, "close_claude"
        ) as close:
            with self.assertRaises(FileNotFoundError):
                service.handle("save", {"slot": 1, "confirmed": True})
            close.assert_not_called()

    def encrypt_cache(self, value, prefix=b"v10", password=b"peanuts"):
        key = hashlib.pbkdf2_hmac("sha1", password, b"saltysalt", 1, 16)
        pad = padding.PKCS7(128).padder()
        data = pad.update(backend.encoded(value)) + pad.finalize()
        cipher = Cipher(algorithms.AES(key), modes.CBC(b" " * 16)).encryptor()
        return base64.b64encode(
            prefix + cipher.update(data) + cipher.finalize()
        ).decode()

    def test_new_cache_failure_does_not_hide_valid_legacy_usage_token(self):
        cache = {
            "acct:mine|x:https://api.anthropic.com:user:profile": {"token": "sample"}
        }
        config = {
            "lastKnownAccountUuid": "mine",
            "oauth:tokenCacheV2": base64.b64encode(b"v20unsupported").decode(),
            "oauth:tokenCache": self.encrypt_cache(cache),
        }
        self.assertEqual(backend.claude_token(config)[0], "sample")
        config["lastKnownAccountUuid"] = "different"
        with self.assertRaises(backend.UsageError) as error:
            backend.claude_token(config)
        self.assertEqual(error.exception.code, "token_missing")

    def test_v11_secret_lookup_is_scoped_to_claude_and_decrypts(self):
        cache = {
            "acct:mine|x:https://api.anthropic.com:user:profile": {"token": "sample"}
        }
        value = self.encrypt_cache(cache, b"v11", b"sample-safe-storage")

        def lookup(command, **kwargs):
            self.assertEqual(command[:3], ["secret-tool", "lookup", "application"])
            self.assertIn(command[3], ("claude", "Claude", "claude-desktop"))
            return type("Result", (), {"stdout": b"sample-safe-storage\n"})()

        with patch.object(backend.subprocess, "run", side_effect=lookup):
            self.assertEqual(
                backend.claude_token(
                    {"lastKnownAccountUuid": "mine", "oauth:tokenCacheV2": value}
                )[0],
                "sample",
            )

    def test_expired_iso_token_is_not_sent(self):
        value = self.encrypt_cache(
            {
                "acct:mine|x:https://api.anthropic.com:user:profile": {
                    "token": "sample",
                    "expiresAt": "2000-01-01T00:00:00Z",
                }
            }
        )
        with self.assertRaises(backend.UsageError) as error:
            backend.claude_token(
                {"lastKnownAccountUuid": "mine", "oauth:tokenCache": value}
            )
        self.assertEqual(error.exception.code, "token_expired")

    def test_usage_errors_and_diagnostics_never_export_credentials(self):
        from urllib.error import HTTPError

        for error in (
            ValueError("do-not-expose-token"),
            HTTPError(
                "https://api.anthropic.com/api/oauth/usage",
                403,
                "do-not-expose-token",
                {},
                None,
            ),
        ):
            message = backend.usage_error_message(error)
            self.assertNotIn("do-not-expose-token", message)
        self.assertIn(
            "HTTP 403",
            backend.usage_error_message(
                HTTPError("https://api.anthropic.com", 403, "", {}, None)
            ),
        )
        config = {
            "lastKnownAccountUuid": "private-account-id",
            "oauth:tokenCache": self.encrypt_cache(
                {
                    "acct:private-account-id|x:https://api.anthropic.com:user:profile": {
                        "token": "private-token"
                    }
                }
            ),
        }
        with patch.object(backend, "claude_config", return_value=config):
            report = backend.claude_diagnostics()
        self.assertEqual(report["credential_status"], "ready_for_usage_request")
        self.assertNotIn("private-account-id", json.dumps(report))
        self.assertNotIn("private-token", json.dumps(report))


if __name__ == "__main__":
    unittest.main()
