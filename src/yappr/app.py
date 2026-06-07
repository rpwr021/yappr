#!/usr/bin/env python3
"""Yappr: a local push-to-talk dictation + voice-chat menu-bar app.

Hold the hotkey (Right Option) -> record -> transcribe -> paste at cursor.
Hold Cmd + the hotkey          -> record -> ask Gemma  -> speak the reply.

Backend is a local llama-server serving Gemma 4 E4B (audio-capable mmproj).
All processing is on-device. See config.ini for settings.
"""

import base64
import configparser
import io
import json
import os
import queue
import subprocess
import sys
import threading
import time
import wave
from time import monotonic

import numpy as np
import requests
import rumps
import sounddevice as sd
from pynput import keyboard

PKG_DIR = os.path.dirname(os.path.abspath(__file__))   # the installed package
ASSET_DIR = os.path.join(PKG_DIR, "assets")
DEFAULT_CONFIG = os.path.join(PKG_DIR, "config.default.ini")
# User config + runtime data live in ~/.yappr (survives app rebuilds).
YAPPR_HOME = os.path.join(os.path.expanduser("~"), ".yappr")
CONFIG_PATH = os.path.join(YAPPR_HOME, "config.ini")

# Keys we know how to watch with pynput.
KEY_MAP = {
    "right_option": keyboard.Key.alt_r,
    "left_option": keyboard.Key.alt_l,
    "right_cmd": keyboard.Key.cmd_r,
    "left_cmd": keyboard.Key.cmd_l,
    "right_ctrl": keyboard.Key.ctrl_r,
    "fn": keyboard.Key.cmd_r,  # placeholder; fn rarely delivers events, warn user
}


def load_config():
    cfg = configparser.ConfigParser()
    # seed the user config from the packaged default on first run
    if not os.path.exists(CONFIG_PATH):
        os.makedirs(YAPPR_HOME, exist_ok=True)
        if os.path.exists(DEFAULT_CONFIG):
            import shutil
            shutil.copy(DEFAULT_CONFIG, CONFIG_PATH)
    cfg.read([DEFAULT_CONFIG, CONFIG_PATH])  # default first, user overrides
    return cfg


def save_config(cfg):
    os.makedirs(YAPPR_HOME, exist_ok=True)
    with open(CONFIG_PATH, "w") as f:
        cfg.write(f)


# ---------------- backend (llama-server) management ----------------

YAPPR_BIN = os.path.join(YAPPR_HOME, "bin", "llama-server")


def resolve_binary(configured):
    """Find llama-server: explicit path > ~/.yappr/bin (installed) > PATH (brew)."""
    import shutil
    if configured and configured != "auto":
        return configured if os.path.exists(configured) else None
    if os.path.exists(YAPPR_BIN):
        return YAPPR_BIN
    return shutil.which("llama-server")


def model_paths(cfg):
    """Return (weights_path, mmproj_path) in the HF cache, or (None, None) if absent."""
    try:
        from huggingface_hub import try_to_load_from_cache
    except Exception:
        return (None, None)
    repo = cfg.get("model", "repo", fallback="google/gemma-4-E4B-it-qat-q4_0-gguf")
    w = try_to_load_from_cache(repo, cfg.get("model", "weights",
                                            fallback="gemma-4-E4B_q4_0-it.gguf"))
    m = try_to_load_from_cache(repo, cfg.get("model", "mmproj",
                                            fallback="gemma-4-E4B-it-mmproj.gguf"))
    w = w if isinstance(w, str) and os.path.exists(w) else None
    m = m if isinstance(m, str) and os.path.exists(m) else None
    return (w, m)


def download_model(cfg, on_progress=None):
    """Download weights+mmproj into the HF cache. Returns (weights, mmproj) paths."""
    from huggingface_hub import hf_hub_download
    repo = cfg.get("model", "repo", fallback="google/gemma-4-E4B-it-qat-q4_0-gguf")
    if on_progress:
        on_progress("Downloading model weights (~4.8 GB)…")
    w = hf_hub_download(repo, cfg.get("model", "weights",
                                      fallback="gemma-4-E4B_q4_0-it.gguf"))
    if on_progress:
        on_progress("Downloading audio projector (~0.9 GB)…")
    m = hf_hub_download(repo, cfg.get("model", "mmproj",
                                      fallback="gemma-4-E4B-it-mmproj.gguf"))
    return (w, m)


def server_healthy(port):
    try:
        requests.get(f"http://127.0.0.1:{port}/health", timeout=2).raise_for_status()
        return True
    except Exception:
        return False


def install_script_path():
    """Locate the engine installer (packaged as engine_install.sh)."""
    p = os.path.join(PKG_DIR, "engine_install.sh")
    return p if os.path.exists(p) else None


def ensure_engine(on_progress=None):
    """If llama-server isn't found, run install.sh to fetch it. Returns binary path."""
    existing = resolve_binary("auto")
    if existing:
        return existing
    script = install_script_path()
    if not script:
        log("install.sh not found; cannot auto-install engine")
        return None
    if on_progress:
        on_progress("⬇️ Installing engine (first run)…")
    log("running install.sh to fetch llama.cpp")
    try:
        subprocess.run(["bash", script], check=True,
                       stdout=open("/tmp/yappr-install.log", "w"),
                       stderr=subprocess.STDOUT, timeout=600)
    except Exception as e:
        log("engine install failed:", repr(e))
        return None
    return resolve_binary("auto")


def start_server(cfg, weights, mmproj):
    """Launch llama-server as a child process. Returns the Popen (or None)."""
    binary = resolve_binary(cfg.get("server", "binary", fallback="auto"))
    if not binary:
        log("llama-server binary not found")
        return None
    port = cfg.get("server", "port", fallback="8089")
    args = [binary, "-m", weights, "--mmproj", mmproj, "-fa", "on", "--jinja",
            "-c", cfg.get("model", "ctx_size", fallback="8192"),
            "-ngl", cfg.get("model", "ngl", fallback="99"),
            "--port", str(port)]
    log("starting llama-server:", binary)
    logf = open("/tmp/yappr-llama-server.log", "w")
    return subprocess.Popen(args, stdout=logf, stderr=logf)


def list_input_devices():
    """Return [(index, name)] for devices with input channels."""
    out = []
    try:
        for i, d in enumerate(sd.query_devices()):
            if d.get("max_input_channels", 0) > 0:
                out.append((i, d["name"]))
    except Exception as e:
        print("device query failed:", e)
    return out


def resolve_device(name_substr):
    """Map a config device-name substring to a sounddevice index, or None for default."""
    if not name_substr:
        return None
    for idx, name in list_input_devices():
        if name_substr.lower() in name.lower():
            return idx
    return None


class Recorder:
    """Captures mono float32 audio at a fixed samplerate until stopped."""

    def __init__(self, samplerate, device_index, max_seconds, tail_seconds=0.4):
        self.samplerate = samplerate
        self.device_index = device_index
        self.max_seconds = max_seconds
        self.tail_seconds = tail_seconds
        self._q = queue.Queue()
        self._stream = None
        self._frames = []
        self._start = 0.0

    def _callback(self, indata, frames, time_info, status):
        self._q.put(indata.copy())

    def start(self):
        self._frames = []
        self._q = queue.Queue()
        self._start = time.time()
        self._stream = sd.InputStream(
            samplerate=self.samplerate,
            channels=1,
            dtype="float32",
            device=self.device_index,
            callback=self._callback,
        )
        self._stream.start()

    def stop(self):
        """Stop and return 16-bit PCM WAV bytes (or None if nothing/too short)."""
        if self._stream is None:
            return None
        # Capture a short trailing tail so the last word isn't clipped when the
        # user releases the key right as they finish speaking.
        if self.tail_seconds > 0:
            time.sleep(self.tail_seconds)
        self._stream.stop()
        self._stream.close()
        self._stream = None
        while not self._q.empty():
            self._frames.append(self._q.get())
        # Fully release the CoreAudio device so macOS clears the orange "mic in
        # use" indicator. Closing the stream alone leaves PortAudio holding the
        # HAL open; terminate forces release (we re-init on next start()).
        try:
            sd._terminate()
            sd._initialize()
        except Exception as e:
            log("portaudio reset failed:", e)
        if not self._frames:
            return None
        audio = np.concatenate(self._frames, axis=0).flatten()
        # enforce max length
        max_samples = int(self.max_seconds * self.samplerate)
        if len(audio) > max_samples:
            audio = audio[:max_samples]
        if len(audio) < int(0.2 * self.samplerate):  # < 200ms, ignore
            return None
        pcm16 = np.clip(audio * 32767, -32768, 32767).astype(np.int16)
        buf = io.BytesIO()
        with wave.open(buf, "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(self.samplerate)
            w.writeframes(pcm16.tobytes())
        return buf.getvalue()

    def elapsed(self):
        return time.time() - self._start if self._start else 0.0


def call_model(endpoint, timeout, text_prompt, wav_bytes=None, history=None,
               tools=None, return_message=False):
    """POST to llama-server. Thinking disabled.

    history: optional list of prior messages prepended (conversation context).
    tools: optional list of OpenAI tool schemas (enables tool_choice=auto).
    return_message: if True, return the full assistant message dict (so callers
        can inspect tool_calls); else return the stripped text content.
    """
    content = [{"type": "text", "text": text_prompt}]
    if wav_bytes is not None:
        b64 = base64.b64encode(wav_bytes).decode()
        content.append(
            {"type": "input_audio", "input_audio": {"data": b64, "format": "wav"}}
        )
    messages = list(history) if history else []
    messages.append({"role": "user", "content": content})
    payload = {
        "messages": messages,
        "temperature": 0,
        "max_tokens": 512,
        "reasoning_effort": "none",
        "chat_template_kwargs": {"enable_thinking": False},
    }
    if tools:
        payload["tools"] = tools
        payload["tool_choice"] = "auto"
    r = requests.post(endpoint, json=payload, timeout=timeout)
    r.raise_for_status()
    msg = r.json()["choices"][0]["message"]
    if return_message:
        return msg
    return (msg.get("content") or "").strip()


def call_messages(endpoint, timeout, messages, tools=None, return_message=False):
    """Like call_model but takes a prebuilt messages list (for multi-turn tool loops)."""
    payload = {
        "messages": messages,
        "temperature": 0,
        "max_tokens": 512,
        "reasoning_effort": "none",
        "chat_template_kwargs": {"enable_thinking": False},
    }
    if tools:
        payload["tools"] = tools
        payload["tool_choice"] = "auto"
    r = requests.post(endpoint, json=payload, timeout=timeout)
    r.raise_for_status()
    msg = r.json()["choices"][0]["message"]
    if return_message:
        return msg
    return (msg.get("content") or "").strip()


WEB_SEARCH_TOOL = {
    "type": "function",
    "function": {
        "name": "web_search",
        "description": (
            "Search the web for current, recent, or time-sensitive information "
            "(news, prices, events, recent releases, anything after your training "
            "cutoff). Do NOT use for general knowledge, math, or definitions."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": (
                        "A specific, keyword-style search query. Include the "
                        "explicit date (e.g. 'June 6 2026') for time-sensitive "
                        "topics and a place name for local info. Prefer concrete "
                        "keywords over conversational phrasing — e.g. "
                        "'Trump news June 6 2026', not 'what did Trump do today'."
                    ),
                }
            },
            "required": ["query"],
        },
    },
}


def web_search(endpoint, query, max_results=5, timeout=15):
    """Query local SearXNG JSON API; return top results as formatted lines.

    Never raises into the caller: returns a short notice string on error/empty.
    """
    try:
        resp = requests.get(endpoint, params={"q": query, "format": "json"},
                            timeout=timeout)
        resp.raise_for_status()
        results = resp.json().get("results", [])[:max_results]
    except Exception as e:
        log("web_search error:", repr(e))
        return "No search results (search backend unavailable)."
    if not results:
        return "No search results found."
    lines = []
    for r in results:
        title = r.get("title", "").strip()
        content = (r.get("content", "") or "").strip()[:160]
        url = r.get("url", "")
        lines.append(f"- {title}: {content} ({url})")
    return "\n".join(lines)


def transcribe_prompt(target_lang, source_lang="auto"):
    """Build the audio prompt following Gemma's documented templates.

    ASR:  "Transcribe the following speech segment in {LANG} into {LANG} text."
    AST:  "Transcribe the following speech segment in {SRC}, then translate it
           into {TGT}." with bilingual output (we strip to target in code).
    The "digits not words" instruction matches Google's formatting guidance and
    improves dictation quality. Per the docs, audio goes AFTER this text.
    """
    src = source_lang if source_lang and source_lang.lower() != "auto" else "the original language"
    digits = ("Write digits rather than words (e.g. write 1.7 not one point seven, "
              "and 3 not three).")

    if not target_lang or target_lang.lower() == "auto":
        # ASR / verbatim
        return (
            f"Transcribe the following speech segment in {src} into text. "
            f"Output only the transcription with no extra commentary and no newlines. "
            f"{digits}"
        )
    # AST / translation: documented bilingual format; we parse out the target.
    return (
        f"Transcribe the following speech segment in {src}, then translate it into "
        f"{target_lang}. First output the transcription, then a newline, then "
        f"'{target_lang}: ' followed by the translation. {digits}"
    )


def parse_translation(raw, target_lang):
    """Extract just the target-language text from Gemma's bilingual AST output.

    Expected shape: "<source transcript>\\n{TARGET}: <translation>".
    Falls back to the raw text (last non-empty line) if the marker is absent.
    """
    if not raw:
        return raw
    marker = f"{target_lang}:"
    idx = raw.rfind(marker)
    if idx != -1:
        return raw[idx + len(marker):].strip()
    # fallback: last non-empty line
    lines = [ln.strip() for ln in raw.splitlines() if ln.strip()]
    return lines[-1] if lines else raw.strip()


def _post_key(keycode, flags=0):
    """Post a key down+up via Quartz (thread-safe, unlike pynput's TSM path)."""
    import Quartz
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStateHIDSystemState)
    for down in (True, False):
        ev = Quartz.CGEventCreateKeyboardEvent(src, keycode, down)
        if flags:
            Quartz.CGEventSetFlags(ev, flags)
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)


def paste_text(text):
    """Copy to clipboard and paste with Cmd-V using Quartz key events.

    pynput's Controller routes through macOS Text Input Sources, which asserts
    main-thread-only and SIGTRAPs from our worker thread. Quartz CGEventPost is
    thread-safe, so we use it instead.
    """
    import Quartz
    p = subprocess.Popen(["pbcopy"], stdin=subprocess.PIPE)
    p.communicate(text.encode("utf-8"))
    time.sleep(0.05)
    _post_key(9, Quartz.kCGEventFlagMaskCommand)  # 9 = 'v'


def type_text(text):
    """Type unicode directly via Quartz (thread-safe), no keymap needed."""
    import Quartz
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStateHIDSystemState)
    for down in (True, False):
        ev = Quartz.CGEventCreateKeyboardEvent(src, 0, down)
        Quartz.CGEventKeyboardSetUnicodeString(ev, len(text), text)
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)


def accessibility_trusted(prompt=False):
    """Return True if this process is trusted for Accessibility (synthetic input).

    If prompt=True, ask macOS to show the 'grant access' dialog for this process.
    """
    try:
        from ApplicationServices import (
            AXIsProcessTrustedWithOptions,
            kAXTrustedCheckOptionPrompt,
        )
        return bool(AXIsProcessTrustedWithOptions({kAXTrustedCheckOptionPrompt: bool(prompt)}))
    except Exception:
        try:
            from ApplicationServices import AXIsProcessTrusted
            return bool(AXIsProcessTrusted())
        except Exception:
            return False


def input_monitoring_status(request=False):
    """Return Input Monitoring (key listening) status: 'granted'|'denied'|'unknown'.

    The hotkey listener silently sees zero keys without this permission — it is
    SEPARATE from Accessibility. If request=True, ask macOS to prompt for it.
    """
    import ctypes, ctypes.util
    try:
        iokit = ctypes.CDLL(ctypes.util.find_library("IOKit"))
        iokit.IOHIDCheckAccess.restype = ctypes.c_int
        iokit.IOHIDCheckAccess.argtypes = [ctypes.c_uint32]
        rv = iokit.IOHIDCheckAccess(1)  # 1 = kIOHIDRequestTypeListenEvent
        status = {0: "granted", 1: "denied", 2: "unknown"}.get(rv, "unknown")
        if request and status != "granted":
            iokit.IOHIDRequestAccess.restype = ctypes.c_bool
            iokit.IOHIDRequestAccess.argtypes = [ctypes.c_uint32]
            iokit.IOHIDRequestAccess(1)  # triggers the system prompt once
        return status
    except Exception as e:
        log("input_monitoring_status error:", e)
        return "unknown"


def clean_for_speech(text):
    """Strip emoji/markdown so `say` reads clean prose, not '🐧'->'penguin head'
    or '###'/'*' aloud."""
    import re
    t = text
    # drop fenced/inline code, markdown headers, list/emphasis markers
    t = re.sub(r"```.*?```", " ", t, flags=re.S)
    t = re.sub(r"`([^`]*)`", r"\1", t)
    t = re.sub(r"^\s{0,3}#{1,6}\s*", "", t, flags=re.M)   # headers
    t = re.sub(r"^\s*[-*•]\s+", "", t, flags=re.M)        # bullets
    t = re.sub(r"^\s*\d+\.\s+", "", t, flags=re.M)        # numbered lists
    t = re.sub(r"[*_~]{1,3}", "", t)                      # strip emphasis markers
    # remove emoji & pictographs
    t = re.sub(
        "[\U0001F000-\U0001FAFF\U00002600-\U000027BF\U0001F1E6-\U0001F1FF←-⇿⌀-⏿]",
        " ", t)
    t = re.sub(r"[ \t]+", " ", t)
    t = re.sub(r"\n{2,}", "\n", t)
    return t.strip()


def speak(text, voice, rate):
    args = ["say"]
    if voice:
        args += ["-v", voice]
    if rate:
        args += ["-r", str(rate)]
    args.append(text)
    return subprocess.Popen(args)


# menu-bar icons (PNG frames generated by make_icons.py). Worker threads set
# self.state (a string); a main-thread Timer renders the matching icon, so all
# AppKit icon mutation stays on the main thread and recording can animate.
ICON_DIR = os.path.join(ASSET_DIR, "icons")
REC_FRAMES = sorted(
    os.path.join(ICON_DIR, f) for f in os.listdir(ICON_DIR)
    if f.startswith("rec_")) if os.path.isdir(ICON_DIR) else []
IDLE_ICON = os.path.join(ICON_DIR, "idle.png")
THINK_ICON = os.path.join(ICON_DIR, "think.png")
# transient text statuses still use a glyph in the dropdown line
ICON_OK = "✅"
ICON_ERR = "⚠️"


def log(*parts):
    """Print a timestamped event line to stderr (visible when run from terminal)."""
    ts = time.strftime("%H:%M:%S")
    msg = " ".join(str(p) for p in parts)
    print(f"[{ts}] {msg}", file=sys.stderr, flush=True)


def cmd_is_down():
    """True if Command is physically held right now (order-independent)."""
    try:
        import Quartz
        flags = Quartz.CGEventSourceFlagsState(Quartz.kCGEventSourceStateHIDSystemState)
        return bool(flags & Quartz.kCGEventFlagMaskCommand)
    except Exception:
        return False


def beep(name):
    """Play a system sound by name, or skip if name is empty/'none'."""
    if not name or name.lower() == "none":
        return
    subprocess.Popen(["afplay", f"/System/Library/Sounds/{name}.aiff"])


def notify(title, subtitle, message=""):
    """Best-effort notification; never crash if the bundle lacks an id."""
    try:
        rumps.notification(title, subtitle, message)
    except Exception:
        pass


class YapprApp(rumps.App):
    def __init__(self):
        super().__init__("Yappr", quit_button=None)
        self.cfg = load_config()
        self.last_transcript = ""
        self.busy = False
        self._say_proc = None      # current `say` subprocess, if speaking
        self._cancel = False       # set when user interrupts an in-flight action
        self._chat_history = []     # [{"role","content"}] recent chat turns
        self._chat_last_ts = 0.0    # when the last chat turn happened

        # icon state machine (worker threads set self.state; Timer renders it)
        self.state = "idle"        # idle | recording | thinking | speaking
        self._rec_frame = 0
        self.title = None
        self._apply_idle_icon()

        # audio + recorder
        sr = self.cfg.getint("audio", "samplerate", fallback=16000)
        max_s = self.cfg.getint("audio", "max_seconds", fallback=28)
        tail = self.cfg.getfloat("audio", "tail_seconds", fallback=0.4)
        dev = resolve_device(self.cfg.get("audio", "device", fallback=""))
        self.recorder = Recorder(sr, dev, max_s, tail)

        self._build_menu()
        self._start_hotkey_listener()

        # Main-thread timer drives icon rendering/animation (AppKit is not
        # thread-safe; workers only mutate self.state).
        self._icon_timer = rumps.Timer(self._render_icon, 0.12)
        self._icon_timer.start()

        # Check both permissions at startup and surface clear status.
        # Input Monitoring (key listening) is SEPARATE from Accessibility (typing);
        # without it the hotkey listener silently sees nothing.
        im = input_monitoring_status(request=True)
        ax = accessibility_trusted(prompt=True)
        log(f"permissions: input_monitoring={im} accessibility={'yes' if ax else 'no'}")
        missing = []
        if im != "granted":
            missing.append("Input Monitoring (hotkeys)")
        if not ax:
            missing.append("Accessibility (typing)")
        if missing:
            self._set_status("⚠️ Grant: " + " + ".join(missing) + ", then restart")
            notify(
                "Yappr", "Permissions needed: " + ", ".join(missing),
                "Enable Yappr in System Settings > Privacy & Security for "
                + (" and ".join(missing)) + ", then relaunch Yappr.")

        # Bring up the backend (detect/download model, start llama-server) in the
        # background so the menu bar stays responsive during a first-run download.
        self._server_proc = None
        if self.cfg.getboolean("server", "manage", fallback=True):
            threading.Thread(target=self._ensure_backend, daemon=True).start()

    def _ensure_backend(self):
        """Detect model (download if missing) and start llama-server if needed."""
        try:
            port = self.cfg.getint("server", "port", fallback=8089)
            if server_healthy(port):
                log("llama-server already running")
                self._set_status("Ready")
                return
            # Ensure the engine is installed (auto-fetch prebuilt if missing).
            if not resolve_binary(self.cfg.get("server", "binary", fallback="auto")):
                if not ensure_engine(on_progress=self._set_status):
                    self._set_status("⚠️ Engine install failed (see log)")
                    notify("Yappr", "Engine install failed",
                           "Run install.sh manually, or 'brew install llama.cpp'.")
                    return
            weights, mmproj = model_paths(self.cfg)
            if not weights or not mmproj:
                self._set_status("⬇️ Downloading model (first run)…")
                log("model missing; downloading")
                weights, mmproj = download_model(self.cfg, on_progress=self._set_status)
            self._set_status("Starting model server…")
            self._server_proc = start_server(self.cfg, weights, mmproj)
            if not self._server_proc:
                self._set_status("⚠️ Could not start engine")
                return
            for _ in range(120):  # wait up to ~60s for model load
                if server_healthy(port):
                    log("llama-server healthy")
                    self._set_status("Ready")
                    return
                time.sleep(0.5)
            self._set_status("⚠️ Server didn't become ready")
        except Exception as e:
            log("backend startup error:", repr(e))
            self._set_status("⚠️ Backend error (see log)")

    # ---------- icon / status feedback ----------
    def _apply_idle_icon(self):
        if os.path.exists(IDLE_ICON):
            self.template = True   # tint to menu-bar theme (light/dark)
            self.icon = IDLE_ICON
            self.title = None
        else:
            self.title = "🎙️"      # fallback if icons not generated

    def _render_icon(self, _timer):
        """Runs on the main thread; paints the icon for the current state."""
        st = self.state
        if st == "recording" and REC_FRAMES:
            self.template = False  # keep the colors
            self.icon = REC_FRAMES[self._rec_frame % len(REC_FRAMES)]
            self.title = None
            self._rec_frame += 1
        elif st == "thinking" and os.path.exists(THINK_ICON):
            self.template = True
            self.icon = THINK_ICON
            self.title = None
        elif st == "speaking":
            # reuse the colored waveform but hold a single lively frame
            if REC_FRAMES:
                self.template = False
                self.icon = REC_FRAMES[3]
                self.title = None
        else:  # idle
            self._apply_idle_icon()

    def _set_state(self, state):
        self.state = state
        if state == "recording":
            self._rec_frame = 0

    def _set_recording(self, mode):
        self._set_state("recording")
        self._set_status(f"● Recording ({mode})…")
        beep(self.cfg.get("audio", "beep_start", fallback="none"))

    def _flash(self, glyph, status, revert_after=1.5):
        # glyph kept for compatibility; icon returns to idle after the delay
        self._set_state("idle")
        self._set_status(f"{glyph} {status}" if glyph else status)

    # ---------- menu ----------
    def _build_menu(self):
        self.menu.clear()
        # Reflect real backend state; _ensure_backend flips this to Ready.
        initial = "Ready" if server_healthy(
            self.cfg.getint("server", "port", fallback=8089)) else "Starting…"
        self.status_item = rumps.MenuItem(getattr(self, "_status_text", initial))
        self.menu.add(self.status_item)
        self.menu.add(rumps.separator)

        # Microphone submenu
        mic_menu = rumps.MenuItem("Microphone")
        current_dev = self.cfg.get("audio", "device", fallback="")
        default_item = rumps.MenuItem("System Default", callback=self._pick_mic)
        default_item.state = 1 if not current_dev else 0
        mic_menu.add(default_item)
        for idx, name in list_input_devices():
            it = rumps.MenuItem(name, callback=self._pick_mic)
            it.state = 1 if current_dev and current_dev.lower() in name.lower() else 0
            mic_menu.add(it)
        self.menu.add(mic_menu)

        # Model submenu
        model_menu = rumps.MenuItem("Model")
        active = self.cfg.get("models", "active", fallback="")
        for sec in self.cfg.sections():
            if sec.startswith("model:"):
                key = sec.split(":", 1)[1]
                label = self.cfg.get(sec, "label", fallback=key)
                it = rumps.MenuItem(label, callback=self._pick_model)
                it._model_key = key
                it.state = 1 if key == active else 0
                model_menu.add(it)
        self.menu.add(model_menu)

        # Language submenu
        lang_menu = rumps.MenuItem("Output Language")
        target = self.cfg.get("language", "target", fallback="auto")
        opts = [o.strip() for o in self.cfg.get("language", "options", fallback="auto").split(",")]
        for o in opts:
            label = "Verbatim (no translation)" if o == "auto" else o
            it = rumps.MenuItem(label, callback=self._pick_language)
            it._lang = o
            it.state = 1 if o == target else 0
            lang_menu.add(it)
        self.menu.add(lang_menu)

        self.menu.add(rumps.separator)
        self.menu.add(rumps.MenuItem("Copy Last Transcript", callback=self._copy_last))
        self.menu.add(rumps.separator)
        self.menu.add(rumps.MenuItem("Quit", callback=self._quit))

    def _set_status(self, text):
        self._status_text = text  # survive menu rebuilds
        self.status_item.title = text
        log("status:", text)

    # ---------- menu callbacks ----------
    def _pick_mic(self, sender):
        name = "" if sender.title == "System Default" else sender.title
        self.cfg.set("audio", "device", name)
        save_config(self.cfg)
        self.recorder.device_index = resolve_device(name)
        self._build_menu()
        self._set_status(f"Mic: {sender.title}")

    def _pick_model(self, sender):
        self.cfg.set("models", "active", sender._model_key)
        save_config(self.cfg)
        self._build_menu()
        self._set_status(f"Model: {sender.title}")
        notify("Yappr", "Model changed",
                           f"{sender.title}. Make sure llama-server is serving it.")

    def _pick_language(self, sender):
        self.cfg.set("language", "target", sender._lang)
        save_config(self.cfg)
        self._build_menu()
        self._set_status(f"Language: {sender.title}")

    def _copy_last(self, _):
        if self.last_transcript:
            p = subprocess.Popen(["pbcopy"], stdin=subprocess.PIPE)
            p.communicate(self.last_transcript.encode("utf-8"))
            self._set_status("Copied last transcript")
        else:
            self._set_status("No transcript yet")

    def _quit(self, _):
        # stop the llama-server child if we started it
        if getattr(self, "_server_proc", None) and self._server_proc.poll() is None:
            log("stopping managed llama-server")
            self._server_proc.terminate()
        rumps.quit_application()

    # ---------- hotkey + gesture ----------
    def _start_hotkey_listener(self):
        key_name = self.cfg.get("hotkey", "key", fallback="right_option")
        self.hotkey = KEY_MAP.get(key_name, keyboard.Key.alt_r)
        if key_name == "fn":
            notify("Yappr", "Heads up",
                               "fn key is unreliable on macOS; using right_cmd fallback.")
        self._key_down = False
        self._mode = None  # 'transcribe' or 'chat'

        listener = keyboard.Listener(on_press=self._on_press, on_release=self._on_release)
        listener.daemon = True
        listener.start()

    def _on_press(self, key):
        if key != self.hotkey:
            return
        log(f"hotkey press seen (key_down={self._key_down} busy={self.busy} state={self.state})")
        if self._key_down:
            return
        # If something is already running, a press means "stop/interrupt".
        if self.busy:
            log("hotkey while busy -> interrupt")
            self._interrupt()
            return
        self._key_down = True
        # Query the REAL Cmd state now (order-independent), not a tracked flag.
        self._mode = "chat" if cmd_is_down() else "transcribe"
        log("hotkey down ->", self._mode)
        try:
            self.recorder.start()
            self._set_recording(self._mode)
        except Exception as e:
            self.busy = False
            self._set_status("Mic error")
            log("record start FAILED:", e)

    def _on_release(self, key):
        if key != self.hotkey or not self._key_down:
            return
        self._key_down = False
        mode = self._mode
        log("hotkey up; processing as", mode)
        # process in a worker thread so the UI/listener stay responsive
        t = threading.Thread(target=self._process, args=(mode,), daemon=True)
        t.start()

    def _interrupt(self):
        """Stop the in-flight action: kill speech and flag the worker to bail."""
        self._cancel = True
        if self._say_proc and self._say_proc.poll() is None:
            self._say_proc.terminate()
            log("killed speech")
        self._flash("", "Stopped", revert_after=1.0)

    def _answer_chat(self, endpoint, timeout, question):
        """Produce a chat answer, letting the model call web_search when it needs
        current info. Returns the final answer text."""
        now = time.strftime("%A, %B %d, %Y, %I:%M %p %Z").strip()
        system = (f"The current date and time is {now}. "
                  "Answer the user's spoken question concisely in plain spoken "
                  "prose. Do not use markdown, headings, bullet points, code "
                  "blocks, or emoji. "
                  "When a question is time-sensitive or local, search with a "
                  "specific keyword query that includes today's date and any place "
                  "name. After searching, give the best direct answer you can from "
                  "the results, including specific facts, names, and numbers found. "
                  "Do not tell the user to check websites themselves; summarize "
                  "what the results say.")
        # System role + clean user question. (Embedding the instruction inside the
        # user turn suppresses tool calling — keep them separate.)
        messages = [{"role": "system", "content": system}]
        messages += list(self._chat_history)
        messages.append({"role": "user", "content": question})

        search_on = self.cfg.getboolean("search", "enabled", fallback=False)
        tools = [WEB_SEARCH_TOOL] if search_on else None

        msg = call_messages(endpoint, timeout, messages, tools=tools, return_message=True)

        # one tool hop: if the model asked to search, run it and synthesize
        calls = msg.get("tool_calls") if isinstance(msg, dict) else None
        if calls:
            messages.append(msg)  # the assistant tool-call turn
            for tc in calls:
                if tc.get("function", {}).get("name") != "web_search":
                    continue
                try:
                    query = json.loads(tc["function"]["arguments"]).get("query", question)
                except Exception:
                    query = question
                self._set_status(f"🔎 Searching: {query[:40]}")
                log("web_search:", query)
                ep = self.cfg.get("search", "endpoint")
                mx = self.cfg.getint("search", "max_results", fallback=5)
                to = self.cfg.getint("search", "timeout", fallback=15)
                results = web_search(ep, query, mx, to)
                messages.append({"role": "tool", "tool_call_id": tc.get("id", ""),
                                "name": "web_search", "content": results})
            if self._cancel:
                return ""
            self._set_status("Thinking…")
            return call_messages(endpoint, timeout, messages)  # no tools: force an answer
        return (msg.get("content") or "").strip()

    # ---------- processing ----------
    def _process(self, mode):
        self.busy = True
        self._cancel = False
        try:
            wav = self.recorder.stop()
            beep(self.cfg.get("audio", "beep_stop", fallback="none"))
            if not wav:
                log("no audio captured")
                self._flash(ICON_ERR, "Nothing captured")
                return
            log(f"captured {len(wav)} bytes wav; calling model ({mode})")
            self._set_state("thinking")
            endpoint = self.cfg.get("server", "endpoint")
            timeout = self.cfg.getint("server", "timeout", fallback=60)
            target = self.cfg.get("language", "target", fallback="auto")
            source = self.cfg.get("language", "source", fallback="auto")

            if mode == "chat":
                # First transcribe the question (verbatim), then answer it.
                self._set_status("Transcribing question…")
                question = call_model(endpoint, timeout, transcribe_prompt("auto", source), wav)
                if self._cancel:
                    log("cancelled after transcription"); return
                self.last_transcript = question
                self._set_status(f"Thinking… ({question[:40]})")
                # short-lived context: keep history only within the time window
                window = self.cfg.getint("chat", "context_seconds", fallback=60)
                now = monotonic()
                if now - self._chat_last_ts > window:
                    if self._chat_history:
                        log("context expired, clearing history")
                    self._chat_history = []
                answer = self._answer_chat(endpoint, timeout, question)
                if self._cancel:
                    log("cancelled before speaking"); return
                # record the turn for context (text-only, keep last few)
                self._chat_history.append({"role": "user", "content": question})
                self._chat_history.append({"role": "assistant", "content": answer})
                self._chat_history = self._chat_history[-8:]  # cap ~4 turns
                self._chat_last_ts = now
                log("heard:", repr(question))
                log("answer:", repr(answer))
                voice = self.cfg.get("chat", "voice", fallback="")
                rate = self.cfg.getint("chat", "rate", fallback=190)
                spoken = clean_for_speech(answer)
                log("speaking:", repr(spoken[:60]))
                self._set_state("speaking")
                self._set_status(f"🔊 {answer[:50]}")
                # Keep the proc handle so a hotkey press can interrupt speech,
                # and wait for it so we stay "busy" until the reply finishes.
                self._say_proc = speak(spoken, voice, rate)
                self._say_proc.wait()
                self._say_proc = None
                if self._cancel:
                    return
                self._flash(ICON_OK, "Replied", revert_after=2.0)
            else:
                self._set_status("Transcribing…")
                raw = call_model(endpoint, timeout, transcribe_prompt(target, source), wav)
                if self._cancel:
                    log("cancelled after transcription"); return
                if not raw:
                    self._flash(ICON_ERR, "Empty transcript")
                    return
                # translation mode returns bilingual output; strip to target
                if target and target.lower() != "auto":
                    text = parse_translation(raw, target)
                else:
                    text = raw
                self.last_transcript = text
                log("transcript:", repr(text))
                inject = self.cfg.get("output", "inject", fallback="paste")
                # paste/type need Accessibility; fall back to clipboard if not trusted
                if not accessibility_trusted(prompt=False):
                    log("NOT accessibility-trusted -> clipboard fallback")
                    subprocess.Popen(["pbcopy"], stdin=subprocess.PIPE).communicate(
                        text.encode("utf-8"))
                    self._flash(ICON_ERR, "Copied (grant Accessibility to auto-type)",
                                revert_after=3.0)
                    notify(
                        "Yappr", "Transcript copied to clipboard",
                        "Enable Yappr in Accessibility to type at the cursor. Paste with ⌘V.")
                else:
                    log("accessibility OK -> inject via", inject)
                    if inject == "type":
                        type_text(text)
                    else:
                        paste_text(text)
                    self._flash(ICON_OK, f"Inserted: {text[:50]}")
        except requests.exceptions.ConnectionError:
            log("server offline")
            self._flash(ICON_ERR, "Server offline", revert_after=3.0)
            notify("Yappr", "Backend offline",
                               "Start llama-server on the configured port.")
        except Exception as e:
            log("process ERROR:", repr(e))
            self._flash(ICON_ERR, "Error")
        finally:
            self.busy = False
            self._set_state("idle")


def _acquire_single_instance_lock():
    """Ensure only one Yappr runs. A NEW launch wins: it terminates any prior
    instance (read from the lockfile) before taking the lock, so rebuilds never
    leave stale stacked icons.
    """
    import fcntl, signal
    lock_path = os.path.join(os.path.expanduser("~"), ".yappr.lock")
    # try to terminate a previously-recorded instance
    try:
        if os.path.exists(lock_path):
            with open(lock_path) as f:
                old = f.read().strip()
            if old.isdigit() and int(old) != os.getpid():
                try:
                    os.kill(int(old), signal.SIGTERM)
                    log(f"terminated prior Yappr instance (pid {old})")
                    time.sleep(0.5)
                except ProcessLookupError:
                    pass
    except Exception as e:
        log("prior-instance check failed:", e)
    fd = open(lock_path, "w")
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        return False  # another instance grabbed it in the race; we yield
    fd.write(str(os.getpid()))
    fd.flush()
    _acquire_single_instance_lock._fd = fd  # keep alive
    return True


def main():
    if sys.platform != "darwin":
        print("This app targets macOS.")
        sys.exit(1)
    if not _acquire_single_instance_lock():
        log("another Yappr instance is already running; exiting")
        sys.exit(0)
    YapprApp().run()


if __name__ == "__main__":
    main()
