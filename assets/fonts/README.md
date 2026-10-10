# Reborn UI v3 type and shapes

Open Sans (SIL Open Font License 1.1, `OpenSans-OFL.txt`) is rendered once, at
build time, into coverage-mask pages — one byte per texel, sampled by the
renderer as luminance-alpha and tinted by the draw colour:

| Page | File | Contents |
| --- | --- | --- |
| 0 | `reborn-regular.a8` | Regular 11/13/15/17 px, Light 23 px, Light 44 px wordmark |
| 1 | `reborn-semibold.a8` | SemiBold 10/13/15/17/20/25/28/36 px |
| 2 | `reborn-fallback.a8` | Regular 24 px, scaled, for every character an exact set lacks |

Every `text::Face` is one weight at one pixel size, so glyphs are drawn 1:1 on
the pixel grid rather than scaled from one bitmap. The exact sets cover basic
and extended Latin (U+0020–017F), common punctuation and the euro sign; the
fallback page adds the rest of Latin Extended-B, Greek, Cyrillic, arrows and
U+FFFD, which stands in for anything else (CJK, emoji and shaping are not
supported). `provenance.json` records the font file hashes, set sizes and page
heights. `crates/reborn-ui/src/fontdata.rs` holds the matching metrics.

Regenerate everything (fonts and `assets/icons`) with
`tools/assets/build_ui_assets.py`. Build inputs: Pillow and the Open Sans TTFs;
no font file is installed on the player.
