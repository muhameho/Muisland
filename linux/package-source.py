"""Build an architecture-independent Linux runtime archive. No account data."""

from pathlib import Path
import gzip, io, re, tarfile

root = Path(__file__).resolve().parent.parent
version = re.search(r'^version = "([^"]+)"', (root / "Cargo.toml").read_text(), re.M)[1]
folder = "muisland-" + version + "-linux-any"
out = root / "dist" / (folder + ".tar.gz")
out.parent.mkdir(exist_ok=True)
files = {
    "muisland": "linux/launcher.py",
    "app.py": "linux/app.py",
    "backend.py": "linux/backend.py",
    "island.html": "linux/island.html",
    "install.sh": "linux/install.sh",
    "dependencies.sh": "linux/dependencies.sh",
    "muisland.desktop": "linux/muisland.desktop",
    "muisland.svg": "linux/muisland.svg",
    "README.md": "docs/LINUX.md",
    "images/linux.png": "docs/images/linux.png",
}
with out.open("wb") as raw, gzip.GzipFile(
    filename="", fileobj=raw, mode="wb", mtime=0
) as compressed, tarfile.open(fileobj=compressed, mode="w|") as archive:
    for name, data in [
        (name, (root / source).read_bytes()) for name, source in files.items()
    ] + [("VERSION", (version + "\n").encode())]:
        info = tarfile.TarInfo(folder + "/" + name)
        info.size = len(data)
        info.mode = (
            0o755 if name in ("muisland", "install.sh", "dependencies.sh") else 0o644
        )
        archive.addfile(info, io.BytesIO(data))
print(out.relative_to(root))
