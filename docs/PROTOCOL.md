# Surmen V400 lighting protocol (from lighting1/2.pcapng)

Sent as HID **output report ID 4**, 64 bytes, to the vendor channel (usage page `0xFF1C`, interface MI_01 Col04).

```
04 | CK CK | CMD | LEN | OFF OFF OFF | DATA (up to 56 bytes) ...
     checksum = sum of bytes 3..63, 16-bit little-endian
     offset   = 3 bytes little-endian
```

| CMD | Meaning |
|---|---|
| `01` | start (empty) |
| `02` | end / apply (empty) |
| `06` | set mode, LEN=0x22, offset 0 |
| `0B` | saved per-key colors (for "customise"), address 0x0200..0x037F, 128 slots x RGB, 7 packets (6x56 + 48 bytes) |
| `12` | **live** per-key frame, same layout but offset 0x0000..0x017F. The app streams this about 50 times/s in audio-wave mode (`FE`) |

Mode change: `01`, `06`, `02`.

**The keyboard echoes every packet** back as a 64-byte input report (endpoint 0x82). Read the echo before sending the next packet; if you don't, packets get dropped and keys show stale colors. With echoes, one live frame (7 packets) takes about 13 ms, so 60 fps works.

## Mode packet data (cmd 06)

| Byte | Meaning |
|---|---|
| 1 | effect ID (table below) |
| 2 | brightness 0-4 |
| 3 | speed 0-4 |
| 6..8 | effect color RGB |
| rest | fixed, copied from capture |

| ID | Effect | ID | Effect |
|---|---|---|---|
| 01 | currogated (inferred) | 0B | fireworks |
| 02 | cloud | 0C | roll |
| 03 | serpentine | 0D | wavebar |
| 04 | spectrum | 0E | cartoon |
| 05 | breath | 0F | rain |
| 06 | light on constantly | 10 | scan |
| 07 | keypress reaction | 11 | surmount |
| 08 | ripples | 12 | speed |
| 09 | traverse | 14 | customise (uses `0B` colors) |
| 0A | stars | FE | live/audio wave (PC streams `12`) |

## Key slots

The 128 slots are laid out column by column: slot = column x 8 + row (row 0 = F-row … row 5 = Ctrl row).
The exact map is in `tools/keymap.json` (made with `tools/map_keys.py`), and 82 slots are visible keys.
36 more slots aren't under any key. The vendor app lights exactly these in audio mode, so they are probably edge/under-case LEDs.
The 10 slots the app always sends as 000000 (47, 55, 63, 71, 79, 87, 95, 103, 104, 111) are gaps.

Still unknown: the effect direction option (left/right) in the mode packet.
