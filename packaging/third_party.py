import json
import os
import subprocess
import sys

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(root, "THIRD-PARTY-LICENSES.txt")
NAMES = ("license", "licence", "copying", "notice", "unlicense", "authors", "ofl", "ufl", "hack-regular.txt", "emoji-icon-font-mit")
STANDARD = os.path.join(root, "packaging", "licenses")

tree = subprocess.run(
    ["cargo", "tree", "-e", "normal", "--target", "x86_64-pc-windows-msvc", "-p", "quarz", "-p", "quarz_shell", "--prefix", "none", "--format", "{p}"],
    cwd=root, capture_output=True, text=True, encoding="utf-8", check=True,
).stdout
used = set()
for line in tree.splitlines():
    parts = line.replace("(*)", "").split()
    if len(parts) >= 2 and parts[1].startswith("v"):
        used.add((parts[0], parts[1][1:]))

meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1"], cwd=root, capture_output=True, text=True, encoding="utf-8", check=True).stdout)
packages = sorted(
    (p for p in meta["packages"] if (p["name"], p["version"]) in used and p["source"]),
    key=lambda p: (p["name"].lower(), p["version"]),
)

texts = {}
order = []
listing = []
for package in packages:
    folder = os.path.dirname(package["manifest_path"])
    found = []
    for base, dirs, files in os.walk(folder):
        dirs[:] = [d for d in dirs if d not in ("target", "tests", "benches", "examples", ".git")]
        depth = os.path.relpath(base, folder).count(os.sep)
        if depth > 2:
            continue
        for name in files:
            if name.lower().startswith(NAMES) and not name.lower().endswith((".rs", ".toml", ".py", ".sh")):
                found.append(os.path.join(base, name))
    listing.append(f"{package['name']} {package['version']}  ({package.get('license') or 'see license file'})")
    for path in sorted(found):
        text = open(path, encoding="utf-8", errors="replace").read().strip()
        key = text
        if key not in texts:
            texts[key] = []
            order.append(key)
        texts[key].append(f"{package['name']} {package['version']} ({os.path.relpath(path, folder)})")
    if not found:
        key = f"License: {package.get('license')}"
        if key not in texts:
            texts[key] = []
            order.append(key)
        texts[key].append(f"{package['name']} {package['version']}")

with open(out, "w", encoding="utf-8", newline="\n") as f:
    f.write("Third-party software in Quarz\n")
    f.write("=============================\n\n")
    f.write("Quarz itself is licensed under the PolyForm Noncommercial License 1.0.0 (see LICENSE).\n")
    f.write("It contains the following third-party components, each under its own license.\n\n")
    f.write("The RAR extraction code is based on the UnRAR source of Alexander L. Roshal. It may only be\n")
    f.write("used to extract RAR archives, not to re-create the RAR compression algorithm (see the UnRAR\n")
    f.write("license below).\n\n")
    f.write("Quarz uses the library \"cabac\" (https://github.com/mcroomp/cabac), which is licensed under the\n")
    f.write("GNU Lesser General Public License version 3 or later. Its source code is available at that\n")
    f.write("address. You may replace it with a modified version by building Quarz from its source code with\n")
    f.write("your version of cabac (see the README, \"Building it yourself\"). The texts of the LGPL 3.0 and\n")
    f.write("of the GPL 3.0 it refers to are included at the end of this file.\n\n")
    f.write("Components\n----------\n\n")
    f.write("\n".join(listing))
    f.write("\n\n\nLicense texts\n-------------\n")
    for key in order:
        f.write("\n" + "=" * 78 + "\n")
        f.write("Used by: " + ", ".join(texts[key]) + "\n")
        f.write("=" * 78 + "\n\n")
        f.write(key + "\n")
    f.write("\n\nStandard license texts referred to above\n----------------------------------------\n")
    for name in ("MIT", "Apache-2.0", "BSL-1.0", "OFL-1.1", "Ubuntu-font-1.0", "LGPL-3.0-only", "GPL-3.0-only"):
        f.write("\n" + "=" * 78 + "\n" + name + "\n" + "=" * 78 + "\n\n")
        f.write(open(os.path.join(STANDARD, name + ".txt"), encoding="utf-8").read().strip() + "\n")
print(len(packages), "components,", len(order), "distinct license texts ->", out)
