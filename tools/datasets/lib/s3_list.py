#!/usr/bin/env python3
"""List objects of a public (anonymous) S3-compatible bucket, stdlib only.

Usage:
    s3_list.py ENDPOINT BUCKET PREFIX [--summary]

Prints one line per object: "<size>\t<key>". With --summary, prints counts and
sizes grouped by the first two key components and file extension instead.
"""

import collections
import sys
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET

NS = {"s": "http://s3.amazonaws.com/doc/2006-03-01/"}
UA = "dataset-fetch/1.0"


def list_objects(endpoint, bucket, prefix):
    token = None
    while True:
        query = {"list-type": "2", "prefix": prefix}
        if token:
            query["continuation-token"] = token
        url = f"{endpoint.rstrip('/')}/{bucket}?{urllib.parse.urlencode(query)}"
        req = urllib.request.Request(url, headers={"User-Agent": UA})
        with urllib.request.urlopen(req, timeout=120) as resp:
            root = ET.fromstring(resp.read())
        for c in root.findall("s:Contents", NS):
            yield c.find("s:Key", NS).text, int(c.find("s:Size", NS).text)
        if root.find("s:IsTruncated", NS).text != "true":
            return
        token = root.find("s:NextContinuationToken", NS).text


def main(argv):
    if len(argv) < 4:
        sys.exit(__doc__)
    endpoint, bucket, prefix = argv[1:4]
    summary = "--summary" in argv[4:]
    counts, sizes = collections.Counter(), collections.Counter()
    for key, size in list_objects(endpoint, bucket, prefix):
        if not summary:
            print(f"{size}\t{key}")
            continue
        parts = key.split("/")
        group = "/".join(parts[:2])
        if len(parts) > 2:
            group += "/*." + key.rsplit(".", 1)[-1]
        counts[group] += 1
        sizes[group] += size
    for group in sorted(counts):
        print(f"{group}\t{counts[group]}\t{sizes[group] / 1e6:.1f} MB")


if __name__ == "__main__":
    main(sys.argv)
