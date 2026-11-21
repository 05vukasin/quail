#!/usr/bin/env bash
# RVL-CDIP-N (jordyvl/RVL-CDIP-N): 991 out-of-distribution document images, 16 classes.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir rvl-cdip-n)"
need_space "$dest" 1
hf_get jordyvl/RVL-CDIP-N "$dest"
done_marker "$dest"
