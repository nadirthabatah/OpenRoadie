//! What the desk panel is showing, with no GPUI in it.
//!
//! The panel draws monitors and network lights, and neither can be seen from
//! a test. So everything that decides *what* is on screen — which controls a
//! monitor offers, what a value reads as, which answers are too late to
//! believe — lives here, where it can be, and the view stays a thin drawing
//! of it.

use std::collections::BTreeMap;

use roadie_ddc::InputSource;
use roadie_ipc::desk::{
    AudioInterfaceSummary, ControllerSummary, DisplayControl, DisplayReading, DisplaySettings,
    DisplaySummary, MacroPadSummary, NetworkLightSummary, StreamDeckSummary,
};

/// The panel's whole state.
#[derive(Debug, Default)]
pub struct DeskModel {
    /// Monitors, in the order the agent found them.
    displays: Vec<DisplaySummary>,
    /// What each reachable monitor's controls read, by monitor handle.
    readings: BTreeMap<String, Vec<DisplayReading>>,
    /// Lights on the network.
    lights: Vec<NetworkLightSummary>,
    /// Stream Decks on USB.
    decks: Vec<StreamDeckSummary>,
    /// Audio interfaces, each carrying its own inputs.
    interfaces: Vec<AudioInterfaceSummary>,
    /// TourBox controllers on serial ports.
    controllers: Vec<ControllerSummary>,
    /// VIA keyboards and macro pads.
    pads: Vec<MacroPadSummary>,
    /// Whether a scan is in flight.
    scanning: bool,
    /// Which scan. Answers stamped with an older one are dropped.
    ///
    /// A scan takes seconds — multicast discovery alone is three — and in that
    /// time somebody can press refresh again, or unplug a monitor. Without
    /// this, the slower of two overlapping scans wins simply by finishing
    /// last, and the panel settles on the older truth.
    generation: u64,
}

impl DeskModel {
    /// Begin a scan, returning the generation its answers must carry.
    pub fn begin_scan(&mut self) -> u64 {
        self.generation += 1;
        self.scanning = true;
        self.generation
    }

    /// Whether a scan is still running.
    #[must_use]
    pub const fn is_scanning(&self) -> bool {
        self.scanning
    }

    /// Take the monitors from a scan, if it is still the current one.
    pub fn accept_displays(&mut self, generation: u64, found: Vec<DisplaySummary>) -> bool {
        if generation != self.generation {
            return false;
        }
        // Readings belong to handles from the scan that produced them. Keeping
        // the old ones would leave a monitor showing a brightness read from a
        // different monitor that happened to reuse its handle.
        self.readings
            .retain(|id, _| found.iter().any(|display| &display.id == id));
        self.displays = found;
        true
    }

    /// Take the lights from a scan, if it is still the current one.
    pub fn accept_lights(&mut self, generation: u64, found: Vec<NetworkLightSummary>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.lights = found;
        true
    }

    /// Take the Stream Decks from a scan, if it is still the current one.
    pub fn accept_decks(&mut self, generation: u64, found: Vec<StreamDeckSummary>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.decks = found;
        true
    }

    /// Take the audio interfaces from a scan, if it is still the current one.
    pub fn accept_interfaces(
        &mut self,
        generation: u64,
        found: Vec<AudioInterfaceSummary>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.interfaces = found;
        true
    }

    /// Take the controllers from a scan, if it is still the current one.
    pub fn accept_controllers(&mut self, generation: u64, found: Vec<ControllerSummary>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.controllers = found;
        true
    }

    /// Take the macro pads from a scan, if it is still the current one.
    pub fn accept_pads(&mut self, generation: u64, found: Vec<MacroPadSummary>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.pads = found;
        true
    }

    /// Store a Stream Deck's state after a write.
    ///
    /// Carries no settings — a deck cannot be asked what its brightness is —
    /// so what this actually refreshes is whether the deck is still reachable,
    /// which a write is the most reliable evidence of there is.
    pub fn accept_deck(&mut self, updated: StreamDeckSummary) {
        match self.decks.iter_mut().find(|deck| deck.id == updated.id) {
            Some(existing) => *existing = updated,
            None => self.decks.push(updated),
        }
    }

    /// Store an audio interface's state after a write.
    ///
    /// Not fenced by generation, for the same reason [`Self::apply_reading`] is
    /// not: a write is something a person just did to a device in front of
    /// them, and dropping its result because a rescan started in between would
    /// leave the panel showing the old value after the interface had moved.
    pub fn apply_interface(&mut self, updated: AudioInterfaceSummary) {
        match self
            .interfaces
            .iter_mut()
            .find(|interface| interface.id == updated.id)
        {
            Some(existing) => *existing = updated,
            None => self.interfaces.push(updated),
        }
    }

    /// Mark the scan finished.
    pub fn finish_scan(&mut self, generation: u64) {
        if generation == self.generation {
            self.scanning = false;
        }
    }

    /// Store what one monitor answered.
    pub fn accept_settings(&mut self, generation: u64, settings: DisplaySettings) -> bool {
        if generation != self.generation {
            return false;
        }
        self.readings.insert(settings.id, settings.readings);
        true
    }

    /// Store one control's new value after a write.
    ///
    /// Not fenced by generation: a write is something a person just did to a
    /// monitor they can see, and dropping its result because a background
    /// rescan started in between would leave the slider showing the old value
    /// after the monitor had already changed.
    pub fn apply_reading(&mut self, id: &str, reading: DisplayReading) {
        let Some(readings) = self.readings.get_mut(id) else {
            return;
        };
        match readings
            .iter_mut()
            .find(|existing| existing.control == reading.control)
        {
            Some(existing) => *existing = reading,
            None => readings.push(reading),
        }
    }

    /// Store a light's state after a write.
    pub fn apply_light(&mut self, updated: NetworkLightSummary) {
        match self.lights.iter_mut().find(|light| light.id == updated.id) {
            Some(existing) => *existing = updated,
            None => self.lights.push(updated),
        }
    }

    /// The monitors, in display order.
    #[must_use]
    pub fn displays(&self) -> &[DisplaySummary] {
        &self.displays
    }

    /// What one monitor's controls read, if it has answered.
    #[must_use]
    pub fn readings(&self, id: &str) -> &[DisplayReading] {
        self.readings.get(id).map_or(&[], Vec::as_slice)
    }

    /// The lights.
    #[must_use]
    pub fn lights(&self) -> &[NetworkLightSummary] {
        &self.lights
    }

    /// The Stream Decks.
    #[must_use]
    pub fn decks(&self) -> &[StreamDeckSummary] {
        &self.decks
    }

    /// The audio interfaces.
    #[must_use]
    pub fn interfaces(&self) -> &[AudioInterfaceSummary] {
        &self.interfaces
    }

    /// The controllers.
    #[must_use]
    pub fn controllers(&self) -> &[ControllerSummary] {
        &self.controllers
    }

    /// The macro pads.
    #[must_use]
    pub fn pads(&self) -> &[MacroPadSummary] {
        &self.pads
    }

    /// Whether the panel found nothing at all, with no scan running.
    ///
    /// Its own question because the empty state has to say something useful,
    /// and it has to account for every family: a desk with only a TourBox on
    /// it is not an empty desk, and a panel that said so would be telling
    /// somebody their hardware is missing while drawing it underneath.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.scanning
            && self.displays.is_empty()
            && self.lights.is_empty()
            && self.decks.is_empty()
            && self.interfaces.is_empty()
            && self.controllers.is_empty()
            && self.pads.is_empty()
    }
}

/// How one input's gain should read to a person.
///
/// A bare number, because that is all these interfaces give. Unlike a monitor,
/// which reports its own maximum alongside every value, a Focusrite answers
/// with a byte and nothing to scale it against — this desk's Vocaster stores
/// and reads back every value to 255 without complaint. So a percentage here
/// would be computed against a ceiling nobody stated, which is exactly the
/// invented maximum [`describe_value`] exists to avoid for monitors.
#[must_use]
pub fn describe_gain(gain: Option<u8>) -> String {
    gain.map_or_else(|| "—".to_owned(), |value| value.to_string())
}

/// How one control's value should read to a person.
///
/// A monitor's maximum is its own: plenty report 100 for brightness and
/// plenty do not, so a raw number means nothing without it, and a percentage
/// computed against an assumed 100 is wrong on the ones that do not.
#[must_use]
pub fn describe_value(reading: DisplayReading) -> String {
    match reading.control {
        DisplayControl::Input => describe_input(reading.current),
        DisplayControl::Brightness | DisplayControl::Contrast | DisplayControl::Volume => {
            if reading.maximum == 0 {
                // A monitor reporting a zero maximum is broken or lying, and
                // dividing by it would panic. Show the raw number instead of
                // inventing a percentage.
                return reading.current.to_string();
            }
            let percent = (u32::from(reading.current) * 100).div_ceil(u32::from(reading.maximum));
            format!("{percent}%")
        }
    }
}

/// The name of an input, or its number where the standard has none.
///
/// Above `0x12` is vendor territory — USB-C in particular has no standard
/// number at all — so a number is the honest answer there rather than a guess
/// that would send somebody looking for a cable they have not got.
#[must_use]
pub fn describe_input(value: u16) -> String {
    u8::try_from(value)
        .ok()
        .and_then(|code| InputSource::from_code(code).name())
        .map_or_else(|| format!("input {value:#04x}"), ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(id: &str, reachable: bool) -> DisplaySummary {
        DisplaySummary {
            id: id.to_owned(),
            name: format!("monitor {id}"),
            reachable,
            unreachable_reason: (!reachable).then(|| "permission denied".to_owned()),
        }
    }

    fn reading(control: DisplayControl, current: u16, maximum: u16) -> DisplayReading {
        DisplayReading {
            control,
            current,
            maximum,
        }
    }

    fn deck(id: &str) -> StreamDeckSummary {
        StreamDeckSummary {
            id: id.to_owned(),
            name: "Stream Deck XL".to_owned(),
            model: "Stream Deck XL".to_owned(),
            keys: 32,
            dials: 0,
            reachable: true,
            unreachable_reason: None,
        }
    }

    fn pad(id: &str) -> MacroPadSummary {
        MacroPadSummary {
            id: id.to_owned(),
            name: "SmartCloud".to_owned(),
            vendor_id: 0x5343,
            product_id: 0x0080,
            protocol: 12,
            layers: 6,
            reachable: true,
            unreachable_reason: None,
        }
    }

    fn light(id: &str, on: bool) -> NetworkLightSummary {
        NetworkLightSummary {
            id: id.to_owned(),
            name: format!("light {id}"),
            on,
            brightness: 40,
            kelvin: 4000,
            reachable: true,
            unreachable_reason: None,
        }
    }

    #[test]
    fn an_answer_from_a_scan_that_was_superseded_is_dropped() {
        // A scan takes seconds. Pressing refresh twice must not let the
        // slower of the two win by finishing last.
        let mut model = DeskModel::default();
        let first = model.begin_scan();
        let second = model.begin_scan();
        assert!(!model.accept_displays(first, vec![display("old", true)]));
        assert!(model.displays().is_empty(), "the stale answer was taken");
        assert!(model.accept_displays(second, vec![display("new", true)]));
        assert_eq!(model.displays()[0].id, "new");
    }

    #[test]
    fn a_superseded_scan_cannot_stop_the_spinner_either() {
        let mut model = DeskModel::default();
        let first = model.begin_scan();
        let second = model.begin_scan();
        model.finish_scan(first);
        assert!(
            model.is_scanning(),
            "the current scan is still running; the old one finishing says nothing"
        );
        model.finish_scan(second);
        assert!(!model.is_scanning());
    }

    #[test]
    fn readings_do_not_outlive_the_monitor_they_were_read_from() {
        // Handles are reused. A monitor unplugged and another plugged into the
        // same port can take the same one, and showing the first monitor's
        // brightness under the second one's name is worse than showing none.
        let mut model = DeskModel::default();
        let scan = model.begin_scan();
        model.accept_displays(scan, vec![display("i2c-7", true)]);
        model.accept_settings(
            scan,
            DisplaySettings {
                id: "i2c-7".to_owned(),
                readings: vec![reading(DisplayControl::Brightness, 40, 100)],
            },
        );
        assert_eq!(model.readings("i2c-7").len(), 1);

        let rescan = model.begin_scan();
        model.accept_displays(rescan, vec![display("i2c-9", true)]);
        assert!(
            model.readings("i2c-7").is_empty(),
            "a monitor that is gone kept its readings"
        );
    }

    #[test]
    fn a_write_lands_even_when_a_rescan_started_behind_it() {
        // The opposite rule from a scan answer, and deliberately so: a write
        // is something a person just did to a monitor in front of them.
        // Dropping its result would leave the slider showing the old value
        // after the monitor had already moved.
        let mut model = DeskModel::default();
        let scan = model.begin_scan();
        model.accept_displays(scan, vec![display("i2c-7", true)]);
        model.accept_settings(
            scan,
            DisplaySettings {
                id: "i2c-7".to_owned(),
                readings: vec![reading(DisplayControl::Brightness, 40, 100)],
            },
        );
        model.begin_scan();
        model.apply_reading("i2c-7", reading(DisplayControl::Brightness, 70, 100));
        assert_eq!(model.readings("i2c-7")[0].current, 70);
    }

    #[test]
    fn a_write_to_a_monitor_that_is_gone_changes_nothing() {
        let mut model = DeskModel::default();
        model.apply_reading("i2c-7", reading(DisplayControl::Brightness, 70, 100));
        assert_eq!(model.readings("i2c-7").len(), 0);
    }

    #[test]
    fn a_light_is_replaced_rather_than_duplicated() {
        let mut model = DeskModel::default();
        let scan = model.begin_scan();
        model.accept_lights(scan, vec![light("a", true), light("b", false)]);
        model.apply_light(light("a", false));
        assert_eq!(model.lights().len(), 2);
        assert!(!model.lights()[0].on, "the write did not land");
    }

    #[test]
    fn a_percentage_is_computed_against_the_monitor_own_maximum() {
        // 60 of 80 is 75 percent. Assuming a maximum of 100 would call it 60,
        // which is wrong on every monitor that does not use that scale.
        assert_eq!(
            describe_value(reading(DisplayControl::Contrast, 60, 80)),
            "75%"
        );
        assert_eq!(
            describe_value(reading(DisplayControl::Brightness, 40, 100)),
            "40%"
        );
    }

    #[test]
    fn a_monitor_claiming_a_zero_maximum_does_not_divide_by_it() {
        // Broken or lying, but either way it must not panic the panel.
        assert_eq!(describe_value(reading(DisplayControl::Volume, 7, 0)), "7");
    }

    #[test]
    fn an_input_is_named_where_the_standard_names_it() {
        assert_eq!(describe_input(0x0F), "DisplayPort 1");
        assert_eq!(describe_input(0x11), "HDMI 1");
    }

    #[test]
    fn an_input_the_standard_does_not_name_is_shown_as_its_number() {
        // Above 0x12 is vendor territory and USB-C has no standard number at
        // all, so a guess here would send somebody looking for a cable they
        // have not got.
        assert_eq!(describe_input(0x1B), "input 0x1b");
        assert_eq!(describe_input(999), "input 0x3e7");
    }

    fn interface(id: &str, gain: u8) -> AudioInterfaceSummary {
        AudioInterfaceSummary {
            id: id.to_owned(),
            name: "Vocaster Two".to_owned(),
            firmware: 1749,
            mass_storage: Some(true),
            inputs: vec![roadie_ipc::desk::AudioInputSettings {
                input: 1,
                gain: Some(gain),
                muted: Some(false),
                phantom: Some(false),
            }],
            reachable: true,
            unreachable_reason: None,
        }
    }

    fn controller(id: &str) -> ControllerSummary {
        ControllerSummary {
            id: id.to_owned(),
            name: "TourBox Elite".to_owned(),
            buttons: 14,
            wheels: 3,
            haptics: true,
            serial_number: None,
        }
    }

    #[test]
    fn an_interface_is_replaced_rather_than_duplicated_by_a_write() {
        let mut model = DeskModel::default();
        let scan = model.begin_scan();
        model.accept_interfaces(scan, vec![interface("a", 70)]);
        model.apply_interface(interface("a", 42));
        assert_eq!(model.interfaces().len(), 1, "the write added a second row");
        assert_eq!(model.interfaces()[0].inputs[0].gain, Some(42));
    }

    #[test]
    fn a_write_to_an_interface_lands_even_when_a_rescan_started_behind_it() {
        // Same rule as a monitor write, and for the same reason: somebody just
        // turned a knob on a box in front of them.
        let mut model = DeskModel::default();
        let scan = model.begin_scan();
        model.accept_interfaces(scan, vec![interface("a", 70)]);
        model.begin_scan();
        model.apply_interface(interface("a", 42));
        assert_eq!(model.interfaces()[0].inputs[0].gain, Some(42));
    }

    #[test]
    fn a_superseded_scan_cannot_deliver_any_of_the_new_families() {
        // Every family goes through the same fence. A per-family accessor that
        // forgot it would let the slower of two scans win for that family
        // alone, which is worse than either answer: the panel would be half
        // one scan and half the other.
        let mut model = DeskModel::default();
        let first = model.begin_scan();
        let second = model.begin_scan();
        assert!(!model.accept_decks(first, vec![deck("old")]));
        assert!(!model.accept_interfaces(first, vec![interface("old", 70)]));
        assert!(!model.accept_controllers(first, vec![controller("old")]));
        assert!(!model.accept_pads(first, vec![pad("old")]));
        assert_eq!(model.decks().len(), 0);
        assert_eq!(model.interfaces().len(), 0);
        assert_eq!(model.controllers().len(), 0);
        assert_eq!(model.pads().len(), 0);
        assert!(model.accept_decks(second, vec![deck("new")]));
        assert_eq!(model.decks()[0].id, "new");
    }

    #[test]
    fn a_desk_with_only_a_controller_on_it_is_not_an_empty_desk() {
        // The empty state would otherwise tell somebody nothing was found
        // while drawing their TourBox underneath the sentence.
        let mut model = DeskModel::default();
        let scan = model.begin_scan();
        model.accept_controllers(scan, vec![controller("/dev/cu.usbmodem1")]);
        model.finish_scan(scan);
        assert!(!model.is_empty());
    }

    #[test]
    fn a_gain_the_interface_does_not_have_reads_as_absent_rather_than_zero() {
        // Zero is a real gain — all the way down. An input with no gain
        // control at all is a different thing and must not look like it.
        assert_eq!(describe_gain(Some(0)), "0");
        assert_eq!(describe_gain(None), "—");
        assert_eq!(describe_gain(Some(70)), "70");
    }

    #[test]
    fn nothing_found_is_only_empty_once_the_scan_has_finished() {
        // Otherwise the panel says "no monitors" for the three seconds it
        // spends looking, which is the wrong answer at the wrong moment.
        let mut model = DeskModel::default();
        assert!(model.is_empty(), "nothing has been asked for yet");
        let scan = model.begin_scan();
        assert!(!model.is_empty(), "it is still looking");
        model.finish_scan(scan);
        assert!(model.is_empty());
    }
}
