#!/bin/sh
# Restart only the stand-in services, waiting until the old ones are gone.
pkill -f "mocks.py /tmp/ex"
sleep 1
cd "$(dirname "$0")" && nohup python3 mocks.py /tmp/ex/torrents.json > /tmp/ex/mocks.log 2>&1 &
sleep 1; grep -q "mocks up" /tmp/ex/mocks.log && echo mocks-restarted || cat /tmp/ex/mocks.log
