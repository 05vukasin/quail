#!/usr/bin/env bash
# postocr-sr (Sagicc/postocr-sr): Serbian OCR-noise / corrected text pairs (JSONL).
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir postocr-sr)"
need_space "$dest" 1
hf_get Sagicc/postocr-sr "$dest"
done_marker "$dest"
