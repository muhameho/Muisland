"""Linux platform services. No GUI imports and no credentials in UI state/logs."""

import base64, copy, datetime, hashlib, json, os, queue, secrets, shutil, signal, ssl, subprocess, threading, time, urllib.request, urllib.error

try:
    import tomllib
except ImportError:
    tomllib = None
from pathlib import Path, PurePosixPath
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives import padding

AUTH_KEYS = ("oauth:tokenCacheV2", "oauth:tokenCache", "lastKnownAccountUuid")
AUTH_ITEMS = ("Local State", "Network", "Local Storage", "Session Storage")
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
            previous = "saved-versions/" + str(time.time_ns()) + "-" + Path(name).name
            atomic(self.root / previous, (self.root / name).read_bytes())
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


def decrypt_claude_cache(value):
    encrypted = base64.b64decode(value, validate=True)
    if encrypted[:3] not in (b"v10", b"v11"):
        raise ValueError("Unsupported Claude Linux credential format")
    passwords = [b"peanuts"] if encrypted[:3] == b"v10" else []
    if encrypted[:3] == b"v11":
        for application in ("claude", "Claude", "claude-desktop"):
            result = subprocess.run(
                ["secret-tool", "lookup", "application", application],
                capture_output=True,
                timeout=10,
            )
            if result.stdout.strip():
                passwords.append(result.stdout.strip())
        if not passwords:
            raise ValueError(
                "Claude Safe Storage key is unavailable in your unlocked desktop keyring"
            )
    for password in passwords:
        try:
            key = hashlib.pbkdf2_hmac("sha1", password, b"saltysalt", 1, 16)
            decoder = Cipher(algorithms.AES(key), modes.CBC(b" " * 16)).decryptor()
            plain = decoder.update(encrypted[3:]) + decoder.finalize()
            unpad = padding.PKCS7(128).unpadder()
            plain = unpad.update(plain) + unpad.finalize()
            # Chromium Linux caches can prefix a 32-byte host digest.
            for candidate in (plain, plain[32:]):
                try:
                    return json.loads(candidate)
                except (ValueError, UnicodeDecodeError):
                    pass
        except ValueError:
            continue
    raise ValueError(
        "Claude session encryption is not supported by this Linux build; no credential was modified"
    )


def claude_token(config):
    account = claude_identity(config)
    for field in AUTH_KEYS[:2]:
        if not isinstance(config.get(field), str):
            continue
        cache = decrypt_claude_cache(config[field])
        for key, value in cache.items():
            if (
                key.startswith("acct:" + account + "|")
                and ":https://api.anthropic.com:" in key
                and "user:profile"
                in key.split(":https://api.anthropic.com:", 1)[1].split()
                and isinstance(value, dict)
            ):
                token = value.get("token")
                expires = value.get("expiresAt")
                if (
                    isinstance(expires, (int, float))
                    and (expires / 1000 if expires > 100_000_000_000 else expires)
                    < time.time() + 15
                ):
                    continue
                if (
                    isinstance(token, str)
                    and 0 < len(token) < 16384
                    and "\n" not in token
                    and "\r" not in token
                ):
                    return token, str(value.get("subscriptionType", "Claude"))[:32]
    raise ValueError(
        "No usage credential belongs to this Claude profile. Sign in and save it again."
    )


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


def launch_claude():
    executable = shutil.which("claude-desktop")
    if not executable:
        raise FileNotFoundError("claude-desktop executable was not found")
    subprocess.Popen(
        [executable],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )


def capture_claude():
    root = claude_dir()
    config = claude_config()
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


def restore_claude(snapshot):
    root = claude_dir()
    config = claude_config()
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
        self.settings = {
            "provider": "claude",
            "theme": 0,
            "menu_scale": 1.0,
            "float_scale": 1.0,
            "bubble_scale": 1.0,
            "circle": True,
            "reduced_motion": False,
            "protect_local": False,
            "claude": [],
            "codex": [],
        }
        if not preview and self.vault.exists("settings.bin"):
            self.settings.update(self.vault.read_json("settings.bin"))
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
                else claude_identity(claude_config())
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

    def poll(self):
        if self.preview or not self.network or self.busy:
            return
        for provider in ("claude", "codex"):
            current = self.current(provider)
            with self.lock:
                rows = copy.deepcopy(self.settings[provider])
            for row in rows:
                key = (provider, row["slot"])
                if row.get("draft") or self.next_poll.get(key, 0) > time.monotonic():
                    continue
                self.next_poll[key] = time.monotonic() + 120
                try:
                    auth = self.vault.read_json(self.snapshot_name(*key))
                    saved_identity = (
                        codex_identity(auth)
                        if provider == "codex"
                        else claude_identity(auth["config"])
                    )
                    if saved_identity != row.get("identity"):
                        raise ValueError("Profile identity changed; save it again")
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
                    self.next_poll[key] = time.monotonic() + (
                        30 if current == row.get("identity") else 120
                    )
                except Exception as e:
                    # Never expose protocol payloads, response bodies, or credentials.
                    self.errors[key] = (
                        "Usage unavailable. Check the session and network, then save again if needed."
                    )
                    delay = 120
                    if isinstance(e, urllib.error.HTTPError):
                        if e.code in (401, 403):
                            delay = 900
                        elif e.code == 429:
                            try:
                                delay = max(
                                    120,
                                    min(3600, int(e.headers.get("Retry-After", "120"))),
                                )
                            except ValueError:
                                pass
                    self.next_poll[key] = time.monotonic() + delay

    def handle(self, action, data=None):
        data = data or {}
        with self.lock:
            if action == "state":
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
                expected = (
                    claude_identity(claude_config()) if action == "save" else None
                )
                was_running = bool(claude_processes())
                close_claude()
                try:
                    if action == "save":
                        auth = capture_claude()
                        if claude_identity(auth["config"]) != expected:
                            raise ValueError(
                                "Claude changed accounts while saving. Previous snapshot kept."
                            )
                        self.vault.save_profile(
                            self.snapshot_name(p, row["slot"]), encoded(auth)
                        )
                        row.update(draft=False, identity=expected, usage=None)
                    else:
                        if self.settings["protect_local"]:
                            self.vault.checkpoint(claude_dir())
                        auth = self.vault.read_json(
                            "claude/previous.bin"
                            if action == "rollback"
                            else self.snapshot_name(p, row["slot"])
                        )
                        previous = capture_claude()
                        self.vault.write_json("claude/previous.bin", previous)
                        restore_claude(auth)
                finally:
                    if was_running or action == "save":
                        launch_claude()
            self.persist()
            self.next_poll.clear()
            return self.state()
        finally:
            self.busy = False
