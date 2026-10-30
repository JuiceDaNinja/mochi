"""Mochi's voice, running on this computer.

A tiny HTTP server on 127.0.0.1 that turns text into speech with Kokoro (an open
82M-parameter model) through onnxruntime. Nothing is sent anywhere: the only network
access is a one-time download of the model files from the kokoro-onnx GitHub release
into the app data folder, verified by SHA-256.

    mochi_tts.py --port 51000 --data-dir "%APPDATA%\\app.mochi.desktop"

    GET  /health        -> {"state": "downloading"|"loading"|"ready"|"error", "progress": 0..1, "detail": "..."}
    POST /tts           <- {"text": "...", "speed": 1.1?}   -> audio/wav (503 while not ready)

The desktop app spawns this as a sidecar and points HostedTtsVoice at /tts; while the
model is still downloading the app falls back to the Windows voice.
"""

import argparse
import hashlib
import io
import json
import os
import sys
import threading
import time
import urllib.request
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

VOICE = "af_jessica"
SPEED = 1.1
MAX_CHARS = 600

RELEASE = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/"
FILES = {
    "kokoro-v1.0.onnx": "7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5",
    "voices-v1.0.bin": "bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d",
}

state = {"state": "starting", "progress": 0.0, "detail": ""}
engine = None
engine_lock = threading.Lock()


def log(msg):
    print(f"[mochi-tts] {msg}", flush=True)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def ensure_models(model_dir):
    """Download any missing/corrupt model file. Returns when both are verified."""
    os.makedirs(model_dir, exist_ok=True)
    total = len(FILES)
    for i, (name, digest) in enumerate(FILES.items()):
        path = os.path.join(model_dir, name)
        if os.path.exists(path) and sha256(path) == digest:
            continue
        state.update(state="downloading", progress=i / total, detail=name)
        log(f"downloading {name}")
        tmp = path + ".part"
        req = urllib.request.Request(RELEASE + name, headers={"User-Agent": "mochi-tts"})
        with urllib.request.urlopen(req, timeout=60) as r, open(tmp, "wb") as out:
            size = int(r.headers.get("Content-Length") or 0)
            done = 0
            while True:
                chunk = r.read(1 << 20)
                if not chunk:
                    break
                out.write(chunk)
                done += len(chunk)
                if size:
                    state["progress"] = (i + done / size) / total
        if sha256(tmp) != digest:
            os.remove(tmp)
            raise RuntimeError(f"{name}: checksum mismatch")
        os.replace(tmp, path)
    state["progress"] = 1.0


def load_engine(model_dir):
    global engine
    state.update(state="loading", detail="")
    from kokoro_onnx import Kokoro  # imported late: slow, and only once the files exist

    k = Kokoro(os.path.join(model_dir, "kokoro-v1.0.onnx"), os.path.join(model_dir, "voices-v1.0.bin"))
    k.create("Ready.", voice=VOICE, speed=SPEED, lang="en-us")  # warm up
    with engine_lock:
        engine = k
    state.update(state="ready", progress=1.0, detail=VOICE)
    log("ready")


def prepare(model_dir):
    try:
        ensure_models(model_dir)
        load_engine(model_dir)
    except Exception as e:  # noqa: BLE001 - reported through /health
        state.update(state="error", detail=str(e))
        log(f"error: {e}")


def to_wav(samples, sample_rate):
    import numpy as np

    pcm = (np.clip(samples, -1.0, 1.0) * 32767).astype("<i2").tobytes()
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sample_rate)
        w.writeframes(pcm)
    return buf.getvalue()


class Handler(BaseHTTPRequestHandler):
    server_version = "mochi-tts/1"

    def log_message(self, *_):  # keep stdout for our own lines
        pass

    def _send(self, code, body, ctype="application/json"):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        self.wfile.write(body)

    def do_OPTIONS(self):
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Headers", "content-type")
        self.send_header("Access-Control-Allow-Methods", "POST, GET, OPTIONS")
        self.end_headers()

    def do_GET(self):
        if self.path.startswith("/health"):
            self._send(200, json.dumps(state).encode())
        else:
            self._send(404, b"{}")

    def do_POST(self):
        if self.path != "/tts":
            return self._send(404, b"{}")
        if state["state"] != "ready":
            return self._send(503, json.dumps(state).encode())
        try:
            n = int(self.headers.get("Content-Length") or 0)
            body = json.loads(self.rfile.read(n) or b"{}")
            text = str(body.get("text", "")).strip()[:MAX_CHARS]
            speed = float(body.get("speed", SPEED))
            if not text:
                return self._send(400, b'{"error":"empty text"}')
            with engine_lock:
                samples, sr = engine.create(text, voice=VOICE, speed=max(0.5, min(2.0, speed)), lang="en-us")
            self._send(200, to_wav(samples, sr), "audio/wav")
        except Exception as e:  # noqa: BLE001
            log(f"tts failed: {e}")
            self._send(500, json.dumps({"error": str(e)}).encode())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--data-dir", required=True)
    ap.add_argument("--parent-pid", type=int, default=0, help="exit when this process is gone")
    args = ap.parse_args()
    model_dir = os.path.join(args.data_dir, "tts")

    threading.Thread(target=prepare, args=(model_dir,), daemon=True).start()
    if args.parent_pid:
        threading.Thread(target=watch_parent, args=(args.parent_pid,), daemon=True).start()

    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    log(f"listening on 127.0.0.1:{args.port}")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


def watch_parent(pid):
    """The desktop app kills us on quit, but if it crashes we must not linger."""
    while True:
        time.sleep(3)
        if not pid_alive(pid):
            log("parent gone; exiting")
            os._exit(0)


def pid_alive(pid):
    if sys.platform == "win32":
        import ctypes

        SYNCHRONIZE = 0x00100000
        h = ctypes.windll.kernel32.OpenProcess(SYNCHRONIZE, False, pid)
        if not h:
            return False
        # WAIT_TIMEOUT (258) means still running.
        alive = ctypes.windll.kernel32.WaitForSingleObject(h, 0) == 258
        ctypes.windll.kernel32.CloseHandle(h)
        return alive
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


if __name__ == "__main__":
    main()
