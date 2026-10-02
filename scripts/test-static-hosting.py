#!/usr/bin/env python3
"""Exercise native static hosting against an isolated Raahi instance.

Run after `cargo build`: python3 scripts/test-static-hosting.py
Uses a temporary database and loopback listeners, never the running service.
"""

import copy
import gzip
from contextlib import closing
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def run(binary):
    temporary_root = "/tmp/opencode" if Path("/tmp/opencode").is_dir() else None
    with tempfile.TemporaryDirectory(prefix="raahi-static-", dir=temporary_root) as temporary:
        work = Path(temporary)
        root = work / "site"
        root.mkdir()
        (root / "index.html").write_text("<h1>home</h1>")
        (root / "app.js").write_text("0123456789")
        (root / "nested").mkdir()
        (root / "nested" / "index.html").write_text("nested")
        (root / "empty").mkdir()
        large = b"large streaming response\n" * 100000
        (root / "large.txt").write_bytes(large)
        (root / ".env").write_text("secret")
        (work / "secret.txt").write_text("outside root")
        (root / "outside.txt").symlink_to("../secret.txt")
        (root / "inside.js").symlink_to("app.js")
        outside = work / "site-outside"
        outside.mkdir()
        sentinel = b"OUTSIDE SECRET MUST NEVER BE SERVED"
        (outside / "secret.txt").write_bytes(sentinel)
        (outside / "index.html").write_bytes(sentinel)
        (root / "outside-directory").symlink_to("../site-outside", target_is_directory=True)
        (root / "nested" / "escaping-link").symlink_to("../../site-outside/secret.txt")
        (root / "chain.txt").symlink_to("nested/escaping-link")
        os.mkfifo(root / "fifo")
        proxy_port, admin_port, tls_port = free_port(), free_port(), free_port()
        with (work / "server.log").open("w+") as log:
            admin_token = None
            server = subprocess.Popen(
                [str(binary), "--db", f"sqlite://{work / 'test.db'}", "--http-addr",
                 f"127.0.0.1:{proxy_port}", "--admin-addr", f"127.0.0.1:{admin_port}",
                 "--https-addr", f"127.0.0.1:{tls_port}", "--threads", "2"],
                stdout=log, stderr=log,
            )

            def api(path, body=None, method=None, credentials=None):
                authentication = credentials if credentials is not None else (
                    {"Authorization": f"Bearer {admin_token}"} if admin_token else {})
                request = urllib.request.Request(
                    f"http://127.0.0.1:{admin_port}/api/v1/{path}",
                    data=json.dumps(body).encode() if body is not None else None,
                    headers={"Content-Type": "application/json", **authentication}, method=method,
                )
                with urllib.request.urlopen(request, timeout=5) as response:
                    return json.load(response)

            def request(path="/", method="GET", headers=None, host="site.test", connection=None):
                own_connection = connection is None
                connection = connection or http.client.HTTPConnection("127.0.0.1", proxy_port, timeout=5)
                connection.request(method, path, headers={"Host": host, **(headers or {})})
                response = connection.getresponse()
                result = response.status, response.headers, response.read()
                if own_connection:
                    connection.close()
                return result

            def plugin(kind, service_id, config):
                return api("plugins", {"type": kind, "scope": "service", "service_id": service_id, "config": config})

            def rejected(path, body, expected, method="POST", credentials=None):
                try:
                    api(path, body, method=method, credentials=credentials)
                except urllib.error.HTTPError as error:
                    assert error.code == expected, (path, error.code, error.read())
                else:
                    raise AssertionError(f"{path} unexpectedly accepted the request")

            def login(email):
                connection = http.client.HTTPConnection("127.0.0.1", admin_port, timeout=5)
                try:
                    connection.request("POST", "/api/v1/auth/login", json.dumps({"email": email, "password": "regression-password"}),
                                       {"Content-Type": "application/json"})
                    response = connection.getresponse()
                    assert response.status == 200, response.read()
                    response.read()
                    return {"Cookie": response.getheader("Set-Cookie").split(";", 1)[0]}
                finally:
                    connection.close()

            try:
                deadline = time.monotonic() + 15
                while True:
                    if server.poll() is not None:
                        raise RuntimeError("test server exited before becoming ready")
                    try:
                        api("services")
                        break
                    except (OSError, urllib.error.URLError):
                        if time.monotonic() > deadline:
                            raise RuntimeError("test server did not become ready")
                        time.sleep(0.05)
                site = api("services", {"name": "site", "kind": "static", "root": str(root), "spa_fallback": True})
                sid = site["id"]
                api("routes", {"name": "site", "service_id": sid, "hosts": ["site.test"], "paths": ["/"]})
                api("routes", {"name": "mount", "service_id": sid, "hosts": ["mount.test"], "paths": ["/docs"], "strip_path": True})

                # Real sessions verify both body-dependent service authorization
                # and the existing admin-only apply/import guards.
                admin_token = api("admin/token", {})["token"]
                for role in ["admin", "editor", "viewer"]:
                    api("users", {"email": f"{role}@test.local", "name": role, "role": role, "password": "regression-password"})
                admin_session = login("admin@test.local")
                editor_session = login("editor@test.local")
                viewer_session = login("viewer@test.local")
                proxy = api("services", {"name": "editor-proxy"}, credentials=editor_session)
                static_spec = {"name": "site", "kind": "static", "root": str(root), "spa_fallback": True}
                for credentials in [editor_session, viewer_session]:
                    rejected("services", {**static_spec, "name": "unauthorized"}, 403, credentials=credentials)
                    rejected(f"services/{proxy['id']}", {**static_spec, "name": proxy["name"]}, 403, method="PUT", credentials=credentials)
                    rejected(f"services/{sid}", static_spec, 403, method="PUT", credentials=credentials)
                    rejected(f"services/{sid}", {"name": "site"}, 403, method="PUT", credentials=credentials)
                    rejected(f"services/{sid}", None, 403, method="DELETE", credentials=credentials)
                    rejected("config/apply", {"raahi_config": 1, "services": [static_spec]}, 403, credentials=credentials)
                    rejected("import", {"raahi_export_version": 1, "services": []}, 403, credentials=credentials)
                for fields in [{"root": str(work)}, {"spa_fallback": True}]:
                    rejected("services", {"name": "static-fields", **fields}, 403, credentials=editor_session)
                    rejected(f"services/{proxy['id']}", {"name": proxy["name"], **fields}, 403,
                             method="PUT", credentials=editor_session)
                api(f"services/{proxy['id']}", {"name": "editor-proxy"}, method="PUT", credentials=editor_session)
                api(f"services/{sid}", static_spec, method="PUT", credentials=admin_session)
                # Admin sessions can create, change roots, convert both ways and delete.
                disposable = api("services", {**static_spec, "name": "admin-site"}, credentials=admin_session)
                disposable_id = disposable["id"]
                api(f"services/{disposable_id}", {**static_spec, "name": "admin-site", "root": str(root / "nested")},
                    method="PUT", credentials=admin_session)
                api(f"services/{disposable_id}", {"name": "admin-site"}, method="PUT", credentials=admin_session)
                api(f"services/{disposable_id}", {**static_spec, "name": "admin-site"}, method="PUT", credentials=admin_session)
                api(f"services/{disposable_id}", method="DELETE", credentials=admin_session)
                for forbidden_root in ["/", "/proc", "/sys", "/dev", str(work), str(work.parent)]:
                    rejected("services", {**static_spec, "name": "forbidden-root", "root": forbidden_root}, 400)
                    rejected(f"services/{sid}", {**static_spec, "root": forbidden_root}, 400, method="PUT")
                database_alias = work / "database-alias"
                database_alias.symlink_to(work, target_is_directory=True)
                rejected(f"services/{sid}", {**static_spec, "root": str(database_alias)}, 400, method="PUT")
                dumped = api("config/current?format=json")
                bad_dump = copy.deepcopy(dumped)
                next(s for s in bad_dump["services"] if s["name"] == "site")["root"] = str(work)
                rejected("config/apply", bad_dump, 400)
                exported = api("export?include_secrets=true")
                bad_export = copy.deepcopy(exported)
                next(s for s in bad_export["services"] if s["service"]["name"] == "site")["service"]["root"] = str(work)
                rejected("import", bad_export, 400)
                assert api(f"services/{sid}")["root"] == str(root)
                assert request()[2] == b"<h1>home</h1>"

                # A proxy route and a static route share the same listener/host.
                class Upstream(BaseHTTPRequestHandler):
                    def do_GET(self):
                        body = b'{"from":"upstream"}'
                        self.send_response(200)
                        self.send_header("Content-Length", str(len(body)))
                        self.end_headers()
                        self.wfile.write(body)

                    def log_message(self, *_):
                        pass

                upstream = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
                threading.Thread(target=upstream.serve_forever, daemon=True).start()
                try:
                    backend = api("services", {"name": "backend"})
                    api(f"services/{backend['id']}/targets", {"host": "127.0.0.1", "port": upstream.server_port})
                    api("routes", {"name": "api", "service_id": backend["id"], "hosts": ["site.test"], "paths": ["/api"]})
                    assert request("/api/ping")[2] == b'{"from":"upstream"}'
                    assert request()[2] == b"<h1>home</h1>"
                finally:
                    upstream.shutdown()
                    upstream.server_close()

                # A test-only certificate verifies the existing TLS listener and
                # response plugins, without touching real certificates or ACME.
                certificate, key = work / "cert.pem", work / "key.pem"
                subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                                "-subj", "/CN=site.test", "-addext", "subjectAltName=DNS:site.test",
                                "-keyout", str(key), "-out", str(certificate)], check=True,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                cert = api("certificates", {"name": "test", "sni": ["site.test"], "cert_pem": certificate.read_text(), "key_pem": key.read_text()})
                settings = api("settings")
                api("settings", {**settings, "active_certificate_id": cert["id"]}, method="PUT")
                plugin("hsts", sid, {"max_age_secs": 3600})
                context = ssl.create_default_context(cafile=str(certificate))
                context.check_hostname = False  # TCP connects to loopback; Host still selects site.test.
                with closing(http.client.HTTPSConnection("127.0.0.1", tls_port, context=context, timeout=5)) as connection:
                    status, headers, body = request(connection=connection)
                    assert status == 200 and body == b"<h1>home</h1>" and headers["Strict-Transport-Security"] == "max-age=3600"
                assert "Strict-Transport-Security" not in request()[1]

                status, headers, body = request()
                assert status == 200 and body == b"<h1>home</h1>"
                assert headers["Content-Type"] == "text/html"
                etag, modified = headers["ETag"], headers["Last-Modified"]
                status, headers, body = request(method="HEAD")
                assert status == 200 and body == b"" and headers["Content-Length"] == "13"
                assert request(headers={"If-None-Match": etag})[0] == 304
                assert request(headers={"If-Modified-Since": modified})[0] == 304
                assert request(headers={"If-Match": '"no-match"'})[0] == 412
                assert request("/client/route")[2] == b"<h1>home</h1>"
                assert request("/missing.js")[0] == 404
                assert request("/empty/")[0] == 404
                assert request("/nested?x=1")[1]["Location"] == "/nested/?x=1"
                assert request("/docs?x=1", host="mount.test")[1]["Location"] == "/docs/?x=1"
                assert request("/docs/app.js", host="mount.test")[2] == b"0123456789"
                assert request("/docs/nested", host="mount.test")[1]["Location"] == "/docs/nested/"
                assert request("/inside.js")[2] == b"0123456789"
                for path in ["/.env", "/%2eenv", "/%2e%2e/secret.txt", "/outside.txt", "/fifo"]:
                    assert request(path)[0] in (403, 404), path
                # http.client sends these request targets without normalizing dot
                # segments, unlike clients that can hide traversal from a test.
                attacks = [
                    "/../secret.txt", "/nested/../../secret.txt", "/..%2fsecret.txt",
                    "/%2e%2E%2Fsecret.txt", "/nested%2f..%2f..%2fsecret.txt",
                    "/..%5csecret.txt", "/%5c%5cserver%5cshare", "/%00", "/%ff",
                    "/%c0%ae%c0%ae%2fsecret.txt", "/%0d%0a", "/%", "/%2", "/%GG",
                    "/outside-directory/secret.txt", "/outside-directory/", "/chain.txt",
                    "/%252e%252e%252fsecret.txt",
                ]
                for host, prefix in [("site.test", ""), ("mount.test", "/docs")]:
                    for method in ["GET", "HEAD"]:
                        for attack in attacks:
                            status, _, body = request(prefix + attack, method=method, host=host,
                                                      headers={"Range": "bytes=0-4"})
                            assert status in (400, 403, 404), (host, method, attack, status)
                            assert sentinel not in body, attack
                # The implicitly opened index and SPA fallback are also confined.
                (root / "nested" / "index.html").unlink()
                (root / "nested" / "index.html").symlink_to("../../site-outside/index.html")
                assert request("/nested/")[0] == 403
                (root / "nested" / "index.html").unlink()
                (root / "nested" / "index.html").write_text("nested")
                (root / "index.html").unlink()
                (root / "index.html").symlink_to("../site-outside/index.html")
                assert request("/")[0] == 403
                assert request("/client/fallback")[0] == 403
                (root / "index.html").unlink()
                (root / "index.html").write_text("<h1>home</h1>")
                etag = request()[1]["ETag"]
                assert request(method="POST")[0] == 405
                assert request(host="unknown.test")[0] == 404

                status, headers, body = request("/app.js", headers={"Range": "bytes=2-4"})
                assert status == 206 and body == b"234" and headers["Content-Range"] == "bytes 2-4/10"
                assert request("/app.js", headers={"Range": "bytes=-3"})[2] == b"789"
                assert request("/app.js", headers={"Range": "bytes=100-"})[0] == 416
                assert request("/large.txt")[2] == large
                with closing(http.client.HTTPConnection("127.0.0.1", proxy_port, timeout=5)) as connection:
                    assert request(connection=connection)[0] == 200
                    first_socket = connection.sock
                    assert first_socket is not None
                    assert request("/app.js", connection=connection)[0] == 200
                    assert connection.sock is first_socket, "static responses should reuse the connection"

                plugin("response-transform", sid, {"add": {"x-static-test": "yes"}})
                plugin("request-id", sid, {})
                plugin("response-compression", sid, {"level": 5})
                status, headers, body = request("/large.txt", headers={"Accept-Encoding": "gzip"})
                assert status == 200 and gzip.decompress(body) == large
                assert headers["x-static-test"] == "yes" and headers["x-request-id"]
                status, headers, body = request("/large.txt", headers={"Range": "bytes=0-4", "Accept-Encoding": "gzip"})
                assert status == 206 and body == large[:5] and "Content-Encoding" not in headers
                transform = plugin("response-body-transform", sid, {"replace": [{"from": "home", "to": "transformed"}]})
                status, headers, body = request(headers={"If-None-Match": etag, "Range": "bytes=0-2"})
                assert status == 200 and body == b"<h1>transformed</h1>" and "ETag" not in headers
                api(f"plugins/{transform['id']}", method="DELETE")
                cache = plugin("proxy-cache", sid, {"ttl_secs": 60})
                assert request()[1]["x-cache"] == "MISS"
                assert request()[1]["x-cache"] == "HIT"
                assert request(headers={"If-None-Match": etag})[0] == 304
                api(f"plugins/{cache['id']}", method="DELETE")
                auth = plugin("key-auth", sid, {})
                assert request()[0] == 401
                consumer = api("consumers", {"username": "reader"})
                api(f"consumers/{consumer['id']}/credentials", {"type": "key-auth", "identifier": "test-key"})
                assert request(headers={"apikey": "test-key"})[0] == 200
                api(f"plugins/{auth['id']}", method="DELETE")
                records = api("requests?limit=100")
                assert any(record["service_id"] == sid and record["status"] == 200 and record["upstream"] is None for record in records)

                # Root changes hot-reload, and deploying new bytes needs no restart.
                (root / "index.html").write_text("updated")
                assert request()[2] == b"updated"
                second_root = work / "second"
                second_root.mkdir()
                (second_root / "index.html").write_text("second root")
                api(f"services/{sid}", {"name": "site", "kind": "static", "root": str(second_root)}, method="PUT")
                assert request()[2] == b"second root"
                # File trees can be deployed by atomically switching a root symlink.
                deployment = work / "current"
                deployment.symlink_to(root, target_is_directory=True)
                api(f"services/{sid}", {"name": "site", "kind": "static", "root": str(deployment)}, method="PUT")
                assert request()[2] == b"updated"
                replacement = work / "next"
                replacement.symlink_to(second_root, target_is_directory=True)
                replacement.replace(deployment)
                assert request()[2] == b"second root"
                dumped = api("config/current?format=json")
                assert api("config/apply", dumped)["changes"] == []
                exported = api("export?include_secrets=true")
                api("import", exported)
                assert request()[2] == b"second root"
                print("Static hosting smoke test passed: role authorization, root policy, HTTP/HTTPS, mixed proxy routes, files, streaming, ranges, validators, security, mounts, keepalive, plugins, metrics, hot reload and config roundtrips.")
            except Exception:
                log.flush()
                log.seek(0)
                print(log.read()[-10000:], file=sys.stderr)
                raise
            finally:
                server.terminate()
                try:
                    server.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()


if __name__ == "__main__":
    run(Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/raahi").resolve())
