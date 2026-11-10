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

    def test_auto_switch_rejects_stale_and_missing_quotas(self):
        service = backend.Service(preview=True)
        rows = service.settings["claude"]
        rows[0]["usage"]["session"]["used"] = 90
        self.assertEqual(service.auto_candidate()["slot"], 2)
        rows[1]["usage"]["updated"] = 0
        self.assertIsNone(service.auto_candidate())
        rows[1]["usage"]["updated"] = backend.time.time()
        rows[1]["usage"]["weekly"] = None
        self.assertIsNone(service.auto_candidate())

    def test_auto_switch_countdown_and_cancel_are_enforced(self):
        service = backend.Service(preview=True)
        service.settings.update(auto_switch=True, pause_for_work=False)
        service.settings["claude"][0]["usage"]["session"]["used"] = 90
        service.check_limits()
        alert = service.state()["usage_alert"]
        self.assertNotIn("source", alert)
        with self.assertRaises(ValueError):
            service.handle("auto_execute", {"id": alert["id"]})
        service.handle("auto_cancel")
        self.assertIsNone(service.state()["usage_alert"])
        service.check_limits()
        self.assertIsNone(service.state()["usage_alert"])

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
            backend, "claude_current_identity", return_value="first"
        ), patch.object(
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
            backend, "claude_current_identity", return_value="first"
        ), patch.object(
            backend, "claude_config", return_value={"lastKnownAccountUuid": "first"}
        ), patch.object(
            backend, "claude_executable", return_value=None
        ), patch.object(
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
        self.assertEqual(error.exception.code, "account_absent")
        config["lastKnownAccountUuid"] = "mine"
        config["oauth:tokenCache"] = self.encrypt_cache(
            {"acct:mine|x:https://api.anthropic.com:user:inference": {"token": "no"}}
        )
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

    def claude_home(self, account, cookie=True):
        import sqlite3

        home = self.root / "claude-home"
        (home / "Network").mkdir(parents=True, exist_ok=True)
        (home / "Local Storage").mkdir(exist_ok=True)
        (home / "Local Storage" / "data").write_bytes(account.encode())
        (home / "config.json").write_bytes(
            backend.encoded({"lastKnownAccountUuid": account, "theme": "dark"})
            if account
            else backend.encoded({"theme": "dark"})
        )
        (home / "os-entry-points.json").write_bytes(
            backend.encoded({"account": account, "org": "o", "enabled": True})
        )
        import time

        os.utime(
            home / "os-entry-points.json", (time.time() - 3600, time.time() - 3600)
        )
        cookies = home / "Network" / "Cookies"
        if cookies.exists():
            cookies.unlink()
        db = sqlite3.connect(cookies)
        db.execute(
            "create table cookies (host_key text, name text, encrypted_value blob)"
        )
        if cookie:
            db.execute("insert into cookies values ('.claude.ai', 'sessionKey', x'01')")
        db.commit()
        db.close()
        return home

    def claude_patches(self, home):
        return (
            patch.dict(os.environ, {"MUISLAND_CLAUDE_CONFIG": str(home)}),
            patch.object(
                backend, "claude_executable", return_value="/opt/claude/claude"
            ),
            patch.object(backend, "claude_processes", return_value=[]),
            patch.object(backend, "close_claude"),
            patch.object(backend, "launch_claude"),
        )

    def test_saving_a_second_slot_opens_sign_in_without_duplicating_or_signing_out(
        self,
    ):
        home = self.claude_home("first")
        service = backend.Service(vault=self.vault, network=False)
        patches = self.claude_patches(home)
        with patches[0], patches[1], patches[2], patches[3], patches[4] as reopen:
            service.handle("add")
            service.handle("save", {"slot": 1, "confirmed": True})
            service.handle("add")
            state = service.handle("save", {"slot": 2, "confirmed": True})
            self.assertIn("sign-in screen", state["notice"])
            self.assertEqual(reopen.call_count, 2)
            # Slot 1 keeps a restorable session; slot 2 is still an unsaved draft.
            saved = self.vault.read_json("claude/account-1.bin")
            self.assertEqual(saved["config"]["lastKnownAccountUuid"], "first")
            self.assertIn("Network/Cookies", saved["files"])
            self.assertFalse(self.vault.exists("claude/account-2.bin"))
            self.assertTrue(service.settings["claude"][1]["draft"])
            # Only this computer forgot the sign-in; unrelated preferences stay.
            live = json.loads((home / "config.json").read_bytes())
            self.assertEqual(live, {"theme": "dark"})
            self.assertFalse((home / "Network").exists())
            # The user signs in to another account and saves it normally.
            self.claude_home("second")
            state = service.handle("save", {"slot": 2, "confirmed": True})
            self.assertNotIn("notice", state)
            self.assertEqual(service.settings["claude"][1]["identity"], "second")

    def test_switch_refreshes_the_account_being_left_and_works_when_signed_out(self):
        home = self.claude_home("first")
        service = backend.Service(vault=self.vault, network=False)
        patches = self.claude_patches(home)
        with patches[0], patches[1], patches[2], patches[3], patches[4]:
            service.handle("add")
            service.handle("save", {"slot": 1, "confirmed": True})
            self.claude_home("second")
            service.handle("add")
            service.handle("save", {"slot": 2, "confirmed": True})
            # Claude renewed the second account's session after it was saved.
            (home / "Local Storage" / "data").write_bytes(b"second-renewed")
            service.handle("switch", {"slot": 1, "confirmed": True})
            self.assertEqual(service.current("claude"), "first")
            renewed = self.vault.read_json("claude/account-2.bin")
            self.assertEqual(
                base64.b64decode(renewed["files"]["Local Storage/data"]),
                b"second-renewed",
            )
            # A signed-out Claude must neither block a switch nor replace a good copy.
            self.claude_home("first", cookie=False)
            (home / "Local Storage" / "data").write_bytes(b"signed-out")
            service.handle("switch", {"slot": 2, "confirmed": True})
            self.assertEqual(service.current("claude"), "second")
            kept = self.vault.read_json("claude/account-1.bin")
            self.assertEqual(
                base64.b64decode(kept["files"]["Local Storage/data"]), b"first"
            )
            self.claude_home("", cookie=False)
            service.handle("switch", {"slot": 1, "confirmed": True})
            self.assertEqual(service.current("claude"), "first")

    def test_rate_limited_active_account_shows_claudes_own_local_sample(self):
        import time
        from urllib.error import HTTPError

        home = self.claude_home("first")
        config = json.loads((home / "config.json").read_bytes())
        config["oauth:tokenCache"] = "mocked-api-credential-fixture"
        (home / "config.json").write_bytes(backend.encoded(config))
        service = backend.Service(vault=self.vault, network=True)
        patches = self.claude_patches(home)
        limited = HTTPError(
            "https://api.anthropic.com", 429, "", {"Retry-After": "1800"}, None
        )
        with patches[0], patches[1], patches[2], patches[3], patches[4]:
            service.handle("add")
            service.handle("save", {"slot": 1, "confirmed": True})
            with patch.object(backend, "claude_usage", side_effect=limited) as fetch:
                service.poll()
                row = service.state()["accounts"][0]
                self.assertIn("HTTP 429", row["error"])
                self.assertIsNone(row["usage"])
                # The server's pause is respected; no second request is sent.
                service.poll()
                self.assertEqual(fetch.call_count, 1)
                self.assertGreater(
                    service.next_poll[("claude", 1)] - time.monotonic(), 1700
                )
                (home / "plan-usage-history.json").write_bytes(
                    backend.encoded(
                        {
                            "version": 2,
                            "samples": [
                                {"t": 1000, "org": "o", "u": {"fh": 99, "sd": 99}},
                                {
                                    "t": int(time.time() * 1000),
                                    "org": "o",
                                    "u": {"fh": 41, "sd": 17},
                                },
                            ],
                        }
                    )
                )
                service.poll()
                self.assertEqual(fetch.call_count, 1)
            row = service.state()["accounts"][0]
            self.assertIn("HTTP 429", row["error"])
            self.assertEqual(row["usage"]["session"]["used"], 41)
            self.assertEqual(row["usage"]["weekly"]["used"], 17)
            # A rejected session is never papered over with local numbers.
            denied = HTTPError("https://api.anthropic.com", 401, "", {}, None)
            service.next_poll.clear()
            with patch.object(backend, "claude_usage", side_effect=denied):
                service.poll()
            self.assertIn("HTTP 401", service.state()["accounts"][0]["error"])

    def test_retry_after_and_cache_shape_are_safe(self):
        self.assertEqual(backend.retry_after_seconds("120"), 120)
        self.assertEqual(
            backend.retry_after_seconds("Mon, 09 Nov 2026 12:02:00 GMT", 1794225600),
            120,
        )
        self.assertIsNone(backend.retry_after_seconds("soon"))
        shape = backend.cache_shape(
            {
                "acct:private-id|client:https://api.anthropic.com:user:inference user:profile": {
                    "token": "private-token"
                },
                "acct:someone|client:https://api.anthropic.com:user:profile": {},
            },
            "private-id",
        )
        self.assertEqual(shape["entries_for_this_account"], 1)
        self.assertEqual(
            shape["scopes_for_this_account"], ["user:inference", "user:profile"]
        )
        self.assertNotIn("private", json.dumps(shape))

    def test_root_cookie_layout_is_saved_and_cleared_without_losing_storage_key(self):
        home = self.claude_home("first")
        (home / "Network" / "Cookies").rename(home / "Cookies")
        (home / "Local State").write_text("storage-key-fixture")
        with patch.object(backend, "claude_dir", return_value=home):
            self.assertTrue(backend.claude_session_cookie(home))
            snapshot = backend.capture_claude()
            self.assertIn("Cookies", snapshot["files"])
            backend.clear_claude_sign_in()
            self.assertFalse((home / "Cookies").exists())
            self.assertEqual((home / "Local State").read_text(), "storage-key-fixture")
            backend.restore_claude(snapshot)
            self.assertTrue(backend.claude_session_cookie(home))

    def test_save_while_signed_out_opens_sign_in_without_overwriting_saved_profile(
        self,
    ):
        home = self.claude_home("", cookie=False)
        service = backend.Service(vault=self.vault, network=False)
        service.handle("add")
        self.vault.write_json("claude/account-1.bin", {"kept": True})
        patches = self.claude_patches(home)
        with patches[0], patches[1], patches[2], patches[3], patches[4] as launch:
            state = service.handle("save", {"slot": 1, "confirmed": True})
            self.assertIn("Complete sign-in", state["notice"])
            launch.assert_called_once()
        self.assertEqual(self.vault.read_json("claude/account-1.bin"), {"kept": True})
        self.assertTrue(service.settings["claude"][0]["draft"])

    def test_saved_versions_do_not_prune_the_other_provider(self):
        for i in range(5):
            self.vault.save_profile(
                "claude/account-1.bin", backend.encoded({"value": i})
            )
            self.vault.save_profile(
                "codex/account-1.bin", backend.encoded({"value": i})
            )
        versions = self.vault.root / "saved-versions"
        self.assertEqual(len(list((versions / "claude").iterdir())), 3)
        self.assertEqual(len(list((versions / "codex").iterdir())), 3)

    def test_remembered_account_without_session_is_not_logged_in(self):
        home = self.claude_home("first", cookie=False)
        with patch.object(backend, "claude_dir", return_value=home):
            self.assertIsNone(
                backend.Service(vault=self.vault, network=False).current("claude")
            )

    def test_cookie_only_login_reads_bound_local_usage_without_oauth(self):
        import time

        home = self.claude_home("first")
        (home / "Network" / "Cookies").rename(home / "Cookies")
        (home / "plan-usage-history.json").write_bytes(
            backend.encoded(
                {"samples": [{"t": time.time() * 1000, "org": "o", "u": {"fh": 43}}]}
            )
        )
        service = backend.Service(vault=self.vault, network=True)
        patches = self.claude_patches(home)
        with patches[0], patches[1], patches[2], patches[3], patches[4], patch.object(
            backend, "claude_usage"
        ) as network:
            service.handle("add")
            service.handle("save", {"slot": 1, "confirmed": True})
            service.poll()
            row = service.state()["accounts"][0]
            self.assertTrue(row["active"])
            self.assertEqual(row["usage"]["session"]["used"], 43)
            self.assertIsNone(row["usage"]["weekly"])
            self.assertEqual(row["error"], "")
            network.assert_not_called()

    def test_local_usage_rejects_wrong_account_org_and_prelogin_sample(self):
        import time

        home = self.claude_home("first")
        history = home / "plan-usage-history.json"
        history.write_bytes(
            backend.encoded(
                {"samples": [{"t": time.time() * 1000, "org": "o", "u": {"fh": 43}}]}
            )
        )
        with patch.object(backend, "claude_dir", return_value=home):
            self.assertIsNotNone(backend.claude_local_usage(expected_account="first"))
            self.assertIsNone(backend.claude_local_usage(expected_account="second"))
            history.write_bytes(
                backend.encoded(
                    {
                        "samples": [
                            {"t": time.time() * 1000, "org": "wrong", "u": {"fh": 43}}
                        ]
                    }
                )
            )
            self.assertIsNone(backend.claude_local_usage(expected_account="first"))
            history.write_bytes(
                backend.encoded(
                    {
                        "samples": [
                            {
                                "t": (time.time() - 3500) * 1000,
                                "org": "o",
                                "u": {"fh": 43},
                            }
                        ]
                    }
                )
            )
            os.utime(home / "os-entry-points.json", None)
            self.assertIsNone(backend.claude_local_usage(expected_account="first"))

    def test_hyprland_uses_available_secure_keyring_without_basic_fallback(self):
        with patch.dict(
            os.environ,
            {"XDG_CURRENT_DESKTOP": "Hyprland", "MUISLAND_CLAUDE_PASSWORD_STORE": ""},
        ), patch.object(
            backend.shutil, "which", return_value="secret-tool"
        ), patch.object(
            backend.subprocess, "run"
        ) as lookup:
            lookup.return_value.returncode = 0
            lookup.return_value.stdout = b"disposable-key-fixture"
            self.assertEqual(
                backend.claude_launch_arguments(), ["--password-store=gnome-libsecret"]
            )
            lookup.return_value.returncode = 1
            self.assertEqual(backend.claude_launch_arguments(), [])
        with patch.dict(os.environ, {"MUISLAND_CLAUDE_PASSWORD_STORE": "default"}):
            self.assertEqual(backend.claude_launch_arguments(), [])

    def test_unverified_old_local_cache_is_not_shown_as_this_accounts_usage(self):
        service = backend.Service(vault=self.vault, network=False)
        service.settings["claude"] = [
            {
                "slot": 1,
                "name": "Old",
                "identity": "first",
                "draft": False,
                "usage": {"local": True, "session": {"used": 99}},
            }
        ]
        service.persist()
        loaded = backend.Service(vault=self.vault, network=False)
        self.assertIsNone(loaded.settings["claude"][0]["usage"])

    def test_missing_old_session_is_rejected_before_closing_claude(self):
        home = self.claude_home("first")
        service = backend.Service(vault=self.vault, network=False)
        service.settings["claude"] = [
            {"slot": 1, "identity": "second", "name": "Other", "draft": False}
        ]
        self.vault.write_json(
            "claude/account-1.bin",
            {"config": {"lastKnownAccountUuid": "second"}, "files": {}},
        )
        with patch.object(backend, "claude_dir", return_value=home), patch.object(
            backend, "claude_executable", return_value="claude-desktop"
        ), patch.object(backend, "close_claude") as close:
            with self.assertRaisesRegex(ValueError, "missing its browser session"):
                service.handle("switch", {"slot": 1, "confirmed": True})
            close.assert_not_called()


if __name__ == "__main__":
    unittest.main()
