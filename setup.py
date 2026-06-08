"""py2app build for Yappr (alias mode, personal use).

Alias mode (`py2app -A`, via scripts/build_app.sh) symlinks to the dev venv —
fast, fine for running it yourself. It does NOT produce a clean distributable:
the venv python's ad-hoc identity breaks macOS TCC permission persistence.
The real distributable is a native Rust rewrite — see docs/RUST_REWRITE_HANDOFF.md.

The app code lives in src/yappr/; scripts/yappr_main.py adds src/ to sys.path
and launches it.
"""

import sys
from setuptools import setup

sys.path.insert(0, "src")

APP = ["scripts/yappr_main.py"]
OPTIONS = {
    "argv_emulation": False,
    "plist": {
        "CFBundleName": "Yappr",
        "CFBundleDisplayName": "Yappr",
        "CFBundleIdentifier": "com.rpwr021.yappr",
        "CFBundleVersion": "0.1.0",
        "CFBundleShortVersionString": "0.1.0",
        "LSUIElement": True,  # menu-bar only, no Dock icon
        "NSMicrophoneUsageDescription":
            "Yappr records your voice for on-device transcription.",
        "NSHighResolutionCapable": True,
    },
    "packages": ["rumps", "pynput", "sounddevice", "numpy", "requests"],
}

setup(
    app=APP,
    options={"py2app": OPTIONS},
)
