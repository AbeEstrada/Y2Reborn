# Reborn documentation

Begin with [current Reborn state](CURRENT_REBORN_STATE.md) and the paired
[Y2Linux state](../../Y2Linux/docs/CURRENT_PLATFORM_STATE.md). The
[Platform 1.0 performance pass](validation/REBORN-PLATFORM-V1-PERFORMANCE.md)
records scoped application measurements on installed Baseline02, with temporary
executables and Product v2 preserved. The older
[Fix01 physical qualification](../../Y2Linux/docs/validation/Y2-CPU-FINAL-FIX01-PHYSICAL-QUALIFICATION.md)
retains that image's overall CPU-platform failure and narrow workload successes.

| Need | Start here |
| --- | --- |
| Exact current application and physical scope | [Current Reborn state](CURRENT_REBORN_STATE.md) |
| Responsiveness, rendering, startup and idle measurements | [Performance pass](validation/REBORN-PLATFORM-V1-PERFORMANCE.md) |
| All pages by scope | [Complete catalog](DOCUMENTATION_CATALOG.md) |
| Media / live volume | [FFmpeg stack](architecture/reborn-audio-stack-ffmpeg9.md), [volume](architecture/live-volume.md) |
| Platform 1.0 metadata and ingest evidence | [FFmpeg/metadata/library pass](validation/REBORN-PLATFORM-V1-FFMPEG-METADATA.md) |
| UI source and software candidate | [Product UI v2](ui/REBORN-PRODUCT-UI-V2.md), [navigation v2](ui/REBORN-PRODUCT-NAVIGATION-V2.md), [pass v2](review/REBORN-PRODUCT-PASS-V2.md); historical [UI v1](ui/REBORN-UI-V1.md), [candidate report](ui/REBORN-UI-V1-REPORT.md) |
| Boot progress and display hand-off | [Boot hand-off](architecture/boot-handoff.md) |
| SQLite / large-library performance | [Platform 1.0 database pass](validation/REBORN-PLATFORM-V1-LIBRARY-DB.md) (host and bounded native Y2 through 50k; ARM build) |
| Platform data/ownership | [Linux API](../../Y2Linux/docs/architecture/platform-api-v1.md) |
| Earlier correctness closure | [Luna closure](validation/LUNA-CORRECTNESS-CLOSURE-01.md) |
| Earlier system/codec audits | [Audit entry](audit/current-system/00_SCOPE_AND_EVIDENCE.md), [codec design](architecture/bluetooth-codecs.md) |

Historical audit, UI, validation and asset documents retain their recorded
date/candidate. A source audit or passed host test does not certify physical
Y2 behavior; each physical report identifies its tested image and artifact.
Follow the shared [knowledge rules](../../Y2Linux/docs/KNOWLEDGE_BASE.md).
Raw private Y2 captures remain in ignored `out/`; sanitized performance summaries
are retained beside their report.
