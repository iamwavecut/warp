'''
Generates the `ExternalFontFamily` definitions used in `app/src/font_fallback.rs`.
These definitions contain the URLs to each external fallback font we use in Warp.
Generated code is sent to stdout.

This script reads fallback fonts from a local directory and generates the code
required to initialize static references for each font family.

Usage:
1. Put fallback `.ttf` files under `downloaded_fonts/`, grouped by family, or set
   `WARP_FALLBACK_FONTS_DIR` to another local directory.
2. Run `python3 generate_families.py`
3. Manually inspect the name for each font. The script will generate the name in
   title-case, but this isn't correct for some fonts (e.g. Noto Sans SC).
'''

import os
import sys
from collections import defaultdict

FONT_SOURCE_DIR = os.environ.get("WARP_FALLBACK_FONTS_DIR", "downloaded_fonts")


def list_fonts():
    if not os.path.isdir(FONT_SOURCE_DIR):
        sys.exit(f"Fallback font directory not found: {FONT_SOURCE_DIR}")

    font_paths = []
    for root, _, files in os.walk(FONT_SOURCE_DIR):
        for filename in files:
            if not filename.endswith(".ttf"):
                continue
            relative_path = os.path.relpath(os.path.join(root, filename), FONT_SOURCE_DIR)
            parts = relative_path.split(os.sep)
            if len(parts) == 1:
                family_name = os.path.splitext(filename)[0]
                font_paths.append(f"{family_name}/{filename}")
            else:
                font_paths.append("/".join(parts))
    return font_paths


def generate_families(font_uris):
    family_map = defaultdict(list)
    for uri in font_uris:
        parts = uri.split('/')
        family_name = parts[0]
        font_name = parts[1]
        family_map[family_name].append(font_name)

    for family_name, font_names in family_map.items():
        print_family(family_name, font_names)


def indent_level(level, s):
    indent = "    " * level
    return indent + s


def print_family(family_name, font_names):
    variable_name = family_name.replace('-', '_').upper()
    title_case_name = family_name.replace('-', ' ').title()

    print(f"static ref {variable_name}: ExternalFontFamily = ExternalFontFamily {{")
    # Title-case is not correct for some fonts, e.g. "Noto Sans SC", so we add
    # a todo to make any manual adjustments.
    print(indent_level(1, f"name: \"{title_case_name}\", // TODO: double-check the title is correct"))
    print(indent_level(1, "font_urls: Arc::new(vec!["))
    for font_name in font_names:
        print(indent_level(2, f"url_for_font(\"{family_name}\", \"{font_name}\"),"))
    print(indent_level(1, "]),"))
    print("};")


def main():
    font_uris = list_fonts()
    generate_families(font_uris)


if __name__ == "__main__":
    main()
