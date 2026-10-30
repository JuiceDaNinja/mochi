# Builds the local-voice sidecar into apps/desktop/src-tauri/binaries/mochi-tts/ (one folder,
# started by the desktop app). Run from the repo root on Windows:
#   powershell -ExecutionPolicy Bypass -File tts/build.ps1
$ErrorActionPreference = "Stop"
python -m pip install -r tts/requirements.txt
$out = "apps/desktop/src-tauri/binaries"
python -m PyInstaller --noconfirm --clean --onedir --noconsole --name mochi-tts `
  --distpath $out --workpath build/pyinstaller --specpath build/pyinstaller `
  --collect-all kokoro_onnx --collect-all espeakng_loader --collect-all phonemizer --collect-all onnxruntime `
  --exclude-module scipy --exclude-module pandas --exclude-module matplotlib --exclude-module PIL --exclude-module tkinter `
  tts/mochi_tts.py
Get-ChildItem "$out/mochi-tts" | Measure-Object -Property Length -Sum | ForEach-Object { "sidecar size: {0:N0} MB" -f ($_.Sum / 1MB) }
