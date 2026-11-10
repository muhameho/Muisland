"""Linux platform services. No GUI imports and no credentials in UI state/logs."""

import base64, copy, datetime, email.utils, hashlib, json, math, os, queue, secrets, shutil, signal, sqlite3, ssl, subprocess, threading, time, urllib.parse, urllib.request, urllib.error

try:
    import tomllib
except ImportError:
    tomllib = None
from pathlib import Path, PurePosixPath
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives import padding

AUTH_KEYS = ("oauth:tokenCacheV2", "oauth:tokenCache", "lastKnownAccountUuid")
AUTH_ITEMS = (
    "Local State",
    "Network",
    "Local Storage",
    "Session Storage",
    "Cookies",
    "Cookies-wal",
    "Cookies-shm",
    "Cookies-journal",
)
THEMES = ("Lavender", "Ocean", "Sunset", "Graphite")


def encoded(value):
    return json.dumps(value, separators=(",", ":")).encode()


def atomic(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temp = path.with_name(path.name + ".tmp-" + secrets.token_hex(5))
    try:
        fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as file:
            file.write(data)
            file.flush()
            os.fsync(file.fileno())
        os.replace(temp, path)
    finally:
        if temp.exists():
            temp.unlink()


def relative(name):
    p = PurePosixPath(name)
    if not name or p.is_absolute() or ".." in p.parts or "\\" in name or ":" in name:
        raise ValueError("Unsafe backup path")
    return p


def safe_files(root):
    root = Path(root)
    if root.is_symlink():
        raise ValueError("Linked app-data folders are not supported")
    for folder, dirs, files in os.walk(
        root, followlinks=False, onerror=lambda error: (_ for _ in ()).throw(error)
    ):
        for name in dirs + files:
            path = Path(folder) / name
            if path.is_symlink():
                raise ValueError("Backups do not follow symbolic links")
        for name in files:
            path = Path(folder) / name
            if path.is_file():
                yield path


class Vault:
    def __init__(self, root=None, key=None):
        self.root = Path(
            root
            or Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))
            / "muisland"
        )
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(self.root, 0o700)
        self.key = key

    def cipher(self):
        if self.key is None:
            if not shutil.which("secret-tool"):
                raise RuntimeError(
                    "Install libsecret-tools and unlock your desktop keyring. No plaintext fallback is used."
                )
            result = subprocess.run(
                [
                    "secret-tool",
                    "lookup",
                    "application",
                    "muisland",
                    "key",
                    "master-v1",
                ],
                capture_output=True,
                timeout=20,
            )
            value = result.stdout.strip()
            if not value:
                if any(self.root.rglob("*.bin")):
                    raise RuntimeError(
                        "The existing Muisland encryption key is unavailable. Unlock the original desktop keyring; encrypted profiles were kept."
                    )
                value = base64.b64encode(secrets.token_bytes(32))
                saved = subprocess.run(
                    [
                        "secret-tool",
                        "store",
                        "--label=Muisland encrypted profiles",
                        "application",
                        "muisland",
                        "key",
                        "master-v1",
                    ],
                    input=value,
                    capture_output=True,
                    timeout=30,
                )
                if saved.returncode:
                    raise RuntimeError(
                        "The desktop Secret Service is locked or unavailable. Unlock it and retry."
                    )
            try:
                self.key = base64.b64decode(value, validate=True)
            except ValueError:
                raise RuntimeError("Muisland keyring entry is invalid") from None
            if len(self.key) != 32:
                raise RuntimeError("Muisland keyring entry is invalid")
        return AESGCM(self.key)

    def write(self, name, data):
        name = str(relative(name))
        nonce = secrets.token_bytes(12)
        atomic(
            self.root / name,
            b"MUI1" + nonce + self.cipher().encrypt(nonce, data, b"Muisland Linux v1"),
        )

    def read(self, name):
        data = (self.root / str(relative(name))).read_bytes()
        if not data.startswith(b"MUI1"):
            raise ValueError("Unrecognized encrypted profile")
        return self.cipher().decrypt(data[4:16], data[16:], b"Muisland Linux v1")

    def write_json(self, name, value):
        self.write(name, encoded(value))

    def save_profile(self, name, data):
        if self.exists(name):
            previous = (
                "saved-versions/"
                + Path(name).parent.name
                + "/"
                + str(time.time_ns())
                + "-"
                + Path(name).name
            )
            atomic(self.root / previous, (self.root / name).read_bytes())
            # Safety net, not an archive; switching refreshes profiles often.
            suffix = "-" + Path(name).name
            older = sorted(
                (
                    path
                    for path in (
                        self.root / "saved-versions" / Path(name).parent.name
                    ).iterdir()
                    if path.name.endswith(suffix)
                    and path.name[: -len(suffix)].isdigit()
                    and path.is_file()
                ),
                key=lambda path: int(path.name[: -len(suffix)]),
            )
            for path in older[:-3]:
                path.unlink()
        self.write(name, data)

    def read_json(self, name):
        return json.loads(self.read(name))

    def exists(self, name):
        return (self.root / name).is_file()

    def archive(self, name):
        if not self.exists(name):
            return None
        target = "removed/" + str(time.time_ns()) + "-" + Path(name).name
        (self.root / target).parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.replace(self.root / name, self.root / target)
        return target

    def checkpoint(self, source):
        stamp = str(time.time_ns())
        prefix = "backups/" + stamp
        rows = []
        counter = 0
        for path in safe_files(source):
            name = path.relative_to(source).as_posix()
            relative(name)
            chunks = []
            with path.open("rb") as file:
                while True:
                    data = file.read(4 * 1024 * 1024)
                    if not data:
                        break
                    key = f"{prefix}/{counter}.chunk"
                    counter += 1
                    self.write(key, data)
                    chunks.append(key)
            rows.append({"path": name, "chunks": chunks})
        self.write_json(prefix + "/manifest.bin", rows)
        self.write_json("latest-backup.bin", {"prefix": prefix})
        return stamp

    def recover(self):
        prefix = self.read_json("latest-backup.bin")["prefix"]
        relative(prefix)
        rows = self.read_json(prefix + "/manifest.bin")
        target = self.root / "recovered" / str(time.time_ns())
        target.mkdir(parents=True, mode=0o700)
        for row in rows:
            path = target / str(relative(row["path"]))
            path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            with path.open("xb") as file:
                os.chmod(path, 0o600)
                for chunk in row["chunks"]:
                    if not chunk.startswith(prefix + "/"):
                        raise ValueError("Backup chunk escaped its checkpoint")
                    file.write(self.read(chunk))
        return str(target)


def codex_home():
    return Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))


def file_store_policy(text):
    if tomllib is not None:
        return tomllib.loads(text).get("cli_auth_credentials_store", "file") == "file"
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("["):
            break
        if "=" in line:
            key, value = line.split("=", 1)
            if key.strip() == "cli_auth_credentials_store":
                return value.split("#", 1)[0].strip().strip("\"'") == "file"
    return True


def require_file_store():
    config = codex_home() / "config.toml"
    if config.exists() and not file_store_policy(config.read_text()):
        raise ValueError(
            "Codex uses keyring/auto/ephemeral credentials. Muisland will not change that storage policy."
        )


def codex_identity(auth):
    value = auth.get("tokens", {}).get("account_id")
    if not isinstance(value, str) or not value:
        raise ValueError(
            "Use a Codex ChatGPT login with file credential storage; API-key and OS-keyring profiles are not supported by this switcher."
        )
    return value


def limit_window(value):
    if not isinstance(value, dict):
        return None
    used = value.get("usedPercent", value.get("utilization"))
    if (
        isinstance(used, bool)
        or not isinstance(used, (int, float))
        or not 0 <= used <= 100
    ):
        return None
    reset = value.get("resetsAt", value.get("resets_at"))
    if isinstance(reset, str):
        try:
            reset = int(
                datetime.datetime.fromisoformat(
                    reset.replace("Z", "+00:00")
                ).timestamp()
            )
        except ValueError:
            reset = None
    return {"used": used, "resets": reset if isinstance(reset, (int, float)) else None}


def codex_limits(value):
    limits = (
        value.get("rateLimitsByLimitId", {}).get("codex")
        or value.get("rateLimits")
        or {}
    )
    result = {
        "session": limit_window(limits.get("primary")),
        "weekly": limit_window(limits.get("secondary")),
        "plan": limits.get("planType") or value.get("planType") or "Codex",
        "updated": int(time.time()),
    }
    if not result["session"] and not result["weekly"]:
        raise ValueError("Codex returned no quota windows")
    return result


def codex_usage(auth):
    account_id = codex_identity(auth)
    token = auth.get("tokens", {}).get("access_token")
    if not token:
        raise ValueError("Codex access token is unavailable")
    executable = os.environ.get("MUISLAND_CODEX_BIN") or shutil.which("codex")
    if not executable:
        raise ValueError("Install Codex CLI or set MUISLAND_CODEX_BIN")
    child = subprocess.Popen(
        [
            executable,
            "app-server",
            "--listen",
            "stdio://",
            "-c",
            'cli_auth_credentials_store="ephemeral"',
            "-c",
            "analytics.enabled=false",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    messages = queue.Queue()

    def reader():
        for line in child.stdout:
            try:
                messages.put(json.loads(line))
            except ValueError:
                pass
        messages.put(None)

    threading.Thread(target=reader, daemon=True).start()

    def send(value):
        child.stdin.write(json.dumps(value) + "\n")
        child.stdin.flush()

    def call(number, method, params):
        send({"id": number, "method": method, "params": params})
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            message = messages.get(timeout=max(0.01, deadline - time.monotonic()))
            if message is None:
                raise RuntimeError("Codex app-server stopped")
            if message.get("id") == number and "method" not in message:
                if "error" in message:
                    raise ValueError(
                        "Codex could not read usage. Sign in again and save the profile."
                    )
                return message.get("result", {})
            if "id" in message and "method" in message:
                send(
                    {
                        "id": message["id"],
                        "error": {"code": -32601, "message": "Read-only quota client"},
                    }
                )
        raise TimeoutError("Codex usage timed out")

    try:
        call(
            1,
            "initialize",
            {
                "clientInfo": {"name": "muisland", "version": "0.4.0"},
                "capabilities": {"experimentalApi": True},
            },
        )
        send({"method": "initialized"})
        call(
            2,
            "account/login/start",
            {
                "type": "chatgptAuthTokens",
                "accessToken": token,
                "chatgptAccountId": account_id,
            },
        )
        return codex_limits(call(3, "account/rateLimits/read", {}))
    finally:
        child.kill()
        child.wait(timeout=5)
        child.stdin.close()
        child.stdout.close()


# The usage service rate-limits per credential, and Claude Desktop polls it too.
ACTIVE_SECONDS = 60
OTHER_SECONDS = 300


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


def claude_dir():
    config = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
    candidates = (
        [Path(os.environ["MUISLAND_CLAUDE_CONFIG"])]
        if "MUISLAND_CLAUDE_CONFIG" in os.environ
        else [config / "Claude", config / "claude", config / "claude-desktop"]
    )
    for path in candidates:
        if (path / "config.json").is_file() and not path.is_symlink():
            return path
    raise FileNotFoundError(
        "Claude config.json was not found. Start Claude Desktop first, or set MUISLAND_CLAUDE_CONFIG."
    )


def claude_config():
    return json.loads((claude_dir() / "config.json").read_bytes())


def claude_identity(config):
    value = config.get("lastKnownAccountUuid")
    if not isinstance(value, str) or not value:
        raise ValueError(
            "Claude is not signed in on disk. Finish signing in before saving."
        )
    return value


USAGE_ERRORS = {
    "cache_missing": "Claude usage: no on-disk OAuth cache yet. Open Claude, finish signing in, then save the account again.",
    "cache_format": "Claude usage: this Linux credential format is not supported. Run muisland --diagnose-usage to identify the format safely.",
    "cache_invalid": "Claude usage: the saved credential cache could not be decoded. Open Claude and save the account again.",
    "keyring_missing": "Claude usage: the Claude Safe Storage key is unavailable. Unlock the desktop keyring used by Claude and retry.",
    "keyring_timeout": "Claude usage: the desktop keyring did not respond. Unlock it; usage will retry automatically.",
    "decrypt_failed": "Claude usage: the cache could not be decrypted with Claude's Safe Storage key. Keep Claude and Muisland in the same Linux user session.",
    "token_missing": "Claude usage: Claude has stored credentials for this account, but none can read usage. Keep Claude open on this account; usage retries automatically.",
    "account_absent": "Claude usage: Claude has not stored a usage credential for this account yet. Keep Claude open and signed in to it; usage retries automatically.",
    "token_expired": "Claude usage: the on-disk credential has expired. Open Claude so it can refresh the session; usage will retry automatically.",
    "profile_mismatch": "Claude usage: the saved profile identity changed. Save this account again.",
}


class UsageError(ValueError):
    def __init__(self, code):
        self.code = code if code in USAGE_ERRORS else "cache_invalid"
        super().__init__(USAGE_ERRORS[self.code])


def usage_error_message(error, provider="claude"):
    # Deliberately map exception TYPES / our codes, never response bodies or repr(error).
    if provider == "codex" and not isinstance(
        error,
        (urllib.error.URLError, TimeoutError, subprocess.TimeoutExpired, queue.Empty),
    ):
        return "Codex usage unavailable. Check the CLI installation and ChatGPT file session, then save again."
    if isinstance(error, UsageError):
        return str(error)
    if isinstance(error, urllib.error.HTTPError):
        return {
            401: "Claude usage: Anthropic rejected this session (HTTP 401). Open Claude, sign in if needed, and save again.",
            403: "Claude usage: Anthropic denied access to usage for this credential (HTTP 403). Your account snapshot is still saved.",
            429: "Claude usage: Anthropic requested a pause (HTTP 429). Automatic retry will respect the server limit.",
        }.get(
            error.code,
            "Usage service returned HTTP "
            + str(error.code)
            + ". Automatic retry is enabled.",
        )
    if isinstance(error, (ssl.SSLError, urllib.error.URLError)):
        if isinstance(getattr(error, "reason", error), ssl.SSLError):
            return "Usage connection: TLS certificate verification failed. Check system certificates, clock, and any HTTPS proxy."
        return "Usage connection failed. Check internet access and proxy settings; automatic retry is enabled."
    if isinstance(error, (TimeoutError, subprocess.TimeoutExpired, queue.Empty)):
        return "Usage request timed out. Automatic retry is enabled."
    return "Usage could not be read. The saved account was kept. Run muisland --diagnose-usage for a private local check."


def decrypt_claude_cache(value):
    try:
        encrypted = base64.b64decode(value, validate=True)
    except (ValueError, TypeError):
        raise UsageError("cache_invalid") from None
    if encrypted[:3] not in (b"v10", b"v11"):
        raise UsageError("cache_format")
    passwords = [b"peanuts"] if encrypted[:3] == b"v10" else []
    if encrypted[:3] == b"v11":
        # Scope every lookup to Claude; never enumerate other apps' secrets.
        for application in ("claude", "Claude", "claude-desktop"):
            for schema in (
                "chrome_libsecret_os_crypt_password_v2",
                "chrome_libsecret_os_crypt_password_v1",
                None,
            ):
                command = ["secret-tool", "lookup", "application", application]
                if schema:
                    command += ["xdg:schema", schema]
                try:
                    result = subprocess.run(command, capture_output=True, timeout=5)
                except subprocess.TimeoutExpired:
                    raise UsageError("keyring_timeout") from None
                if result.stdout.strip() and result.stdout.strip() not in passwords:
                    passwords.append(result.stdout.strip())
        if not passwords:
            raise UsageError("keyring_missing")
    for password in passwords:
        try:
            key = hashlib.pbkdf2_hmac("sha1", password, b"saltysalt", 1, 16)
            decoder = Cipher(algorithms.AES(key), modes.CBC(b" " * 16)).decryptor()
            plain = decoder.update(encrypted[3:]) + decoder.finalize()
            unpad = padding.PKCS7(128).unpadder()
            plain = unpad.update(plain) + unpad.finalize()
            for candidate in (plain, plain[32:]):
                try:
                    cache = json.loads(candidate)
                    if isinstance(cache, dict):
                        return cache
                except (ValueError, UnicodeDecodeError):
                    pass
        except ValueError:
            continue
    raise UsageError("decrypt_failed")


def claude_token(config):
    account = claude_identity(config)
    errors = []
    expired = False
    found = False
    decoded = False
    owned = False
    for field in AUTH_KEYS[:2]:
        if not isinstance(config.get(field), str) or not config[field]:
            continue
        found = True
        try:
            cache = decrypt_claude_cache(config[field])
        except UsageError as error:
            errors.append(error)
            continue
        decoded = True
        # Like Windows, prefer the least broadly scoped matching credential.
        for key, value in sorted(cache.items(), key=lambda item: len(item[0])):
            owned = owned or key.startswith("acct:" + account + "|")
            if not (
                key.startswith("acct:" + account + "|")
                and ":https://api.anthropic.com:" in key
                and "user:profile"
                in key.split(":https://api.anthropic.com:", 1)[1].split()
                and isinstance(value, dict)
            ):
                continue
            token = value.get("token")
            expires = value.get("expiresAt")
            if isinstance(expires, str):
                try:
                    expires = datetime.datetime.fromisoformat(
                        expires.replace("Z", "+00:00")
                    ).timestamp()
                except ValueError:
                    expires = None
            if (
                isinstance(expires, (int, float))
                and (expires / 1000 if expires > 100_000_000_000 else expires)
                < time.time() + 15
            ):
                expired = True
                continue
            if (
                isinstance(token, str)
                and 0 < len(token) < 16384
                and "\n" not in token
                and "\r" not in token
            ):
                return token, str(value.get("subscriptionType", "Claude"))[:32]
    if expired:
        raise UsageError("token_expired")
    if errors and not decoded:
        raise errors[0]
    if not found:
        raise UsageError("cache_missing")
    raise UsageError("token_missing" if owned else "account_absent")


def cache_shape(cache, account):
    # Counts and fixed scope/host words only; never IDs, tokens or key text.
    entries = [(key, value) for key, value in cache.items() if isinstance(key, str)]
    mine = [(k, v) for k, v in entries if k.startswith("acct:" + account + "|")]
    scopes = set()
    hosts = set()
    for key, _ in mine:
        if ":https://" in key:
            host, _, scope = key.split(":https://", 1)[1].partition(":")
            hosts.add(
                host
                if host in ("api.anthropic.com", "claude.ai", "claude.com")
                else "other"
            )
            scopes.update(
                word[:32]
                for word in scope.split()
                if word
                in (
                    "user:profile",
                    "user:inference",
                    "user:sessions:claude_code",
                    "org:create_api_key",
                    "user:file_upload",
                    "user:mcp_servers",
                )
            )
    return {
        "entries": len(entries),
        "entries_with_account_prefix": sum(k.startswith("acct:") for k, _ in entries),
        "entries_for_this_account": len(mine),
        "hosts_for_this_account": sorted(hosts),
        "scopes_for_this_account": sorted(scopes),
        "token_fields_present": sum(
            isinstance(v, dict) and isinstance(v.get("token"), str) for _, v in mine
        ),
    }


def claude_session_cookie(root):
    """Read cookie names only, including a live WAL, in both Electron layouts."""
    for relative_path in ("Network/Cookies", "Cookies"):
        path = Path(root) / relative_path
        if not path.is_file() or path.is_symlink():
            continue
        try:
            db = sqlite3.connect(
                "file:" + urllib.parse.quote(str(path)) + "?mode=ro",
                uri=True,
                timeout=1,
            )
            try:
                if (
                    db.execute(
                        "select 1 from cookies where name='sessionKey' and host_key in ('claude.ai','.claude.ai','claude.com','.claude.com') limit 1"
                    ).fetchone()
                    is not None
                ):
                    return True
            finally:
                db.close()
        except sqlite3.Error:
            continue
    return False


def claude_current_identity():
    config = claude_config()
    identity = claude_identity(config)
    # lastKnownAccountUuid is remembered after sign-out and is not proof of login.
    if not (
        any(isinstance(config.get(key), str) and config[key] for key in AUTH_KEYS[:2])
        or claude_session_cookie(claude_dir())
    ):
        raise ValueError("Claude is not signed in")
    return identity


def claude_local_usage(newer_than=0, expected_account=None):
    """Only use a sample tied to the live account/org and its sign-in boundary."""
    try:
        root = claude_dir()
        expected_org = None
        if expected_account is not None:
            entry = root / "os-entry-points.json"
            if entry.is_symlink() or entry.stat().st_size > 1024 * 1024:
                return None
            binding = json.loads(entry.read_bytes())
            if (
                binding.get("account") != expected_account
                or not isinstance(binding.get("org"), str)
                or not binding["org"]
            ):
                return None
            expected_org = binding["org"]
            newer_than = max(newer_than, entry.stat().st_mtime)
        path = root / "plan-usage-history.json"
        if path.is_symlink() or path.stat().st_size > 4 * 1024 * 1024:
            return None
        samples = json.loads(path.read_bytes()).get("samples")
        sample = max(
            (
                item
                for item in samples
                if isinstance(item, dict)
                and isinstance(item.get("u"), dict)
                and isinstance(item.get("t"), (int, float))
                and not isinstance(item.get("t"), bool)
                and (expected_org is None or item.get("org") == expected_org)
            ),
            key=lambda item: item["t"],
        )
    except (OSError, ValueError, TypeError, AttributeError):
        return None
    stamp = sample["t"] / 1000 if sample["t"] > 100_000_000_000 else sample["t"]
    if stamp < newer_than or not time.time() - 3600 <= stamp <= time.time() + 300:
        return None
    result = {
        "session": limit_window({"utilization": sample["u"].get("fh")}),
        "weekly": limit_window({"utilization": sample["u"].get("sd")}),
        "plan": "Claude",
        "updated": stamp,
        "local": True,
    }
    return result if result["session"] or result["weekly"] else None


def retry_after_seconds(value, now=None):
    if not value:
        return None
    try:
        return max(1, int(value.strip()))
    except ValueError:
        pass
    try:
        when = email.utils.parsedate_to_datetime(value).timestamp()
    except (TypeError, ValueError):
        return None
    return max(1, int(when - (time.time() if now is None else now)))


def claude_diagnostics():
    # Fixed metadata only: never account IDs, paths, tokens, cache contents or keys.
    report = {"provider": "claude", "profile_found": False, "fields": {}}
    try:
        config = claude_config()
        report["profile_found"] = True
        report["account_identity_present"] = bool(config.get("lastKnownAccountUuid"))
        for field in AUTH_KEYS[:2]:
            value = config.get(field)
            entry = {"present": value is not None, "type": type(value).__name__}
            if isinstance(value, str):
                try:
                    prefix = base64.b64decode(value, validate=True)[:3]
                    entry["format"] = (
                        prefix.decode()
                        if len(prefix) == 3
                        and prefix[:1] == b"v"
                        and prefix[1:].isdigit()
                        else "unrecognized"
                    )
                except (ValueError, UnicodeError):
                    entry["format"] = "invalid_base64"
                try:
                    entry["cache"] = cache_shape(
                        decrypt_claude_cache(value),
                        str(config.get("lastKnownAccountUuid") or ""),
                    )
                except UsageError as error:
                    entry["decode"] = error.code
            report["fields"][field] = entry
        try:
            root = claude_dir()
            report["claude_running"] = bool(claude_processes())
            report["session_cookie_present"] = claude_session_cookie(root)
            report["signed_in_on_disk"] = bool(
                report["session_cookie_present"] and config.get("lastKnownAccountUuid")
            )
            report["cookie_files"] = {
                name: (root / name).is_file() for name in ("Cookies", "Network/Cookies")
            }
            report["local_usage_history_present"] = (
                root / "plan-usage-history.json"
            ).is_file()
            report["local_usage_sample_usable"] = (
                claude_local_usage(
                    expected_account=str(config.get("lastKnownAccountUuid") or "")
                )
                is not None
            )
        except OSError:
            pass
        claude_token(config)
        report["credential_status"] = "ready_for_usage_request"
    except UsageError as error:
        report["credential_status"] = error.code
    except FileNotFoundError:
        report["credential_status"] = "profile_or_secret_tool_missing"
    except Exception:
        report["credential_status"] = "profile_unreadable"
    return report


def claude_usage(config):
    token, plan = claude_token(config)
    request = urllib.request.Request(
        "https://api.anthropic.com/api/oauth/usage",
        headers={
            "Authorization": "Bearer " + token,
            "anthropic-beta": "oauth-2025-04-20",
            "Accept": "application/json",
            "User-Agent": "Muisland/0.4",
        },
    )
    opener = urllib.request.build_opener(
        NoRedirect(), urllib.request.HTTPSHandler(context=ssl.create_default_context())
    )
    with opener.open(request, timeout=20) as response:
        value = json.load(response)
    result = {
        "session": limit_window(value.get("five_hour")),
        "weekly": limit_window(value.get("seven_day")),
        "plan": plan,
        "updated": int(time.time()),
    }
    if not result["session"] and not result["weekly"]:
        raise ValueError("Claude returned no quota windows")
    return result


def claude_processes():
    result = []
    for item in Path("/proc").iterdir():
        if not item.name.isdigit():
            continue
        try:
            if item.stat().st_uid != os.getuid():
                continue
            exe = (item / "exe").resolve()
            lower = str(exe).lower()
            if b"--type=" in (item / "cmdline").read_bytes():
                continue
            if (
                exe.name.lower() in ("claude", "claude-desktop")
                and any(x in lower for x in ("/opt/claude", "/usr/lib/claude"))
                and "claude-code" not in lower
            ):
                result.append(int(item.name))
        except (OSError, RuntimeError):
            pass
    return result


def close_claude():
    pids = claude_processes()
    for pid in pids:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    deadline = time.monotonic() + 15
    while claude_processes():
        if time.monotonic() > deadline:
            raise RuntimeError(
                "Claude did not exit normally. Quit it from the tray and retry; it will not be force-killed."
            )
        time.sleep(0.15)


def claude_executable():
    executable = os.environ.get("MUISLAND_CLAUDE_BIN") or shutil.which("claude-desktop")
    if not executable:
        for pid in claude_processes():
            candidate = Path(f"/proc/{pid}/exe").resolve()
            if candidate.is_file():
                return str(candidate)
    return executable


def claude_launch_arguments():
    explicit = os.environ.get("MUISLAND_CLAUDE_PASSWORD_STORE")
    if explicit:
        if explicit == "default":
            return []
        if explicit not in ("gnome-libsecret", "kwallet", "kwallet5", "kwallet6"):
            raise ValueError("Use a secure Claude password-store backend or 'default'.")
        return ["--password-store=" + explicit]
    desktop = os.environ.get("XDG_CURRENT_DESKTOP", "").lower()
    if any(name in desktop for name in ("hyprland", "sway", "river")) and shutil.which(
        "secret-tool"
    ):
        try:
            result = subprocess.run(
                [
                    "secret-tool",
                    "lookup",
                    "application",
                    "muisland",
                    "key",
                    "master-v1",
                ],
                capture_output=True,
                timeout=3,
            )
            if result.returncode == 0 and result.stdout.strip():
                return ["--password-store=gnome-libsecret"]
        except (OSError, subprocess.TimeoutExpired):
            pass
    return []


def launch_claude(executable=None):
    executable = executable or claude_executable()
    if not executable:
        raise FileNotFoundError("claude-desktop executable was not found")
    subprocess.Popen(
        [executable, *claude_launch_arguments()],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )


def capture_claude(require_identity=True):
    root = claude_dir()
    config = claude_config()
    if require_identity:
        claude_identity(config)
    files = {}
    total = 0
    for name in AUTH_ITEMS:
        path = root / name
        if path.is_symlink():
            raise ValueError("Linked authentication data is not supported")
        for file in (
            [path] if path.is_file() else safe_files(path) if path.is_dir() else []
        ):
            total += file.stat().st_size
            if total > 128 * 1024 * 1024:
                raise ValueError(
                    "Claude authentication data exceeds the 128 MiB snapshot limit"
                )
            files[file.relative_to(root).as_posix()] = base64.b64encode(
                file.read_bytes()
            ).decode()
    return {
        "config": {key: config[key] for key in AUTH_KEYS if key in config},
        "files": files,
    }


def clear_claude_sign_in():
    # Never contact Log out. Restore's staged transaction preserves live files if
    # clearing one of the auth folders fails part-way through.
    root = claude_dir()
    state = root / "Local State"
    files = {}
    if state.is_symlink():
        raise ValueError("Refusing to replace linked Claude data")
    if state.is_file():
        files["Local State"] = base64.b64encode(state.read_bytes()).decode()
    restore_claude({"config": {}, "files": files}, require_identity=False)


def restore_claude(snapshot, require_identity=True):
    root = claude_dir()
    config = claude_config()
    if require_identity:
        claude_identity(snapshot["config"])
    # Stage all bytes before mutating the live authentication folders.
    stage = root / (".muisland-stage-" + secrets.token_hex(6))
    stage.mkdir(mode=0o700)
    old = root / (".muisland-old-" + secrets.token_hex(6))
    old.mkdir(mode=0o700)
    moved = []
    installed = []
    original = (root / "config.json").read_bytes()
    committed = False
    rolled_back = False
    try:
        for name, data in snapshot["files"].items():
            rel = relative(name)
            if rel.parts[0] not in AUTH_ITEMS:
                raise ValueError("Snapshot contained a non-authentication path")
            atomic(stage / str(rel), base64.b64decode(data, validate=True))
        for name in AUTH_ITEMS:
            target = root / name
            if target.is_symlink():
                raise ValueError("Refusing to replace linked Claude data")
            if target.exists():
                target.rename(old / name)
                moved.append(name)
            if (stage / name).exists():
                (stage / name).rename(target)
                installed.append(name)
        for key in AUTH_KEYS:
            config.pop(key, None)
            if key in snapshot["config"]:
                config[key] = snapshot["config"][key]
        atomic(root / "config.json", encoded(config))
        committed = True
    except Exception:
        for name in installed:
            path = root / name
            if path.is_dir():
                shutil.rmtree(path)
            elif path.exists():
                path.unlink()
        for name in moved:
            (old / name).rename(root / name)
        atomic(root / "config.json", original)
        rolled_back = True
        raise
    finally:
        shutil.rmtree(stage, ignore_errors=True)
        if committed or rolled_back:
            shutil.rmtree(old, ignore_errors=True)


class Service:
    def __init__(self, preview=False, vault=None, network=True):
        self.preview = preview
        self.preview_current = {"claude": "demo-claude0", "codex": "demo-codex0"}
        self.vault = vault if vault is not None else (None if preview else Vault())
        self.network = network
        self.lock = threading.RLock()
        self.busy = False
        self.errors = {}
        self.next_poll = {}
        self.failures = {}
        self.rate_limited = {}
        # (identity, wall-clock time it was first seen signed in during this run)
        self.claude_seen = (None, 0)
        self.settings = {
            "provider": "claude",
            "theme": 0,
            "menu_scale": 1.0,
            "float_scale": 1.0,
            "bubble_scale": 1.0,
            "circle": True,
            "reduced_motion": False,
            "protect_local": False,
            "position": [100, 80],
            "claude": [],
            "codex": [],
        }
        if not preview and self.vault.exists("settings.bin"):
            self.settings.update(self.vault.read_json("settings.bin"))
        if not preview:
            self.vault.cipher()
        if preview:
            for provider in ("claude", "codex"):
                self.settings[provider] = [
                    {
                        "slot": i + 1,
                        "name": name,
                        "usage": {
                            "session": {
                                "used": 28 + 37 * i,
                                "resets": time.time() + 7200,
                            },
                            "weekly": {
                                "used": 46 + 9 * i,
                                "resets": time.time() + 259200,
                            },
                            "plan": "Sample",
                            "updated": time.time(),
                        },
                        "identity": "demo-" + provider + str(i),
                        "draft": False,
                    }
                    for i, name in enumerate(("Personal", "Work"))
                ]

    def persist(self):
        if not self.preview:
            clean = copy.deepcopy(self.settings)
            for p in ("claude", "codex"):
                clean[p] = [a for a in clean[p] if not a.get("draft")]
            self.vault.write_json("settings.bin", clean)

    def snapshot_name(self, p, slot):
        return f"{p}/account-{int(slot)}.bin"

    def current(self, p):
        if self.preview:
            return self.preview_current[p]
        try:
            return (
                codex_identity(json.loads((codex_home() / "auth.json").read_bytes()))
                if p == "codex"
                else claude_current_identity()
            )
        except (ValueError, OSError):
            return None

    def state(self):
        with self.lock:
            p = self.settings["provider"]
            current = self.current(p)
            rows = []
            for a in self.settings[p]:
                rows.append(
                    {
                        "slot": a["slot"],
                        "name": a["name"],
                        "usage": a.get("usage"),
                        "draft": a.get("draft", False),
                        "saved": not a.get("draft")
                        and (
                            self.preview
                            or self.vault.exists(self.snapshot_name(p, a["slot"]))
                        ),
                        "active": current is not None and a.get("identity") == current,
                        "error": self.errors.get((p, a["slot"]), ""),
                        "provider": p,
                    }
                )
            return {
                "provider": p,
                "theme": self.settings["theme"],
                "accounts": rows,
                "settings": {
                    k: v
                    for k, v in self.settings.items()
                    if k not in ("claude", "codex")
                },
                "busy": self.busy,
                "preview": self.preview,
            }

    def refresh_local_usage(self, current):
        if self.claude_seen[0] != current:
            self.claude_seen = (current, time.time())
        if not current:
            return
        local = claude_local_usage(expected_account=current)
        if not local or self.current("claude") != current:
            return
        with self.lock:
            changed = False
            for row in self.settings["claude"]:
                if not row.get("draft") and row.get("identity") == current:
                    shown = row.get("usage") or {}
                    if local["updated"] > shown.get("updated", 0):
                        row["usage"] = dict(local)
                        changed = True
            if changed:
                self.persist()

    def poll(self):
        if self.preview or self.busy:
            return
        self.refresh_local_usage(self.current("claude"))
        if not self.network:
            return
        for provider in ("claude", "codex"):
            current = self.current(provider)
            with self.lock:
                rows = copy.deepcopy(self.settings[provider])
            for row in rows:
                key = (provider, row["slot"])
                if row.get("draft") or self.next_poll.get(key, 0) > time.monotonic():
                    continue
                self.next_poll[key] = time.monotonic() + OTHER_SECONDS
                active = current is not None and current == row.get("identity")
                try:
                    auth = self.vault.read_json(self.snapshot_name(*key))
                    saved_identity = (
                        codex_identity(auth)
                        if provider == "codex"
                        else claude_identity(auth["config"])
                    )
                    if saved_identity != row.get("identity"):
                        raise UsageError("profile_mismatch")
                    if provider == "codex":
                        try:
                            fresh = json.loads(
                                (codex_home() / "auth.json").read_bytes()
                            )
                            if codex_identity(fresh) == row.get("identity"):
                                auth = fresh
                        except (OSError, ValueError):
                            pass
                        usage = codex_usage(auth)
                    else:
                        config = auth["config"]
                        try:
                            live = claude_config()
                            if claude_identity(live) == row.get("identity"):
                                config = live
                        except (OSError, ValueError):
                            pass
                        if active and not any(
                            config.get(field) for field in AUTH_KEYS[:2]
                        ):
                            local = claude_local_usage(expected_account=current)
                            if local:
                                usage = local
                            else:
                                raise UsageError("cache_missing")
                        else:
                            usage = claude_usage(config)
                    with self.lock:
                        account = next(
                            (
                                a
                                for a in self.settings[provider]
                                if a["slot"] == row["slot"]
                            ),
                            None,
                        )
                        if account and account.get("identity") == row.get("identity"):
                            account["usage"] = usage
                            self.errors.pop(key, None)
                            self.persist()
                    self.failures.pop(key, None)
                    self.next_poll[key] = time.monotonic() + (
                        ACTIVE_SECONDS if active else OTHER_SECONDS
                    )
                except Exception as e:
                    # Never expose protocol payloads, response bodies, or credentials.
                    message = usage_error_message(e, provider)
                    rejected = isinstance(e, urllib.error.HTTPError) and e.code in (
                        401,
                        403,
                    )
                    count = self.failures[key] = self.failures.get(key, 0) + 1
                    base = ACTIVE_SECONDS if active else OTHER_SECONDS
                    delay = min(900, base * 2 ** min(count, 5))
                    if rejected:
                        delay = 900
                    elif isinstance(e, urllib.error.HTTPError) and e.code == 429:
                        delay = max(
                            delay,
                            retry_after_seconds(e.headers.get("Retry-After")) or 0,
                        )
                    self.next_poll[key] = time.monotonic() + delay
                    if isinstance(e, urllib.error.HTTPError) and e.code == 429:
                        self.rate_limited[key] = self.next_poll[key]
                    local = None
                    if (
                        provider == "claude"
                        and active
                        and not rejected
                        and not (
                            isinstance(e, UsageError) and e.code == "profile_mismatch"
                        )
                    ):
                        # Claude Desktop records its own quota samples on disk; show
                        # those for the signed-in account instead of an empty card.
                        local = claude_local_usage(expected_account=current)
                    with self.lock:
                        account = next(
                            (
                                a
                                for a in self.settings[provider]
                                if a["slot"] == row["slot"]
                                and a.get("identity") == row.get("identity")
                            ),
                            None,
                        )
                        if local and account is not None:
                            shown = account.get("usage") or {}
                            if shown.get("updated", 0) < local["updated"]:
                                if shown.get("plan") and not shown.get("local"):
                                    local["plan"] = shown["plan"]
                                account["usage"] = local
                                self.persist()
                            self.errors[key] = (
                                "Showing a recent Claude local sample; reset time is unavailable. "
                                + message
                            )
                        else:
                            self.errors[key] = message

    def handle(self, action, data=None):
        data = data or {}
        with self.lock:
            if action == "state":
                return self.state()
            if action == "position":
                self.settings["position"] = validated_position(data.get("position"))
                self.persist()
                return self.state()
            if self.busy:
                raise RuntimeError("Wait for the current operation")
            p = self.settings["provider"]
            rows = self.settings[p]
            if (
                action in ("save", "switch", "rename", "remove", "rollback", "recover")
                and data.get("provider", p) != p
            ):
                raise ValueError(
                    "The selected provider changed. Review the operation again."
                )

            if action == "state":
                return self.state()
            if action == "provider":
                if data.get("provider") not in ("claude", "codex"):
                    raise ValueError("Unknown provider")
                self.settings[p] = [a for a in rows if not a.get("draft")]
                self.settings["provider"] = data["provider"]
                self.persist()
                return self.state()
            if action == "discard":
                self.settings[p] = [a for a in rows if not a.get("draft")]
                return self.state()
            if action == "settings":
                for key, lo, hi in [
                    ("menu_scale", 0.75, 1.6),
                    ("float_scale", 0.6, 1.8),
                    ("bubble_scale", 0.85, 1.6),
                ]:
                    if key in data:
                        self.settings[key] = max(lo, min(hi, float(data[key])))
                for key in ("circle", "reduced_motion", "protect_local"):
                    if key in data:
                        self.settings[key] = bool(data[key])
                if "theme" in data:
                    self.settings["theme"] = int(data["theme"]) % 4
                self.persist()
                return self.state()
            if action == "add":
                if len(rows) >= 64:
                    raise ValueError("Maximum 64 accounts per provider")
                slot = next(
                    i for i in range(1, 65) if all(a["slot"] != i for a in rows)
                )
                rows.append(
                    {"slot": slot, "name": f"Account {len(rows)+1}", "draft": True}
                )
                return self.state()
            row = next((a for a in rows if a["slot"] == int(data.get("slot", 0))), None)
            if action in ("rename", "remove", "save", "switch") and not row:
                raise ValueError("Account slot not found")
            if action == "rename":
                name = str(data.get("name", "")).strip()
                if not name or len(name) > 32 or any(ord(c) < 32 for c in name):
                    raise ValueError("Use a name of 1–32 characters")
                row["name"] = name
                self.persist()
                return self.state()
            if action == "remove":
                if not data.get("confirmed"):
                    raise ValueError("Confirm removal first")
                if not self.preview and not row.get("draft"):
                    self.vault.archive(self.snapshot_name(p, row["slot"]))
                rows.remove(row)
                self.persist()
                return self.state()
            if action not in ("save", "switch", "rollback", "recover"):
                raise ValueError("Unknown operation")
            if not data.get("confirmed"):
                raise ValueError("Confirm the operation before continuing")
            if (
                action == "switch"
                and self.current(p) is not None
                and row.get("identity") == self.current(p)
            ):
                return self.state()
            self.busy = True
        notice = ""
        try:
            if self.preview:
                if action == "switch":
                    self.preview_current[p] = row["identity"]
                if action == "save":
                    row["draft"] = False
                    row["identity"] = self.current(p)
                return self.state()
            if action == "recover":
                return {"folder": self.vault.recover()}
            if p == "codex":
                require_file_store()
                target = codex_home() / "auth.json"
                if action == "save":
                    first = target.read_bytes()
                    auth = json.loads(first)
                    identity = codex_identity(auth)
                    if first != target.read_bytes():
                        raise ValueError(
                            "Codex refreshed its login while saving. Retry."
                        )
                    self.vault.save_profile(self.snapshot_name(p, row["slot"]), first)
                    row.update(draft=False, identity=identity, usage=None)
                else:
                    saved = self.vault.read(
                        "codex/previous.bin"
                        if action == "rollback"
                        else self.snapshot_name(p, row["slot"])
                    )
                    codex_identity(json.loads(saved))
                    if target.exists():
                        self.vault.write("codex/previous.bin", target.read_bytes())
                    atomic(target, saved)
            else:
                expected = self.current("claude") if action == "save" else None
                executable = claude_executable()
                if not executable:
                    raise FileNotFoundError(
                        "Install Claude Desktop or set MUISLAND_CLAUDE_BIN before changing profiles."
                    )
                if action == "save" and expected is None:
                    launch_claude(executable)
                    state = self.state()
                    state["notice"] = (
                        "Claude opened. Complete sign-in there, then choose Save again. Existing saved accounts were kept."
                    )
                    return state
                # Saving into a different slot while the signed-in account is already
                # saved elsewhere means "I want to add another account".
                other = (
                    next(
                        (
                            a
                            for a in rows
                            if a is not row
                            and not a.get("draft")
                            and a.get("identity") == expected
                        ),
                        None,
                    )
                    if action == "save" and row.get("identity") != expected
                    else None
                )
                was_running = bool(claude_processes())
                close_claude()
                try:
                    if other is not None:
                        auth = capture_claude()
                        if claude_identity(auth["config"]) != expected:
                            raise ValueError(
                                "Claude changed accounts while closing. Nothing was changed."
                            )
                        self.vault.write_json("claude/previous.bin", auth)
                        if claude_session_cookie(claude_dir()):
                            self.vault.save_profile(
                                self.snapshot_name(p, other["slot"]), encoded(auth)
                            )
                        clear_claude_sign_in()
                        notice = (
                            other["name"]
                            + " is already saved, so Claude reopened at its sign-in screen. "
                            "Sign in to the other account there, then choose Save again. "
                            + other["name"]
                            + " was not signed out and can still be switched to. "
                            "Avoid Claude's own Log out: it ends that account's saved session."
                        )
                    elif action == "save":
                        auth = capture_claude()
                        if claude_identity(auth["config"]) != expected:
                            raise ValueError(
                                "Claude changed accounts while saving. Previous snapshot kept."
                            )
                        self.vault.save_profile(
                            self.snapshot_name(p, row["slot"]), encoded(auth)
                        )
                        row.update(
                            draft=False,
                            identity=expected,
                            usage=(
                                row.get("usage")
                                if row.get("identity") == expected
                                else None
                            ),
                        )
                    else:
                        if self.settings["protect_local"]:
                            self.vault.checkpoint(claude_dir())
                        auth = self.vault.read_json(
                            "claude/previous.bin"
                            if action == "rollback"
                            else self.snapshot_name(p, row["slot"])
                        )
                        # Claude may be signed out right now; that must not block a switch.
                        previous = capture_claude(require_identity=False)
                        self.vault.write_json("claude/previous.bin", previous)
                        leaving = previous["config"].get("lastKnownAccountUuid")
                        outgoing = next(
                            (
                                a
                                for a in rows
                                if not a.get("draft")
                                and leaving
                                and a.get("identity") == leaving
                            ),
                            None,
                        )
                        if outgoing is not None and claude_session_cookie(claude_dir()):
                            # Keep the account being left current: Claude renews its
                            # session while running, and an old copy can stop working.
                            self.vault.save_profile(
                                self.snapshot_name(p, outgoing["slot"]),
                                encoded(previous),
                            )
                        restore_claude(auth, require_identity=action != "rollback")
                finally:
                    if was_running or action == "save":
                        launch_claude(executable)
            self.persist()
            if action == "save":
                key = (p, row["slot"])
                # Keep an explicit server cooldown even if Save was clicked again.
                if self.rate_limited.get(key, 0) <= time.monotonic():
                    self.next_poll.pop(key, None)
                    self.failures.pop(key, None)
            state = self.state()
            if notice:
                state["notice"] = notice
            return state
        finally:
            self.busy = False


def validated_position(value):
    if not isinstance(value, (list, tuple)) or len(value) != 2:
        raise ValueError("Invalid floating position")
    if any(
        isinstance(v, bool)
        or not isinstance(v, (int, float))
        or not math.isfinite(v)
        or abs(v) > 1000000
        for v in value
    ):
        raise ValueError("Invalid floating position")
    return [int(v) for v in value]


def clamped_position(position, size, area, padding=8):
    x, y = validated_position(position)
    ax, ay, aw, ah = area
    w, h = size
    return (
        max(ax + padding, min(x, ax + max(padding, aw - w - padding))),
        max(ay + padding, min(y, ay + max(padding, ah - h - padding))),
    )


def autostart_path():
    return (
        Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
        / "autostart"
        / "io.github.muhameho.Muisland.desktop"
    )


def set_autostart(enabled):
    path = autostart_path()
    if enabled:
        # PATH is resolved by the desktop session; no shell or credential arguments.
        binary = shutil.which("muisland")
        if not binary:
            candidate = Path.home() / ".local/bin/muisland"
            binary = str(candidate) if candidate.is_file() else "muisland"
        # A desktop Exec field has its own quoting rules, not shell syntax.
        escaped = (
            binary.replace("\\", "\\\\")
            .replace('"', '\\"')
            .replace("`", "\\`")
            .replace("$", "\\$")
            .replace("%", "%%")
        )
        atomic(
            path,
            (
                '[Desktop Entry]\nType=Application\nName=Muisland\nExec="'
                + escaped
                + '" --background\nIcon=io.github.muhameho.Muisland\nTerminal=false\nX-GNOME-Autostart-enabled=true\n'
            ).encode(),
        )
    elif path.exists():
        path.unlink()
    return path.exists()
