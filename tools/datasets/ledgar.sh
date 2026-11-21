#!/usr/bin/env bash
# LEDGAR (LexGLUE config `ledgar`): contract provisions labelled with 100 clause types.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir ledgar)"
need_space "$dest" 1
hf_get coastalcph/lex_glue "$dest" --include README.md --include "ledgar/*"
done_marker "$dest"
