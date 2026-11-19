#!/bin/sh
# api.sh ROUTE [JSON]
if [ -n "$2" ]; then curl -s -X POST -H 'Content-Type: application/json' -d "$2" "http://127.0.0.1:5471/api?r=$1"; else curl -s "http://127.0.0.1:5471/api?r=$1"; fi
