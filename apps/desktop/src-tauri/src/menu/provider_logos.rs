//! Brand logos for a File Provider's own actions in the file context menu (macOS).
//!
//! Each line `file_provider_items.rs` draws for a known provider carries that provider's
//! logo, the way Finder shows them, so Dropbox's or Box's lines stand out in a long menu.
//! `context_menu_icons.rs` puts them on the items; this file only says which logo is whose.
//!
//! **Matched by the APP's bundle ID, on a dot boundary.** File Provider names a domain's
//! provider by its EXTENSION's bundle ID (`com.getdropbox.dropbox.fileprovider`), and macOS
//! requires an app extension's bundle ID to start with its containing app's plus a dot. So
//! an app that renames or adds an extension keeps its logo, and `com.microsoft.OneDrive`
//! never claims `com.microsoft.OneDrive-mac.FileProvider`.
//!
//! **The SVGs in `provider_logos/` are normalized for CoreSVG**, which `NSImage` reads them
//! with: explicit hex fills (no `currentColor`, no CSS `<style>`), no fixed `width` /
//! `height`, and a square `viewBox` so sizing the image to a square can't skew it. Box's
//! wordmark is wider than tall, so its `viewBox` pads it equally above and below.
//! Sources: Dropbox is `mdi:dropbox` (Material Design Icons by Pictogrammers, Apache 2.0),
//! MacDroid is `material-symbols:android` (Material Symbols by Google, Apache 2.0), Google
//! Drive and OneDrive are selfh.st icons (CC BY 4.0, attribution kept in each file), and
//! Box is Box's own mark. Adding, renaming, or re-sourcing a logo? Update its credit in
//! `scripts/check/checks/third-party-vendored.json`, which the Acknowledgements dialog lists.

/// One provider's logo and the apps that ship it.
pub struct ProviderLogo {
    /// Who the logo is, for log lines.
    pub provider: &'static str,
    /// The containing apps' bundle IDs. A provider's extension sits inside one of them.
    pub app_bundle_ids: &'static [&'static str],
    /// The logo, as normalized SVG (see the module doc).
    pub svg: &'static [u8],
}

/// Every provider with a logo. A provider missing here shows its actions without one.
///
/// App IDs verified 2026-09-12: Dropbox, Google Drive, and MacDroid by their installed
/// extensions (`pluginkit -m -p com.apple.fileprovider-nonui`); OneDrive (standalone,
/// 26.153.0809, extension `com.microsoft.OneDrive.FileProvider`) and Box Drive (2.53.223,
/// extension `com.box.desktop.boxfileprovider`) by reading the extension plists in the
/// vendors' signed installers, unpacked with `pkgutil --expand-full`, never installed.
/// `com.microsoft.OneDrive-mac` is the Mac App Store build, which that route can't reach,
/// so it's unverified.
pub const PROVIDER_LOGOS: &[ProviderLogo] = &[
    ProviderLogo {
        provider: "Dropbox",
        app_bundle_ids: &["com.getdropbox.dropbox"],
        svg: include_bytes!("provider_logos/dropbox.svg"),
    },
    ProviderLogo {
        provider: "Google Drive",
        app_bundle_ids: &["com.google.drivefs"],
        svg: include_bytes!("provider_logos/google-drive.svg"),
    },
    ProviderLogo {
        provider: "MacDroid",
        app_bundle_ids: &["us.electronic.mas.macdroid"],
        svg: include_bytes!("provider_logos/macdroid.svg"),
    },
    ProviderLogo {
        provider: "OneDrive",
        app_bundle_ids: &["com.microsoft.OneDrive", "com.microsoft.OneDrive-mac"],
        svg: include_bytes!("provider_logos/onedrive.svg"),
    },
    ProviderLogo {
        provider: "Box",
        app_bundle_ids: &["com.box.desktop"],
        svg: include_bytes!("provider_logos/box.svg"),
    },
];

/// The logo for the provider File Provider names `provider_id` (its extension's bundle ID).
pub fn logo_for_provider(provider_id: &str) -> Option<&'static ProviderLogo> {
    PROVIDER_LOGOS.iter().find(|logo| {
        logo.app_bundle_ids
            .iter()
            .any(|app_bundle_id| is_inside_bundle(provider_id, app_bundle_id))
    })
}

/// Whether `bundle_id` is `app_bundle_id` itself or something it contains, which is
/// `app_bundle_id` followed by a dot and more.
fn is_inside_bundle(bundle_id: &str, app_bundle_id: &str) -> bool {
    bundle_id
        .strip_prefix(app_bundle_id)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_of(provider_id: &str) -> Option<&'static str> {
        logo_for_provider(provider_id).map(|logo| logo.provider)
    }

    /// The extensions each provider ships today. The OneDrive pair is illustrative (it isn't
    /// installed on the machine this was written on); the rest are real.
    #[test]
    fn a_providers_extension_finds_its_apps_logo() {
        for (extension, provider) in [
            ("com.getdropbox.dropbox.fileprovider", "Dropbox"),
            ("com.google.drivefs.fpext", "Google Drive"),
            ("us.electronic.mas.macdroid.mountprovider", "MacDroid"),
            ("com.microsoft.OneDrive.FileProvider", "OneDrive"),
            ("com.microsoft.OneDrive-mac.FileProvider", "OneDrive"),
            ("com.box.desktop.boxfileprovider", "Box"),
        ] {
            assert_eq!(provider_of(extension), Some(provider), "`{extension}`");
        }
    }

    /// A bundle ID that merely starts with the same characters belongs to another app.
    #[test]
    fn an_app_id_matches_only_on_a_dot_boundary() {
        assert!(is_inside_bundle("com.getdropbox.dropbox", "com.getdropbox.dropbox"));
        assert!(is_inside_bundle(
            "com.getdropbox.dropbox.fileprovider",
            "com.getdropbox.dropbox"
        ));
        assert!(!is_inside_bundle(
            "com.microsoft.OneDrive-mac.FileProvider",
            "com.microsoft.OneDrive"
        ));
        assert!(!is_inside_bundle(
            "com.getdropbox.dropboxer.fileprovider",
            "com.getdropbox.dropbox"
        ));
        assert!(!is_inside_bundle("com.box", "com.box.desktop"));
    }

    #[test]
    fn a_provider_without_a_logo_gets_none() {
        for provider_id in [
            "com.apple.CloudDocs.iCloudDriveFileProvider",
            "com.box",
            "com.boxdesktop.fileprovider",
            "",
        ] {
            assert_eq!(provider_of(provider_id), None, "`{provider_id}`");
        }
    }

    /// A bundle ID is a string macOS compares at runtime, so a typo is silent. Pin the ones
    /// we ship so a change has to be deliberate.
    #[test]
    fn the_logos_are_the_ones_we_chose() {
        let table: Vec<(&str, &[&str])> = PROVIDER_LOGOS
            .iter()
            .map(|logo| (logo.provider, logo.app_bundle_ids))
            .collect();
        assert_eq!(
            table,
            [
                ("Dropbox", &["com.getdropbox.dropbox"][..]),
                ("Google Drive", &["com.google.drivefs"][..]),
                ("MacDroid", &["us.electronic.mas.macdroid"][..]),
                (
                    "OneDrive",
                    &["com.microsoft.OneDrive", "com.microsoft.OneDrive-mac"][..]
                ),
                ("Box", &["com.box.desktop"][..]),
            ]
        );
    }

    /// CoreSVG draws what the file spells out: a fill it can't resolve renders black, and a
    /// non-square `viewBox` stretches once the image is sized to a square.
    #[test]
    fn every_logo_is_a_square_svg_with_explicit_colors() {
        for logo in PROVIDER_LOGOS {
            let svg = std::str::from_utf8(logo.svg).expect("an SVG is text");
            let root = svg
                .strip_prefix("<svg ")
                .and_then(|rest| rest.split_once('>'))
                .map(|(attributes, _)| attributes)
                .unwrap_or_else(|| panic!("{}'s logo starts with its `<svg>` tag", logo.provider));
            let view_box: Vec<f64> = root
                .split_once("viewBox=\"")
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(value, _)| value.split_whitespace().filter_map(|n| n.parse().ok()).collect())
                .unwrap_or_default();
            assert!(
                view_box.len() == 4 && view_box[2] == view_box[3],
                "{}'s viewBox is square: {view_box:?}",
                logo.provider
            );
            assert!(
                !root.contains("width=") && !root.contains("height="),
                "{}'s logo has no fixed size",
                logo.provider
            );
            assert!(
                !svg.contains("currentColor"),
                "{}'s logo has explicit fills",
                logo.provider
            );
            assert!(!svg.contains("<style"), "{}'s logo has no CSS", logo.provider);
        }
    }

    /// Every file in `provider_logos/` is one the table embeds, so a logo can't be added
    /// without a provider, or replaced on disk while an old copy ships.
    #[test]
    fn every_svg_on_disk_is_a_logo_in_the_table() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/menu/provider_logos");
        let files: Vec<_> = std::fs::read_dir(&folder)
            .expect("the logo folder exists")
            .map(|entry| entry.expect("the logo folder lists").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "svg"))
            .collect();
        assert_eq!(files.len(), PROVIDER_LOGOS.len(), "{files:?}");
        for file in files {
            let bytes = std::fs::read(&file).expect("a logo file reads");
            assert!(
                PROVIDER_LOGOS.iter().any(|logo| logo.svg == bytes.as_slice()),
                "{file:?} is embedded by no table entry"
            );
        }
    }
}
