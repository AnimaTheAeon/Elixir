"""Stand-ins for TheMovieDB, TheTVDB, Lifestream, Midgar and Turks (test only)."""
import json, os, re, sys, threading, base64, time
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs

ROOT = os.environ.get("EX_ROOT", "/tmp/ex/root")
V1 = ROOT + "/volume1"
TOR = V1 + "/data/torrents"
STATE = {"commands": {}, "next": 1, "midgar_files": set(), "lifestream_down": False, "slow": 0}

ROP = {"tvdb": 367506, "tmdb": 84773, "title": "The Lord of the Rings: The Rings of Power", "year": 2022}
EPS = {
    367506: [(3, n, t, f"2026-0{n}-01") for n, t in [(1, "Marnûkh"), (2, "The Gift of Men"), (3, "The Secret Fire"), (4, "Gundabad"), (5, "The Lady of Lórien"), (6, "From Valinor")]],
    366924: [(2, n, t, f"2023-06-{20+n}") for n, t in [(1, "Beef"), (2, "Pasta"), (3, "Sundae")]],
    424536: [(1, n, f"Episode {n}", None) for n in range(1, 29)] + [(2, n, t, None) for n, t in [(1, "Frieren Two A"), (2, "B")]],
    390430: [(1, 1, "Anjin", "2024-02-27"), (1, 2, "Servants of Two Masters", "2024-02-27")],
    77777: [(1, 1, "Pilot", "1980-09-15")],
}
SHOWS = [
    {"id": 367506, "tmdb": 84773, "name": "The Lord of the Rings: The Rings of Power", "year": "2022"},
    {"id": 366924, "tmdb": 136315, "name": "The Bear", "year": "2022"},
    {"id": 424536, "tmdb": 209867, "name": "Frieren: Beyond Journey's End", "year": "2023", "aliases": ["Sousou no Frieren"]},
    {"id": 390430, "tmdb": 126308, "name": "Shōgun", "year": "2024"},
    {"id": 77777, "tmdb": 4000, "name": "Shōgun", "year": "1980"},
]
MOVIES = [
    {"id": 693134, "title": "Dune: Part Two", "release_date": "2024-02-27", "genre_ids": [878, 12]},
    {"id": 438631, "title": "Dune", "release_date": "2021-09-15", "genre_ids": [878, 12]},
]
GENRES = {878: "Science Fiction", 12: "Adventure", 35: "Comedy"}

def norm(s):
    return re.sub(r"[^a-z0-9]+", " ", s.lower().replace("ō", "o")).strip()

def hits(q):
    q = norm(q)
    return [s for s in SHOWS if q and (q in norm(s["name"]) or any(q in norm(a) for a in s.get("aliases", [])))]

TORRENTS = []  # filled at start

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, code, obj, headers=None):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(n) or b"null")
    def do_GET(self): self.route("GET")
    def do_POST(self): self.route("POST")
    def do_PUT(self): self.route("PUT")

class TMDB(H):
    def route(self, m):
        u = urlparse(self.path); q = parse_qs(u.query); p = u.path
        if q.get("api_key", [""])[0] != "tmdbkey" and self.headers.get("Authorization") != "Bearer eyJtest":
            return self.send(401, {"status_message": "Invalid API key"})
        if p == "/3/search/tv":
            res = [{"id": s["tmdb"], "name": s["name"], "original_name": s["name"], "first_air_date": s["year"] + "-01-01"} for s in hits(q["query"][0])]
            y = q.get("first_air_date_year")
            if y: res = [r for r in res if r["first_air_date"].startswith(y[0])]
            return self.send(200, {"results": res})
        if p == "/3/search/movie":
            qq = norm(q["query"][0]); res = [m for m in MOVIES if qq in norm(m["title"])]
            y = q.get("year")
            if y: res = [m for m in res if m["release_date"].startswith(y[0])]
            return self.send(200, {"results": [dict(m, original_title=m["title"]) for m in res]})
        if p == "/3/genre/movie/list":
            return self.send(200, {"genres": [{"id": k, "name": v} for k, v in GENRES.items()]})
        m = re.match(r"/3/movie/(\d+)$", p)
        if m:
            mv = next((x for x in MOVIES if x["id"] == int(m[1])), None)
            return self.send(200, dict(mv, genres=[{"id": g, "name": GENRES[g]} for g in mv["genre_ids"]])) if mv else self.send(404, {})
        m = re.match(r"/3/tv/(\d+)$", p)
        if m:
            s = next((x for x in SHOWS if x["tmdb"] == int(m[1])), None)
            if not s: return self.send(404, {})
            seasons = sorted({e[0] for e in EPS[s["id"]]})
            return self.send(200, {"id": s["tmdb"], "name": s["name"], "first_air_date": s["year"] + "-01-01", "seasons": [{"season_number": n} for n in seasons], "external_ids": {"tvdb_id": s["id"]}})
        m = re.match(r"/3/tv/(\d+)/season/(\d+)$", p)
        if m:
            s = next((x for x in SHOWS if x["tmdb"] == int(m[1])), None)
            return self.send(200, {"episodes": [{"episode_number": e[1], "name": e[2], "air_date": e[3]} for e in EPS[s["id"]] if e[0] == int(m[2])]})
        self.send(404, {})

class TVDB(H):
    def route(self, m):
        u = urlparse(self.path); q = parse_qs(u.query); p = u.path
        if p == "/v4/login":
            b = self.body()
            return self.send(200, {"data": {"token": "tok"}}) if b.get("apikey") == "tvdbkey" else self.send(401, {"status": "failure"})
        if self.headers.get("Authorization") != "Bearer tok":
            return self.send(401, {})
        if p == "/v4/search":
            res = hits(q["query"][0])
            if q.get("year"): res = [s for s in res if s["year"] == q["year"][0]]
            return self.send(200, {"data": [{"tvdb_id": str(s["id"]), "name": s["name"], "year": s["year"], "aliases": s.get("aliases", []), "translations": {"eng": s["name"]}, "remote_ids": [{"sourceName": "TheMovieDB.com", "id": str(s["tmdb"])}]} for s in res]})
        m = re.match(r"/v4/series/(\d+)/episodes/default/eng$", p)
        if m:
            sid = int(m[1]); eps = EPS.get(sid, [])
            absn = 0; out = []
            for e in eps:
                absn += 1
                out.append({"seasonNumber": e[0], "number": e[1], "absoluteNumber": absn, "name": e[2], "aired": e[3]})
            return self.send(200, {"data": {"episodes": out}, "links": {"next": None}})
        m = re.match(r"/v4/series/(\d+)(/translations/eng)?$", p)
        if m:
            s = next((x for x in SHOWS if x["id"] == int(m[1])), None)
            return self.send(200, {"data": {"name": s["name"], "year": s["year"]}}) if s else self.send(404, {})
        self.send(404, {})

class LIFE(H):
    def route(self, m):
        if STATE["lifestream_down"]:
            self.connection.close(); return
        if self.path != "/transmission/rpc":
            return self.send(404, {})
        auth = self.headers.get("Authorization", "")
        if auth != "Basic " + base64.b64encode(b"admin:pw").decode():
            return self.send(401, {})
        if self.headers.get("X-Transmission-Session-Id") != "sid1":
            return self.send(409, {}, {"X-Transmission-Session-Id": "sid1"})
        b = self.body()
        out = []
        for t in TORRENTS:
            files = []
            for f in t["files"]:
                full = os.path.join(t["dir"], f)
                ln = os.path.getsize(full) if os.path.exists(full) else 0
                files.append({"name": f, "length": ln, "bytesCompleted": ln if t.get("done", True) else 0})
            out.append({"hashString": t["hash"], "downloadDir": t["dir"], "totalSize": sum(x["length"] for x in files), "percentDone": 1.0 if t.get("done", True) else 0.5, "status": 6, "files": files})
        self.send(200, {"result": "success", "arguments": {"torrents": out}})

class ARR(H):
    app = "midgar"
    def route(self, m):
        if STATE["slow"]: time.sleep(STATE["slow"])
        if self.headers.get("X-Api-Key") != self.app + "key":
            return self.send(401, {})
        u = urlparse(self.path); q = parse_qs(u.query); p = u.path.replace("/api/v3/", "")
        if p == "system/status": return self.send(200, {"version": "1.28" if self.app == "midgar" else "0.5.23"})
        if p == "rootfolder": return self.send(200, [{"path": V1 + ("/TV" if self.app == "midgar" else "/Movies")}])
        if p == "queue": return self.send(200, {"records": []})
        if p == "history":
            recs = [{"downloadId": "AAAA1111", "eventType": 1}] if self.app == "midgar" else [{"downloadId": "TTTT"}]
            return self.send(200, {"records": recs})
        if p == "config/mediamanagement": return self.send(200, {"copyUsingHardlinks": True})
        if self.app == "turks":
            if p == "movie": return self.send(200, [])
            if p == "command": return self.send(201, {"id": 1})
            return self.send(404, {})
        if p == "series":
            return self.send(200, [{"id": 7, "title": ROP["title"], "year": 2022, "tvdbId": ROP["tvdb"], "tmdbId": ROP["tmdb"], "path": V1 + "/TV/The Lord of the Rings - The Rings of Power", "seriesType": "standard"}])
        if p == "config/naming":
            return self.send(200, {"renameEpisodes": True, "standardEpisodeFormat": "{Series Title} - S{season:00}E{episode:00} - {Episode Title}", "dailyEpisodeFormat": "{Series Title} - {Air-Date} - {Episode Title}", "animeEpisodeFormat": "{Series Title} - S{season:00}E{episode:00} - {Episode Title}", "seriesFolderFormat": "{Series Title}", "seasonFolderFormat": "Season {season}", "specialsFolderFormat": "Specials", "multiEpisodeStyle": 5, "colonReplacementFormat": 4})
        if p == "episode":
            return self.send(200, [{"id": 100 + e[1], "seasonNumber": e[0], "episodeNumber": e[1], "hasFile": (e[0], e[1]) in STATE["midgar_files"]} for e in EPS[ROP["tvdb"]]])
        if p == "manualimport":
            folder = q["folder"][0]; out = []
            for f in sorted(os.listdir(folder)):
                if f.endswith(".mkv"):
                    mm = re.search(r"S(\d\d)E(\d\d)", f)
                    out.append({"path": os.path.join(folder, f), "series": {"id": 7}, "episodes": [{"id": 100 + int(mm[2])}] if mm else [], "quality": {"quality": {"id": 9}}, "languages": [{"id": 1}], "releaseGroup": "MeGusta", "rejections": []})
            return self.send(200, out)
        if p == "command" and m == "POST":
            b = self.body(); cid = STATE["next"]; STATE["next"] += 1
            STATE["commands"][cid] = b
            if b.get("name") == "ManualImport":
                for f in b["files"]:
                    eid = f["episodeIds"][0] - 100
                    dst_dir = V1 + "/TV/The Lord of the Rings - The Rings of Power/Season 3"
                    os.makedirs(dst_dir, exist_ok=True)
                    dst = os.path.join(dst_dir, f"The Lord of the Rings - The Rings of Power - S03E{eid:02d}.mkv")
                    if not os.path.exists(dst):
                        (os.link if b["importMode"] == "copy" else os.rename)(f["path"], dst)
                    STATE["midgar_files"].add((3, eid))
            return self.send(201, {"id": cid, "name": b.get("name")})
        mm = re.match(r"command/(\d+)$", p)
        if mm: return self.send(200, {"id": int(mm[1]), "status": "completed"})
        self.send(404, {})

class TURKS(ARR):
    app = "turks"

class CTL(H):
    """Test control: toggle failures."""
    def route(self, m):
        b = self.body() if m == "POST" else {}
        STATE.update(b or {})
        self.send(200, {k: v for k, v in STATE.items() if k in ("lifestream_down", "slow")})

def serve(cls, port):
    s = ThreadingHTTPServer(("127.0.0.1", port), cls)
    threading.Thread(target=s.serve_forever, daemon=True).start()

if __name__ == "__main__":
    TORRENTS.extend(json.load(open(sys.argv[1])))
    for cls, port in [(TMDB, 7801), (TVDB, 7802), (LIFE, 7803), (ARR, 7804), (TURKS, 7805), (CTL, 7809)]:
        serve(cls, port)
    print("mocks up", flush=True)
    while True: time.sleep(3600)
