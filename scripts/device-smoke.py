#!/usr/bin/env python3
"""Two centers + two independently keyed clients; direct P2P or optional legacy relay."""
import argparse
import json
from pathlib import Path
import signal
import socket
import sqlite3
import ssl
import subprocess
import tempfile
import urllib.error
import urllib.request
from smoke import api, free_port, wait_for, BODY, ENV


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[1] / "target/debug/topicairn")
    parser.add_argument("--direct", action="store_true", help="direct P2P between centers; no relay process")
    args = parser.parse_args()
    binary = args.binary.resolve()
    processes, logs = [], []
    # Native clients must not inherit an unrelated workstation proxy in this local test.
    env = {k: v for k, v in ENV.items() if k.lower() not in ("http_proxy", "https_proxy", "all_proxy")}
    with tempfile.TemporaryDirectory(prefix="topicairn-device-smoke-") as tmp:
        root = Path(tmp)
        relay, alice, bob, one, two = ["http://127.0.0.1:" + str(free_port()) for _ in range(5)]
        device_url = "https://127.0.0.1:" + str(free_port())
        peer_urls = {name: "https://127.0.0.1:" + str(free_port()) for name in ("alice", "bob")}
        tls = root / "tls"
        subprocess.run([str(binary), "tls-init", "--host", "127.0.0.1", "--out", str(tls)], env=env, capture_output=True, check=True)
        tls_http = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=ssl.create_default_context(cafile=str(tls / "ca.pem"))))

        def run(*args):
            return subprocess.run([str(binary), *map(str, args)], env=env, check=True, text=True, capture_output=True).stdout

        def start(*args):
            log = open(root / f"process-{len(logs)}.log", "w+")
            logs.append(log)
            p = subprocess.Popen([str(binary), *map(str, args)], env=env, stdout=log, stderr=log)
            processes.append(p)
            return p

        def stop(p):
            if p.poll() is None:
                p.send_signal(signal.SIGINT)
                try:
                    p.wait(timeout=12)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.wait()

        def ready(p, url, name):
            token_file = root / name / "admin.token"
            wait_for(lambda: token_file.exists())
            token = token_file.read_text().strip()
            wait_for(lambda: api(url, "/status", token))
            assert p.poll() is None
            return token

        def center(name, url):
            flags = [] if name == "bob" else ["--device-bind", device_url.split("://")[1], "--device-tls-cert", tls / "server.pem", "--device-tls-key", tls / "server-key.pem", "--device-ca", tls / "ca.pem", "--device-url", device_url]
            transport = ["--peer-bind", peer_urls[name].split("://")[1], "--peer-url", peer_urls[name], "--peer-tls-cert", tls / "server.pem", "--peer-tls-key", tls / "server-key.pem", "--peer-ca", tls / "ca.pem"] if args.direct else ["--relay", relay]
            p = start("serve", "--name", name, "--data", root / name, "--bind", url.split("://")[1], "--sync-seconds", "1", *transport, *flags)
            return p, ready(p, url, name)

        def client(name, url, first=False, domain=None):
            flags = ["--pairing", root / f"{name}.pair.json", "--trust-domain", domain] if first else []
            p = start("connect", "--data", root / name, "--bind", url.split("://")[1], "--sync-seconds", "1", *flags)
            return p, ready(p, url, name)

        def history(url, token, topic):
            return api(url, f'/topics/{topic["id"]}/messages', token)

        try:
            if not args.direct:
                start("relay", "--bind", relay.split("://")[1], "--database", root / "relay.sqlite")
                wait_for(lambda: api(relay, "/health"))
            a, at = center("alice", alice)
            b, bt = center("bob", bob)
            ac = api(alice, "/identity", at)
            bc = api(bob, "/identity", bt)
            if args.direct:
                api(alice, "/p2p/peers", at, api(bob, "/p2p/contact", bt))
                api(bob, "/p2p/peers", bt, api(alice, "/p2p/contact", at))
                assert api(alice, "/status", at)["transport"] == "direct"
                assert api(bob, "/status", bt)["transport"] == "direct"
            else:
                api(alice, "/peers", at, bc)
                api(bob, "/peers", bt, ac)
            api(bob, "/sync", bt, {})
            math = api(alice, "/topics", at, {"peer_id": bc["user_id"], "title": "设备同步·数学"})
            nas = api(alice, "/topics", at, {"peer_id": bc["user_id"], "title": "设备同步·NAS"})
            initial = api(alice, f'/topics/{math["id"]}/messages', at, {"body": BODY})
            wait_for(lambda: len(history(bob, bt, math)) == 1)
            api(bob, f'/topics/{math["id"]}/messages', bt, {"body": "配对前的来信", "reply_to": initial["id"]})
            wait_for(lambda: len(history(alice, at, math)) == 2)
            cards = []
            for name in ("one", "two"):
                card = json.loads(run("device-init", "--data", root / name, "--name", name))
                cards.append(card)
                pair = api(alice, "/devices", at, card)
                assert pair["device"]["signing_key"] != ac["signing_key"]
                (root / f"{name}.pair.json").write_text(json.dumps(pair))
            c1, t1 = client("one", one, True, ac["user_id"])
            c2, t2 = client("two", two, True, ac["user_id"])
            for url, token in [(one, t1), (two, t2)]:
                wait_for(lambda: len(history(url, token, math)) == 2)
                assert history(url, token, math)[0]["body"] == BODY
                assert api(url, "/identity", token) == ac
                assert len(api(url, "/topics", token)) == 2
                assert not history(url, token, nas)
                wait_for(lambda: api(url, "/status", token)["device"]["acknowledgedCursor"] > 0)

            # Mutations traverse the same client queue, center journal and external ratchet.
            third = api(one, "/topics", t1, {"peer_id": bc["user_id"], "title": "从客户端创建"})
            sent = api(one, f'/topics/{math["id"]}/messages', t1, {"body": "客户端发送 $x$", "reply_to": initial["id"]})
            wait_for(lambda: len(history(bob, bt, math)) == 3)
            wait_for(lambda: len(history(two, t2, math)) == 3)
            assert history(two, t2, math)[2]["senderId"] == ac["user_id"]
            assert history(two, t2, math)[2]["id"] == sent["id"]
            wait_for(lambda: any(t["id"] == third["id"] for t in api(two, "/topics", t2)))
            api(two, f'/topics/{third["id"]}', t2, {"title": "两个客户端", "archived": False})
            wait_for(lambda: any(t["title"] == "两个客户端" for t in api(one, "/topics", t1)))

            # Center outage + client restart: plaintext queue stays durable, exactly one eventual send.
            stop(a)
            queued = api(one, f'/topics/{nas["id"]}/messages', t1, {"body": "离线排队，重启后只发一次"})
            assert any(p["operation"].get("message_id") == queued["id"] for p in api(one, "/device-pending", t1))
            stop(c1)
            c1, t1 = client("one", one)
            assert history(one, t1, nas)[0]["body"] == queued["body"]
            a, at = center("alice", alice)
            wait_for(lambda: len(history(bob, bt, nas)) == 1, seconds=45)
            wait_for(lambda: len(history(two, t2, nas)) == 1)
            wait_for(lambda: not api(one, "/device-pending", t1))
            assert len(history(alice, at, nas)) == 1
            assert history(bob, bt, nas)[0]["id"] == queued["id"]
            before = api(one, "/status", t1)["device"]["cursor"]
            stop(c1)
            c1, t1 = client("one", one)
            assert api(one, "/status", t1)["device"]["cursor"] >= before
            assert len(history(one, t1, nas)) == 1

            # Device listener never permits plaintext or unauthenticated access to histories/admin.
            try:
                tls_http.open(device_url + "/device/v1/changes", timeout=5)
                raise AssertionError("unsigned device request accepted")
            except urllib.error.HTTPError as e:
                assert e.code == 401
            try:
                tls_http.open(urllib.request.Request(device_url + "/identity", headers={"Authorization": "Bearer " + at}), timeout=5)
                raise AssertionError("remote admin route accessible")
            except urllib.error.HTTPError as e:
                assert e.code in (401, 404)
            old_tls = ssl.create_default_context(cafile=str(tls / "ca.pem"))
            old_tls.maximum_version = ssl.TLSVersion.TLSv1_2
            for context in [ssl.create_default_context(), old_tls]:
                try:
                    with socket.create_connection(("127.0.0.1", int(device_url.rsplit(":", 1)[1])), timeout=5) as raw:
                        with context.wrap_socket(raw, server_hostname="127.0.0.1"):
                            raise AssertionError("wrong CA / old TLS was accepted")
                except ssl.SSLError:
                    pass

            api(alice, f'/devices/{cards[0]["id"]}/revoke', at, {})
            rejected = api(one, "/sync", t1, {})
            assert rejected["errors"] and "401" in str(rejected["errors"])
            assert len(history(one, t1, math)) == 3  # previously trusted local cache retained
            api(two, f'/topics/{nas["id"]}/messages', t2, {"body": "另一设备继续同步"})
            wait_for(lambda: len(history(bob, bt, nas)) == 2)
            assert len(history(one, t1, nas)) == 1
            assert len(history(alice, at, nas)) == 2
            stop(a)
            a, at = center("alice", alice)
            assert any(d["revoked"] and d["card"]["id"] == cards[0]["id"] for d in api(alice, "/devices", at)["devices"])
            assert api(one, "/sync", t1, {})["errors"]
            if args.direct:
                assert not (root / "relay.sqlite").exists()
            else:
                with sqlite3.connect(root / "relay.sqlite") as db:
                    raw = " ".join(row[0] for row in db.execute("SELECT envelope FROM envelopes WHERE envelope IS NOT NULL"))
                    assert "客户端发送" not in raw and "离线排队" not in raw
            print(f"PASS: unified binary, {4 if args.direct else 5} real processes; {'direct P2P, no relay' if args.direct else 'legacy relay'}; independent devices; old incoming/SENT history; exact Markdown; client mutations; offline queue/client+center restarts; dedupe; per-device ACK; TLS trust/version enforcement; no remote admin; permanent revocation")
        except Exception:
            for log in logs:
                log.flush()
                log.seek(0)
                print(log.read())
            raise
        finally:
            for p in processes:
                stop(p)
            for log in logs:
                log.close()


if __name__ == "__main__":
    main()
