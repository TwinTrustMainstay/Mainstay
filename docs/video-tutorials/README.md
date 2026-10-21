# Video Tutorial Series

> **Last updated:** 2026-10-16 · **Doc version:** 0.1.0

This series teaches the main Mainstay workflows on video. It implements the planning, scripting and playback parts of issue [#1722](https://github.com/TwinTrustMainstay/Mainstay/issues/1722).

## Status

**The videos are not recorded yet.** Each tutorial has a finished recording script, draft closed captions with chapter markers, and a transcript. The `.mp4` files go in `videos/` once someone records them, and then the status in `series.json` moves to `recorded` and then `published`.

| # | Tutorial | Status | Captions | Transcript |
|---|----------|--------|----------|------------|
| 1 | [Register an asset and record maintenance](transcripts/01-first-asset-and-maintenance.md) | Script ready | [VTT](captions/01-first-asset-and-maintenance.en.vtt) | [Transcript](transcripts/01-first-asset-and-maintenance.md) |
| 2 | [Deploy to testnet](transcripts/02-deploy-to-testnet.md) | Script ready | [VTT](captions/02-deploy-to-testnet.en.vtt) | [Transcript](transcripts/02-deploy-to-testnet.md) |
| 3 | [Debugging and troubleshooting](transcripts/03-debugging-and-troubleshooting.md) | Script ready | [VTT](captions/03-debugging-and-troubleshooting.en.vtt) | [Transcript](transcripts/03-debugging-and-troubleshooting.md) |
| 4 | [Testing a contract upgrade](transcripts/04-upgrade-testing.md) | Script ready | [VTT](captions/04-upgrade-testing.en.vtt) | [Transcript](transcripts/04-upgrade-testing.md) |

## Files

| Path | Purpose |
|------|---------|
| `series.json` | Manifest: titles, status, video path, chapters, and caption file for each tutorial. |
| `captions/*.en.vtt` | Closed captions (WebVTT). Draft cue timings are placeholders until the recording is made. |
| `transcripts/*.md` | Transcripts generated from the captions. Do not edit these by hand. |
| `player.html` | Browser player with chapter navigation, captions and a transcript panel. |
| `videos/` | Recorded `.mp4` files. Not committed until recorded. |

## Watching

Serve the repository root over HTTP so the player can load the manifest. Browsers block this from `file://`.

```bash
python3 -m http.server 8000
# open http://localhost:8000/docs/video-tutorials/player.html
```

## Recording and Publishing a Tutorial

1. Follow the recording script in `captions/<id>.en.vtt` and the matching transcript.
2. Record at 1080p or higher. Export as `.mp4` (H.264, AAC) to `docs/video-tutorials/videos/<id>.mp4`.
3. Re-time each caption cue to the final recording, then set `duration_seconds` and move each chapter `start` to the matching point.
4. Run `python3 scripts/video_tutorials.py transcripts` to regenerate the transcript.
5. Set `status` to `recorded` in `series.json`. Set it to `published` once the video is live.

## Validation

`scripts/video_tutorials.py check` runs in the CI docs job. It fails when:

- a video id is duplicated, or a status is not one of `script-ready`, `recorded`, `published`;
- chapters do not start at 0, or their start times are not strictly increasing;
- a caption file is missing, is not WebVTT, or has overlapping cues;
- a video marked `published` has no `.mp4` file.

## Accessibility

Every video must have captions. The transcript is generated from the same caption file, so a viewer who cannot use the video still gets the full content.
