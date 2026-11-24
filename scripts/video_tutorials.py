#!/usr/bin/env python3
"""Validate the video tutorial series and build transcripts (issue #1722).

Subcommands:
  check        Validate docs/video-tutorials/series.json and its captions (default).
  transcripts  Write docs/video-tutorials/transcripts/<id>.md from each caption file.

The video files themselves are optional until a video is recorded. A video
with status "script-ready" may have no file yet; a video marked "published"
must have one.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SERIES_DIR = ROOT / "docs" / "video-tutorials"
SERIES_FILE = SERIES_DIR / "series.json"
TRANSCRIPT_DIR = SERIES_DIR / "transcripts"

ALLOWED_STATUS = {"script-ready", "recorded", "published"}
TIMESTAMP_RE = re.compile(r"^(\d{2}):(\d{2}):(\d{2})\.(\d{3}) --> (\d{2}):(\d{2}):(\d{2})\.(\d{3})")


def parse_vtt(path):
    """Return a list of (start_seconds, end_seconds, text) cues."""
    lines = path.read_text(encoding="utf-8").splitlines()
    if not lines or lines[0].strip() != "WEBVTT":
        raise ValueError("first line must be WEBVTT")
    cues = []
    i = 0
    while i < len(lines):
        match = TIMESTAMP_RE.match(lines[i])
        if not match:
            i += 1
            continue
        h1, m1, s1, ms1, h2, m2, s2, ms2 = (int(g) for g in match.groups())
        start = h1 * 3600 + m1 * 60 + s1 + ms1 / 1000
        end = h2 * 3600 + m2 * 60 + s2 + ms2 / 1000
        i += 1
        text_lines = []
        while i < len(lines) and lines[i].strip():
            text_lines.append(lines[i].strip())
            i += 1
        cues.append((start, end, " ".join(text_lines)))
    return cues


def check():
    errors = []
    if not SERIES_FILE.exists():
        print(f"ERROR: {SERIES_FILE} is missing", file=sys.stderr)
        return 1

    series = json.loads(SERIES_FILE.read_text(encoding="utf-8"))
    ids = set()

    for video in series.get("videos", []):
        vid = video.get("id", "<missing id>")

        if vid in ids:
            errors.append(f"{vid}: duplicate id")
        ids.add(vid)

        if video.get("status") not in ALLOWED_STATUS:
            errors.append(f"{vid}: status must be one of {sorted(ALLOWED_STATUS)}")

        chapters = video.get("chapters", [])
        if not chapters:
            errors.append(f"{vid}: at least one chapter is required")
        starts = [c.get("start") for c in chapters]
        if starts and starts[0] != 0:
            errors.append(f"{vid}: first chapter must start at 0")
        if starts != sorted(starts) or len(set(starts)) != len(starts):
            errors.append(f"{vid}: chapter starts must be strictly increasing")
        for chapter in chapters:
            if not chapter.get("title"):
                errors.append(f"{vid}: every chapter needs a title")

        caption_rel = video.get("captions")
        if not caption_rel:
            errors.append(f"{vid}: captions are required (closed captions, issue #1722)")
            cues = []
        else:
            caption_path = SERIES_DIR / caption_rel
            if not caption_path.exists():
                errors.append(f"{vid}: caption file {caption_rel} is missing")
                cues = []
            else:
                try:
                    cues = parse_vtt(caption_path)
                except ValueError as exc:
                    errors.append(f"{vid}: {caption_rel}: {exc}")
                    cues = []
                if cues and video.get("duration_seconds"):
                    if cues[-1][1] > video["duration_seconds"]:
                        errors.append(f"{vid}: captions run past duration_seconds")

        for prev, cur in zip(cues, cues[1:]):
            if cur[0] < prev[1]:
                errors.append(f"{vid}: caption cues overlap near {cur[0]:.1f}s")
                break

        video_rel = video.get("file")
        if not video_rel or not video_rel.endswith(".mp4"):
            errors.append(f"{vid}: file must be an .mp4 path")
        elif video.get("status") == "published" and not (SERIES_DIR / video_rel).exists():
            errors.append(f"{vid}: status is published but {video_rel} does not exist")

    if errors:
        for err in errors:
            print(f"ERROR: {err}", file=sys.stderr)
        return 1

    print(f"✓ Video tutorial check passed ({len(ids)} videos)")
    return 0


def transcripts():
    series = json.loads(SERIES_FILE.read_text(encoding="utf-8"))
    TRANSCRIPT_DIR.mkdir(parents=True, exist_ok=True)
    for video in series.get("videos", []):
        cues = parse_vtt(SERIES_DIR / video["captions"])
        lines = [f"# Transcript: {video['title']}", "", video["description"], ""]
        lines.append("Generated from the caption file. Edit the caption file, then rerun this command.")
        lines.append("")
        chapter_queue = list(video.get("chapters", []))
        for start, _end, text in cues:
            while chapter_queue and chapter_queue[0]["start"] <= start:
                chapter = chapter_queue.pop(0)
                lines.extend(["", f"## {chapter['title']}", ""])
            stamp = f"{int(start // 60):02d}:{int(start % 60):02d}"
            lines.append(f"- **{stamp}** {text}")
        lines.append("")
        out = TRANSCRIPT_DIR / f"{video['id']}.md"
        out.write_text("\n".join(lines), encoding="utf-8")
        print(f"wrote {out.relative_to(ROOT)}")
    return 0


def main(argv):
    command = argv[1] if len(argv) > 1 else "check"
    if command == "check":
        return check()
    if command == "transcripts":
        return transcripts()
    print(f"unknown command: {command}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
