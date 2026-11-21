#!/usr/bin/env bash
# MAUD v1: merger-agreement understanding (theatticusproject/maud), CSV + contract TXT.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir maud)"
need_space "$dest" 1
hf_get theatticusproject/maud "$dest"
done_marker "$dest"
