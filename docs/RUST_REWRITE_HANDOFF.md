# Yappr — Rust Rewrite Handoff

This document hands off a planned **native Rust rewrite** of Yappr's app shell.
It captures everything proven in the Python prototype so the Rust build starts
from verified facts, not rediscovery.

> **TL;DR**: Keep the backend (local `llama-server` + Gemma 4 E4B) and `install.sh`
> exactly as-is — they're language-agnostic and proven. Rewrite only the **macOS
> app shell** (menu bar, hotkeys, audio capture, text injection, server mgmt) in
> Rust to escape Python/py2app's code-signing & TCC-permission hell.

---

## 1. Why Rust (the motivation)

The Python prototype works *functionally* but the **packaging is fundamentally
broken on macOS**. In one session, py2app failed three distinct ways:

1. **Alias mode** (`py2app -A`): the bundle symlinks to the venv's `python`,
   whose ad-hoc code signature ≠ the app's signature. macOS TCC keys permissions
   to the *running process's* identity → it never matches → **re-prompts every
   launch, silently drops mic/hotkey access**. This caused the maddening
   "sometimes dictation works, sometimes not — depends how I launch it."
2. **Standalone + conda Python**: py2app couldn't bundle conda-forge's external
   `libffi.8.dylib` → crash on launch (`_ctypes` import).
3. **Standalone + uv standalone Python**: py2app 0.28 chokes on
   `zlib.__file__` (built-in module) from python-build-standalone.

**Root cause**: Python's "interpreter + script" model means the *interpreter* is
the OS-level process identity, not your app. macOS TCC (Transparency, Consent,
Control) binds permissions to a stable code signature + path. These fight.

**Rust eliminates the entire class**: one statically-linked, signed Mach-O binary
**is** the app identity. Grant once, it sticks. No interpreter, no dylib
relocation, no bundler. ~5 MB vs ~80 MB. Better fit for the LocalLLaMA/HN audience.

| Python bug hit this session | Rust outcome |
|---|---|
| Re-prompts every launch | one signed binary = stable identity, grant sticks |
| conda `libffi` missing | statically linked, no external interpreter dylibs |
| standalone `zlib` crash | no py2app, no bundler at all |
| "depends how I launch it" | TCC matches the one stable Mach-O identity |
| pynput TSM SIGTRAP on paste | use CGEvent APIs directly (see §6) |

---

## 2. Decisions already made (do not re-litigate)

- **Hotkey model**: keep **push-to-talk (hold)**, same as Python. Right Option
  held = dictate; Cmd+Right Option held = chat; tap hotkey while busy = interrupt.
  This requires a low-level CGEventTap (needs Input Monitoring) — fine, because a
  properly-signed binary grants once and persists.
- **Scope**: **full parity in one go** (dictation + chat + web_search + interrupt
  + menu + translation + context). The logic is all proven; porting it is
  mechanical.
- **Backend stays**: shell out to `llama-server` (do NOT link libllama). Same as
  Python. The engine install path (`install.sh` / brew / prebuilt) is unchanged.
- **Model**: Gemma 4 E4B QAT, `google/gemma-4-E4B-it-qat-q4_0-gguf` (weights +
  audio mmproj). The mmproj is **BF16** (the precision llama.cpp recommends for
  audio). Downloaded on first run, not bundled.
- **App identity**: `com.rpwr021.yappr`, signed with a stable self-signed cert
  (see §8). Install to `/Applications/Yappr.app` for one canonical path.

---

## 3. Architecture (unchanged from Python)

```
[Right Option held] → record mic (16kHz mono WAV)
        │
        ├─ dictate: POST audio → llama-server /v1/chat/completions
        │           → transcript → paste at cursor (Cmd+V via CGEvent)
        │
        └─ chat (Cmd+Option): POST audio → transcript (the question)
                  → POST question (+ history + tools) → answer
                  → optional web_search tool-call → SearXNG → re-POST → answer
                  → speak via macOS `say`
```

Three local pieces:
1. **Yappr.app** (the Rust shell you're building) — UI, hotkeys, audio, mgmt.
2. **llama-server** — Gemma 4 E4B, OpenAI-compatible, on `127.0.0.1:8089`.
   Yappr starts it as a child on launch, stops it on quit.
3. **SearXNG** (optional) — local Docker container on `127.0.0.1:8888` for the
   web_search tool. Degrades gracefully when absent.

Gemma is **input-only for audio** (ASR + translation). It CANNOT generate speech.
Spoken replies are macOS `say`. (Confirmed against Google docs + llama.cpp PR
#21421 "audio conformer encoder" — audio→text only.)

---

## 4. The llama-server contract (PROVEN — copy exactly)

Endpoint: `POST http://127.0.0.1:8089/v1/chat/completions` (OpenAI-compatible).

### Critical request params (every call)
```json
{
  "messages": [...],
  "temperature": 0,
  "max_tokens": 512,
  "reasoning_effort": "none",
  "chat_template_kwargs": {"enable_thinking": false}
}
```
**`reasoning_effort:none` + `enable_thinking:false` are MANDATORY.** Without them
Gemma emits hidden "thinking" tokens and returns empty `content` with
`finish_reason:"length"` — looks broken. This was a real bug.

### Audio input
Add to the user message `content` array (audio AFTER any text, per Gemma docs):
```json
{"type": "input_audio", "input_audio": {"data": "<base64 wav>", "format": "wav"}}
```

### Transcription prompt (follow Gemma's documented template)
ASR (verbatim): system or user text =
```
Transcribe the following speech segment in <SOURCE or "the original language">
into text. Output only the transcription with no extra commentary and no
newlines. Write digits rather than words (e.g. write 1.7 not one point seven,
and 3 not three).
```
Translation (AST): Gemma emits **bilingual** output
`"<source transcript>\n<TARGET>: <translation>"`. Send:
```
Transcribe the following speech segment in <SRC>, then translate it into
<TARGET>. First output the transcription, then a newline, then '<TARGET>: '
followed by the translation. <digits instruction>
```
Then **parse out only the target** (text after the last `"<TARGET>:"` marker;
fallback to last non-empty line). Verified: English speech → clean Spanish.

### Chat answer (system role is critical)
Use a **separate `system` message** + clean `user` question. Embedding the
instruction inside the user turn **suppresses tool calling** (real bug found).
```
system: "The current date and time is <now>. Answer the user's spoken question
concisely in plain spoken prose. Do not use markdown, headings, bullet points,
code blocks, or emoji. When time-sensitive or local, search with a specific
keyword query that includes today's date and any place name. After searching,
give the best direct answer from the results, including specific facts and
numbers. Do not tell the user to check websites; summarize what the results say."
user: "<the transcribed question>"
```
**Inject the current date/time into the system prompt** — Gemma has no clock and
otherwise says "I need to know the current date." Verified fix.

### Tool calling (web_search) — VERIFIED WORKING
Gemma 4 E4B supports OpenAI tools. Pass:
```json
"tools": [{
  "type": "function",
  "function": {
    "name": "web_search",
    "description": "Search the web for current, recent, or time-sensitive
      information (news, prices, events, recent releases, anything after your
      training cutoff). Do NOT use for general knowledge, math, or definitions.",
    "parameters": {"type": "object",
      "properties": {"query": {"type": "string",
        "description": "A specific, keyword-style query. Include the explicit
          date (e.g. 'June 6 2026') for time-sensitive topics and a place name
          for local info. Prefer concrete keywords over conversational phrasing —
          e.g. 'Trump news June 6 2026', not 'what did Trump do today'."}},
      "required": ["query"]}}}],
"tool_choice": "auto"
```
Flow: first call returns `finish_reason:"tool_calls"` with
`tool_calls[0].function.arguments = {"query":"..."}`. Run the search, append the
assistant tool-call message + a `{"role":"tool","tool_call_id":...,"name":
"web_search","content":"<results>"}` message, then POST again (no tools) to get
the synthesized answer. Verified end-to-end against live SearXNG.

Math questions correctly do NOT trigger search (Gemma answers directly).
Caveat: Gemma 4 E4B is **bad at mental arithmetic** (model limitation, not a bug).

### SearXNG query
`GET http://127.0.0.1:8888/search?q=<query>&format=json` → `results[]` with
`title`, `content`, `url`. Format top-N (default 5) as
`"- {title}: {content[:160]} ({url})"` lines. Return a short "no results" string
on error — never propagate the failure into the chat loop.
**Snippets only** (no page fetching) — decided for speed. Note: snippets often
lack live values (exact current temperature etc.); that's an accepted limitation.

---

## 5. Exact behaviors to replicate

- **Gestures**: hold Right Option = transcribe; hold Cmd + Right Option = chat.
  Detect Cmd via real modifier state at press time (not event-order tracking).
- **Interrupt**: a hotkey press while busy stops the current action — kill the
  `say` process; set a cancel flag the worker checks between server calls.
- **Trailing tail**: keep recording ~0.4s AFTER key release (`tail_seconds`) so
  the last word isn't clipped. Real bug fix — push-to-talk users release early.
- **Silent-mic detection**: compute peak amplitude of captured audio; if ~0
  (< 0.001), the Microphone permission is missing (macOS feeds silence with no
  error). Show "No audio — grant Microphone" instead of sending zeros. Critical:
  this exact failure wasted hours.
- **Conversation context**: keep chat history within a ~60s rolling window
  (`context_seconds`), cap ~4 turns. Clear after the window. Store only the final
  question + answer text (not tool plumbing).
- **Audio cap**: 28s max (Gemma's limit is 30s); cut early.
- **Audio format**: 16 kHz mono, 16-bit PCM WAV.
- **Text injection**: `paste` (clipboard + Cmd+V) by default, or `type`. Paste
  needs Accessibility.
- **Menu**: status line (Starting…/Ready/recording/etc.), Microphone picker,
  Model, Output Language (auto/verbatim + translate targets), Copy Last
  Transcript, Quit.
- **Menu-bar icon states**: idle (monochrome waveform, template-tinted), recording
  (animated colored waveform), thinking, speaking. Icons in `src/yappr/assets/
  icons/` (regenerate via `scripts/make_icons.py`). For Rust, either reuse the
  PNGs or draw via the menu-bar lib; idle should be a **template image** so macOS
  tints it to light/dark menu bars.
- **Single instance**: new launch should terminate any prior instance (lockfile
  with PID at `~/.yappr.lock`), so rebuilds never stack menu-bar icons.
- **Permission checks at startup**: check Input Monitoring (IOHIDCheckAccess),
  Accessibility (AXIsProcessTrusted), and **Microphone** (AVCaptureDevice auth) —
  surface clearly what's missing. The Python version forgot Microphone; don't.

---

## 6. Suggested Rust crates / APIs

These are starting points — verify current versions/maintenance before relying.

| Need | Option(s) | Notes |
|---|---|---|
| Menu-bar (tray) + menu | `tray-icon` + `muda` (Tauri ecosystem), or `cacao`/`objc2` directly | tray-icon handles template images + dynamic icon swap for animation |
| Event loop | `tao` (pairs with tray-icon) or `winit` | needed to host the tray + timer for icon animation |
| Global hotkey (push-to-talk hold) | **CGEventTap** via `core-graphics` / `objc2-core-graphics` | `global-hotkey` crate only does registered combos, NOT hold-detection of a bare modifier. You need a tap to see Right-Option down/up. Requires Input Monitoring. |
| Real modifier state | `CGEventSourceFlagsState` (core-graphics) | for the Cmd-held check |
| Mic capture | `cpal` | 16kHz mono f32 → convert to i16 WAV. Triggers the Microphone TCC prompt. |
| WAV encode | `hound` | simple PCM WAV writer |
| HTTP | `reqwest` (blocking or async) | to llama-server + SearXNG |
| JSON | `serde` / `serde_json` | requests + tool-call parsing |
| Paste (Cmd+V) / type | **CGEventCreateKeyboardEvent + CGEventPost** | NOT a high-level lib that routes through TSM (that crashed in Python). Post key 9 (`v`) with Command flag. For arbitrary text use `CGEventKeyboardSetUnicodeString`. |
| Clipboard | `arboard` or pbcopy via `std::process` | |
| TTS | shell out to `/usr/bin/say` via `std::process::Command` | keep proc handle to kill on interrupt |
| Accessibility check | `AXIsProcessTrustedWithOptions` (objc2 / FFI) | |
| Input Monitoring check | `IOHIDCheckAccess(kIOHIDRequestTypeListenEvent)` via IOKit FFI | |
| Microphone check | `AVCaptureDevice authorizationStatusForMediaType` (objc2 AVFoundation) | |
| Config | `toml` or keep INI via `rust-ini` | user config at `~/.yappr/config.ini` |

**Bundle/sign**: build the binary, assemble a minimal `Yappr.app` (Info.plist with
`LSUIElement=true`, `NSMicrophoneUsageDescription`, `CFBundleIdentifier=
com.rpwr021.yappr`), copy the binary to `Contents/MacOS/Yappr`, `codesign` with
the stable cert. No py2app. Consider `cargo-bundle` as a starting point but it may
need manual plist tweaks.

---

## 7. What to REUSE verbatim (do not rewrite)

- **`src/yappr/engine_install.sh`** — installs llama.cpp (brew-first, else
  prebuilt `llama-b9544-bin-macos-arm64` → `~/.yappr/bin`). VERIFIED: the prebuilt
  release runs Gemma audio with Metal, no libomp crash, fully self-contained.
  Rust just calls this script when `llama-server` isn't found.
- **`install.sh`** (repo root) — the curl one-liner bootstrap. Adapt to clone +
  `cargo build --release` + assemble/sign the .app instead of uv/py2app.
- **`scripts/make_cert.sh`** — creates the stable "Yappr Self-Signed" identity.
  Reuse unchanged; sign the Rust .app with it.
- **`scripts/make_icons.py`** + the PNGs in `assets/icons/` — reuse the generated
  icons; only the loading code changes.
- **The pinned llama.cpp release tag `b9544`** — verified to handle Gemma audio.
  Don't bump without re-verifying audio doesn't regress.
- **All prompt strings and request params in §4** — these are the real product.

---

## 8. Code-signing (the thing that must be right)

1. One-time: `./scripts/make_cert.sh` creates "Yappr Self-Signed" in the login
   keychain (RSA, codeSigning EKU). Sign by SHA-1 hash; the cert need not be
   "trusted" in the chain sense — TCC keys on the designated requirement.
2. The app's designated requirement becomes:
   `identifier "com.rpwr021.yappr" and certificate leaf = H"<hash>"`.
   With a **stable cert**, this is constant across rebuilds → **grants persist**.
   (The Python alias build failed because the *python* process had a different
   ad-hoc identity than the signed wrapper. A single Rust binary has no such gap.)
3. Sign with `codesign --force --options runtime --sign "<cert>" Yappr.app`.
   `--deep` if there are embedded dylibs (there shouldn't be many for static Rust).
4. Install to `/Applications/Yappr.app` and `lsregister -f` it so Spotlight/Dock/
   `open` all resolve the same path/identity.
5. Not notarized — fine for personal/repo use. For public distribution to others,
   Apple notarization is a separate (Developer ID) step.

---

## 9. Verification plan (how to know it works)

The author of the Rust build can compile and run, but **cannot fully verify GUI
runtime behavior** (hotkeys, mic, paste) without the user — same as the Python
build. Plan:

1. **Backend smoke test first** (language-agnostic): `curl` the transcription and
   tool-call flows against a running llama-server to confirm the contract (§4)
   before wiring the UI. (These were all verified working in Python.)
2. **Permissions**: launch the signed .app, confirm it prompts for Microphone +
   Input Monitoring + Accessibility as **Yappr**, grant once, **relaunch and
   confirm it does NOT re-prompt** (the key win over Python).
3. **Dictation**: hold Right Option, speak, release → text types at cursor.
   Check captured-audio peak > 0 (silent = mic perm missing).
4. **Chat**: Cmd+Option, ask → spoken reply. Follow-up within 60s uses context.
5. **Search**: ask something current → tool-call fires → grounded spoken answer.
6. **Interrupt**: tap hotkey during a long reply → speech stops.
7. **Launch consistency**: launch via Spotlight, Dock, and `open` — all must
   behave identically (the Python bug was that they didn't).

Add a stderr/file log mirroring the Python app's event lines
(`hotkey down -> chat`, `captured N bytes (peak=...)`, `heard:`, `answer:`,
`web_search: <query>`, `inject via paste`, permission status) — it was essential
for debugging and let the user paste logs.

---

## 10. Current repo state (as of handoff)

- GitHub: `rpwr021/yappr` (PRIVATE). Commit author = `rpwr021` + GitHub noreply.
  License: Apache-2.0. NO Claude co-author trailers (keep it that way).
- Layout: `src/yappr/` (Python app), `scripts/` (build/run/serve/cert/icons),
  `install.sh` (curl bootstrap), `setup.py` (py2app — to be retired for Rust).
- User config + runtime live in `~/.yappr/` (config.ini, bin/, lock).
- The **Python alias build runs** and is usable personally (occasional re-grant).
  Keep it working until Rust reaches parity, then retire py2app.
- The standalone py2app path is abandoned (the 3 failures in §1).

### Suggested Rust layout (proposal)
```
yappr/
├── Cargo.toml
├── src/                  # Rust: main.rs, hotkey.rs, audio.rs, server.rs,
│                         #       chat.rs, search.rs, inject.rs, menu.rs, perms.rs
├── resources/            # Info.plist template, icons (reuse PNGs)
├── scripts/              # build_app.sh (cargo build + bundle + sign), make_cert.sh
├── engine_install.sh     # reuse verbatim
└── install.sh            # curl bootstrap (clone + cargo build + bundle + sign)
```
Decide whether Rust replaces the Python in-place (same repo, retire `src/yappr/`)
or lives in a branch first. Recommend a branch until parity is verified.

---

## 11. Known facts / gotchas cheat-sheet

- Gemma audio = **input only**; no TTS. Spoken output = macOS `say`.
- Gemma needs **thinking disabled** or returns empty content.
- Chat instruction must be a **system message**, not folded into the user turn
  (folding suppresses tool calls).
- Gemma has **no clock** → inject current date/time.
- Translation output is **bilingual** → parse out the target.
- **Microphone permission missing = silent audio, no error** → detect peak≈0.
- **Input Monitoring ≠ Accessibility ≠ Microphone** — three separate TCC grants.
- `say` reads emoji/markdown aloud ("🐧"→"penguin head") → **strip before speaking**.
- Prebuilt llama.cpp release (not Homebrew-relocated) is the clean self-contained
  engine; Homebrew's links `libomp` which **crashes on audio when relocated**.
- macOS bundles only need `/usr/lib` + `/System` libs to be considered system;
  everything else (libomp, libffi, ggml, openssl) is third-party.
