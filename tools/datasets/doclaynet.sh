#!/usr/bin/env bash
# DocLayNet v1.2 (docling-project/DocLayNet-v1.2): layout GT + PDF cells. Test split only (~2.4 GB).
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir doclaynet-v1.2)"
need_space "$dest" 3
hf_get docling-project/DocLayNet-v1.2 "$dest" --include README.md --include "data/test-*"
done_marker "$dest"
