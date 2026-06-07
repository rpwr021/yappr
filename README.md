# Yappr

A local, on-device push-to-talk **dictation + voice-chat** menu-bar app for
macOS. Hold a hotkey, speak, and Yappr either types the transcription at your
cursor or speaks an answer back — all running locally via
[llama.cpp](https://github.com/ggml-org/llama.cpp) and Google's **Gemma 4 E4B**
(audio-capable). No cloud, no account.

> Apple Silicon only. Gemma transcribes/answers; speech output uses macOS `say`
> (Gemma cannot synthesize speech).

## Features

- **Dictation** — hold **Right Option**, speak, release → text is typed at your cursor.
- **Voice chat** — hold **Cmd + Right Option**, ask a question → spoken answer.
  - Keeps ~60s of conversation context for follow-ups.
  - **Web search tool**: when a question needs current info, Gemma can query a
    local [SearXNG](https://github.com/searxng/searxng) instance (optional).
- **Interrupt** — tap the hotkey while it's working to stop (e.g. cut off a reply).
- **Translation** — set an output language to transcribe-then-translate.
- **Live menu-bar icon** — animated waveform while recording, themed when idle.
- **Self-setup** — on first run, auto-installs the llama.cpp engine and
  downloads the model.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/rpwr021/yappr/main/install.sh | bash
```

This installs `uv` if needed, clones Yappr to `~/.yappr/app`, builds `Yappr.app`,
and launches it. On first launch Yappr will:
1. install the engine (Homebrew `llama.cpp`, or a prebuilt binary into `~/.yappr/bin`),
2. download the model (~5.8 GB) into your Hugging Face cache,
3. start the local server and show **Ready**.

Then grant permissions (one time) — see below.

### From source

```bash
git clone https://github.com/rpwr021/yappr && cd yappr
uv sync
./scripts/run.sh --build
```

## Requirements

- macOS on Apple Silicon
- [`uv`](https://docs.astral.sh/uv/)
- Everything else (engine + model) is fetched automatically on first run.
  - Engine: `brew install llama.cpp` if Homebrew is present, else a pinned
    prebuilt release downloaded to `~/.yappr/bin` (see `install.sh`).
  - Model: `google/gemma-4-E4B-it-qat-q4_0-gguf` (weights + audio mmproj).

## macOS permissions (one time)

Yappr needs two **separate** permissions. Enable **Yappr** in
**System Settings → Privacy & Security**:

| Permission | Why |
|---|---|
| **Input Monitoring** | detect the global hotkey |
| **Accessibility** | type / paste at the cursor |
| **Microphone** | record your voice (prompted on first use) |

The app checks these at startup and tells you what's missing. Until
Accessibility is granted, transcripts are copied to the clipboard (paste with ⌘V).

> **Why a signed `.app`?** Permissions attach to an app's code signature.
> `build_app.sh` signs Yappr with a stable self-signed identity so your grants
> survive rebuilds. Create it once (see *Stable signing* below); otherwise the
> build falls back to ad-hoc signing and macOS re-asks after each rebuild.

## Usage

```bash
./scripts/run.sh            # launch current build
./scripts/run.sh --build    # rebuild, then launch
```

- **Right Option** (hold) → dictate.
- **Cmd + Right Option** (hold) → chat (spoken reply).
- Tap the hotkey while busy → interrupt.
- Logs: `tail -f /tmp/yappr.log` and `/tmp/yappr-llama-server.log`.

## Web search (optional)

Chat can call a `web_search` tool backed by a local SearXNG container:

```bash
docker run -d --name local-searxng -p 8888:8080 searxng/searxng
```

Enable the JSON API in SearXNG, then set `[search] enabled = true` (default) and
`endpoint` in `config.ini`. With it off, chat answers from model knowledge only.

## Configuration

Settings live in `~/.yappr/config.ini` (seeded from the shipped defaults on first
run) and most are editable from the menu:

| Section / key | Meaning |
|---|---|
| `[hotkey] key` | `right_option` (default), `left_option`, `right_cmd` |
| `[audio] tail_seconds` | extra capture after key release so the last word isn't clipped |
| `[language] target` | `auto` (verbatim) or a language to translate into |
| `[chat] voice` | macOS `say` voice (empty = system default) |
| `[chat] context_seconds` | how long to keep conversation context |
| `[search] enabled` | allow the web_search tool |
| `[server] manage` | let Yappr start/stop the engine itself |
| `[model] repo/weights/mmproj` | which model to detect/download |

## Stable signing (recommended for development)

So permission grants persist across rebuilds, create a self-signed code-signing
cert once:

```bash
./scripts/make_cert.sh   # creates "Yappr Self-Signed" in your login keychain
```

`build_app.sh` uses it automatically when present.

## How it works

```
hotkey ──> record (sounddevice) ──> llama-server /v1/chat/completions (Gemma 4 E4B)
                                          │
            dictation: transcribe ────────┤
            chat:      transcribe + answer ┴─> macOS `say`   (+ optional web_search)
```

- The app manages a local `llama-server` child process (start on launch, stop on quit).
- Audio is sent as `input_audio`; thinking is disabled for clean, fast output.
- Engine + model are installed/downloaded on first run, not bundled (keeps the
  app tiny and avoids fragile dylib relocation).

## Project layout

```
yappr/
├── install.sh              one-line installer (curl | bash)
├── setup.py                py2app bundle config
├── pyproject.toml          deps + entry point
├── src/yappr/
│   ├── app.py              the app (menu bar, hotkeys, audio, backend mgmt)
│   ├── __main__.py         `python -m yappr`
│   ├── config.default.ini  shipped defaults (user copy lives in ~/.yappr/config.ini)
│   ├── engine_install.sh   engine installer (brew-first, prebuilt fallback)
│   └── assets/icons/       menu-bar icon frames
└── scripts/
    ├── build_app.sh        build the signed .app
    ├── run.sh              clean launch (kills old instance, starts via open)
    ├── serve.sh            manually run the engine (dev/debug)
    ├── make_cert.sh        create the stable signing identity
    └── make_icons.py       regenerate icon frames
```

User config and runtime data live in `~/.yappr/` (config, engine binary, lock),
so they survive app rebuilds.

## Limitations

- Apple Silicon only; speech output is macOS `say` (no voice cloning).
- Gemma 4 E4B is weak at mental arithmetic.
- The build is not notarized — fine for personal use; distributing to others
  would need Apple notarization.

## License

Apache-2.0 — see [LICENSE](LICENSE).
