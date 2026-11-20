#!/usr/bin/env bash
# SEC EDGAR Exhibit-10 material contracts (chenghao/sec-material-contracts).
# Full set is 304 parquet shards (~40 GB); we take only the first 2 shards.
# Each row embeds the original filing file bytes (`file`, `extension`).
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir sec-material-contracts)"
need_space "$dest" 1
hf_get chenghao/sec-material-contracts "$dest" \
    --include README.md \
    --include "data/train-00000-of-00304.parquet" \
    --include "data/train-00001-of-00304.parquet"
done_marker "$dest"
