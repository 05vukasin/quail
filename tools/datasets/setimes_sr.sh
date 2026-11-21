#!/usr/bin/env bash
# SETimes.SR (classla/setimes_sr): Serbian news, gold NER + UD annotations.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir setimes-sr)"
need_space "$dest" 1
hf_get classla/setimes_sr "$dest"
for z in data_ner data_ud; do
    if [[ ! -d "$dest/$z" ]]; then unzip -q -o "$dest/$z.zip" -d "$dest/$z"; fi
done
done_marker "$dest"
