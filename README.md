# Yappr

Local push-to-talk dictation and voice chat for macOS.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/rpwr021/yappr/main/install.sh | bash
```

This installs `/Applications/Yappr.app` and launches it.

## Use

- Hold `Right Option` to dictate.
- Hold `Cmd + Right Option` to chat.
- Use the menu-bar icon to change microphone/model/language or quit.

Grant Yappr in System Settings > Privacy & Security:

- Input Monitoring
- Accessibility
- Microphone

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
