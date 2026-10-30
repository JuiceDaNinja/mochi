# Mochi's local voice

`mochi_tts.py` is a sidecar the desktop app starts: Kokoro TTS (open model, 82M params) through
onnxruntime on the CPU, serving `POST /tts {text}` → WAV on 127.0.0.1. Speech never leaves the PC.
On first start it downloads the two model files (~350 MB, SHA-256 pinned) into
`%APPDATA%\app.mochi.desktop\tts\`; until then Mochi uses the Windows voice.

Voice: `af_jessica` at 1.1× (chosen 2026-10-30 from samples of all 15 English female voices).

- Build for the installer: `tts/build.ps1` (CI does this in the Windows job).
- Run by hand: `python tts/mochi_tts.py --port 51000 --data-dir <dir>` then
  `curl -X POST localhost:51000/tts -d '{"text":"Send it?"}' -o out.wav`.
