//! Choosing and keeping the audio output: automatic output, a lost Bluetooth peer. Codec choices are
//! the radio daemon's (it applies the owner's choice when a headset connects or its stream stops).
use crate::*;

/// Bluetooth headphones win over the headphone jack. The first time a
/// connected peer's playback PCM is ready, audio moves to it (playback
/// continues there). Once per connection: choosing the jack afterwards sticks
/// until the peer reconnects, and a codec change that recreates the PCM is not
/// a new connection. A working Bluetooth output is never displaced. Losing the
/// peer returns to the jack and pauses (`Event::BluetoothDisconnected`).
///
/// Ready means what `Runtime::switch` needs: a stereo PCM with its rate known.
/// A PCM that BlueALSA lists before its codec is configured is not ready yet,
/// and a switch that fails is tried again (`failed`) instead of leaving the
/// music on the jack for the whole connection (Y2Reborn #103: the first
/// connect after Bluetooth on stayed silent until a reconnect).
#[derive(Default)]
pub(crate) struct AutoOutput {
    pub(crate) routed: HashSet<String>,
    /// Failed switches per peer: how many and when the last one was.
    pub(crate) failures: std::collections::HashMap<String, (u32, Instant)>,
}

/// After a failed automatic switch, the next try waits this long...
pub(crate) const AUTO_OUTPUT_RETRY: Duration = Duration::from_secs(2);
/// ...and after this many failures in one connection the jack stays.
pub(crate) const AUTO_OUTPUT_TRIES: u32 = 5;

impl AutoOutput {
    pub(crate) fn next(
        &mut self,
        status: &bluetooth::Status,
        output: &AudioOutput,
    ) -> Option<String> {
        if !status.available {
            return None; // A failed observation is not a disconnection.
        }
        let connected = |address: &String| {
            status.powered
                && status
                    .devices
                    .iter()
                    .any(|d| d.connected && d.address.eq_ignore_ascii_case(address))
        };
        self.routed.retain(connected);
        self.failures.retain(|address, _| connected(address));
        if !status.powered {
            return None;
        }
        let ready = |address: &str| status.playback_rate(address).is_ok();
        let resting = |address: &String| {
            self.failures
                .get(address)
                .is_some_and(|(_, at)| at.elapsed() < AUTO_OUTPUT_RETRY)
        };
        let peer = status.devices.iter().find(|d| {
            d.connected
                && d.audio
                && !self.routed.contains(&d.address)
                && !resting(&d.address)
                && ready(&d.address)
        })?;
        match output {
            AudioOutput::Bluetooth(current) if current.eq_ignore_ascii_case(&peer.address) => {
                // Already there (restored or chosen by hand): this connection is done.
                self.routed.insert(peer.address.clone());
                None
            }
            AudioOutput::Bluetooth(current) if ready(current) => None,
            _ => Some(peer.address.clone()),
        }
    }
    pub(crate) fn routed(&mut self, address: String) {
        self.failures.remove(&address);
        self.routed.insert(address);
    }
    /// A switch to `address` failed: try again after `AUTO_OUTPUT_RETRY`, at most
    /// `AUTO_OUTPUT_TRIES` times per connection.
    pub(crate) fn failed(&mut self, address: String) {
        let tries = self.failures.get(&address).map_or(0, |(n, _)| *n) + 1;
        if tries >= AUTO_OUTPUT_TRIES {
            self.routed(address);
        } else {
            self.failures.insert(address, (tries, Instant::now()));
        }
    }
}

/// Bluetooth headphones that were the output until they dropped (F51).
pub(crate) struct LostPeer {
    pub(crate) address: String,
    /// The loss, or the wake after it.
    pub(crate) since: Instant,
    /// A Play is waiting for them since then.
    pub(crate) play: Option<Instant>,
}

impl LostPeer {
    pub(crate) fn new(address: String, since: Instant) -> Self {
        Self {
            address,
            since,
            play: None,
        }
    }
    /// What a loss of the selected output leaves to wait for: nothing when it was not Bluetooth, and
    /// nothing when the listener switched Bluetooth off (the loss is that choice; Y2Linux #136: Play
    /// said "Waiting for …" for minutes with Bluetooth off).
    pub(crate) fn after_loss(
        output: &AudioOutput,
        switched_off: bool,
        now: Instant,
    ) -> Option<Self> {
        match output {
            AudioOutput::Bluetooth(address) if !switched_off => {
                Some(Self::new(address.clone(), now))
            }
            _ => None,
        }
    }
    pub(crate) fn is(&self, output: &AudioOutput) -> bool {
        matches!(output, AudioOutput::Bluetooth(a) if a.eq_ignore_ascii_case(&self.address))
    }
    /// Play waits: the jack took over from them a short while ago and holds
    /// no headphones (a plug there is a choice of its own).
    pub(crate) fn applies(
        &self,
        output: &AudioOutput,
        headphones: Option<bool>,
        now: Instant,
    ) -> bool {
        matches!(output, AudioOutput::Wired)
            && headphones != Some(true)
            && !self.gave_up(now)
            && (self.play.is_some() || !self.expired(now))
    }
    pub(crate) fn gave_up(&self, now: Instant) -> bool {
        self.play
            .is_some_and(|at| now.saturating_duration_since(at) >= LOST_PEER_PLAY_WAIT)
    }
    pub(crate) fn expired(&self, now: Instant) -> bool {
        self.play.is_none() && now.saturating_duration_since(self.since) >= LOST_PEER_WINDOW
    }
}

/// A start failed because the selected Bluetooth peer is not connected, and
/// the last observation agrees: the disconnect was missed, not just late.
pub(crate) fn selected_peer_gone(
    output: &AudioOutput,
    state: &bluetooth::Status,
    error: &str,
) -> bool {
    let AudioOutput::Bluetooth(address) = output else {
        return false;
    };
    error.contains("A2DP peer required")
        && state.available
        && !state
            .devices
            .iter()
            .any(|d| d.address.eq_ignore_ascii_case(address) && d.connected)
}
