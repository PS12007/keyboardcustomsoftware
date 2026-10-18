"""Physical key positions for the Surmen V400 (75% layout with knob).

Units are key widths: x = center of the key from the left edge, y = row (F-row = 0).
Names match what was typed into keymap.json.
"""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))

ROWS = {
    0.0: [("escape", .5), ("f1", 1.75), ("f2", 2.75), ("f3", 3.75), ("f4", 4.75),
          ("f5", 6), ("f6", 7), ("f7", 8), ("f8", 9),
          ("f9", 10.25), ("f10", 11.25), ("f11", 12.25), ("f12", 13.25), ("delete", 14.5)],
    1.25: [("` ~ recording key left of number 1", .5)]
          + [(str(n), n + .5) for n in range(1, 10)]
          + [("0", 10.5), ("- _", 11.5), ("= +", 12.5), ("back space", 14), ("home / ins", 15.75)],
    2.25: [("tab", .75)]
          + [(c, 2 + i) for i, c in enumerate("qwertyuiop")]
          + [("[ {", 12), ("] }", 13), ("\\ |", 14.25), ("pageup /printscreen", 15.75)],
    3.25: [("caps lock", .875)]
          + [(c, 2.25 + i) for i, c in enumerate("asdfghjkl")]
          + [(";", 11.25), ("' \"", 12.25), ("enter", 13.875), ("page down / pause", 15.75)],
    4.25: [("left shift", 1.125)]
          + [(c, 2.75 + i) for i, c in enumerate("zxcvbnm")]
          + [(", <", 9.75), (".", 10.75), ("/ ?", 11.75), ("right shift", 13.125),
             ("up arrow", 14.75), ("end", 15.75)],
    5.25: [("left control", .625), ("windows key", 1.875), ("left alt", 3.125), ("space", 6.875),
           ("right alt", 10.5), ("fn", 11.5), ("right control", 12.5),
           ("left arrow", 13.75), ("down arrow", 14.75), ("right arrow", 15.75)],
}
WIDTH, HEIGHT = 16.25, 5.25

_NAME_POS = {name: (x, y) for y, keys in ROWS.items() for name, x in keys}


def load():
    """Returns {slot: (name, x, y)} for every real key."""
    with open(os.path.join(HERE, "keymap.json")) as f:
        keymap = json.load(f)
    out, missing = {}, []
    for slot, name in keymap.items():
        if name == "none":
            continue
        if name not in _NAME_POS:
            missing.append(name)
            continue
        out[int(slot)] = (name, *_NAME_POS[name])
    if missing:
        raise ValueError(f"no position for: {missing}")
    return out


KEYS = load()

if __name__ == "__main__":
    print(f"{len(KEYS)} keys placed")
    unused = set(_NAME_POS) - {n for n, _, _ in KEYS.values()}
    print("layout keys not in keymap:", unused or "none")
