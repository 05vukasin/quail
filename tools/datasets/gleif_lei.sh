#!/usr/bin/env bash
# GLEIF LEI Level-1 Golden Copy (CSV, zipped). Resolves "latest" via the public
# Golden Copy API and keeps the zip as-is (the CSV inflates to several GB; read
# it streamed with `unzip -p`). Re-running fetches a newer publish if one exists.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir gleif-lei)"
need_space "$dest" 1
api="https://goldencopy.gleif.org/api/v2/golden-copies/publishes/lei2/latest"
url="$(curl -fsSL -A "$UA" "$api" | python3 -I -c \
    'import json, sys; print(json.load(sys.stdin)["data"]["full_file"]["csv"]["url"])')"
[[ "$url" == https://goldencopy.gleif.org/* ]] || die "unexpected Golden Copy URL: $url"
fetch "$url" "$dest/$(basename "$url")"
done_marker "$dest"
