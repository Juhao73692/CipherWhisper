#!/usr/bin/env python3
"""Exercise the packaged launcher in browser mode without touching user data."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.request import Request, build_opener, ProxyHandler
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "dist/CipherWhisper.app/Contents/MacOS/CipherWhisper"
HTTP = build_opener(ProxyHandler({}))


def main():
    with tempfile.TemporaryDirectory(prefix="cipherwhisper-app-") as temp:
        folder = Path(temp)
        mock = folder / "bin"
        mock.mkdir()
        opener = mock / "open"
        opener.write_text('#!/bin/sh\nprintf "%s" "$1" > "$CIPHERWHISPER_CAPTURE_URL"\n')
        opener.chmod(0o700)
        capture = folder / "url"
        data = folder / "workspace"
        env = {**os.environ, "PATH": f"{mock}:{os.environ['PATH']}", "CIPHERWHISPER_CAPTURE_URL": str(capture)}
        command = [str(APP), "ui", "--browser", "--data", str(data), "--bind", "127.0.0.1:0"]
        processes = []

        def launch():
            capture.unlink(missing_ok=True)
            process = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            processes.append(process)
            for _ in range(200):
                if capture.exists() and capture.read_text():
                    return capture.read_text()
                time.sleep(0.1)
            raise AssertionError("app did not open its UI")

        first = launch()
        parsed = urlsplit(first)
        origin = f"{parsed.scheme}://{parsed.netloc}"
        token = (data / "admin.token").read_text().strip()

        def request(path, payload=None):
            req = Request(origin + path,
                          data=json.dumps(payload).encode() if payload is not None else None,
                          headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
            with HTTP.open(req, timeout=40) as response:
                return json.load(response)

        try:
            build = request("/ui/build")
            commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
            count = int(subprocess.check_output(["git", "rev-list", "--count", "HEAD"], cwd=ROOT, text=True))
            dirty = bool(subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=normal"], cwd=ROOT, text=True).strip())
            assert build["commit"] == commit, "app embeds the wrong commit"
            assert build["dirty"] == dirty, "app embeds the wrong Git status"
            assert build["number"] == f"{count + int(dirty):03}", "app embeds the wrong commit count"
            reopened = launch()
            assert urlsplit(reopened).netloc == parsed.netloc, "reopen started another UI"
            assert reopened != first, "reopen reused a bootstrap code"
            assert request("/launcher")["config"] is None
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            result = request("/launcher/start", {
                "name": "Packaged app test", "passphrase": "isolated-app-test-passphrase",
                "role": "center", "network": {"host": "localhost", "peerPort": port, "devicePort": port + 1 if port < 65535 else port - 1, "devices": False},
            })
            assert result["running"]
            assert request("/p2p/contact")["endpoint"] == f"https://localhost:{port}"
        finally:
            request("/launcher/quit", {})
            for process in processes:
                process.wait(timeout=20)
            for _ in range(200):
                try:
                    request("/ui/launcher")
                except OSError:
                    # Wait for the child's lock to be released as well.
                    import fcntl
                    with (data / "domain.lockfile").open("r") as lock:
                        try:
                            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                            break
                        except BlockingIOError:
                            pass
                time.sleep(0.1)
            else:
                raise AssertionError("app did not shut down cleanly")
    print(f"PASS: build {build['number']} ({build['commit']}{'-dirty' if build['dirty'] else ''}) at {build['builtAt']}; app opens, reopens one workspace, configures TLS and exits cleanly")


if __name__ == "__main__":
    main()
