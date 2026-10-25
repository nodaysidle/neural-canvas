# Latency: can Hyprland settings cut the measured 20 ms p95? (2026-10-25)

**No.** The measured pointer event → scanout p95 stays at 22.8 to 24.0 ms. The sub-20 ms figure stays a
derived one (about 14 ms with predicted ink, `phase-3.md`) until a 240 fps camera run confirms it.

Method: `nc-bench window input <mode> 20 auto` with `nc-bench drive 16 500` on workspace 9, Hyprland
0.56.2 on the reference machine, 8,000 pointer events per run. Settings were applied at runtime with
`hyprctl eval` and cleared with `hyprctl reload`; no config file was touched.

| Run | Hyprland | Present mode | zero-copy | event → scanout p50 / p95 |
|---|---|---|---|---|
| 1 | defaults (load average 4) | Fifo | 0 | 22.96 / 32.82 ms |
| 2 | `allow_tearing` + `immediate` rule on the window title | Immediate | 0 | 23.45 / 33.11 ms |
| 3 | `render:direct_scanout = 1`, same rule | Immediate | 1920 / 1920 | 19.25 / 23.95 ms |
| 4 | `render:direct_scanout = 1` | Fifo | 1920 / 1920 | 18.25 / 22.82 ms |
| 5 | `direct_scanout = 1`, `immediate` for every fullscreen window | Immediate | 1920 / 1920 | 18.85 / 23.63 ms |

- Tearing never engaged: every frame in every run carries the compositor's `vsync` flag, with the rule
  in place. Immediate therefore equals Fifo.
- Direct scanout (zero-copy) is a compositor setting, only applies to an unobstructed fullscreen window,
  and moves p95 from about 23.2 ms (Phase 2) to 22.8 ms. Runs 1 and 2 were taken while other
  processes loaded the machine, so they are not a fair baseline; runs 3 to 5 are.
- Not tried: implementing `wp_tearing_control_v1` in `nc-wayland` (the rule did not engage tearing, so
  the hint is unlikely to either), and it would tear visible ink anyway.
