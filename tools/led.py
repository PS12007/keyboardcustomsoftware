"""Surmen V400 lighting control (rebuilt from lighting1.pcapng).

Usage:
    python led.py selftest          # check packet builder against the capture (no keyboard needed)
    python led.py color FF00FF      # set every key to one color (hex RGB)
"""
import sys

VID, PID = 0x320F, 0x505B
USAGE_PAGE = 0xFF1C  # vendor-defined channel (MI_01 Col04)

# One per-key color frame captured from the app (color 3153FF), used to learn
# which slots in the 384-byte buffer are real LEDs (the app leaves gaps as 000000).
_CAPTURED_KEYS = bytes.fromhex(
    "3153ff" * 18 + "3153"  # chunk 0 (56 bytes)
)
_CAPTURED_CHUNKS = [
    "04ff1b0b380002003153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153",
    "04e31c0b38380200ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff31",
    "04371a0b3870020053ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff0000003153ff3153ff3153ff3153ff3153ff3153ff3153ff000000",
    "04a1190b38a802003153ff3153ff3153ff3153ff3153ff3153ff3153ff0000003153ff3153ff3153ff3153ff3153ff3153ff3153ff0000003153ff3153ff3153",
    "04851a0b38e00200ff3153ff3153ff3153ff3153ff0000003153ff3153ff3153ff3153ff3153ff3153ff3153ff0000003153ff3153ff3153ff3153ff3153ff31",
    "04da160b3818030053ff3153ff0000003153ff3153ff3153ff3153ff3153ff3153ff3153ff0000000000003153ff3153ff3153ff3153ff3153ff3153ff000000",
    "04931b0b305003003153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff3153ff53ff3153ff000000",
]
# Mode packet captured for "custom / per-key" mode (mode byte 0x14), color FF0000.
_CAPTURED_MODE = "0446010622000000001404010000ff00000000000000000000000000000000000001000000000000010400000000000000000000000000000000000000000000"

KEY_BASE = 0x0200   # per-key buffer address
KEY_LEN = 384       # 128 LED slots x 3 bytes (R, G, B)
CHUNK = 56          # bytes of data per 64-byte packet


def packet(cmd, data=b"", offset=0, length=None):
    """Build one 64-byte output report (report ID 4).

    Layout: 04 | checksum(2, little-endian) | cmd | len | offset(3, LE) | data...
    checksum = sum of bytes 3..63.
    """
    length = len(data) if length is None else length
    body = bytes([cmd, length]) + offset.to_bytes(3, "little") + data
    body = body.ljust(61, b"\x00")
    s = sum(body) & 0xFFFF
    return bytes([0x04]) + s.to_bytes(2, "little") + body


def _key_mask():
    buf = b"".join(bytes.fromhex(h)[8:8 + bytes.fromhex(h)[4]] for h in _CAPTURED_CHUNKS)
    assert len(buf) == KEY_LEN
    return [buf[i:i + 3] != b"\x00\x00\x00" for i in range(0, KEY_LEN, 3)]


MASK = _key_mask()


def key_packets(colors):
    """colors: list of 128 (r,g,b) tuples. Gap slots are forced to black."""
    buf = bytearray()
    for used, (r, g, b) in zip(MASK, colors):
        buf += bytes([r, g, b]) if used else b"\x00\x00\x00"
    out = []
    for off in range(0, KEY_LEN, CHUNK):
        part = bytes(buf[off:off + CHUNK])
        out.append(packet(0x0B, part, KEY_BASE + off))
    return out


def mode_packet(mode=0x14, brightness=0x04, speed=0x01, rgb=(0xFF, 0, 0)):
    tmpl = bytearray(bytes.fromhex(_CAPTURED_MODE)[8:8 + 0x22])
    tmpl[1], tmpl[2], tmpl[3] = mode, brightness, speed
    tmpl[6:9] = bytes(rgb)
    return packet(0x06, bytes(tmpl))


MODES = {  # effect IDs, from lighting2.pcapng (01 inferred: currogated wasn't re-sent)
    "currogated": 0x01, "cloud": 0x02, "serpentine": 0x03, "spectrum": 0x04,
    "breath": 0x05, "static": 0x06, "keypress": 0x07, "ripples": 0x08,
    "traverse": 0x09, "stars": 0x0A, "fireworks": 0x0B, "roll": 0x0C,
    "wavebar": 0x0D, "cartoon": 0x0E, "rain": 0x0F, "scan": 0x10,
    "surmount": 0x11, "speed": 0x12, "custom": 0x14, "live": 0xFE,
}


def live_packets(colors):
    """Cmd 0x12: same layout as per-key (0x0B) but offset starts at 0.
    Used by the app's audio-wave mode to stream frames (mode 0xFE)."""
    return [packet(0x12, p[8:8 + p[4]], int.from_bytes(p[5:8], "little") - KEY_BASE)
            for p in key_packets(colors)]


START = packet(0x01)
END = packet(0x02)


def selftest():
    cols = [(0x31, 0x53, 0xFF)] * 128
    for mine, cap in zip(key_packets(cols), _CAPTURED_CHUNKS):
        cap = bytes.fromhex(cap)
        n = 8 + cap[4]
        # checksum rule holds for the captured packet (it sums the app's leftover padding too)
        assert int.from_bytes(cap[1:3], "little") == sum(cap[3:]) & 0xFFFF
        # same header + data as the app (checksum can differ only because our padding is zeros)
        assert mine[3:n] == cap[3:n], (mine.hex(), cap.hex())
    assert mode_packet()[:42] == bytes.fromhex(_CAPTURED_MODE)[:42]
    assert START.hex().startswith("04010001") and END.hex().startswith("04020002")
    print(f"selftest OK ({sum(MASK)} real LED slots)")


def open_kb():
    import hid
    for d in hid.enumerate(VID, PID):
        if d["usage_page"] == USAGE_PAGE:
            h = hid.device()
            h.open_path(d["path"])
            return h
    sys.exit("Keyboard lighting channel not found. Is it plugged in?")


def xfer(h, p):
    """Send one packet and wait for the keyboard's echo (it replies to every packet
    with a 64-byte copy). Not reading the echoes makes the keyboard drop packets.
    Returns True if the echo matched."""
    h.write(p)
    r = h.read(64, 100)
    return bytes(r[:8]) == p[:8]


def send(h, pkts):
    for p in pkts:
        xfer(h, p)


def set_color(rgb):
    h = open_kb()
    try:
        send(h, key_packets([rgb] * 128) + [START, mode_packet(rgb=rgb), END])
    finally:
        h.close()


def set_mode(name, brightness=4, speed=2, rgb=(0xFF, 0, 0)):
    """brightness 0-4, speed 0-4."""
    h = open_kb()
    try:
        send(h, [START, mode_packet(MODES[name], brightness, speed, rgb), END])
    finally:
        h.close()


def pos(slot):
    """Physical position of a slot: slots run in columns of 8 (F-row at the top, Ctrl row at the bottom).
    Returns (x 0-15 left->right, y 0-5 top->bottom)."""
    return slot // 8, slot % 8


def animate(frame_fn, seconds=30, fps=60):
    """Stream frames live. frame_fn(t) returns 128 (r,g,b) tuples.
    Uses fixed deadlines so frames are evenly spaced (smoother than sleeping a fixed amount)."""
    import time
    h = open_kb()
    try:
        send(h, [START, mode_packet(MODES["live"]), END])
        t0 = time.perf_counter()
        frame = bad = 0
        while True:
            t = time.perf_counter() - t0
            if t > seconds:
                break
            for p in live_packets(frame_fn(t)):
                bad += not xfer(h, p)
            frame += 1
            wait = t0 + frame / fps - time.perf_counter()
            if wait > 0:
                time.sleep(wait)
        print(f"ran at {frame / (time.perf_counter() - t0):.0f} fps, {bad} of {frame * 7} packets not confirmed")
    finally:
        h.close()


def wave(seconds=30, speed=0.25):
    """Rainbow that sweeps left to right across the real keyboard layout."""
    import colorsys
    from layout import KEYS, WIDTH

    def frame(t):
        cols = [(0, 0, 0)] * 128
        for slot, (_, x, y) in KEYS.items():
            r, g, b = colorsys.hsv_to_rgb((x / WIDTH - t * speed) % 1, 1, 1)
            cols[slot] = (round(r * 255), round(g * 255), round(b * 255))
        return cols

    animate(frame, seconds)


def rainbow(seconds=10, fps=30):
    """Demo: a rainbow scrolling across the LED slots, streamed live."""
    import colorsys
    import time
    h = open_kb()
    try:
        send(h, [START, mode_packet(MODES["live"]), END])
        t0 = time.time()
        while time.time() - t0 < seconds:
            t = time.time() - t0
            cols = []
            for i in range(128):
                r, g, b = colorsys.hsv_to_rgb((i / 40 + t / 3) % 1, 1, 1)
                cols.append((int(r * 255), int(g * 255), int(b * 255)))
            for p in live_packets(cols):
                h.write(p)
            time.sleep(1 / fps)
    finally:
        h.close()


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "selftest":
        selftest()
    elif len(sys.argv) >= 3 and sys.argv[1] == "mode":
        args = [int(x) for x in sys.argv[3:5]]
        set_mode(sys.argv[2], *args)
        print("sent")
    elif len(sys.argv) >= 2 and sys.argv[1] == "wave":
        wave(float(sys.argv[2]) if len(sys.argv) > 2 else 30)
    elif len(sys.argv) >= 2 and sys.argv[1] == "rainbow":
        rainbow(float(sys.argv[2]) if len(sys.argv) > 2 else 10)
    elif len(sys.argv) == 3 and sys.argv[1] == "color":
        v = bytes.fromhex(sys.argv[2])
        set_color((v[0], v[1], v[2]))
        print("sent")
    else:
        print(__doc__)
