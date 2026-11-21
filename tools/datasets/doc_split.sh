#!/usr/bin/env bash
# DocSplit (amazon/doc_split): document-packet splitting benchmark. Ground truth
# only (JSON/CSV); the source PDFs are RVL-CDIP-N-MP, see rvl_cdip_n_mp.sh.
# The full repo is ~52k small JSON files (anonymous HF requests get rate
# limited), so we take the per-benchmark CSVs, the test-split JSONs and code.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir doc-split)"
need_space "$dest" 1
hf_get amazon/doc_split "$dest" \
    --include README.md --include LICENSE --include "src/*" \
    --include "datasets/*/*/*.csv" \
    --include "datasets/*/*/ground_truth_json/test/*"
done_marker "$dest"
