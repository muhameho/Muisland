#!/usr/bin/python3
"""Architecture-independent entry point; uses distro-maintained GTK/WebKit."""

import pathlib, runpy, sys

sys.dont_write_bytecode = True
root = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(root))
try:
    runpy.run_path(
        str(root / "app.py"),
        run_name="__main__",
        init_globals={"MUISLAND_VERSION": (root / "VERSION").read_text().strip()},
    )
except ImportError:
    print(
        "Missing Linux runtime dependencies. Run bash dependencies.sh to see the packages for your distribution.",
        file=sys.stderr,
    )
    sys.exit(1)
