#!/usr/bin/env python3
"""Three-process smoke test: Alice domain, Bob domain, opaque relay. No external network."""
import argparse
import ssl
import uuid
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "target" / "debug"
ENV = {**os.environ, "CIPHERWHISPER_PASSPHRASE": "process-test-only-passphrase"}
BODY = "# 原始 Markdown\n\n$x^2$\n\n$$E=mc^2$$\n\n```rust\nfn main() {}\n```\n"
# Never use a host proxy for localhost integration tests.
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def api(base, path, token=None, data=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    body = json.dumps(data, ensure_ascii=False).encode() if data is not None else None
    with HTTP.open(urllib.request.Request(base + path, data=body, headers=headers), timeout=20) as response:
        return json.load(response)


def wait_for(test, seconds=25):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            result = test()
            if result:
                return result
        except (OSError, ValueError):
            pass
        time.sleep(0.15)
    raise AssertionError("timed out waiting for service state")


def main():
    global HTTP
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, help="Test the unified executable instead of three standalone binaries")
    parser.add_argument("--tls", action="store_true", help="Test HTTPS with the unified executable's generated CA")
    options = parser.parse_args()
    assert not options.tls or options.binary, "--tls requires --binary"
    if options.binary:
        options.binary = options.binary.resolve()
        assert options.binary.exists(), "unified executable missing"
    else:
        for name in ("cipherwhisper-relay", "cipherwhisper-domain", "cipherwhisper-cli"):
            assert (BIN / name).exists(), "run cargo build --workspace first"

    def command(name):
        if options.binary:
            mode = {"cipherwhisper-relay": "relay", "cipherwhisper-domain": "serve", "cipherwhisper-cli": "admin"}[name]
            return [str(options.binary), mode]
        return [str(BIN / name)]
    processes, logs = [], []
    with tempfile.TemporaryDirectory(prefix="cipherwhisper-smoke-") as tmp:
        data = Path(tmp)
        relay_url = ("https" if options.tls else "http") + "://127.0.0.1:" + str(free_port())
        a_url = "http://127.0.0.1:" + str(free_port())
        b_url = "http://127.0.0.1:" + str(free_port())

        ca_args, tls_args = [], []
        if options.tls:
            tls = data / "tls"
            subprocess.run([str(options.binary), "tls-init", "--host", "127.0.0.1", "--out", str(tls)], check=True, capture_output=True)
            ca_args = ["--relay-ca", str(tls / "ca.pem")]
            tls_args = ["--tls-cert", str(tls / "server.pem"), "--tls-key", str(tls / "server-key.pem")]
            HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=ssl.create_default_context(cafile=str(tls / "ca.pem"))))
            assert (tls / "server-key.pem").stat().st_mode & 0o777 == 0o600
            assert not (tls / "ca-key.pem").exists()

        def start(name, args):
            log = open(data / (name + "-" + str(len(logs)) + ".log"), "w+")
            logs.append(log)
            process = subprocess.Popen([*command(name), *args], env=ENV, stdout=log, stderr=log)
            processes.append(process)
            return process

        def stop(process):
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()

        def cli(who, *args, raw=False):
            result = subprocess.run([*command("cipherwhisper-cli"), "--data", str(data / who), "--relay", relay_url, *ca_args, *args], env=ENV, capture_output=True, text=True, check=True)
            return result.stdout.strip() if raw else json.loads(result.stdout)

        def relay():
            process = start("cipherwhisper-relay", ["--bind", relay_url.split("://", 1)[1], "--database", str(data / "relay.sqlite"), *tls_args])
            wait_for(lambda: api(relay_url, "/health"))
            return process

        def domain(who, url):
            process = start("cipherwhisper-domain", ["--data", str(data / who), "--relay", relay_url, "--bind", url.removeprefix("http://"), "--sync-seconds", "1", *ca_args])
            token_file = data / who / "admin.token"
            wait_for(lambda: token_file.exists())
            token = token_file.read_text().strip()
            wait_for(lambda: api(url, "/status", token))
            return process, token

        try:
            r = relay()
            a_card = cli("alice", "init", "--name", "Alice")
            b_card = cli("bob", "init", "--name", "Bob")
            (data / "alice.json").write_text(json.dumps(a_card))
            (data / "bob.json").write_text(json.dumps(b_card))
            cli("alice", "add-peer", str(data / "bob.json"))
            cli("bob", "add-peer", str(data / "alice.json"))
            topic_id = cli("alice", "new-topic", "--peer", b_card["user_id"], "--title", "Local CLI topic", "--id-only", raw=True)
            assert str(uuid.UUID(topic_id)) == topic_id
            b, b_token = domain("bob", b_url)
            api(b_url, "/sync", b_token, {})
            stop(b)
            a, a_token = domain("alice", a_url)
            try:
                api(a_url, "/identity")
                raise AssertionError("management API accepted an unauthenticated request")
            except urllib.error.HTTPError as error:
                assert error.code == 401
            t1 = api(a_url, "/topics", a_token, {"peer_id": b_card["user_id"], "title": "秘密数学"})
            t2 = api(a_url, "/topics", a_token, {"peer_id": b_card["user_id"], "title": "NAS 维护"})
            api(a_url, f'/topics/{t1["id"]}/messages', a_token, {"body": BODY})
            api(a_url, f'/topics/{t2["id"]}/messages', a_token, {"body": "other topic"})
            api(a_url, "/sync", a_token, {})
            with sqlite3.connect(data / "relay.sqlite") as db:
                raw = " ".join(row[0] for row in db.execute("SELECT envelope FROM envelopes WHERE envelope IS NOT NULL"))
                assert "秘密数学" not in raw and "NAS 维护" not in raw and "原始 Markdown" not in raw
                assert db.execute("SELECT COUNT(*) FROM envelopes WHERE acknowledged=0").fetchone()[0] == 2
            stop(a)
            stop(r)
            r = relay()
            a, a_token = domain("alice", a_url)
            b, b_token = domain("bob", b_url)
            wait_for(lambda: len(api(b_url, "/topics", b_token)) == 2)
            history = api(b_url, f'/topics/{t1["id"]}/messages', b_token)
            assert len(history) == 1 and history[0]["body"] == BODY
            assert len(api(b_url, f'/topics/{t2["id"]}/messages', b_token)) == 1
            assert api(a_url, "/identity", a_token) == a_card
            assert api(b_url, "/identity", b_token) == b_card
            wait_for(lambda: api(a_url, f'/topics/{t1["id"]}/messages', a_token)[0]["delivery"] == "delivered")
            stop(b)
            b, b_token = domain("bob", b_url)
            api(b_url, f'/topics/{t1["id"]}/messages', b_token, {"body": "重启后的回复", "reply_to": history[0]["id"]})
            wait_for(lambda: len(api(a_url, f'/topics/{t1["id"]}/messages', a_token)) == 2)
            # Relay outage while an established session continues to queue immutable ciphertext.
            stop(r)
            api(a_url, f'/topics/{t1["id"]}/messages', a_token, {"body": "网络恢复后送达"})
            assert len(api(a_url, "/outbox", a_token)) >= 1
            r = relay()
            api(a_url, "/sync", a_token, {})
            wait_for(lambda: len(api(b_url, f'/topics/{t1["id"]}/messages', b_token)) == 3)
            print("PASS (" + ("unified HTTPS" if options.tls else "HTTP") + "): 3 real processes; offline first message; opaque relay; API auth; topics; exact Markdown; all process restarts; replies; outage recovery")
        except Exception:
            for log in logs:
                log.flush()
                log.seek(0)
                print(log.read())
            raise
        finally:
            for process in processes:
                stop(process)
            for log in logs:
                log.close()


if __name__ == "__main__":
    main()
