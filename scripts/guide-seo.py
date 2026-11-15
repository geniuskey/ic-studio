"""Post-process the built user guide (target/guide) for search engines.

Reads the `<!-- seo title="..." description="..." -->` comment at the top of each page and
writes the page title, meta description, canonical URL, hreflang alternates, Open Graph
tags, JSON-LD for the home page, image sizes and lazy loading, and sitemap.xml.
"""

import html
import json
import re
import struct
import subprocess
import sys
from pathlib import Path

BASE = "https://geniuskey.github.io/ic-studio/"
SITE = "IC Studio"
OG_IMAGE = BASE + "images/og-image.jpg"
THEME_COLOR = "#161923"
SKIP = {"print.html", "toc.html", "404.html"}

root = Path(sys.argv[1] if len(sys.argv) > 1 else "target/guide")
docs = Path(__file__).resolve().parent.parent / "docs"


def url_of(rel: str) -> str:
    if rel == "introduction.html" or rel.endswith("/introduction.html"):
        rel = rel[: -len("introduction.html")]
    elif rel == "index.html" or rel.endswith("/index.html"):
        rel = rel[: -len("index.html")]
    return BASE + rel


def image_size(path: Path):
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if path.suffix == ".webp" and data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        chunk = data[12:16]
        if chunk == b"VP8 ":
            w, h = struct.unpack("<HH", data[26:30])
            return w & 0x3FFF, h & 0x3FFF
        if chunk == b"VP8L":
            b = int.from_bytes(data[21:25], "little")
            return (b & 0x3FFF) + 1, ((b >> 14) & 0x3FFF) + 1
        if chunk == b"VP8X":
            return int.from_bytes(data[24:27], "little") + 1, int.from_bytes(data[27:30], "little") + 1
    if path.suffix == ".svg":
        m = re.search(rb'<svg[^>]*\bwidth="(\d+)"[^>]*\bheight="(\d+)"', data[:2000])
        if m:
            return int(m[1]), int(m[2])
    return None


def fix_images(page: str, page_dir: Path) -> str:
    first = True

    def repl(m):
        nonlocal first
        tag = m[0]
        src = re.search(r'src="([^"]+)"', tag)
        if src and "width=" not in tag and not src[1].startswith("http"):
            size = image_size((page_dir / src[1]).resolve())
            if size:
                tag = tag.replace("<img ", f'<img width="{size[0]}" height="{size[1]}" ', 1)
        if not first and "loading=" not in tag:
            tag = tag.replace("<img ", '<img loading="lazy" decoding="async" ', 1)
        first = False
        return tag

    main = re.search(r"<main>.*</main>", page, re.S)
    if not main:
        return page
    body = re.sub(r"<img\b[^>]*>", repl, main[0])
    return page[: main.start()] + body + page[main.end() :]


def last_modified(rel: str, ko: bool):
    md = docs / ("ko" if ko else "") / Path(rel).with_suffix(".md")
    if rel.endswith("index.html"):
        md = md.with_name("introduction.md")
    try:
        out = subprocess.run(["git", "log", "-1", "--format=%cs", "--", str(md)], capture_output=True, text=True, cwd=docs)
        return out.stdout.strip() or None
    except OSError:
        return None


pages = []
for path in sorted(root.rglob("*.html")):
    rel = path.relative_to(root).as_posix()
    ko = rel.startswith("ko/")
    page = path.read_text(encoding="utf-8")
    if path.name in SKIP:
        path.write_text(re.sub(r'<!-- seo title="[^"]*" description="[^"]*" -->\s*', "", page), encoding="utf-8")
        continue
    seo = re.search(r'<!-- seo title="([^"]*)" description="([^"]*)" -->\s*', page)
    if not seo:
        continue
    title, desc = seo[1], seo[2]
    page = page[: seo.start()] + page[seo.end() :]

    en_rel = rel[3:] if ko else rel
    is_home = en_rel in ("index.html", "introduction.html")
    full_title = title if is_home else f"{title} - {SITE}"
    url = url_of(rel)
    en_url, ko_url = url_of(en_rel), url_of("ko/" + en_rel)
    has_pair = (root / en_rel).exists() and (root / "ko" / en_rel).exists()

    e = html.escape
    head = [
        f'<link rel="canonical" href="{url}">',
        f'<meta property="og:type" content="website">',
        f'<meta property="og:site_name" content="{SITE}">',
        f'<meta property="og:title" content="{e(full_title)}">',
        f'<meta property="og:description" content="{e(desc)}">',
        f'<meta property="og:url" content="{url}">',
        f'<meta property="og:image" content="{OG_IMAGE}">',
        f'<meta property="og:image:width" content="1200">',
        f'<meta property="og:image:height" content="630">',
        f'<meta property="og:locale" content="{"ko_KR" if ko else "en_US"}">',
        '<meta name="twitter:card" content="summary_large_image">',
    ]
    if has_pair:
        head += [
            f'<link rel="alternate" hreflang="en" href="{en_url}">',
            f'<link rel="alternate" hreflang="ko" href="{ko_url}">',
            f'<link rel="alternate" hreflang="x-default" href="{en_url}">',
        ]
    if is_home:
        ld = {
            "@context": "https://schema.org",
            "@type": "SoftwareApplication",
            "name": SITE,
            "description": desc,
            "url": url,
            "image": OG_IMAGE,
            "applicationCategory": "DesignApplication",
            "applicationSubCategory": "Electronic design automation (EDA)",
            "operatingSystem": "Windows, macOS, Linux",
            "license": "https://github.com/geniuskey/ic-studio/blob/main/LICENSE",
            "codeRepository": "https://github.com/geniuskey/ic-studio",
            "downloadUrl": "https://github.com/geniuskey/ic-studio/releases",
            "inLanguage": "ko" if ko else "en",
            "offers": {"@type": "Offer", "price": "0", "priceCurrency": "USD"},
        }
        head.append(f'<script type="application/ld+json">{json.dumps(ld, ensure_ascii=False)}</script>')

    page = re.sub(r"<title>.*?</title>", lambda _: f"<title>{e(full_title)}</title>", page, count=1, flags=re.S)
    page = re.sub(r'<meta name="description" content="[^"]*">', lambda _: f'<meta name="description" content="{e(desc)}">', page, count=1)
    page = page.replace('<meta name="theme-color" content="#ffffff">', f'<meta name="theme-color" content="{THEME_COLOR}">')
    page = page.replace("    </head>", "        " + "\n        ".join(head) + "\n    </head>", 1)
    page = fix_images(page, path.parent)
    path.write_text(page, encoding="utf-8")

    if path.name != "introduction.html":
        pages.append((url, en_url if has_pair else None, ko_url if has_pair else None, last_modified(en_rel, ko)))

lines = [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">',
]
for url, en_url, ko_url, mod in pages:
    lines.append(f"  <url>\n    <loc>{url}</loc>")
    if mod:
        lines.append(f"    <lastmod>{mod}</lastmod>")
    if en_url:
        lines.append(f'    <xhtml:link rel="alternate" hreflang="en" href="{en_url}"/>')
        lines.append(f'    <xhtml:link rel="alternate" hreflang="ko" href="{ko_url}"/>')
        lines.append(f'    <xhtml:link rel="alternate" hreflang="x-default" href="{en_url}"/>')
    lines.append("  </url>")
lines.append("</urlset>")
(root / "sitemap.xml").write_text("\n".join(lines) + "\n", encoding="utf-8")
print(f"seo: {len(pages)} pages in sitemap.xml")
