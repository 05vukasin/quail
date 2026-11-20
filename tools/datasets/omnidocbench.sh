#!/usr/bin/env bash
# OmniDocBench (opendatalab/OmniDocBench): page images + rich layout/reading-order GT.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir omnidocbench)"
need_space "$dest" 2
hf_get opendatalab/OmniDocBench "$dest"
done_marker "$dest"
