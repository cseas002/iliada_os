//! Conservative permission *declarations* inferred from linked libraries.
//!
//! Declaring a capability never grants it; it only allows the app to ask.
//! Inferring declarations for ingested apps keeps them working (an app that
//! links PulseAudio will want the microphone) while every actual use still
//! goes through a user prompt in the broker. The purpose strings say they
//! were inferred so the prompt is honest about where the claim came from.

use std::collections::BTreeSet;

use iliada_bundle::{PermissionRequest, Permissions};
use iliada_types::{Capability, ClipboardAccess, FileScope, LocationPrecision};

const RULES: &[(&str, &[Capability], &str)] = &[
    ("libpulse.so", &[Capability::Microphone], "audio (PulseAudio)"),
    (
        "libpipewire-0.3.so",
        &[Capability::Microphone, Capability::Camera, Capability::ScreenCapture],
        "media (PipeWire)",
    ),
    ("libasound.so", &[Capability::Microphone], "audio (ALSA)"),
    ("libportaudio.so", &[Capability::Microphone], "audio (PortAudio)"),
    ("libv4l2.so", &[Capability::Camera], "video capture (V4L2)"),
    ("libgeoclue-2.so", &[Capability::Location(LocationPrecision::Coarse)], "location (GeoClue)"),
    ("libnotify.so", &[Capability::Notifications], "notifications (libnotify)"),
    (
        "libgtk-3.so",
        &[
            Capability::Files(FileScope::UserPicked),
            Capability::Clipboard(ClipboardAccess::Read),
            Capability::Clipboard(ClipboardAccess::Write),
        ],
        "GUI toolkit (GTK 3)",
    ),
    (
        "libgtk-4.so",
        &[
            Capability::Files(FileScope::UserPicked),
            Capability::Clipboard(ClipboardAccess::Read),
            Capability::Clipboard(ClipboardAccess::Write),
        ],
        "GUI toolkit (GTK 4)",
    ),
    (
        "libQt5Widgets.so",
        &[
            Capability::Files(FileScope::UserPicked),
            Capability::Clipboard(ClipboardAccess::Read),
            Capability::Clipboard(ClipboardAccess::Write),
        ],
        "GUI toolkit (Qt 5)",
    ),
    (
        "libQt6Widgets.so",
        &[
            Capability::Files(FileScope::UserPicked),
            Capability::Clipboard(ClipboardAccess::Read),
            Capability::Clipboard(ClipboardAccess::Write),
        ],
        "GUI toolkit (Qt 6)",
    ),
];

pub fn infer_permissions<'a>(sonames: impl IntoIterator<Item = &'a str>) -> Permissions {
    let sonames: BTreeSet<&str> = sonames.into_iter().collect();
    let mut seen = BTreeSet::new();
    let mut requests = Vec::new();
    for (prefix, caps, why) in RULES {
        if sonames.iter().any(|s| s.starts_with(prefix)) {
            for cap in *caps {
                if seen.insert(*cap) {
                    requests.push(PermissionRequest {
                        capability: *cap,
                        purpose: format!("Inferred: the app links a {why} library and may ask to {cap}."),
                    });
                }
            }
        }
    }
    Permissions { requests }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_tool_gets_nothing() {
        assert!(infer_permissions(["libc.so.6", "libonig.so.5"]).requests.is_empty());
    }

    #[test]
    fn audio_app_may_ask_for_mic_once() {
        let p = infer_permissions(["libpulse.so.0", "libasound.so.2"]);
        assert_eq!(p.requests.len(), 1);
        assert_eq!(p.requests[0].capability, Capability::Microphone);
        p.validate().unwrap();
    }
}
