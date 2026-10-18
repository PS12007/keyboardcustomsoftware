"""Find which LED slot is which key.

Lights one slot green at a time. Type the key's name and press Enter.
  none  = nothing lit up
  back  = redo the previous slot
  quit  = stop (progress is saved; run again to continue)

Answers are saved to keymap.json as you go.
"""
import json
import os
import threading
import time

import led

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "keymap.json")
GREEN = (0, 255, 0)


def show(h, slot):
    cols = [(0, 0, 0)] * 128
    if slot is not None:
        cols[slot] = GREEN
    for _ in range(2):  # send twice in case the first frame is dropped
        for p in led.live_packets(cols):
            h.write(p)
        time.sleep(0.03)


class Keeper(threading.Thread):
    """Keeps re-sending the current slot so the light never fades or gets overwritten."""

    def __init__(self, h):
        super().__init__(daemon=True)
        self.h, self.slot, self.lock, self.running = h, None, threading.Lock(), True

    def run(self):
        while self.running:
            with self.lock:
                show(self.h, self.slot)
            time.sleep(0.2)


def main():
    keymap = {}
    if os.path.exists(OUT):
        with open(OUT) as f:
            keymap = json.load(f)
    slots = [i for i, used in enumerate(led.MASK) if used]
    todo = [s for s in slots if str(s) not in keymap]
    print(__doc__)
    print(f"{len(slots) - len(todo)} of {len(slots)} slots already done.\n")

    h = led.open_kb()
    try:
        led.send(h, [led.START, led.mode_packet(led.MODES["live"]), led.END])
        keeper = Keeper(h)
        keeper.start()
        i = 0
        while i < len(todo):
            slot = todo[i]
            with keeper.lock:
                keeper.slot = slot
                show(h, slot)
            ans = input(f"[{i + 1}/{len(todo)}] slot {slot}: which key is green? ").strip().lower()
            if not ans:
                continue  # empty Enter: re-send and ask again
            if ans == "quit":
                break
            if ans == "back":
                if i > 0:
                    i -= 1
                    keymap.pop(str(todo[i]), None)
                continue
            keymap[str(slot)] = ans
            with open(OUT, "w") as f:
                json.dump(keymap, f, indent=1)
            i += 1
        else:
            print("\nAll done! Saved to keymap.json")
        keeper.running = False
        keeper.join()
        show(h, None)
    finally:
        h.close()


if __name__ == "__main__":
    main()
