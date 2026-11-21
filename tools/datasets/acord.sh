#!/usr/bin/env bash
# ACORD: Atticus Clause Retrieval Dataset (theatticusproject/acord), single zip.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir acord)"
need_space "$dest" 1
hf_get theatticusproject/acord "$dest"
if [[ ! -e "$dest/.unzipped" ]]; then
    unzip -q -o "$dest/ACORD Dataset & ReadMe.zip" -d "$dest/extracted"
    rm -rf "$dest/extracted/__MACOSX"
    touch "$dest/.unzipped"
fi
done_marker "$dest"
