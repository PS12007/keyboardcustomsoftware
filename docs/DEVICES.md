# USB Device IDs

| Device | VID:PID | Notes |
|---|---|---|
| **Surmen V400 keyboard** (wired USB) | `320F:505B` | Target. Lighting messages probably go to a "vendor-defined" entry on interface `MI_01` |
| 2.4 GHz mouse (receiver) | `3554:F55D` | Not a target |
| Laptop built-in keyboard | `048D:C195` | ITE controller. Not a target |

Keyboard ID assumed from wired vs. wireless. Confirm by unplugging the keyboard and checking that `320F:505B` disappears.
