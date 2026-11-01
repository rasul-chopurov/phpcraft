#!/usr/bin/env python3
"""Builds the path.az site into site/public: the home page in English (/), Azerbaijani (/az/)
and Russian (/ru/), plus the PhpCraft page, images and styles."""

import json
import pathlib
import re
import shutil

SITE = pathlib.Path(__file__).resolve().parent
OUT = SITE / "public"
LANGS = {"en": "/", "az": "/az/", "ru": "/ru/"}


def build():
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir()
    template = (SITE / "src" / "home.html").read_text()
    strings = json.loads((SITE / "src" / "i18n.json").read_text())
    for lang, path in LANGS.items():
        values = dict(strings[lang], lang=lang, path=path, home=path)
        values["langs"] = "".join(
            f'<a href="{p}"{" class=\"on\"" if l == lang else ""}>{l.upper()}</a>' for l, p in LANGS.items()
        )

        def fill(match):
            key = match.group(1)
            if key not in values:
                raise SystemExit(f"missing {lang}.{key}")
            return values[key]

        page = re.sub(r"\{\{(\w+)\}\}", fill, template)
        target = OUT / path.strip("/") / "index.html" if path != "/" else OUT / "index.html"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(page)
    shutil.copy(SITE / "src" / "style.css", OUT / "style.css")
    shutil.copytree(SITE / "img", OUT / "img")
    shutil.copytree(SITE / "phpcraft", OUT / "phpcraft")
    print(f"built {OUT}")


if __name__ == "__main__":
    build()
