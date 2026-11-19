# Test harness (Linux)

Stand-ins for Lifestream, Midgar, Turks, TheMovieDB and TheTVDB, plus a fake NAS in `/tmp/ex`.

```
sh dev/setup.sh            # fresh fake NAS: /tmp/ex/root/volume1/{data/torrents,TV,Movies,Anime,Inbox}
cargo build
sh dev/start.sh            # stand-ins on 7801-7809, Elixir API on 5471, page on http://127.0.0.1:7810/
sh dev/api.sh status       # call the API (POST: sh dev/api.sh ROUTE '{"json":1}'; extra query: "items&view=queue")
python3 dev/walk.py        # full browser walk (Playwright + /usr/bin/google-chrome), screenshots in /tmp/ex/shots
python3 dev/names.py       # naming presets
python3 dev/win.py         # the page in Windows mode (typed paths, drag and drop)
sh dev/remock.sh           # restart only the stand-ins
```

Keys the stand-ins accept: Lifestream admin / pw, Midgar `midgarkey`, TMDb `tmdbkey`, TVDB login token `tok`. Quiet times are shortened by start.sh (`ELIXIR_TORRENT_QUIET`, `ELIXIR_FILE_QUIET`, `ELIXIR_PASS_SECS`).
