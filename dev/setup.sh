#!/bin/sh
# Fresh fake NAS under /tmp/ex.
set -e
rm -rf /tmp/ex; R=/tmp/ex/root/volume1; T=$R/data/torrents
mkdir -p $T $R/TV "$R/Movies/Action Adventure" $R/Movies/Comedy $R/Movies/Suspense $R/Movies/Animated $R/Anime $R/Inbox /tmp/ex/var
mk() { mkdir -p "$(dirname "$1")"; truncate -s ${2:-700M} "$1"; }
ROP="$T/www.UIndex.org    -    The.Lord.of.the.Rings.The.Rings.of.Power.S03E05.1080p.HEVC.x265-MeGusta"
mk "$ROP/The.Lord.of.the.Rings.The.Rings.of.Power.S03E05.1080p.HEVC.x265-MeGusta.mkv"
mk "$T/The.Bear.S02E03.1080p.WEB.h264/The.Bear.S02E03.1080p.WEB.h264.mkv"
echo sub > "$T/The.Bear.S02E03.1080p.WEB.h264/The.Bear.S02E03.1080p.WEB.h264.en.srt"
mk "$T/The.Bear.S02E03.1080p.WEB.h264/Sample/the.bear.s02e03.sample.mkv" 20M
mk "$T/Dune.Part.Two.2024.1080p.WEB-DL/Dune.Part.Two.2024.1080p.WEB-DL.mkv"
mk "$T/[SubsPlease] Sousou no Frieren - 25 (1080p) [ABCD1234].mkv"
mk "$T/Grabbed.Show.S01E01.720p/Grabbed.Show.S01E01.720p.mkv"
mk "$R/Inbox/Shogun.2024.S01E01.Anjin.1080p.mkv"
mk "$R/Inbox/Mystery.Thing.mkv" 300M
cat > /tmp/ex/torrents.json <<J
[
 {"hash": "r0p", "dir": "$T", "files": ["$(basename "$ROP")/The.Lord.of.the.Rings.The.Rings.of.Power.S03E05.1080p.HEVC.x265-MeGusta.mkv"]},
 {"hash": "bear", "dir": "$T", "files": ["The.Bear.S02E03.1080p.WEB.h264/The.Bear.S02E03.1080p.WEB.h264.mkv", "The.Bear.S02E03.1080p.WEB.h264/The.Bear.S02E03.1080p.WEB.h264.en.srt", "The.Bear.S02E03.1080p.WEB.h264/Sample/the.bear.s02e03.sample.mkv"]},
 {"hash": "dune", "dir": "$T", "files": ["Dune.Part.Two.2024.1080p.WEB-DL/Dune.Part.Two.2024.1080p.WEB-DL.mkv"]},
 {"hash": "frieren", "dir": "$T", "files": ["[SubsPlease] Sousou no Frieren - 25 (1080p) [ABCD1234].mkv"]},
 {"hash": "aaaa1111", "dir": "$T", "files": ["Grabbed.Show.S01E01.720p/Grabbed.Show.S01E01.720p.mkv"]}
]
J
cat > /tmp/ex/var/config.json <<J
{"tmdb_key":"tmdbkey","tvdb_key":"tvdbkey","lifestream_url":"http://127.0.0.1:7803","lifestream_user":"admin","lifestream_pass":"pw",
 "midgar_url":"http://127.0.0.1:7804","midgar_key":"midgarkey","turks_url":"http://127.0.0.1:7805","turks_key":"turkskey","anime_folder":"$R/Anime"}
J
echo ok
