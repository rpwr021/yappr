"""py2app build for Yappr.

Build a real .app whose own launcher binary is the macOS permission identity
(so Microphone/Accessibility attach to "Yappr", not to uv/python/Terminal):

    uv run python setup.py py2app -A     # alias mode (dev): references this venv

The app code lives in src/yappr/; scripts/yappr_main.py is the launcher.
After building, the first launch prompts for Microphone + Accessibility as Yappr.
"""

from setuptools import setup

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
    # rumps/pynput/sounddevice pull in pyobjc + native libs
    "packages": ["rumps", "pynput", "sounddevice", "numpy", "requests"],
}

setup(
    app=APP,
    options={"py2app": OPTIONS},
)
