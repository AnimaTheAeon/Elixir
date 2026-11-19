"""Serves Elixir's window like DSM does: static files, api.cgi -> the service, a fake login.cgi."""
import http.server, os, sys, urllib.request
UI = sys.argv[1]; PORT = int(sys.argv[2]); API = int(sys.argv[3])
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def fwd(self, method):
        q = self.path.split("?", 1)[1] if "?" in self.path else ""
        n = int(self.headers.get("Content-Length") or 0)
        data = self.rfile.read(n) if n else None
        req = urllib.request.Request(f"http://127.0.0.1:{API}/api?{q}", data=data, method=method, headers={"Content-Type": "application/json"})
        try:
            r = urllib.request.urlopen(req, timeout=120); code, body = r.status, r.read()
        except urllib.error.HTTPError as e:
            code, body = e.code, e.read()
        except Exception:
            code, body = 503, b'{"error":"The Elixir service isn\'t running."}'
        self.send_response(code); self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_POST(self):
        if self.path.startswith("/api.cgi"): return self.fwd("POST")
        self.send_error(404)
    def do_GET(self):
        p = self.path.split("?")[0]
        if p.startswith("/api.cgi"): return self.fwd("GET")
        if p == "/webman/login.cgi":
            b = b'{"SynoToken":"t"}'; self.send_response(200); self.send_header("Content-Type", "application/json"); self.end_headers(); self.wfile.write(b); return
        if p in ("/", ""): p = "/index.html"
        f = os.path.join(UI, p.lstrip("/"))
        if not os.path.isfile(f): return self.send_error(404)
        body = open(f, "rb").read().replace(b"@VERSION@", b"1.0.0")
        ct = {"html": "text/html", "js": "text/javascript", "css": "text/css", "png": "image/png", "woff2": "font/woff2", "ttf": "font/ttf"}.get(f.rsplit(".", 1)[-1], "application/octet-stream")
        self.send_response(200); self.send_header("Content-Type", ct); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
