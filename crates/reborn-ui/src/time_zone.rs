//! Settings > Display > Time Zone: UTC first, then every tz database zone
//! west to east by its offset now, and the clock it gives the status bar.
use crate::{Item, RowKind, Ui};
use reborn_core::{
    clock::{self, offset_label, zone_label, zones, Rule},
    AppModel,
};

/// Before this instant (2025-01-01) the system clock has not been set; an
/// unset clock reads 1970, so nothing is shown rather than a wrong time.
const CLOCK_SET_AFTER: i64 = 1_735_689_600;

fn now(ui: &Ui) -> i64 {
    ui.clock_fixed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    })
}

/// The chosen zone's rule; a name the table no longer has reads as UTC.
fn rule(m: &AppModel) -> Rule {
    clock::zone(&m.settings.time_zone).unwrap_or(Rule::UTC)
}

/// The status-bar clock, e.g. "9:41" or "21:41". Made once a minute: every
/// other call (each main-loop pass, each skin element) reuses it.
pub(crate) fn text(ui: &Ui, m: &AppModel) -> Option<String> {
    let now = now(ui);
    let zone = &m.settings.time_zone;
    let minute = now.div_euclid(60);
    if let Some((z, at, text)) = &*ui.clock_cache.borrow() {
        if z == zone && *at == minute {
            return text.clone();
        }
    }
    let text = (now >= CLOCK_SET_AFTER)
        .then(|| clock::clock_text(&rule(m).local(now), clock::twelve_hour(zone)));
    *ui.clock_cache.borrow_mut() = Some((zone.clone(), minute, text.clone()));
    text
}

/// The zones in picker order: by offset now, then by city.
fn ordered(now: i64) -> Vec<(&'static str, i32)> {
    let mut all: Vec<(&'static str, i32)> = zones()
        .filter(|(name, _)| name.contains('/'))
        .map(|(name, rule)| (name, rule.offset_at(now)))
        .collect();
    all.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| zone_label(a.0).0.cmp(&zone_label(b.0).0))
    });
    all
}

/// The Display row's value: the city, or UTC.
pub(crate) fn summary(m: &AppModel) -> String {
    zone_label(&m.settings.time_zone).0
}

pub(crate) fn rows(ui: &Ui, m: &AppModel) -> Vec<Item> {
    let chosen = m.settings.time_zone.as_str();
    let minute = now(ui).div_euclid(60);
    if let Some((z, at, rows)) = &*ui.zone_rows.borrow() {
        if z == chosen && *at == minute {
            return rows.clone();
        }
    }
    let rows = build_rows(ui, chosen);
    *ui.zone_rows.borrow_mut() = Some((chosen.to_owned(), minute, rows.clone()));
    rows
}

fn build_rows(ui: &Ui, chosen: &str) -> Vec<Item> {
    let mut rows = vec![Item::new("UTC", "tz_set:UTC").with_kind(RowKind::Check(chosen == "UTC"))];
    rows.extend(ordered(now(ui)).into_iter().map(|(name, offset)| {
        let (city, region) = zone_label(name);
        Item::new(city, format!("tz_set:{name}"))
            .with_secondary(format!("{region} · {}", offset_label(offset)))
            .with_kind(RowKind::Check(chosen == name))
    }));
    rows
}

/// The row to focus when the picker opens: the chosen zone.
pub(crate) fn focus(ui: &Ui, m: &AppModel) -> usize {
    let chosen = m.settings.time_zone.as_str();
    ordered(now(ui))
        .iter()
        .position(|(name, _)| *name == chosen)
        .map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_lists_utc_then_zones_west_to_east() {
        let ui = Ui {
            clock_fixed: Some(1_791_775_896),
            ..Default::default()
        };
        let mut m = AppModel::default();
        let rows = rows(&ui, &m);
        assert_eq!(rows[0].key, "tz_set:UTC");
        assert_eq!(rows[0].kind, RowKind::Check(true));
        assert_eq!(focus(&ui, &m), 0);
        assert!(rows.len() > 300);
        assert!(rows[1].secondary.contains("UTC-1"), "{}", rows[1].secondary);
        assert!(rows.last().unwrap().secondary.contains("UTC+1"));
        m.settings.time_zone = "America/New_York".into();
        let at = focus(&ui, &m);
        let rows = super::rows(&ui, &m);
        assert_eq!(rows[at].label, "New York");
        assert_eq!(rows[at].kind, RowKind::Check(true));
        assert_eq!(rows[at].secondary, "America · UTC-4");
        assert_eq!(summary(&m), "New York");
    }

    #[test]
    fn clock_reads_the_zone_and_hides_until_the_time_is_set() {
        // 2026-10-12 03:31 UTC.
        let mut ui = Ui {
            clock_fixed: Some(1_791_775_896),
            ..Default::default()
        };
        let mut m = AppModel::default();
        assert_eq!(text(&ui, &m).as_deref(), Some("03:31"));
        m.settings.time_zone = "America/New_York".into();
        assert_eq!(text(&ui, &m).as_deref(), Some("11:31"));
        m.settings.time_zone = "Europe/Berlin".into();
        assert_eq!(text(&ui, &m).as_deref(), Some("05:31"));
        m.settings.time_zone = "Mars/Olympus".into();
        assert_eq!(text(&ui, &m).as_deref(), Some("03:31"));
        ui.clock_fixed = Some(86_400);
        assert_eq!(text(&ui, &m), None);
    }
}
