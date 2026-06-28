# GUI: Memory tab and music player

## Memory tab overhaul

### Timeline drill-down

The Memory tab opens on a Timeline view. The timeline presents a year-level
summary of stored memories. Clicking a year expands to months; clicking a month
expands to days within that month; clicking a day shows the individual memories
for that date. Each level renders as a floating descriptor card with a subtle
glass backdrop so the ambient scene behind it remains visible. Navigating back
collapses the drill-down one level at a time via the breadcrumb at the top of
the card.

### Inbox (pending memories)

The Inbox tab under Memory lists memories that are awaiting review before they
are committed to long-term storage. Each pending item shows the proposed
content, source, and timestamp. Three inline actions are available:

- **Approve** -- confirms the memory and moves it into the main timeline.
- **Reject** -- discards the pending entry.
- **Edit** -- opens an inline editor so the operator can correct or annotate the
  content before approving it. Edited content is saved and then approved in a
  single round-trip.

The inbox count is shown in the tab label so the operator can see at a glance
whether review is needed.

### Projects field and "Show empty" toggle

The Projects card lists every project that memories have been tagged to. By
default projects with no memories are hidden. A "Show empty" toggle at the top
of the card reveals them. This keeps the default view focused on active projects
while still letting the operator audit the full project list when needed.

---

## Music player setup

The ambient music player is **hidden by default**. It appears automatically
once the server can locate a directory of `.mp3` files.

### Enabling the player

Set the environment variable `KLEOS_GUI_MUSIC_DIR` (or the legacy alias
`ENGRAM_GUI_MUSIC_DIR`) to the absolute path of a directory that contains one
or more `.mp3` files before starting the server:

```
KLEOS_GUI_MUSIC_DIR=/home/operator/music kleos-server
```

The server reads the directory at startup, builds a manifest of the files it
finds, and serves the audio files same-origin at `/media/music/<filename>`. The
GUI fetches the manifest and the player control bar appears automatically if at
least one track is present.

### Pointing at an existing library

You do not need to copy or move files. The variable can point at any directory
the server process can read, including a symlink to an existing music library:

```
ln -s /path/to/existing/library /home/operator/kleos-music
KLEOS_GUI_MUSIC_DIR=/home/operator/kleos-music kleos-server
```

If the variable is unset or the directory is empty the player does not appear
and no `/media/music/` routes are registered.

### Access note

The `/media/music/` routes are served same-origin without a per-request session
check, the same way the static GUI assets are served. The content is
operator-provided audio, so reachability is the trust boundary: bind the server
to a trusted interface (or keep it behind your existing reverse proxy / VPN) if
you do not want the audio enumerable by anyone who can reach the port. Only
files ending in `.mp3` inside the configured directory are served, and path
traversal outside that directory is rejected.

### Optional title sidecar

By default the player shows the bare filename as the track title. To supply
exact display names, place a `names.json` file inside the music directory. The
file is a plain JSON object mapping filename to title:

```json
{
  "late-afternoon.mp3": "Late Afternoon",
  "deep-focus-01.mp3": "Deep Focus 01"
}
```

Filenames not listed in `names.json` fall back to the filename without the
extension.

### Why no audio ships in the repo

No `.mp3` files are included in the Kleos repository for two reasons:

1. **Repository size.** Even a small set of ambient tracks would add tens of
   megabytes to every clone.
2. **Licensing.** Royalty-free tracks still carry per-track licenses with
   attribution or distribution restrictions that vary by source. Shipping
   operator-provided audio sidesteps those obligations entirely.

The operator supplies tracks from their own library (or any royalty-free source
that permits self-hosted use), and nothing needs to be copied into the repo.
