use std::fs;

use super::{connected, graphics_card, i2c_node, read_edid};

/// A throwaway directory shaped like one of the kernel's connector
/// directories.
///
/// Real `/sys` cannot be written to and a machine running these tests has no
/// monitor attached anyway, so the layout is reproduced rather than borrowed.
/// What is being tested is this file's reading of that layout, which is
/// exactly the part that would break if a driver published it differently.
struct Connector(std::path::PathBuf);

impl Connector {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("roadie-display-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a writable temporary directory");
        Self(path)
    }

    fn with_file(self, name: &str, contents: &[u8]) -> Self {
        fs::write(self.0.join(name), contents).expect("a writable temporary file");
        self
    }

    fn with_bus(self, relative: &str, bus: &str) -> Self {
        let directory = self.0.join(relative);
        fs::create_dir_all(&directory).expect("a writable temporary directory");
        fs::create_dir_all(directory.join(bus)).expect("a writable temporary directory");
        self
    }
}

impl Drop for Connector {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn only_a_connected_connector_counts() {
    let plugged = Connector::new("connected").with_file("status", b"connected\n");
    assert!(connected(&plugged.0));

    let empty = Connector::new("disconnected").with_file("status", b"disconnected\n");
    assert!(!connected(&empty.0));

    // A directory under /sys/class/drm with no status file at all is not a
    // connector — the cards themselves look like this.
    let card = Connector::new("card");
    assert!(!connected(&card.0));
}

#[test]
fn a_connected_connector_with_an_empty_edid_has_no_name() {
    // A real state, not a broken one: the kernel reports the link before it
    // has read the block, and some KVM switches never let it. A display with
    // no name is still a display, which is what the enumeration test below
    // pins; this one only pins that no name comes out of it.
    let connector = Connector::new("nameless").with_file("edid", b"");
    assert!(read_edid(&connector.0).is_none());
}

/// A synthetic `/sys/class/drm`, since the real one cannot be written to and
/// the machines this is built on have no monitors attached to it anyway.
struct DrmTree(std::path::PathBuf);

impl DrmTree {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("roadie-drm-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a writable temporary directory");
        Self(path)
    }

    /// A connector directory, with whatever the kernel would have put in it.
    fn connector(self, name: &str, status: &str, edid: Option<&[u8]>, bus: Option<&str>) -> Self {
        let path = self.0.join(name);
        fs::create_dir_all(&path).expect("a writable temporary directory");
        fs::write(path.join("status"), status).expect("a writable temporary file");
        if let Some(edid) = edid {
            fs::write(path.join("edid"), edid).expect("a writable temporary file");
        }
        if let Some(bus) = bus {
            let directory = path.join("ddc").join("i2c-dev").join(bus);
            fs::create_dir_all(&directory).expect("a writable temporary directory");
        }
        self
    }

    /// A directory that is not a connector at all — the cards themselves look
    /// like this, and so does `version`.
    fn clutter(self, name: &str) -> Self {
        fs::create_dir_all(self.0.join(name)).expect("a writable temporary directory");
        self
    }
}

impl Drop for DrmTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A valid EDID base block naming an LG ULTRAFINE.
///
/// Built rather than captured, because a captured one would be a real
/// person's serial number. `0x1E6D` is LG's published PNP code and `0xFC` is
/// the descriptor tag for a display's name.
fn ultrafine_edid() -> Vec<u8> {
    let mut block = [0_u8; 128];
    block[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    block[8] = 0x1E;
    block[9] = 0x6D;
    block[18] = 1;
    block[19] = 4;
    block[54..59].copy_from_slice(&[0x00, 0x00, 0x00, 0xFC, 0x00]);
    block[59..72].copy_from_slice(b"ULTRAFINE\n   ");
    let sum = block[..127]
        .iter()
        .fold(0_u8, |acc, byte| acc.wrapping_add(*byte));
    block[127] = sum.wrapping_neg();
    block.to_vec()
}

#[test]
fn a_display_is_never_dropped_for_being_unreachable() {
    // The central promise of this backend, and the reason enumeration takes a
    // root. The kernel publishes each connector's EDID world-readable while
    // the I2C node is group-owned, so a monitor can be perfectly nameable and
    // still refuse every read. A list that dropped it would answer "why is my
    // screen missing" with nothing at all.
    //
    // Neither display here can actually be opened: the bus nodes named do not
    // exist on this machine. Both must still be listed.
    let tree = DrmTree::new("unreachable")
        .connector(
            "card0-DP-1",
            "connected\n",
            Some(&ultrafine_edid()),
            Some("i2c-7"),
        )
        .connector("card0-DP-2", "connected\n", None, None);

    let displays = super::enumerate_under(&tree.0).expect("the tree is readable");
    assert_eq!(displays.len(), 2, "neither display may be dropped");
}

#[test]
fn a_display_is_named_from_its_own_edid() {
    let tree = DrmTree::new("named").connector(
        "card0-DP-1",
        "connected\n",
        Some(&ultrafine_edid()),
        Some("i2c-7"),
    );
    let displays = super::enumerate_under(&tree.0).expect("the tree is readable");
    assert_eq!(displays[0].describe(), "LG ULTRAFINE");
}

#[test]
fn a_display_with_no_edid_falls_back_to_its_connector() {
    // Not a pretty name, but one a person can match against a cable.
    let tree = DrmTree::new("unnamed").connector("card0-DP-2", "connected\n", None, None);
    let displays = super::enumerate_under(&tree.0).expect("the tree is readable");
    assert_eq!(displays[0].describe(), "card0-DP-2");
}

#[test]
fn an_unreachable_display_explains_itself_when_asked() {
    // Being in the list is half of it. The other half is that asking anything
    // of it says why, rather than failing blankly.
    let tree = DrmTree::new("why").connector("card0-DP-2", "connected\n", None, None);
    let mut displays = super::enumerate_under(&tree.0).expect("the tree is readable");
    let error = displays[0]
        .get(roadie_ddc::Feature::Brightness)
        .expect_err("there is no I2C line for this connector");
    assert!(
        error.to_string().contains("no I2C line"),
        "the reason has to survive to the operation: {error}"
    );
}

#[test]
fn an_unreachable_display_is_named_by_its_panel_not_its_port() {
    // The point of reading the EDID before opening anything. "LG ULTRAFINE
    // cannot be reached" names a screen someone can look at; "card0-DP-1"
    // names a string they would have to go and decode. The bus node this
    // names does not exist here, so the open fails and the message is the
    // whole of what they get.
    let tree = DrmTree::new("panelname").connector(
        "card0-DP-1",
        "connected\n",
        Some(&ultrafine_edid()),
        Some("i2c-nonexistent"),
    );
    let mut displays = super::enumerate_under(&tree.0).expect("the tree is readable");
    let error = displays[0]
        .get(roadie_ddc::Feature::Brightness)
        .expect_err("that bus node does not exist");
    let message = error.to_string();
    assert!(
        message.starts_with("LG ULTRAFINE"),
        "the panel names itself first: {message}"
    );
    assert!(
        message.contains("i2c-nonexistent"),
        "and the node is still there for a bug report: {message}"
    );
}

#[test]
fn nothing_that_is_not_a_connected_connector_is_listed() {
    // Three things that are not displays: an unplugged port, a card
    // directory with no status file, and the `version` file's directory.
    let tree = DrmTree::new("clutter")
        .connector("card0-HDMI-A-1", "disconnected\n", None, None)
        .clutter("card0")
        .clutter("renderD128");
    let displays = super::enumerate_under(&tree.0).expect("the tree is readable");
    assert!(
        displays.is_empty(),
        "nothing here is a connected display: {:?}",
        displays
            .iter()
            .map(super::Display::describe)
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_order_is_the_same_twice_running() {
    // `read_dir` yields in filesystem order. Someone counting positions in a
    // list being read aloud needs that list not to reshuffle between runs.
    let tree = DrmTree::new("order")
        .connector("card0-DP-2", "connected\n", None, None)
        .connector("card0-DP-1", "connected\n", None, None)
        .connector("card0-HDMI-A-1", "connected\n", None, None);
    let names: Vec<String> = super::enumerate_under(&tree.0)
        .expect("the tree is readable")
        .iter()
        .map(super::Display::describe)
        .collect();
    assert_eq!(names, ["card0-DP-1", "card0-DP-2", "card0-HDMI-A-1"]);
}

#[test]
fn the_modern_bus_layout_is_found() {
    // What amdgpu and i915 publish today.
    let connector = Connector::new("modern").with_bus("ddc/i2c-dev", "i2c-7");
    assert_eq!(
        i2c_node(&connector.0),
        Some(std::path::PathBuf::from("/dev/i2c-7"))
    );
}

#[test]
fn the_older_bus_layout_is_found_too() {
    // Some drivers put the bus directly under the connector. Guessing one
    // layout would leave those monitors silently uncontrollable.
    let connector = Connector::new("older").with_bus(".", "i2c-3");
    assert_eq!(
        i2c_node(&connector.0),
        Some(std::path::PathBuf::from("/dev/i2c-3"))
    );
}

#[test]
fn a_connector_with_no_bus_at_all_reports_none() {
    let connector = Connector::new("busless").with_file("status", b"connected\n");
    assert_eq!(i2c_node(&connector.0), None);
}

#[test]
fn a_machine_with_no_display_subsystem_reports_none_rather_than_failing() {
    // The build container's own case, and it went the wrong way the first
    // time: with no /sys/class/drm at all this returned an error, so
    // `roadie display list` said "something went wrong" where the true answer
    // was "you have no monitors". A container, a headless server and a kernel
    // built without DRM are all that shape, and none of them is a fault.
    let displays = super::enumerate().expect("no display subsystem is an answer, not a failure");
    for display in &displays {
        assert!(
            !display.describe().is_empty(),
            "every display in the list has something to call it"
        );
    }
}

/// Paths as this desk's RTX 3060 published them under Ubuntu 26.04.
#[test]
fn a_connector_belongs_to_the_card_above_its_drm_directory() {
    let connector = std::path::Path::new(
        "/sys/devices/pci0000:00/0000:00:01.0/0000:01:00.0/drm/card1/card1-HDMI-A-1",
    );
    assert_eq!(
        graphics_card(connector),
        Some(std::path::Path::new(
            "/sys/devices/pci0000:00/0000:00:01.0/0000:01:00.0"
        ))
    );
    // A platform I2C controller has no drm directory above it at all.
    assert_eq!(
        graphics_card(std::path::Path::new(
            "/sys/devices/platform/i2c_designware.0/i2c-0"
        )),
        None
    );
}

/// The safety property: an SMBus at `0x50` is a memory module's SPD EEPROM,
/// and its adapter's parent is the chipset, never the graphics card.
#[test]
fn only_a_bus_on_the_connectors_own_card_is_a_candidate() {
    let card = graphics_card(std::path::Path::new(
        "/sys/devices/pci0000:00/0000:00:01.0/0000:01:00.0/drm/card1/card1-HDMI-A-1",
    ));
    let nvidia = std::path::Path::new("/sys/devices/pci0000:00/0000:00:01.0/0000:01:00.0/i2c-4");
    let smbus = std::path::Path::new("/sys/devices/pci0000:00/0000:00:1f.4/i2c-3");
    assert_eq!(nvidia.parent(), card);
    assert_ne!(smbus.parent(), card);
}
