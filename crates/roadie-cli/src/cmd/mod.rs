use std::process::ExitCode;

use anyhow::Result;
use clap::Subcommand;

pub mod assets;
pub mod audio;
pub mod backlight;
pub mod camera;
pub mod devices;
pub mod diag;
pub mod display;
pub mod doctor;
pub mod light;
pub mod list;
pub mod mcp;
pub mod profile;
pub mod snapshot;
pub mod streamdeck;
pub mod tourbox;
pub mod via;

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List connected Logitech HID++ devices.
    List(list::ListArgs),
    /// Survey every peripheral attached, whoever made it, and say what
    /// this build can configure on each.
    Devices(devices::DevicesArgs),
    /// Check why devices are not being found, and say what to do about it.
    Doctor(doctor::DoctorArgs),
    /// Read or persistently set the keyboard backlight (HID++ 0x1982).
    Backlight(backlight::BacklightArgs),
    /// Capture one frame from a Logitech webcam to a PNG.
    Snapshot(snapshot::SnapshotArgs),
    /// Read or write device-level UVC image controls on a webcam.
    Camera(camera::CameraArgs),
    /// Read and change a monitor's own settings over DDC: brightness,
    /// contrast, input, and the rest of what its bezel menu offers.
    Display(display::DisplayArgs),
    /// Manage assets fetched from OpenRoadie's asset mirrors.
    #[command(subcommand)]
    Assets(assets::AssetsCmd),
    /// Real-device round-trip smoke tests against the HID++ write path.
    #[command(subcommand)]
    Diag(diag::DiagCmd),
    /// Inspect and control standalone lights: Logitech Litra over USB, and
    /// Elgato Key Lights over the network.
    #[command(subcommand)]
    Light(light::LightCmd),
    /// Drive an Elgato Stream Deck, and check the driver against hardware.
    Streamdeck(streamdeck::StreamDeckArgs),
    /// Find TourBox controllers, and check what each button, knob and dial
    /// sends.
    Tourbox(tourbox::TourBoxArgs),
    /// Read and change a Focusrite Scarlett or Vocaster interface: preamp
    /// gain, mute, and 48 volt phantom power.
    Audio(audio::AudioArgs),
    /// Read and change what each key sends on a QMK keyboard or macro pad
    /// with VIA enabled.
    Via(via::ViaArgs),
    /// Serve the agent to AI assistants over the Model Context Protocol
    /// (stdio transport; register the command `roadie mcp` in the client).
    Mcp(mcp::McpArgs),
    /// Export, inspect and import portable configuration profiles.
    #[command(subcommand)]
    Profile(profile::ProfileCmd),
}

impl Command {
    /// Dispatch the parsed subcommand and report the process exit status.
    ///
    /// Only `list` reports a status of its own (nothing connected); every
    /// other subcommand either succeeds or fails outright.
    pub async fn run(self) -> Result<ExitCode> {
        match self {
            Self::List(args) => return list::run(args).await,
            Self::Devices(args) => return devices::run(args).await,
            Self::Doctor(args) => return doctor::run(args).await,
            Self::Backlight(args) => backlight::run(args).await?,
            // Camera capture is blocking AVFoundation — no need for the async runtime.
            Self::Snapshot(args) => snapshot::run(args)?,
            // UVC control transfers are blocking IOKit — no async runtime needed.
            Self::Camera(args) => camera::run(args)?,
            // DDC is a blocking ioctl or a blocking framework call, and the
            // protocol's timing floors are plain sleeps. No async runtime
            // would have anything to do.
            Self::Display(args) => args.cmd.unwrap_or(display::DisplayCmd::List).run()?,
            // `assets sync` is blocking HTTP — no need for the async runtime.
            Self::Assets(cmd) => cmd.run()?,
            Self::Diag(cmd) => cmd.run().await?,
            Self::Light(cmd) => cmd.run().await?,
            Self::Streamdeck(args) => {
                return args
                    .cmd
                    .unwrap_or(streamdeck::StreamDeckCmd::List)
                    .run(args.device.as_deref())
                    .await;
            }
            // A serial port is a blocking read, and the listen loop is a
            // plain poll with a timeout. No async runtime would have
            // anything to do.
            Self::Tourbox(args) => {
                return args.cmd.unwrap_or(tourbox::TourBoxCmd::List).run();
            }
            // USB control transfers are blocking, and an exchange is two of
            // them in order. No async runtime would have anything to do.
            Self::Audio(args) => args.cmd.unwrap_or(audio::AudioCmd::List).run()?,
            Self::Via(args) => {
                return args
                    .cmd
                    .unwrap_or(via::ViaCmd::List)
                    .run(args.device.as_deref())
                    .await;
            }
            Self::Mcp(args) => mcp::run(args).await?,
            // Profile work is plain file I/O — no async runtime needed.
            Self::Profile(cmd) => return cmd.run(),
        }
        Ok(ExitCode::SUCCESS)
    }
}
