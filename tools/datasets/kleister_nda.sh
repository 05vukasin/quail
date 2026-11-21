#!/usr/bin/env bash
# Kleister-NDA (applicaai/kleister-nda): 540 EDGAR NDAs as PDF, key-value gold.
# Fetched as a GitHub tarball (the PDFs are plain blobs in this repo).
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir kleister-nda)"
need_space "$dest" 1
fetch "https://codeload.github.com/applicaai/kleister-nda/tar.gz/refs/heads/master" "$dest/kleister-nda.tar.gz"
if [[ ! -d "$dest/documents" ]]; then
    tar -xzf "$dest/kleister-nda.tar.gz" -C "$dest" --strip-components=1
fi
done_marker "$dest"
