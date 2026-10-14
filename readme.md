# Y2Reborn

The native music player for the **Innioasis Y2** running
[Y2Linux](https://github.com/SchulzCode/Y2Linux). It is written in Rust, decodes
with FFmpeg 9.0.2, draws with OpenGL ES on the Y2's GPU (DRM + Lima), plays
through ALSA (wired) or BlueALSA (Bluetooth), and keeps its library in SQLite.

> **Status: 0.3.0, part of Y2Linux Baseline 15 (2026-10-12), developer preview.**
> Git tag `baseline-15` in both repositories. Reborn has no installer of its own.
> It is built into the Y2Linux image. To run it on a Y2, follow Y2Linux's
> [BUILDING](https://github.com/SchulzCode/Y2Linux/blob/main/docs/BUILDING.md) and
> [INSTALL](https://github.com/SchulzCode/Y2Linux/blob/main/docs/INSTALL.md) guides.

## What it does

- Library of artists, albums and songs from the internal music storage and the SD card,
  with cover art (thumbnails cached in `/data/reborn/cache`).
- Wired playback at 44.1/48 kHz S16 stereo; higher-rate files are decoded and converted.
  Bluetooth playback with SBC.
- Click-wheel UI (Product UI v3, Open Sans): Now Playing with option icons, volume line,
  settings, hold-to-turn-off, startup and shutdown screens, plus screens for USB computer
  connection and copying, Bluetooth pairing, update download and low battery.
- Skins: Settings → Display → Theme switches between Reborn's own look and skins that
  restyle every screen (colours, fonts, pictures, layouts). Fifteen skins ship with it (Classic, Nano Glass, Walkman, Terminal, Winamp, Paper, Big Type,
  Cassette, Neon Night, Zune and five Mini colours);
  more are added by copying a folder into `Themes` on the player. See [skins](docs/skins.md).
- Works with the platform: sleep/wake preparation, workload hints to the CPU governor,
  `rebornctl` and a control socket at `/run/reborn/control.sock`.

Known limits (details in [current state](docs/CURRENT_REBORN_STATE.md)): native
high-rate output is gated, and only SBC is enabled for Bluetooth. Crossfade and EQ
have not been checked on hardware.

## Development

Use Rust **1.99.0**. `rust-toolchain.toml` pins it together with the
`armv7-unknown-linux-gnueabihf` target. All crates are vendored in `vendor/`,
so builds work offline. Host builds link the system's development libraries
through `pkg-config`: FFmpeg (`libavformat`, `libavcodec`, `libavutil`,
`libavfilter`, `libswresample`, `libswscale`), ALSA, D-Bus, SQLite, libdrm, GBM,
EGL and GLES v2.

```sh
cargo fmt --all -- --check
cargo test --workspace --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
```

UI previews render the production drawing code on the host
(`tools/preview/render_previews.py`, output in `docs/ui/previews/v3/`).

The ARM binary is built only through Y2Linux's Buildroot, which provides the
cross toolchain, sysroot, FFmpeg, SQLite, ALSA, D-Bus and Mesa. Check out this
repository **next to** Y2Linux (`../Y2Reborn`). `tools/build/cross.sh` is
what the Buildroot recipe calls. Host tests don't prove anything about the real Y2:
display, audio and power behaviour need the device.

| Path | Contents |
| --- | --- |
| `app/reborn` | The player binary, `rebornctl` and `reborn-bench` |
| `crates/` | `reborn-core` (state), `reborn-ui`, `reborn-graphics`, `reborn-audio`, `reborn-media` (FFmpeg membrane), `reborn-library` (SQLite), `reborn-platform` (Y2Linux API), `reborn-control`, `reborn-observability` |
| `assets/` | Fonts, UI art and media test fixtures |
| `tools/` | Cross build, previews, asset generation, qualification helpers |
| `vendor/` | Vendored, checksummed crate sources |
| `docs/` | Documentation; start at [docs/README.md](docs/README.md) |

On the device the player uses `/data/reborn` for its state, plays music from
`/media/internal/music` (or `/data/music` before the internal music partition is
set up) and `/media/sd`.

## Licence

MIT (see [LICENSE](LICENSE)). The Open Sans glyph data is under the SIL Open Font
Licence (`assets/fonts/OpenSans-OFL.txt`). No firmware, keys, Bluetooth bonds or
private device data belong in this repository.
