#!/usr/bin/env bash
# TABME++ (rootsautomation/TABMEpp, now redirected to bevaya/TABMEpp):
# page-stream segmentation; parquet rows = page image + OCR. Test + val only.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir tabmepp)"
need_space "$dest" 2
hf_get rootsautomation/TABMEpp "$dest" \
    --include README.md \
    --include "data/test-*" --include "data/val-*" \
    --include "streams/test_folders.txt" --include "streams/val_folders.txt"
done_marker "$dest"
