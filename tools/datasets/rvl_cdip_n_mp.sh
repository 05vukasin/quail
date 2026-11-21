#!/usr/bin/env bash
# RVL-CDIP-N-MP (jordyvl/rvl_cdip_n_mp): the RVL-CDIP-N documents as original
# multi-page PDFs (data.tar.gz). Extracted to pdf/; the tarball is kept so the
# fetch stays resumable and verifiable.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir rvl-cdip-n-mp)"
need_space "$dest" 3
hf_get jordyvl/rvl_cdip_n_mp "$dest"
if [[ ! -e "$dest/pdf/.extracted" ]]; then
    mkdir -p "$dest/pdf"
    log "extract data.tar.gz"
    tar -xzf "$dest/data.tar.gz" -C "$dest/pdf" --no-same-owner
    touch "$dest/pdf/.extracted"
fi
done_marker "$dest"
