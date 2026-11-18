#!/usr/bin/env python3
"""Mirror a sample tree to another machine without copying the audio.

The scanner reads directory listings, file sizes, CUE/log text and a few
sectors of SACD ISOs. `pack` writes a tar with the small files as they are,
the ISO sectors the probe reads (master TOC/text and every area TOC) and a
manifest of every other file's size; `unpack` rebuilds the tree with sparse
files of the same sizes.

    python3 -I parity/mirror.py pack ~/mactagger-samples > samples.tar
    python3 -I parity/mirror.py pack ~/nfs isos rips dsf > nas.tar   # only those subfolders
    python3 -I mirror.py unpack ~/samples/mactagger-samples < samples.tar
"""

import io
import json
import os
import sys
import tarfile

SECTOR = 2048
REAL_EXTENSIONS = {"cue", "log", "txt", "accurip", "m3u", "m3u8", "xml", "nfo", "md5", "ffp", "sfv", "json"}
SMALL = 64 * 1024


def iso_chunks(path):
    """(offset, bytes) for the sectors SACDProbe reads."""
    chunks = []
    with open(path, "rb") as f:
        head = f.read(512 * SECTOR)
        chunks.append((0, head))
        master = head[510 * SECTOR : 511 * SECTOR]
        if master[:8] == b"SACDMTOC":
            for off in (64, 72):
                sector = int.from_bytes(master[off : off + 4], "big")
                if sector and sector * SECTOR >= len(head):
                    f.seek(sector * SECTOR)
                    chunks.append((sector * SECTOR, f.read(16 * SECTOR)))
    return chunks


def pack(src, subdirs):
    src = os.path.abspath(os.path.expanduser(src))
    tops = [os.path.join(src, d) for d in subdirs] or [src]
    manifest = {"dirs": [os.path.relpath(os.path.dirname(t), src) for t in tops if t != src], "files": []}
    out = tarfile.open(fileobj=sys.stdout.buffer, mode="w|")

    def add_bytes(name, data):
        info = tarfile.TarInfo(name)
        info.size = len(data)
        out.addfile(info, io.BytesIO(data))

    n = 0
    walks = (entry for top in tops for entry in os.walk(top))
    for dirpath, dirnames, filenames in walks:
        dirnames.sort()
        rel_dir = os.path.relpath(dirpath, src)
        manifest["dirs"].append(rel_dir)
        for name in sorted(filenames):
            path = os.path.join(dirpath, name)
            if os.path.islink(path):
                continue
            rel = os.path.normpath(os.path.join(rel_dir, name))
            ext = name.rsplit(".", 1)[-1].lower() if "." in name else ""
            try:
                if not os.path.isfile(path):
                    continue  # unreadable: the scanner sees neither a file nor a folder
                size = os.path.getsize(path)
                if ext in REAL_EXTENSIONS or size <= SMALL:
                    with open(path, "rb") as f:
                        chunks = [(0, f.read())]
                elif ext == "iso":
                    chunks = iso_chunks(path)
                else:
                    chunks = []
            except OSError as e:
                print(f"skipped {path}: {e}", file=sys.stderr)
                continue
            entry = {"path": rel, "size": size, "chunks": []}
            for offset, data in chunks:
                member = f"__chunks__/{n}"
                n += 1
                add_bytes(member, data)
                entry["chunks"].append({"offset": offset, "member": member})
            manifest["files"].append(entry)
    add_bytes("__manifest__.json", json.dumps(manifest).encode())
    out.close()


def unpack(dest):
    dest = os.path.abspath(os.path.expanduser(dest))
    members = {}
    with tarfile.open(fileobj=sys.stdin.buffer, mode="r|") as tar:
        for m in tar:
            members[m.name] = tar.extractfile(m).read()
    manifest = json.loads(members["__manifest__.json"])
    for d in manifest["dirs"]:
        os.makedirs(os.path.join(dest, d), exist_ok=True)
    for entry in manifest["files"]:
        path = os.path.join(dest, entry["path"])
        with open(path, "wb") as f:
            for chunk in entry["chunks"]:
                f.seek(chunk["offset"])
                f.write(members[chunk["member"]])
            f.truncate(entry["size"])
    print(f"{len(manifest['files'])} files, {len(manifest['dirs'])} folders in {dest}", file=sys.stderr)


if __name__ == "__main__":
    if len(sys.argv) < 3 or sys.argv[1] not in ("pack", "unpack"):
        sys.exit(__doc__)
    if sys.argv[1] == "pack":
        pack(sys.argv[2], sys.argv[3:])
    else:
        unpack(sys.argv[2])
