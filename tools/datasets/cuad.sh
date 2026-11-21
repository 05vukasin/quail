#!/usr/bin/env bash
# CUAD v1: 510 commercial contracts (PDF + TXT) with 41 clause labels.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir cuad)"
need_space "$dest" 1
fetch "https://zenodo.org/records/4595826/files/CUAD_v1.zip?download=1" "$dest/CUAD_v1.zip"
if [[ ! -d "$dest/CUAD_v1/full_contract_pdf" ]]; then
    log "unzip CUAD_v1.zip"
    unzip -q -o "$dest/CUAD_v1.zip" -d "$dest"
fi
[[ -d "$dest/CUAD_v1/full_contract_pdf" ]] || die "CUAD_v1/full_contract_pdf missing after unzip"
rm -rf "$dest/__MACOSX"
done_marker "$dest"
