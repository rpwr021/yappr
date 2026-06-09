# Configuration

Yappr reads its settings from `~/.yappr/config.ini` on startup. Edit the file,
then quit Yappr from the menu-bar icon and launch it again to apply changes.

Inspect the effective config without launching the UI:

```bash
/Applications/Yappr.app/Contents/MacOS/Yappr --check
```

Any key you omit falls back to the default shown below, so the file only needs
the values you want to override.

## `[models]`

Selects which `[model:<id>]` section is active.

| Key | Default | Description |
| --- | --- | --- |
| `active` | `e2b-qat` | The `<id>` of the `[model:<id>]` section to load. |

## `[model:<id>]`

One section per selectable model. The active model is also mirrored into a
`[model]` section internally. Models must be GGUF with native audio input and a
matching `mmproj` file.

| Key | Default | Description |
| --- | --- | --- |
| `label` | the `<id>` | Display name shown in the menu-bar Model submenu. |
| `repo` | `google/gemma-4-E2B-it-qat-q4_0-gguf` | Hugging Face repo to download from. |
| `weights` | `gemma-4-E2B_q4_0-it.gguf` | GGUF weights filename within the repo. |
| `mmproj` | `gemma-4-E2B-it-mmproj.gguf` | Audio mmproj filename within the repo. |
| `ctx_size` | `8192` | llama.cpp context size (`-c`). |
| `ngl` | `99` | Number of layers offloaded to the GPU (`-ngl`). |

Gemma 4 E2B, E4B, and 12B are the intended audio-capable options. Switching
models in the menu writes `[models] active` and requires a restart.

## `[server]`

Controls the `llama-server` backend.

| Key | Default | Description |
| --- | --- | --- |
| `endpoint` | `http://127.0.0.1:8089/v1/chat/completions` | Chat completions URL. |
| `port` | `8089` | Port Yappr starts/serves `llama-server` on. |
| `manage` | `true` | If `true`, Yappr starts and stops `llama-server` itself. Set `false` to point at a server you run yourself. |
| `binary` | `auto` | `auto` searches `~/.yappr/bin` then `PATH`; otherwise an explicit path to `llama-server`. |
| `timeout` | `60` | Request timeout in seconds. |

## `[audio]`

Microphone capture settings.

| Key | Default | Description |
| --- | --- | --- |
| `device` | *(system default)* | Input device name; empty uses the system default. Set from the menu-bar Microphone submenu. |
| `samplerate` | `16000` | Target sample rate (Hz) fed to ASR; input is resampled to this. |
| `max_seconds` | `28` | Maximum length of a single recording. |
| `tail_seconds` | `0.4` | Extra audio kept after the hotkey is released. |

## `[vad]`

Client-side Silero voice-activity detection, run before ASR. When enabled,
Yappr uses the ONNX Silero path.

| Key | Default | Description |
| --- | --- | --- |
| `enabled` | `true` | Gate audio through VAD before transcribing. |
| `threshold` | `0.5` | Speech-probability threshold (0–1). |
| `min_speech_duration_ms` | `250` | Minimum speech length to count as a segment. |
| `min_silence_duration_ms` | `100` | Silence needed to end a segment. |
| `speech_pad_ms` | `30` | Padding added around detected speech. |

## `[language]`

Transcription source/target languages.

| Key | Default | Description |
| --- | --- | --- |
| `source` | `auto` | Spoken language; `auto` detects it. |
| `target` | `auto` | Output language; `auto` keeps the source language, otherwise translates. Set from the menu-bar Output Language submenu. |
| `options` | `auto,English,Spanish,French,German,Hindi,Japanese,Chinese,Portuguese,Italian` | Languages listed in the menu. |

## `[chat]`

Voice-chat behavior.

| Key | Default | Description |
| --- | --- | --- |
| `context_seconds` | `60` | How many seconds of recent chat history to include for follow-up questions. |

## `[speech]`

Text-to-speech for spoken answers. If a model backend (`supertonic` / `kokoro`)
fails to initialize (e.g. its model files are missing), Yappr logs the failure
and falls back to `say`.

| Key | Default | Description |
| --- | --- | --- |
| `backend` | `say` | `say` (macOS built-in, no download), `supertonic` (local Supertonic 3), or `kokoro` (local Kokoro). |
| `voice` | *(system default)* | macOS `say` voice. |
| `rate` | `190` | macOS `say` speech rate (words/min). |
| `supertonic_model_dir` | `~/.yappr/models/sherpa-onnx-supertonic-3-tts-int8-2026-05-11` | Supertonic model directory. |
| `supertonic_sid` | `0` | Supertonic speaker id. |
| `supertonic_speed` | `1.0` | Supertonic speed multiplier. |
| `supertonic_lang` | `en` | Supertonic language. |
| `supertonic_steps` | `8` | Supertonic generation steps. |
| `supertonic_threads` | `2` | Supertonic inference threads. |
| `kokoro_model_dir` | `~/.yappr/models/kokoro-multi-lang-v1_0` | Kokoro model directory. |
| `kokoro_sid` | `3` | Kokoro speaker id. |
| `kokoro_speed` | `1.0` | Kokoro speed multiplier. |
| `kokoro_lang` | `en` | Kokoro language. |
| `kokoro_threads` | `2` | Kokoro inference threads. |

For Supertonic, download the sherpa-onnx Supertonic package and point
`supertonic_model_dir` at the extracted directory. It must contain the
Supertonic int8 ONNX files, `tts.json`, `unicode_indexer.bin`, and `voice.bin`.

```ini
[speech]
backend = supertonic
supertonic_model_dir = ~/.yappr/models/sherpa-onnx-supertonic-3-tts-int8-2026-05-11
```

## `[search]`

Web search exposed to the model as a `web_search` tool. The tool is only offered
when a backend is reachable: Yappr probes the SearXNG `endpoint` first, then
falls back to DuckDuckGo; if neither responds the tool is not offered.

| Key | Default | Description |
| --- | --- | --- |
| `enabled` | `true` | Enable the `web_search` tool. |
| `endpoint` | `http://127.0.0.1:8888/search` | SearXNG search endpoint. |
| `max_results` | `5` | Maximum results returned to the model. |
| `timeout` | `15` | Search request timeout in seconds. |

## `[logging]`

| Key | Default | Description |
| --- | --- | --- |
| `enabled` | `true` | Write the app log. |
| `debug` | `false` | Also log sensitive content (transcripts, answers, tool queries). Keep `false` unless debugging. |
| `path` | `~/.yappr/yappr.log` | Log file path. |

Tail the log:

```bash
tail -f ~/.yappr/yappr.log
```
