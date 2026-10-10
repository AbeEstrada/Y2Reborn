# Reborn UI v3 shapes and icons

`reborn-masks.a8` is one coverage-mask page (see `assets/fonts/README.md` for
the format) drawn by `tools/assets/build_ui_assets.py`: pixel-exact rounded
corners and quarter rings for every radius the UI uses, inverse corners for
circular crops, a disc, rings, a radial glow, alpha ramps, 48 pre-rendered
ring-progress frames and the icon set. `crates/reborn-ui/src/maskdata.rs`
lists every rectangle. Nothing here is third-party artwork; the icons are
simple vector drawings made for Reborn.
