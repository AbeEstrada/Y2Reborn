# Current Reborn state on the Y2

## 1.0.0-beta.6 — Y2Linux 1.0 Beta 6 (Baseline 57), 2026-10-15 (latest): Playlists, pause in place, collected seeking

**Playlists** (owner: a playlist feature like Y2Player's, its own section, add/remove on the device, "highly
optimized … fix issues Y2Player had"; decisions: playlists anywhere in the music, edits go into the file itself,
a Favorites playlist with a heart, moving with the wheel).
- The .m3u/.m3u8 file *is* the playlist: no database copy (Y2Player kept card files and device playlists in two
  worlds that never met, and exported "Name (2).m3u8" copies). `reborn_platform::playlists` finds them on
  Internal Storage and the SD card (not the Radio folder, hidden or system folders; at most 1 000), reads UTF-8,
  UTF-8/16 with a byte order mark and Windows-1252, and writes them back in their own encoding (switching to UTF-8
  with a BOM only when a new title needs it), keeping comments, other tools' tags and entries the player cannot
  find (Y2Player dropped those for good). Writes go to a hidden temporary file, are flushed and renamed over the
  playlist. Over 8 MB or with lines over 4 096 characters a playlist is view-only, never rewritten.
- Entries are positions, not song ids: the same song may appear twice and Remove takes out that one entry
  (Y2Player removed every copy). Each edit reads the file fresh and checks the entry is still where it was, so a
  change made on a computer meanwhile is never overwritten blindly (`reborn_core::playlist`).
- Matching (`reborn_ui::playlist_index`): relative paths (what Reborn writes, portable to a computer), paths on
  the player, Windows/macOS/Linux paths and file:// links from computers by their ending (most trailing folders in
  common; a bare ambiguous name matches nothing), case-insensitive — all in memory, no file system call per entry
  (Y2Player made several). 20 000 entries on a 20 000-song library: first open 19 ms on the host, a frame 0.3 µs
  (rows cached per change, lent out without copying).
- Section: Home → Playlists (New Playlist, Favorites with its heart, every playlist with a 2×2 mosaic, song count,
  length, SD Card / View only); a playlist's page (mosaic, Play, Shuffle, songs; missing ones dimmed "Not on the
  player"). Hold Select: on a playlist Play Next, Add to Queue, Rename, Duplicate, Delete (confirmed; Favorites
  can be neither renamed nor deleted); on a song Play Next, Add to Queue, Move (the wheel carries it, Select drops,
  Back cancels), Remove from Playlist, Add to Playlist…, Go to Album/Artist. Add to Playlist… on any song, an
  album's or artist's Play row and Now Playing (picker: New, the playlists used last first); Add/Remove from
  Favorites on songs and Now Playing (a heart there when it is one); Save Queue as Playlist. New playlists are
  named on the keyboard and saved in `Playlists/` on Internal Storage.
- File work runs on a `playlists` thread (`app/reborn/src/playlists.rs`); playlists are read again with each
  library publish (files copied over USB, the heart works from the start).


Owner: "when playing music and pausing and playing again the player will always buffer"; "scrolling right and
left also gives an error". Player log (Beta 5.1, AirPods): Pause closed the PCM at once ("alsa closed" 3 ms after
pause); Play reopened the file, sought and reopened `bluealsa:` (open → prepared ≈450 ms), ≈1.3–1.4 s per resume.
Holding Previous/Next seeks 30 s every 90 ms; each step restarted the pipeline and the next one hit "audio
reconfiguration is still pending" (ten errors in one second).

- **Pause in place** (files; a station still stops, it is live): `Playback::hold` keeps decoder, file and device
  open; the audio thread pauses the device (`AudioSink::pause` → `rb_sink_pause`: `snd_pcm_pause` where the
  device has it, BlueALSA included, else drop and prepare on resume) and writes nothing; the decoder waits on the
  full queue (its heartbeat continues). Play continues at once. Wired and Bluetooth alike. The hold is given
  back after 3 minutes (`HOLD_LIMIT`), before sleep, and by any stop or new start; a seek while paused releases
  it so Play starts at the new place.
- **Collected seeking:** steps only move the position (the sound holds); playback restarts once, 350 ms
  (`SEEK_SETTLE`) after the last step and never during a restart in progress, so no error.
- Test: `a_held_pause_stops_writing_and_resumes_without_reopening`.

## 1.0.0-beta.5.1 — Y2Linux 1.0 Beta 5.1 (Baseline 56), 2026-10-15: live search, a faster search index, Radio as its own section

**Radio, reworked** (`views_radio`, owner: "radio should feel like its own premium section", "sort
everything by alphabet", "top 10 radio stations for each country on top"):
- Home of Radio: an On Air card for the station playing (logo, ON AIR, the announced song), then Favorites,
  Popular, Countries, Genres, Search and My Stations as coloured icon tiles with a line each; without Wi-Fi
  a Connect to Wi-Fi row on top (favorites and own lists stay usable).
- Station logos from the directory (`favicon`): downloaded once per logo by a background thread
  (`radio::fetch_logo`, ≤300 kB, PNG/JPEG/WebP, the largest PNG in an .ico; failures remembered three days),
  decoded whole without stretching (`rb_media_logo_file`) and laid on a tile (`art::logo_tile`: a logo's own
  plain background fills the tile, a picture fills it, a transparent logo sits on a light or dark tile).
  Without a logo, a tinted tile with the station's initials. Live check: 21 of 24 popular stations' logos.
- Lists A to Z (stations, favorites, own lists, countries, genres); a country's, a genre's and the popular
  list keep their ten most listened stations on top, ranked, the rest A to Z. Station rows: logo, name,
  country · genre · format, a star on favorites, LIVE on the station playing. Countries carry their code,
  genres are capitalised.
- Station Info (hold Select on a station or on Now Playing): logo, country, language, format, votes,
  genres, homepage; Play / Now Playing and Favorites.
- Now Playing: the logo as the picture and colour, a LIVE badge with the station's name, the announced
  song as title and artist, the station, format and country; Previous/Next switch to the neighbouring
  station of the list it was started from.
- Directory details kept per station (logo, homepage, language, country code, votes, plays); favorites
  saved earlier gain them when the station shows up in a list again.
- Fixed: grouped numbers (1 000 songs and more on Home) used a thin space missing from the fonts.


**Live search.** The library search shows its results above a compact keyboard while you type: three rows
(artists, then albums, then songs, up to 60), the count in the field, a hint while empty. Back on the top key
row moves into the results, Down past the last one returns to the keys, Select opens an artist or album or
plays a song (the full results page comes first, so Back returns to it); hold Select still opens all results.
Password and station search keep the roomy keyboard.

**Search index** (`reborn_ui::search::Index`): artists, albums and songs are folded and sorted once per
library (album counts precomputed); a keystroke searches only what the shorter text found, Delete reuses the
earlier search (48 kept), matching allocates nothing per name. Synthetic 20 000-song library on the host
(release build): per keystroke mean 6.2 ms → 0.05 ms, worst 7.5 ms → 0.42 ms; the index build (≈20 ms on the
host, a few hundred ms on the player for 20 000 songs) happens when the keyboard opens. The results page no
longer scans the library once per artist. Benchmark: `cargo test -p reborn-ui --release -- --ignored
--nocapture speed`.

## 1.0.0-beta.5 — Y2Linux 1.0 Beta 5 (Baseline 55), 2026-10-15: keyboard and search

**Full-screen keyboard** (`reborn_ui::keyboard`) for the Wi-Fi password, radio station search and the
new library search, replacing the one-row letter wheel. QWERTY in four rows (owner's choice), pages
abc/ABC (Shift once, twice = lock), 123, #+= (all printable ASCII, so any WPA passphrase) and àé
(accented letters, searches only; passwords get a show/hide key there). Buttons only (owner's choice):
`ActionRouter::set_keyboard` turns Back, Play/Pause, Previous and Next into up, down, left and right on
press, held Previous/Next repeat every 180 ms, holding Back cancels, the wheel does nothing; Select types,
holding Select is Done. Rows wrap, so Done is one press above the letters. Playback buttons do not skip
tracks while it is open; volume and Power keep their meaning; headphone buttons still control playback.

**Library search** (Home → Search, also in the Music menu): artists, albums and songs where every typed
word begins a word of the name, folded like A to Z order (`reborn_core::sort::fold`); an album is also
found by its artist, a song by title, artist or album. The keyboard shows a live count ("2 artists · 3
albums · 9 songs"); results open as a list (Edit Search, artists, albums, songs); Select on a song plays
the result songs in order. Folded names are cached per library (`search::Index`).

Since 1.0.0-beta.4 (in Beta 4.1): a stopped or paused station no longer hangs playback (`Stream::drop`
closes its pipe first); Radio follows the Wi-Fi view every loop pass; a late platform network record no
longer shows a connected player as offline.

**UI pass** (all 95 preview states, Reborn's look and skins): one capitalization rule (Title Case for titles,
rows, buttons and menu entries; sentence case for descriptions, hints and status values), so "Play Next",
"Go to Album", "Scan for Music" (one name instead of two), "Reset Settings", "Wi-Fi Is Off" and so on; notices
sit in the status bar instead of over the focused row or the cover; the turn-off hold has a nearly opaque
backdrop; the password keyboard's title is "Wi-Fi Password" (the prompt names the network).

Host checks: 437 workspace tests (keyboard layout, movement, pages, limits, Done/Cancel; router keyboard
mode with held buttons; search matching; the Wi-Fi, radio and library flows), fmt, strict Clippy; preview
renders of the keyboard pages and search results in Reborn's look and in skins. Not yet on the device.

## 1.0.0-beta.4 — Y2Linux 1.0 Beta 4 (Baseline 53), 2026-10-15 (latest): internet radio

**Internet radio.** Home → Radio: Favorites (saved in `/data/reborn/state/radio.json`), Popular,
Countries, Genres and Search (wheel keyboard) from radio-browser.info (mirrors found through DNS as the
service asks; plays are counted with its `/json/url/` call), and My Stations from `.m3u`/`.pls` files in a
`Radio` folder on Internal Storage or the SD card. Hold Select on a station, or on Now Playing, to add or
remove a favorite. Now Playing shows LIVE RADIO, the station's announced song (ICY `StreamTitle`) or the
connection state, listening time and LIVE instead of a countdown.

How it works: `reborn_platform::radio` runs `curl` (TLS with certificate checks, redirects, `.pls`/`.m3u`
indirection, 15-s stall limit) and strips the ICY metadata blocks; the audio bytes go through a pipe to
`Decoder::open_stream`, a non-seekable input of the existing FFmpeg decoder (`rbpipe:<fd>`, formats mp3,
aac, ogg, flac). FFmpeg keeps `--disable-network` and its `file`-only protocol policy. A dropped
connection is retried after 1, 2, 4, 8, 15 and 30 s; the decoder waits up to 45 s. A station is never
pre-opened for gapless playback, never seeked, and an SD card change never pauses it. HLS/DASH-only
stations are left out of the directory lists and named when a playlist leads to one.

Host checks: 426 workspace tests (radio: ICY splitting at every byte boundary, header/redirect parsing,
playlists, Radio folders, directory filtering, favorites, a local Shoutcast server that drops and is
reconnected, stream decoding of MP3/AAC/Ogg from a slow pipe, the Radio screens), fmt, strict Clippy.
Live (ignored by default): the real directory and two real stations (France Info MP3, Europe 1 AAC) decode
through the playback path. Not yet on the device.

## 1.0.0-beta.3 — Y2Linux 1.0 Beta 3 (Baseline 52), 2026-10-15

**Bluetooth quality and stability.** The platform now runs AAC with the FDK afterburner at the
highest rate the headphones accept where a frame still fits one packet (AirPods Pro advertise
256 kbit/s VBR-capable; Beta 2 sent BlueALSA's default 220 kbit/s) and steps the rate down
(256 → 192 → 160 → 128 kbit/s) while the link backs up, back up after ~10 s of an empty queue —
the AAC counterpart of LDAC's adaptive rate. The service menu (System) shows **AAC Quality**
(High / Standard) and **AAC Auto Rate** (On / Off) next to the LDAC rows; **SBC Quality** (XQ, XQ+)
no longer needs the experimental switch. Defaults need no action from the owner.

## 1.0.0-beta.2 — Y2Linux 1.0 Beta 2 (Baseline 51), 2026-10-15

**Automatic audio output.** Bluetooth headphones win over the headphone jack: the first time a
connected peer's BlueALSA playback PCM is ready, the output moves to it and playing music continues
there (notice *Audio switched to …*). Once per connection, so choosing the jack by hand sticks until
the headphones reconnect; a codec change that recreates the PCM is not a new connection; a working
Bluetooth output is never displaced by a second peer. Losing the peer returns to the jack and pauses,
as before. Reborn starts on the jack and lets reconnecting headphones take over (a saved Bluetooth
output whose peer was absent used to leave playback with nowhere to go). There is no jack detection
on this board yet, so the jack is simply the output whenever no Bluetooth headphones are connected.
**Wi-Fi passwords** containing `\` or `"` are sent as typed (they were stored escaped and could never
connect); network names go to wpa_supplicant as hex. Live on Beta 1 (2026-10-15): AirPods Pro
negotiate AAC 44.1 kHz.

## 1.0.0-beta.1 — Y2Linux 1.0 Beta 1 (Baseline 50), 2026-10-16

Reborn's first public beta, built into the Y2Linux 1.0 Beta 1 images. Since Baseline 48: Wi-Fi and Bluetooth pages
explain the one-time radio firmware copy (*Firmware* drive) and say when it is ready; a cancelled or older failed update
no longer hides a newer release; versions read "1.0.0 Beta 1"; merged contributions by @samanz: covers that load
reliably (#6), A to Z ignoring case, accents and a leading "The" (#7), genre titles on the album grid (#8). Public
codecs SBC, AAC (fdk-aac-free) and LDAC.

## Baseline 48 — 2026-10-16

Reborn 0.3.0 `62af097` is the player in Y2Linux Baseline 48 (POWER-FINAL-48, tag `baseline-48`, the third release on
the over-the-air channel). Since Baseline 34: **Software Update** for full-system updates over Wi-Fi (Y2Linux OTA-35
to 48): check (also once a day, with a *Software update available* notice), download, **Restart and Install** (the
player restarts into its recovery image and back by itself), *Restoring* and named refusals (charger or half a
battery, needs a computer, couldn't be verified; not enough system space names the MB and says music is not
affected). The **download screen** (48) reads the platform's live progress file: *Step 1 of 2 · Download* with a
percentage, a gliding bar, MB of MB, speed and time left, then *Step 2 of 2 · Check*; when done the page turns to
Ready to Install and a notice says so wherever the owner is. Preview states `34`, `34b`, `34c`.

## Baseline 34 — 2026-10-14

Reborn 0.3.0 `5b69ef5` is the player in Y2Linux Baseline 34 (POWER-FINAL-34, tag `baseline-34`). Since Baseline 24
(`ced5199`): Cover Flow at the panel's rate (no 34-ms frame gate), sub-minute album lengths, macOS `._*` and system
folders skipped (25); Bluetooth codecs seen and chosen automatically (26); folder-first artwork with more cover names,
`.m4b/.oga/.aifc`, `ALBUM ARTIST` (31); the limiter only when the DSP can raise a peak (30); **themes** (33, 34): a
skin engine (strict `skin.json`, skin atlas texture, TTF fonts, layouts per screen with Reborn's own drawing in the
skin's palette as fallback), Settings → Display → Theme, `Themes` folders on Internal Storage and the SD card,
`rebornctl theme`, 15 shipped skins ([skins](skins.md)); and the battery level for the platform's `stock_oam` source
(34). Reborn's own look is unchanged without a skin (every host preview identical to `0892bf2` except the new Theme
row). 398 workspace tests, fmt and strict Clippy pass. **On hardware:** the themes ran on 33 (owner: "it works very
good"; Classic 162 ms, the other 13 shipped themes 87–119 ms to load); 34 (battery level, Zune) awaits the flash.
Cover Flow frame rate after 25, AAC with AirPods, crossfade and EQ are not yet checked on the device.

## Baseline 24 — 2026-10-13

Reborn 0.3.0 `ced5199` is the player in Y2Linux Baseline 24: Cover Flow for Albums, Genres, animation frame pacing.

## Baseline 15 — 2026-10-12

Reborn 0.3.0 `f68e088` is the player in Y2Linux Baseline 15 (POWER-FINAL-15, tag `baseline-15`). The only change since
13 is the move to Rust 1.99.0 with its stricter Clippy (`as_chunks`, `try_update`, a pointer cast for the signal
handler, a zipped queue counter); behaviour is unchanged. 362 workspace tests, fmt and strict Clippy pass on 1.99. On
the device it runs against Mesa 26.2.4, FFmpeg 9.0.2, BlueZ 5.87 and bluez-alsa 5.0.0; the owner confirmed display,
audio, Bluetooth and library after the flash.

## Baseline 13 — 2026-10-11

Reborn 0.3.0 `71ddb0d` is the player in Y2Linux Baseline 13 (POWER-FINAL-13, tag `baseline-13`). The Product UI v3
and the remaining screens (computer connection and copying, Bluetooth pairing, update download, scanning,
low battery, SD removed, songs added) have run on the owner's Y2 since POWER-FINAL-10/11. Reborn was unchanged from 11
to 13; those candidates changed the platform (sleep, charging, stock power settings). Host checks at `71ddb0d`:
`cargo fmt`, `cargo test --workspace --locked --offline` and strict Clippy pass. Not checked on hardware: crossfade
end-of-track blend, EQ, native high-rate output, non-SBC Bluetooth codecs. Platform issues that affect the player
(software-restart dark panel, USB after deep sleep) are listed in the
[Y2Linux state](https://github.com/SchulzCode/Y2Linux/blob/main/docs/CURRENT_PLATFORM_STATE.md).

## Product UI v3 — 2026-10-10

Reborn 0.3.0 redesigns the UI (see [Product UI v3](ui/REBORN-PRODUCT-UI-V3.md)): Open Sans exact-size type, cover-driven accent, cover thumbnails with a disk cache, a bare volume line, option icons on Now Playing, a flatter Settings tree, hold-to-turn-off without menus, a hidden service menu and new startup/shutdown screens. Host tests, fmt and strict Clippy pass; it is **not yet observed on hardware**. Dated sections below describe earlier applications.

## Independent Platform 1.0 baseline — 2026-10-04

Compiled Reborn `9cd12baeeb70b204e08299bd4c91c8c35604c304` (runtime bytes unchanged from master dd73dcc)
with Linux `98daf3f0220f4a1580bb4893094ee9f0e02362a5`. Fresh 324 workspace tests, fmt, strict Clippy,
ARM/QEMU/installed helper/dependency validation PASS. Build `Y2LINUX-PLATFORM-V1-BASELINE-01`,
root `2025.02.18-platform-v1.24`, release `1.0.0-platform-v1-baseline-candidate.1`. Final branch
`codex/platform-v1-baseline-final`; canonical owner package `/home/luca/Dokumente/Code/Y2Linux/out/y2linux-platform-v1-baseline-candidate`.
SOFTWARE_VALIDATED / INTEGRATED_PHYSICAL_NOT_RUN. UART01 CPU and temporary
subsystem measurements keep their historical identities; Baseline02 RTC FAIL
and public firmware/codec/source/collector blockers stay open. Main unchanged.
No flash, push or device qualification by the verifier. Later report-only HEAD
is separate from compiled source. Earlier current/next labels are historical.

## Sealed Platform 1.0 software integration — 2026-10-04

**READY_FOR_PLATFORM_V1_PHYSICAL_QUALIFICATION**. Compiled Reborn `dd73dcc3243045cd33e319dda2f76918ed37ea31`/Linux `4c72466f2f27ddf9f0ca18ba8ddc4d89c52599d2`; actual package/application0.2.0.324tests/fmt/strict Clippy, fresh pinned ARM/QEMU/software/package gates PASS. One candidate `/home/luca/Dokumente/Code/worktrees/platform-v1-integration/Y2Linux/out/y2linux-platform-v1-integration-candidate`. Integrated physical/endurance NOT_RUN and public distribution blocked; current installed stock remains Baseline02/b71 after temporary source-bound benchmarks. See [master](https://github.com/SchulzCode/Y2Linux/blob/main/docs/release/Y2-PLATFORM-V1-MASTER.md) and [software receipt](https://github.com/SchulzCode/Y2Linux/blob/main/docs/validation/Y2-PLATFORM-V1-INTEGRATION.md). Dated progress/physical sections below remain history.

## Platform 1.0 integration — 2026-10-04

Selected source from all eleven finished handoffs is composed on `codex/platform-v1-integration`. All 324 workspace tests, formatting and strict Clippy pass. Fresh production ARM and one integrated physical campaign remain pending. Canonical source/card claims, bounded TEMP scanner, source-aware zero-FD cover cache, one observation worker, sleep quiescence and durable USB refresh share one runtime. Native wired hardware remains S16/44.1/48; wider AFE admission and Bluetooth peer/codecs require evidence.

Installed owner receipts identify Baseline02 Linux `8584ccd8` / Reborn `b71b4688`, boot `88b85fa3` after the failed RTC wake; temporary optimized binary tests are separate from installed stock and the new integrated image. The [master](https://github.com/SchulzCode/Y2Linux/blob/main/docs/release/Y2-PLATFORM-V1-MASTER.md), [source ledger](validation/REBORN-PLATFORM-V1-INTEGRATION.md) and [physical plan](https://github.com/SchulzCode/Y2Linux/blob/main/docs/release/Y2-PLATFORM-V1-PHYSICAL-QUALIFICATION.md) are current; the dated observations below retain their original scope.

Updated 2026-10-01. **Software candidate (not flashed, not physically
qualified):** Reborn **Product UI v2** (0.2.0) on the CPU Final Fix03
platform. Smaller product surface, typed platform boundary, one-detent wheel,
Reborn boot mark and dark shutdown. See [Product UI v2](ui/REBORN-PRODUCT-UI-V2.md),
the [pass record](review/REBORN-PRODUCT-PASS-V2.md) and the
[Y2Linux receipt](https://github.com/SchulzCode/Y2Linux/blob/main/docs/validation/Y2-REBORN-PRODUCT-UI-V2.md).
Everything below describes the last physically observed application.
Updated 2026-10-04. The observed installed application is **Product UI v2**
(0.2.0), Reborn `b71b468860233faa0a42b8448ec5777fa952b8e3`, on
Y2LINUX-BASELINE-02: Linux `8584ccd85f052f351fe51650b2348c83ca894ed4`,
kernel `6.18.0-y2linux-baseline-02`, root `2025.02.18-platform-v1.21`.
Final matched tests/restoration observe boot
`88b85fa3-2282-4d93-b0b8-818a105ddd0a` with taint 0 and
automatic qualified CPU0 C3 policy Enabled, budget -1, disable 0.

The [performance pass](validation/REBORN-PLATFORM-V1-PERFORMANCE.md) records
reversible temporary ARM executable tests, product benchmarks and their limits.
Its source candidate is isolated on `codex/platform-v1-reborn-performance`;
the installed executable and original application data are restored after tests.
Product v2 design and state ownership remain the contract. This pass does not
promote full suspend, electrical battery life or public release qualification.
The final tested temporary source is `069415a`; installed Reborn remains
`b71b4688`. Playing CPU falls from 87.95% to 11.21% of one core, while all
1,000 wheel steps match and sampled static screens produce zero idle redraws.
See [Product UI v2](ui/REBORN-PRODUCT-UI-V2.md), the
[product pass](review/REBORN-PRODUCT-PASS-V2.md), and the paired
[current platform state](https://github.com/SchulzCode/Y2Linux/blob/main/docs/CURRENT_PLATFORM_STATE.md).

The dated Fix01 observations below are retained as historical evidence. They
describe that image, rather than the installed Baseline02 or performance candidate.

Physical observation from the 2026-09-27 CPU Final Fix01 run. This page
describes the exact already-flashed application, not a new image or software
build. The [paired Linux state](https://github.com/SchulzCode/Y2Linux/blob/main/docs/CURRENT_PLATFORM_STATE.md)
and [physical report](https://github.com/SchulzCode/Y2Linux/blob/main/docs/validation/Y2-CPU-FINAL-FIX01-PHYSICAL-QUALIFICATION.md)
are the authority for hardware/platform results.

| Identity | Historical Fix01 physical observation |
| --- | --- |
| Reborn compiled source | `36db1869c6bab1ad2d00b7d8e7807ea6ef5b3803` |
| Linux compiled source | `0de6e951b438bf0f2d701e23e476d2b2405ba3c6` |
| Kernel/root | `6.18.0-y2linux-cpu-final-fix01` / `2025.02.18-platform-v1.7` |
| Release | `1.0.0-cpu-final-fix01-candidate.1` |
| Latest observed boot | `81e422f3-43cd-4ad6-877f-692eb1191b36` |

The user interface, controls, artwork, scanner and platform workers generated
**accepted real workload leases** for Idle, PlaybackNormal, PlaybackHeavy,
Interactive, ArtworkDecode, LibraryScan, NetworkTransfer and Maintenance.
Interaction lasted roughly 250 ms, coalesced and expired; screen-off released it
immediately while playback's lease remained. The kernel governor selects MHz.
Isolated client tests also covered renew/expiry/close/overlap/killed-process
cleanup; actual Reborn process-death restart was not physically tested.

Bounded native 44.1-kHz and 24/96 -> 48-kHz silent fixtures played with no ALSA
XRUN, FFmpeg decode/filter errors or corrupt packets. Silent fixtures establish
software operation, not listening quality. Crossfade was enabled through the real
UI to produce PlaybackHeavy, then restored to zero; an actual track-transition
blend and independent EQ path remain untested. The 200-file scan was a small
fixture run, not a 20k-track endurance result. The measured `decoder_stalls`
counter represents a full producer queue in current source, not corruption or
an ALSA underrun. Source output precision and native high-rate hardware are
separate from decoding and conversion.

The Fix01 platform qualification **FAILS overall**: SLIDLE never entered;
1196/1300 MHz stayed safely gated; no same-boot full suspend restore was
demonstrated; a loaded USB transfer lost connectivity while the UI stayed
usable. Four independent small Wi-Fi roundtrips and the one-retry radio restore
passed. No claim about actual Bluetooth peer playback, visual legibility,
analog fidelity, battery life, full wake or public release is made here.

Current software ownership remains [media](architecture/reborn-audio-stack-ffmpeg9.md),
[live volume](architecture/live-volume.md), [platform API](https://github.com/SchulzCode/Y2Linux/blob/main/docs/architecture/platform-api-v1.md),
and [UI v1](ui/REBORN-UI-V1.md). The [Luna correctness closure](validation/LUNA-CORRECTNESS-CLOSURE-01.md),
[UI candidate report](ui/REBORN-UI-V1-REPORT.md) and
[older system audit](audit/current-system/00_SCOPE_AND_EVIDENCE.md) are dated
software/review records. Refer to the [document catalog](DOCUMENTATION_CATALOG.md)
before treating their older "current" or "next" text as today's policy.

At the Fix01 boundary, next CPU platform work was one coherent Fix02 plan: PM diagnostic safety,
PWRAP readiness, MMC runtime clocks/coordinator reachability and loaded USB
recovery. This page does not authorize those code changes, flashing or a new
application release. It preserves source, candidate and physical evidence tiers.
