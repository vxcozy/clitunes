# Play local files

clitunes can play local audio files and directories alongside internet radio.

## Single file

```
clitunes source local ~/Music/song.flac
```

Supported formats: MP3, FLAC, OGG/Vorbis, WAV, AAC, M4A.

## Directory

```
clitunes source local ~/Music/album/
```

Recursively scans the directory for supported audio files and queues them in
filesystem order.

## From the TUI

Local file playback is a headless verb — run the command above from a second
terminal while the TUI is open. The visualiser will switch to displaying the
local audio.

## Switching back to radio

```
clitunes source radio <station-uuid>
```

Or press **s** in the TUI to reopen the station picker.
