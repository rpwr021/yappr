# Yappr

![Yappr logo](.github/assets/yappr-logo.png)

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
- Use the menu-bar icon to change microphone/model/language or quit.

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
models, ports, llama.cpp params, voice, or web search settings without
rebuilding Yappr. Restart the app after editing.

- `[models] active`: selected model id from the `[model:<id>]` sections.
- `[model:<id>] repo`, `weights`, `mmproj`: Hugging Face GGUF repo and files.
- `[model:<id>] ctx_size`, `ngl`: llama.cpp context size and GPU layers.
- `[server] endpoint`, `port`, `manage`, `binary`, `timeout`: llama-server host/port, whether Yappr starts it, and which binary to use.
- `[chat] voice`, `rate`: macOS `say` voice and speech rate.
- `[chat] context_seconds`: recent chat history included for follow-up questions.
- `[search] enabled`, `endpoint`, `max_results`, `timeout`: web search tool settings.

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

## Development

```bash
./scripts/make_cert.sh
./scripts/run.sh --build
cargo test
cargo clippy --all-targets -- -D warnings
```

Release builds are produced by GitHub Actions from `v*` tags.
