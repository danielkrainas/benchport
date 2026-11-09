#!/usr/bin/env python3
"""Generate a self-contained demo catalog under examples/demo/.

    python3 examples/make-demo.py

Produces catalog.json, news.xml, artwork and playable demo zips, all
referenced with *relative* paths so the launcher can use the folder straight
from disk (see examples/launcher.json) or you can upload it to any static host
as-is. Only the Python standard library is used.
"""

import hashlib
import json
import os
import stat
import zipfile
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent / "demo"

GAMES = [
    {
        "id": "jump-test",
        "name": "Jump Test",
        "version": "0.3.1",
        "developer": "Bench Studio",
        "tags": ["Platformer", "Prototype"],
        "featured": True,
        "hue": 205,
        "shape": "platformer",
        "description": "Early platformer prototype. We're tuning jump arcs, coyote time and wall slides — tell us which settings feel best.\n\nControls: arrow keys / WASD to move, space to jump.",
        "releaseNotes": "• Coyote time raised from 80ms to 110ms\n• Fixed double-jump firing on ledges\n• New test level: \"The Gauntlet\"",
    },
    {
        "id": "orbital-drift",
        "name": "Orbital Drift",
        "version": "1.0.0-beta.2",
        "developer": "Bench Studio",
        "tags": ["Arcade", "Space", "Beta"],
        "featured": True,
        "hue": 275,
        "shape": "orbit",
        "description": "Slingshot your tiny probe between planets using nothing but gravity. A one-button physics toy that turned into a game.",
        "releaseNotes": "• Leaderboard screen (local only)\n• 12 new orbits\n• Smoother trails on high refresh-rate displays",
    },
    {
        "id": "lantern-keep",
        "name": "Lantern Keep",
        "version": "0.0.9",
        "developer": "Night Shift",
        "tags": ["Roguelite", "Pixel art"],
        "featured": True,
        "hue": 28,
        "shape": "keep",
        "description": "Guard a lighthouse through the long night. Spend oil to push back the dark — but the dark pushes back.",
        "releaseNotes": "First playable! Expect rough edges.\n• One biome, three enemy types\n• Save/continue",
    },
    {
        "id": "puzzle-box",
        "name": "Puzzle Box",
        "version": "0.2.0",
        "developer": "Night Shift",
        "tags": ["Puzzle"],
        "featured": False,
        "hue": 150,
        "shape": "grid",
        "description": "A box of tiny sliding puzzles. Short, quiet, and occasionally evil.",
        "releaseNotes": "• 20 new puzzles\n• Undo button (finally)",
    },
    {
        # A browser game: no download, just a page the launcher opens.
        "id": "prism-garden",
        "name": "Prism Garden",
        "web": True,
        "developer": "Bench Studio",
        "tags": ["Toy", "3D", "Browser"],
        "featured": True,
        "hue": 320,
        "shape": "prisms",
        "description": "A tiny three.js toy that runs right in the launcher — nothing to download. Click to grow crystals, drag to spin the garden.\n\nNeeds an internet connection (three.js loads from a CDN).",
        "releaseNotes": "Always the latest version: browser games update the moment the page does.",
    },
]

# ------------------------------------------------------------------ artwork


def hsl(h, s, l):
    return f"hsl({h % 360} {s}% {l}%)"


def motif(shape, h, w, ht):
    """A simple, original geometric motif per game."""
    if shape == "platformer":
        blocks = "".join(
            f'<rect x="{x}" y="{y}" width="{bw}" height="26" rx="6" fill="{hsl(h + 20, 60, 62)}" opacity=".9"/>'
            for x, y, bw in [(120, 520, 260), (460, 430, 200), (760, 340, 240), (1040, 470, 180), (300, 300, 140)]
        )
        return blocks + f'<rect x="805" y="270" width="54" height="64" rx="10" fill="#fff"/><circle cx="845" cy="292" r="6" fill="{hsl(h, 60, 25)}"/>'
    if shape == "orbit":
        rings = "".join(
            f'<ellipse cx="{w*0.68}" cy="{ht*0.5}" rx="{r}" ry="{r*0.42}" fill="none" stroke="{hsl(h + 30, 80, 75)}" stroke-opacity="{0.5 - i*0.1}" stroke-width="3" transform="rotate(-18 {w*0.68} {ht*0.5})"/>'
            for i, r in enumerate([170, 260, 350])
        )
        return rings + f'<circle cx="{w*0.68}" cy="{ht*0.5}" r="90" fill="{hsl(h + 40, 85, 65)}"/><circle cx="{w*0.68 + 250}" cy="{ht*0.5 - 120}" r="18" fill="#fff"/>'
    if shape == "keep":
        return (
            f'<path d="M{w*0.66} {ht} L{w*0.66+30} {ht*0.35} L{w*0.66+110} {ht*0.35} L{w*0.66+140} {ht} Z" fill="{hsl(h, 30, 18)}"/>'
            f'<rect x="{w*0.66+40}" y="{ht*0.27}" width="60" height="50" rx="8" fill="{hsl(h + 15, 95, 65)}"/>'
            f'<circle cx="{w*0.66+70}" cy="{ht*0.32}" r="160" fill="{hsl(h + 15, 95, 60)}" opacity=".18"/>'
            f'<circle cx="{w*0.66+70}" cy="{ht*0.32}" r="320" fill="{hsl(h + 15, 95, 60)}" opacity=".08"/>'
        )
    if shape == "prisms":
        gems = "".join(
            f'<polygon points="{x},{y-s} {x+s*0.7},{y} {x},{y+s*1.3} {x-s*0.7},{y}" fill="{hsl(h + i*25, 75, 55 + (i % 3)*8)}" opacity=".9"/>'
            for i, (x, y, s) in enumerate([(820, 300, 110), (1010, 380, 70), (680, 420, 60), (930, 200, 45), (1120, 250, 38)])
        )
        return gems
    cells = "".join(
        f'<rect x="{700 + c*120}" y="{150 + r*120}" width="104" height="104" rx="18" fill="{hsl(h + (r*3+c)*9, 55, 30 + ((r+c) % 3)*12)}"/>'
        for r in range(4) for c in range(4) if (r, c) != (2, 1)
    )
    return cells


def banner_svg(g):
    w, ht, h = 1280, 720, g["hue"]
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {ht}" width="{w}" height="{ht}">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="{hsl(h, 55, 22)}"/><stop offset="1" stop-color="{hsl(h + 40, 60, 8)}"/>
    </linearGradient>
    <radialGradient id="glow" cx=".72" cy=".4" r=".6">
      <stop offset="0" stop-color="{hsl(h + 10, 90, 60)}" stop-opacity=".55"/><stop offset="1" stop-color="{hsl(h, 90, 60)}" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect width="{w}" height="{ht}" fill="url(#bg)"/>
  <rect width="{w}" height="{ht}" fill="url(#glow)"/>
  {''.join(f'<circle cx="{(i*397) % w}" cy="{(i*211) % ht}" r="{1 + i % 3}" fill="#fff" opacity="{0.15 + (i % 5)*0.08}"/>' for i in range(60))}
  {motif(g["shape"], h, w, ht)}
</svg>"""


def icon_svg(g):
    h = g["hue"]
    letter = g["name"][0]
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256">
  <defs><linearGradient id="i" x1="0" y1="0" x2="1" y2="1">
    <stop offset="0" stop-color="{hsl(h + 10, 75, 58)}"/><stop offset="1" stop-color="{hsl(h + 40, 70, 30)}"/>
  </linearGradient></defs>
  <rect width="256" height="256" rx="56" fill="url(#i)"/>
  <text x="128" y="170" text-anchor="middle" font-family="Helvetica, Arial, sans-serif" font-weight="800" font-size="140" fill="#fff">{letter}</text>
</svg>"""


def screenshot_svg(g, n):
    h = g["hue"] + n * 25
    w, ht = 1280, 720
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {ht}" width="{w}" height="{ht}">
  <rect width="{w}" height="{ht}" fill="{hsl(h, 40, 14)}"/>
  {motif(g["shape"], h, w, ht)}
  <rect x="40" y="40" width="220" height="44" rx="10" fill="#000" opacity=".35"/>
  <text x="60" y="70" font-family="Helvetica, Arial, sans-serif" font-size="22" fill="#fff">Screenshot {n + 1}</text>
</svg>"""


# ------------------------------------------------------------------ the "games"

GAME_HTML = """<!doctype html><meta charset="utf-8"><title>{name}</title>
<style>html,body{{margin:0;height:100%;background:{bg};color:#fff;font:16px system-ui;display:grid;place-items:center;overflow:hidden}}
canvas{{position:fixed;inset:0}}h1{{position:relative;font-size:48px;margin:0}}p{{position:relative;opacity:.7;text-align:center}}</style>
<canvas id=c></canvas><div><h1>{name}</h1><p>v{version} — demo build launched by Benchport.<br>Close this tab whenever you like.</p></div>
<script>const c=document.getElementById('c'),x=c.getContext('2d');let W,H,b=[];function r(){{W=c.width=innerWidth;H=c.height=innerHeight}}r();onresize=r;
for(let i=0;i<40;i++)b.push({{x:Math.random()*W,y:Math.random()*H,vx:(Math.random()-.5)*4,vy:(Math.random()-.5)*4,r:4+Math.random()*14}});
(function f(){{x.clearRect(0,0,W,H);for(const p of b){{p.x+=p.vx;p.y+=p.vy;if(p.x<0||p.x>W)p.vx*=-1;if(p.y<0||p.y>H)p.vy*=-1;x.beginPath();x.arc(p.x,p.y,p.r,0,7);x.fillStyle='hsla({hue},80%,65%,.5)';x.fill()}}requestAnimationFrame(f)}})()</script>"""

PLAY_SH = """#!/bin/sh
# Demo "game": opens the bundled HTML page and stays alive for a bit so the
# launcher can show it as running.
DIR="$(cd "$(dirname "$0")" && pwd)"
if command -v xdg-open >/dev/null 2>&1; then xdg-open "$DIR/index.html" >/dev/null 2>&1 &
elif command -v open >/dev/null 2>&1; then open "$DIR/index.html"; fi
sleep 20
"""

PLAY_BAT = """@echo off
start "" "%~dp0index.html"
timeout /t 20 /nobreak >nul
"""


def make_zip(g, path):
    folder = g["name"].replace(" ", "")
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        # Wrapped in a top-level folder on purpose: the launcher unwraps it.
        def add(name, data, mode=0o644):
            info = zipfile.ZipInfo(f"{folder}/{name}", date_time=(2026, 11, 8, 12, 0, 0))
            info.external_attr = (stat.S_IFREG | mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)

        bg = hsl(g["hue"], 45, 14)
        add("index.html", GAME_HTML.format(name=g["name"], version=g["version"], bg=bg, hue=g["hue"]))
        add("play.sh", PLAY_SH, 0o755)
        add("play.bat", PLAY_BAT)
        add("README.txt", f"{g['name']} v{g['version']}\nThis is a placeholder build generated by examples/make-demo.py.\n")


WEB_GAME_HTML = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Prism Garden</title>
<style>
  html, body { margin: 0; height: 100%; background: #12081a; overflow: hidden; font-family: system-ui, sans-serif; }
  #hud { position: fixed; left: 16px; bottom: 14px; color: #f3d9ff; opacity: .75; font-size: 13px; pointer-events: none; }
  #err { position: fixed; inset: 0; display: none; place-items: center; color: #f3d9ff; text-align: center; }
</style>
<script type="importmap">{ "imports": { "three": "https://cdn.jsdelivr.net/npm/three@0.160.0/build/three.module.js" } }</script>
</head>
<body>
<div id="hud">Click to grow a crystal · drag to spin · <span id="n">0</span> crystals</div>
<div id="err"><div><h2>Couldn't load three.js</h2><p>This demo needs an internet connection.</p></div></div>
<script>setTimeout(() => { if (!window.__ok) document.getElementById('err').style.display = 'grid'; }, 8000);</script>
<script type="module">
import * as THREE from 'three';
window.__ok = true;

const renderer = new THREE.WebGLRenderer({ antialias: true });
renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
renderer.setClearColor(0x12081a);
document.body.appendChild(renderer.domElement);
const scene = new THREE.Scene();
scene.fog = new THREE.Fog(0x12081a, 8, 26);
const camera = new THREE.PerspectiveCamera(50, 1, 0.1, 100);
camera.position.set(0, 5, 11);
camera.lookAt(0, 0.8, 0);

scene.add(new THREE.HemisphereLight(0xffd6ff, 0x1a0830, 1.2));
const sun = new THREE.DirectionalLight(0xffffff, 2);
sun.position.set(4, 8, 5);
scene.add(sun);

const garden = new THREE.Group();
scene.add(garden);
const ground = new THREE.Mesh(new THREE.CylinderGeometry(6, 6.4, 0.4, 48), new THREE.MeshStandardMaterial({ color: 0x2a1238, roughness: 0.9 }));
ground.position.y = -0.2;
garden.add(ground);

const geo = new THREE.OctahedronGeometry(1, 0);
const crystals = [];
function grow(x, z) {
  const hue = (0.8 + Math.random() * 0.35) % 1;
  const m = new THREE.Mesh(geo, new THREE.MeshStandardMaterial({ color: new THREE.Color().setHSL(hue, 0.75, 0.6), roughness: 0.15, metalness: 0.2, flatShading: true }));
  const h = 0.6 + Math.random() * 1.6;
  m.userData = { h, t: 0, spin: (Math.random() - 0.5) * 0.02 };
  m.position.set(x, 0, z);
  m.scale.set(0.001, 0.001, 0.001);
  garden.add(m);
  crystals.push(m);
  document.getElementById('n').textContent = crystals.length;
}
for (let i = 0; i < 7; i++) { const a = Math.random() * 6.28, r = Math.random() * 4.5; grow(Math.cos(a) * r, Math.sin(a) * r); }

const ray = new THREE.Raycaster(), ptr = new THREE.Vector2();
let drag = null, moved = 0, vel = 0.003;
addEventListener('pointerdown', (e) => { drag = e.clientX; moved = 0; });
addEventListener('pointermove', (e) => { if (drag != null) { const d = e.clientX - drag; drag = e.clientX; moved += Math.abs(d); vel = d * 0.0004; garden.rotation.y += d * 0.008; } });
addEventListener('pointerup', (e) => {
  drag = null;
  if (moved > 4) return;
  ptr.set((e.clientX / innerWidth) * 2 - 1, -(e.clientY / innerHeight) * 2 + 1);
  ray.setFromCamera(ptr, camera);
  const hit = ray.intersectObject(ground)[0];
  if (hit) { const p = garden.worldToLocal(hit.point.clone()); grow(p.x, p.z); }
});

function resize() { renderer.setSize(innerWidth, innerHeight); camera.aspect = innerWidth / innerHeight; camera.updateProjectionMatrix(); }
addEventListener('resize', resize);
resize();

renderer.setAnimationLoop(() => {
  if (drag == null) garden.rotation.y += vel;
  vel += (0.003 - vel) * 0.02;
  for (const c of crystals) {
    const d = c.userData;
    d.t = Math.min(1, d.t + 0.02);
    const k = 1 - Math.pow(1 - d.t, 3);
    c.scale.set(0.45 * k, d.h * k, 0.45 * k);
    c.position.y = d.h * k;
    c.rotation.y += d.spin;
  }
  renderer.render(scene, camera);
});
</script>
</body>
</html>
"""


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    for sub in ("art", "builds"):
        (ROOT / sub).mkdir(parents=True, exist_ok=True)

    entries = []
    for g in GAMES:
        (ROOT / "art" / f"{g['id']}-banner.svg").write_text(banner_svg(g))
        (ROOT / "art" / f"{g['id']}-icon.svg").write_text(icon_svg(g))
        shots = []
        for n in range(3):
            name = f"{g['id']}-shot{n + 1}.svg"
            (ROOT / "art" / name).write_text(screenshot_svg(g, n))
            shots.append(f"art/{name}")

        if g.get("web"):
            page = ROOT / "web" / g["id"] / "index.html"
            page.parent.mkdir(parents=True, exist_ok=True)
            page.write_text(WEB_GAME_HTML)
            entries.append({
                "id": g["id"],
                "name": g["name"],
                "description": g["description"],
                "playUrl": f"web/{g['id']}/index.html",
                "icon": f"art/{g['id']}-icon.svg",
                "banner": f"art/{g['id']}-banner.svg",
                "screenshots": shots,
                "tags": g["tags"],
                "developer": g["developer"],
                "releaseNotes": g["releaseNotes"],
                "featured": g["featured"],
            })
            continue

        zip_name = f"{g['id']}-{g['version']}.zip"
        zip_path = ROOT / "builds" / zip_name
        for old in (ROOT / "builds").glob(f"{g['id']}-*.zip"):
            if old != zip_path:
                old.unlink()
        make_zip(g, zip_path)

        entries.append({
            "id": g["id"],
            "name": g["name"],
            "version": g["version"],
            "description": g["description"],
            "downloadUrl": f"builds/{zip_name}",
            "sha256": sha256(zip_path),
            "size": zip_path.stat().st_size,
            "executable": "play.sh",
            "platforms": {"windows": {"executable": "play.bat"}},
            "icon": f"art/{g['id']}-icon.svg",
            "banner": f"art/{g['id']}-banner.svg",
            "screenshots": shots,
            "tags": g["tags"],
            "developer": g["developer"],
            "releaseNotes": g["releaseNotes"],
            "released": date.today().isoformat(),
            "featured": g["featured"],
        })

    catalog = {"version": date.today().isoformat(), "games": entries}
    (ROOT / "catalog.json").write_text(json.dumps(catalog, indent=2) + "\n")

    now = datetime.now(timezone.utc)
    posts = [
        ("Jump Test 0.3.1 is out", "Coyote time got a buff and the double-jump bug on ledges is gone. Thanks to everyone who sent clips!", 0, "jump-test"),
        ("Orbital Drift enters beta", "The leaderboard is in, along with twelve new orbits. This is the build we'd love the most feedback on.", 3, "orbital-drift"),
        ("Say hello to Lantern Keep", "Our newest prototype is about keeping a light on through the night. It's rough, it's early, and we're excited.", 9, "lantern-keep"),
        ("How to send us feedback", "Hit reply on any of our emails or drop a note in the group chat. Screenshots and short videos help a ton.", 15, None),
    ]
    items = "".join(
        f"""
    <item>
      <title>{title}</title>
      <link>https://example.com/news/{i}</link>
      <guid>https://example.com/news/{i}</guid>
      <pubDate>{(now - timedelta(days=d)).strftime('%a, %d %b %Y %H:%M:%S +0000')}</pubDate>
      <description>{body}</description>
    </item>"""
        for i, (title, body, d, _gid) in enumerate(posts)
    )
    (ROOT / "news.xml").write_text(f"""<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Bench Studio news</title>
    <link>https://example.com</link>
    <description>Updates from the bench</description>{items}
  </channel>
</rss>
""")
    print(f"Demo catalog written to {ROOT}")


if __name__ == "__main__":
    main()
