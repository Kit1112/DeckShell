#!/usr/bin/env python3
"""Bound the live launcher log, retaining the previous segment on rotation."""
import os
import sys

LIMIT = 1024 * 1024
path = sys.argv[1]

# Read bounded chunks even if a child writes an exceptionally long line.
def main():
    output = open(path, "ab", buffering=0)
    size = output.tell()
    while chunk := sys.stdin.buffer.read1(65536):
        offset = 0
        while offset < len(chunk):
            if size >= LIMIT:
                output.close()
                os.replace(path, path + ".old")
                output = open(path, "ab", buffering=0)
                size = 0
            part = chunk[offset:offset + LIMIT - size]
            output.write(part)
            size += len(part)
            offset += len(part)
    output.close()


if __name__ == "__main__":
    main()
