use super::*;
fn status(address: &str, path: &str) -> bluetooth::Status {
    let mut status = bluetooth::Status {
        available: true,
        powered: true,
        bluealsa: true,
        ..Default::default()
    };
    add(&mut status, address, path);
    status
}
fn add(status: &mut bluetooth::Status, address: &str, path: &str) {
    status.devices.push(bluetooth::Device {
        address: address.into(),
        path: path.into(),
        name: "AirPods Pro".into(),
        connected: true,
        audio: true,
        paired: true,
        bonded: true,
        ..Default::default()
    });
    status.pcms.push(reborn_core::BluetoothPcm {
        device: path.into(),
        transport: "A2DP-source".into(),
        mode: "sink".into(),
        running: Some(false),
        transport_generation: 1,
        channels: Some(2),
        rate: Some(44_100),
        format: Some(0x8210),
        ..Default::default()
    });
}
#[test]
fn play_waits_briefly_for_bluetooth_headphones_just_lost() {
    let t0 = Instant::now();
    let mut lost = LostPeer::new("AA:BB".into(), t0);
    let wired = AudioOutput::Wired;
    // Jack empty or not read yet: wait. Headphones in the jack: play there.
    assert!(lost.applies(&wired, Some(false), t0));
    assert!(lost.applies(&wired, None, t0));
    assert!(!lost.applies(&wired, Some(true), t0));
    // The headphones are the output again: nothing to wait for.
    assert!(!lost.applies(&AudioOutput::Bluetooth("AA:BB".into()), Some(false), t0));
    assert!(lost.is(&AudioOutput::Bluetooth("aa:bb".into())));
    assert!(!lost.is(&AudioOutput::Bluetooth("CC:DD".into())));
    // Long after the loss (or the wake after it) the jack plays again.
    let late = t0 + LOST_PEER_WINDOW;
    assert!(lost.expired(late));
    assert!(!lost.applies(&wired, Some(false), late));
    // A Play waits its own time, also past the window, then gives up.
    lost.play = Some(t0 + Duration::from_secs(110));
    assert!(!lost.expired(late));
    assert!(lost.applies(&wired, Some(false), late));
    let after = t0 + Duration::from_secs(110) + LOST_PEER_PLAY_WAIT;
    assert!(lost.gave_up(after));
    assert!(!lost.applies(&wired, Some(false), after));
}
#[test]
fn switching_bluetooth_off_leaves_no_headphones_to_wait_for() {
    // #136: Bluetooth switched off by the listener disconnects the headphones; Play then said
    // "Waiting for …" for minutes (the wait restarts after each wake).
    let t0 = Instant::now();
    let bt = AudioOutput::Bluetooth("AA:BB".into());
    assert!(LostPeer::after_loss(&bt, true, t0).is_none());
    let lost = LostPeer::after_loss(&bt, false, t0).unwrap();
    assert!(lost.is(&bt));
    assert!(LostPeer::after_loss(&AudioOutput::Wired, false, t0).is_none());
}
#[test]
fn bluetooth_headphones_win_over_the_jack_once_per_connection() {
    let mut auto = AutoOutput::default();
    let mut s = status("AA", "/a");
    let pcm = s.pcms.clone();
    // Connected but BlueALSA has no playback PCM yet: wait.
    s.pcms.clear();
    assert!(auto.next(&s, &AudioOutput::Wired).is_none());
    s.pcms = pcm.clone();
    assert_eq!(auto.next(&s, &AudioOutput::Wired).as_deref(), Some("AA"));
    auto.routed("AA".into());
    // The owner then picks the jack by hand: that choice sticks.
    assert!(auto.next(&s, &AudioOutput::Wired).is_none());
    // A codec change recreates the PCM; still the same connection.
    s.pcms.clear();
    assert!(auto.next(&s, &AudioOutput::Wired).is_none());
    s.pcms = pcm.clone();
    assert!(auto.next(&s, &AudioOutput::Wired).is_none());
    // A failed observation proves nothing.
    s.available = false;
    s.devices[0].connected = false;
    assert!(auto.next(&s, &AudioOutput::Wired).is_none());
    s.available = true;
    s.devices[0].connected = true;
    assert!(auto.next(&s, &AudioOutput::Wired).is_none());
    // A real reconnect routes again.
    s.devices[0].connected = false;
    assert!(auto.next(&s, &AudioOutput::Wired).is_none());
    s.devices[0].connected = true;
    assert_eq!(auto.next(&s, &AudioOutput::Wired).as_deref(), Some("AA"));
    // Radio off and on again is a new connection too.
    auto.routed("AA".into());
    s.powered = false;
    assert!(auto.next(&s, &AudioOutput::Wired).is_none());
    s.powered = true;
    assert_eq!(auto.next(&s, &AudioOutput::Wired).as_deref(), Some("AA"));
}
#[test]
fn already_on_bluetooth_is_never_displaced() {
    let mut auto = AutoOutput::default();
    let mut s = status("AA", "/a");
    // Restored or chosen by hand: nothing to do, and none later.
    assert!(auto
        .next(&s, &AudioOutput::Bluetooth("aa".into()))
        .is_none());
    assert!(auto.next(&s, &AudioOutput::Wired).is_none());
    // A second peer does not take over a working first one...
    add(&mut s, "BB", "/b");
    assert!(auto
        .next(&s, &AudioOutput::Bluetooth("AA".into()))
        .is_none());
    // ...but takes over once the first is gone and the jack is back.
    s.devices[0].connected = false;
    assert_eq!(auto.next(&s, &AudioOutput::Wired).as_deref(), Some("BB"));
    // A peer without the audio sink profile is never an output.
    let mut keyboard = status("CC", "/c");
    keyboard.devices[0].audio = false;
    assert!(AutoOutput::default()
        .next(&keyboard, &AudioOutput::Wired)
        .is_none());
}
#[test]
fn a_pcm_listed_before_its_codec_is_configured_is_not_ready_yet() {
    // Y2Reborn #103: BlueALSA lists the PCM ~0.7 s after the connection, before its
    // channels and rate are known; switching then failed and the jack kept the music.
    let mut auto = AutoOutput::default();
    let mut s = status("AA", "/a");
    s.pcms[0].channels = None;
    s.pcms[0].rate = None;
    assert!(auto.next(&s, &AudioOutput::Wired).is_none());
    s.pcms[0].channels = Some(2);
    assert!(auto.next(&s, &AudioOutput::Wired).is_none());
    s.pcms[0].rate = Some(48_000);
    assert_eq!(auto.next(&s, &AudioOutput::Wired).as_deref(), Some("AA"));
}
#[test]
fn a_failed_switch_is_tried_again_a_few_times_then_the_jack_stays() {
    let mut auto = AutoOutput::default();
    let s = status("AA", "/a");
    for _ in 1..AUTO_OUTPUT_TRIES {
        assert_eq!(auto.next(&s, &AudioOutput::Wired).as_deref(), Some("AA"));
        auto.failed("AA".into());
        // Not straight away: a short rest between tries.
        assert!(auto.next(&s, &AudioOutput::Wired).is_none());
        let earlier = Instant::now() - AUTO_OUTPUT_RETRY;
        auto.failures.get_mut("AA").unwrap().1 = earlier;
    }
    assert_eq!(auto.next(&s, &AudioOutput::Wired).as_deref(), Some("AA"));
    auto.failed("AA".into());
    assert!(
        auto.next(&s, &AudioOutput::Wired).is_none(),
        "gave up for this connection"
    );
    // A new connection starts over.
    let mut gone = s.clone();
    gone.devices[0].connected = false;
    assert!(auto.next(&gone, &AudioOutput::Wired).is_none());
    assert_eq!(auto.next(&s, &AudioOutput::Wired).as_deref(), Some("AA"));
}
