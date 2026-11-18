#!/usr/bin/env python3
"""Collects the translatable strings and updates po/drtagger.pot and
po/es.po.

Strings: tr("…") / trf("…", …) in the Rust sources, _("…") in the Blueprint
files, and the extra list below (labels the code passes to tr() through a
variable). A new Spanish string takes the Mac app's translation when the
Mac catalog has the same English text (its %@ / %lld / %1$@ placeholders
become {} / {0}); everything else keeps what es.po already says, and the
rest is reported as untranslated.

    scripts/update-po.py [--mac ~/mactagger/App/Resources/Localizable.xcstrings]
"""

import argparse
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SOURCES = ["app/src", "app/resources/ui", "crates/drtagger-core/src"]

# Messages that reach tr() through a variable.
EXTRA = [
    # AlbumState labels
    "Pending", "Scanning", "Ready to identify", "Identifying", "Confident match", "Needs review",
    "Applying", "Done", "Error",
    # AlbumKind display names
    "SACD ISO", "CD image + CUE", "Tracks + CUE", "Track folder",
    # Split phases (drtagger-core::disc)
    "Reading the image…", "Decoding…", "Verifying…",
    # Batch notes the core passes through tr() in helpers
    "extract", "split",
]

RUST = re.compile(r'\btrf?\(\s*"((?:[^"\\]|\\.)*)"', re.S)
BLUEPRINT = re.compile(r'\b_\(\s*"((?:[^"\\]|\\.)*)"\s*\)', re.S)
FORMAT = re.compile(r"%(?:(\d+)\$)?(?:\.\d+)?(?:@|lld|ld|d|f|s)")


def unescape(s):
    return bytes(s, "utf-8").decode("unicode_escape").encode("latin-1").decode("utf-8")


def collect():
    found = {}
    for base in SOURCES:
        for dirpath, _, files in os.walk(os.path.join(ROOT, base)):
            for f in sorted(files):
                path = os.path.join(dirpath, f)
                rel = os.path.relpath(path, ROOT)
                if f.endswith(".rs"):
                    pattern = RUST
                elif f.endswith(".blp"):
                    pattern = BLUEPRINT
                else:
                    continue
                text = open(path, encoding="utf-8").read()
                for m in pattern.finditer(text):
                    line = text.count("\n", 0, m.start()) + 1
                    found.setdefault(unescape(m.group(1)), []).append(f"{rel}:{line}")
    for e in EXTRA:
        found.setdefault(e, []).append("scripts/update-po.py")
    return found


def mac_catalog(path):
    """English text with {} holes → Spanish with {} / {n} holes."""
    if not os.path.exists(path):
        return {}
    data = json.load(open(path, encoding="utf-8"))
    out = {}
    for key, entry in data["strings"].items():
        es = entry.get("localizations", {}).get("es", {}).get("stringUnit", {}).get("value")
        if not es:
            continue
        english = FORMAT.sub("{}", key)
        positional = any(m.group(1) for m in FORMAT.finditer(es))
        spanish = FORMAT.sub(lambda m: "{" + str(int(m.group(1)) - 1) + "}" if positional and m.group(1) else "{}", es)
        out[english] = spanish
    return out


def po_escape(s):
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def read_po(path):
    out = {}
    if not os.path.exists(path):
        return out
    msgid = msgstr = None
    target = None
    for raw in open(path, encoding="utf-8"):
        line = raw.strip()
        if line.startswith("msgid "):
            if msgid is not None and msgstr is not None:
                out[msgid] = msgstr
            msgid, msgstr, target = json.loads(line[6:]), None, "id"
        elif line.startswith("msgstr "):
            msgstr, target = json.loads(line[7:]), "str"
        elif line.startswith('"') and target:
            if target == "id":
                msgid += json.loads(line)
            else:
                msgstr += json.loads(line)
    if msgid is not None and msgstr is not None:
        out[msgid] = msgstr
    out.pop("", None)
    return out


def write_po(path, entries, translations, header):
    with open(path, "w", encoding="utf-8") as f:
        f.write(header)
        for msgid in sorted(entries):
            for ref in entries[msgid][:3]:
                f.write(f"#: {ref}\n")
            if "{}" in msgid or re.search(r"\{\d\}", msgid):
                f.write("#, rust-format\n")
            f.write(f'msgid "{po_escape(msgid)}"\n')
            f.write(f'msgstr "{po_escape(translations.get(msgid, ""))}"\n\n')


HEADER = """# drtagger for GNOME.
# Copyright (C) Carlos Prieto Ortiz
# This file is distributed under the same license as the drtagger package.
msgid ""
msgstr ""
"Project-Id-Version: drtagger\\n"
"Language: {lang}\\n"
"MIME-Version: 1.0\\n"
"Content-Type: text/plain; charset=UTF-8\\n"
"Content-Transfer-Encoding: 8bit\\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\\n"

"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mac", default=os.path.expanduser("~/mactagger/App/Resources/Localizable.xcstrings"))
    args = ap.parse_args()
    entries = collect()
    os.makedirs(os.path.join(ROOT, "po"), exist_ok=True)
    write_po(os.path.join(ROOT, "po/drtagger.pot"), entries, {}, HEADER.format(lang=""))
    es_path = os.path.join(ROOT, "po/es.po")
    existing = read_po(es_path)
    mac = mac_catalog(args.mac)
    translations, from_mac, missing = {}, 0, []
    for msgid in entries:
        if existing.get(msgid):
            translations[msgid] = existing[msgid]
        elif msgid in mac:
            translations[msgid] = mac[msgid]
            from_mac += 1
        else:
            missing.append(msgid)
    write_po(es_path, entries, translations, HEADER.format(lang="es"))
    print(f"{len(entries)} strings; {len(entries) - len(missing)} translated ({from_mac} new from the Mac catalog), "
          f"{len(missing)} untranslated", file=sys.stderr)
    for m in sorted(missing):
        print("  " + m, file=sys.stderr)


if __name__ == "__main__":
    main()
