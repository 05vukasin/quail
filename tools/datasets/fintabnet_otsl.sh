#!/usr/bin/env bash
# FinTabNet-OTSL (docling-project/FinTabNet_OTSL): financial table structure GT. Test split only.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir fintabnet-otsl)"
need_space "$dest" 1
hf_get docling-project/FinTabNet_OTSL "$dest" --include README.md --include "data/test-*"
done_marker "$dest"
