# Yappr

<img src=".github/assets/yappr-logo.png" alt="Yappr logo" width="96">

Local push-to-talk dictation and voice chat for macOS.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/rpwr021/yappr/main/install.sh | bash
```

This installs `/Applications/Yappr.app` and launches it.

## Launch

Start Yappr from Finder, Spotlight, or Terminal:

```bash
open /Applications/Yappr.app
```

After editing `~/.yappr/config.ini`, quit Yappr from the menu-bar icon and
launch it again. To inspect the effective config without launching the UI:

```bash
/Applications/Yappr.app/Contents/MacOS/Yappr --check
```

## Use

- Hold `Right Option` to dictate.
- Hold `Cmd + Right Option` to chat.
- Use the menu-bar icon to change microphone, model, language, speech output, or quit.

Grant Yappr in System Settings > Privacy & Security:

- Input Monitoring: read the global push-to-talk hotkey.
- Accessibility: paste dictated text into the active app.
- Microphone: record while the hotkey is held.

Yappr uses `llama-server` from [llama.cpp](https://github.com/ggml-org/llama.cpp)
for local inference.

Supported models are GGUF models with native audio input and a matching
`mmproj` file. The default is
[`google/gemma-4-E2B-it-qat-q4_0-gguf`](https://huggingface.co/google/gemma-4-E2B-it-qat-q4_0-gguf).
Gemma 4 E2B, E4B, and 12B are the intended audio-capable Gemma 4 options.

## Client Config

Client-side config lives at `~/.yappr/config.ini`. Edit that file to change
models, ports, llama.cpp params, speech, or web search settings without
rebuilding Yappr. Restart the app after editing.

- `[models] active`: selected model id from the `[model:<id>]` sections.
- `[model:<id>] repo`, `weights`, `mmproj`: Hugging Face GGUF repo and files.
- `[model:<id>] ctx_size`, `ngl`: llama.cpp context size and GPU layers.
- `[server] endpoint`, `port`, `manage`, `binary`, `timeout`: llama-server host/port, whether Yappr starts it, and which binary to use.
- `[chat] context_seconds`: recent chat history included for follow-up questions.
- `[vad] enabled`, `threshold`, `min_speech_duration_ms`, `min_silence_duration_ms`, `speech_pad_ms`: client-side Silero VAD gate before ASR. Yappr always uses the ONNX Silero path when VAD is enabled.
- `[speech] backend`: `supertonic` for local Supertonic 3, `kokoro` for local Kokoro TTS, or `say` for macOS speech.
- `[speech] voice`, `rate`: macOS `say` voice and speech rate.
- `[speech] supertonic_model_dir`, `supertonic_sid`, `supertonic_speed`, `supertonic_lang`, `supertonic_steps`, `supertonic_threads`: Supertonic model path and generation settings.
- `[speech] kokoro_model_dir`, `kokoro_sid`, `kokoro_speed`, `kokoro_lang`, `kokoro_threads`: optional Kokoro model path and generation settings.
- `[logging] enabled`, `debug`, `path`: app log switch, sensitive-content debug logging, and file path. Keep `debug = false` unless you want transcripts, answers, and tool queries written to logs.
- `[search] enabled`, `endpoint`, `max_results`, `timeout`: web search tool settings.

For Supertonic speech, download the sherpa-onnx Supertonic package and point Yappr at
the extracted directory:

```ini
[speech]
backend = supertonic
supertonic_model_dir = ~/.yappr/models/sherpa-onnx-supertonic-3-tts-int8-2026-05-11
supertonic_sid = 0
supertonic_speed = 1.0
supertonic_lang = en
supertonic_steps = 8
supertonic_threads = 2
```

The model directory must contain the Supertonic int8 ONNX files, `tts.json`,
`unicode_indexer.bin`, and `voice.bin`. Use `backend = say` if you want macOS
speech without a model download.

When search is enabled, Yappr exposes a `web_search` tool to the local model.
For time-sensitive questions, the model can call the tool, Yappr queries the
configured SearXNG endpoint, then the results are sent back as context for the
final spoken answer. The default search endpoint is
`http://127.0.0.1:8888/search`.

Check the effective client config:

```bash
/Applications/Yappr.app/Contents/MacOS/Yappr --check
```

Logs:

```bash
tail -f ~/.yappr/yappr.log
```

Disable logs with:

```ini
[logging]
enabled = false
```

## Development

```bash
./scripts/make_cert.sh
./scripts/run.sh --build
cargo test
cargo clippy --all-targets -- -D warnings
```

GitHub Actions creates a new `v0.1.<run_number>` release on every `main` push.
Explicit `v*` tags produce releases with that exact version.
