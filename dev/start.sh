#!/bin/sh
# Start the stand-in apps, the Elixir service and the window server (page on http://127.0.0.1:7810/).
# Run setup.sh first to build the fake NAS in /tmp/ex, and `cargo build` in the repo.
D=$(cd "$(dirname "$0")" && pwd); R=$(dirname "$D")
pkill -f "mocks.py /tmp/ex"; pkill -f "dev/serve.py"; pkill -f "target/debug/elixir daemon"; sleep 0.5
cd "$D"
nohup python3 "$D/mocks.py" /tmp/ex/torrents.json > /tmp/ex/mocks.log 2>&1 &
nohup python3 "$D/serve.py" "$R/ui" 7810 5471 > /tmp/ex/serve.log 2>&1 &
cd "$R"
ELIXIR_NO_AUTH=1 ELIXIR_VAR=/tmp/ex/var ELIXIR_BROWSE_ROOT=/tmp/ex/root ELIXIR_TMDB_URL=http://127.0.0.1:7801 ELIXIR_TVDB_URL=http://127.0.0.1:7802 \
ELIXIR_RELEASE_URL=http://127.0.0.1:7809/release ELIXIR_TORRENT_QUIET=3 ELIXIR_FILE_QUIET=4 ELIXIR_PASS_SECS=2 nohup target/debug/elixir daemon > /tmp/ex/daemon.out 2>&1 &
sleep 1.5; echo started
