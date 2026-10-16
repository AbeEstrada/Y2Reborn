#![forbid(unsafe_code)]
//! Settings > Display > Time Zone > Automatic (Beta 7.1): the tz database
//! zone of the player's internet address, from one small HTTPS request when
//! Wi-Fi connects. The address is all these services see; nothing else is
//! sent. A second service answers when the first does not.

/// Plain text, e.g. "Europe/Berlin".
const PRIMARY: &str = "https://ipapi.co/timezone/";
/// JSON with `timezone.id`.
const SECOND: &str = "https://ipwho.is/?fields=timezone";

/// The zone the player is in, as a name the clock knows.
pub fn zone() -> Result<String, String> {
    if let Some(zone) = crate::radio::fetch(PRIMARY)
        .ok()
        .and_then(|body| known(String::from_utf8_lossy(&body).trim()))
    {
        return Ok(zone);
    }
    let body = crate::radio::fetch(SECOND)?;
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| "unreadable answer".to_owned())?;
    let named = value["timezone"]["id"]
        .as_str()
        .or_else(|| value["timezone"].as_str())
        .unwrap_or("");
    known(named).ok_or_else(|| "no time zone in the answer".into())
}

/// A zone of the built-in table, never an arbitrary string from the network.
fn known(name: &str) -> Option<String> {
    (name.contains('/') && reborn_core::clock::zone(name).is_some()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_zones_of_the_table_are_taken() {
        assert_eq!(known("Europe/Berlin").as_deref(), Some("Europe/Berlin"));
        assert_eq!(known("UTC"), None);
        assert_eq!(known("Too many requests"), None);
        assert_eq!(known("Europe/Atlantis"), None);
        assert_eq!(known("<html>Europe/Berlin</html>"), None);
    }
}
