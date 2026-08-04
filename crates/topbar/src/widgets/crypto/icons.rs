//! The three logos, compiled in.
//!
//! `assets/crypto` holds an SVG per asset and a PNG rendered from it at each of
//! the four sizes the panel draws. **The PNGs are what ship**: the dev shell's
//! `cargo`-built binary has no gdk-pixbuf loader cache wired up, so an SVG that
//! decodes perfectly under the wrapped Nix binary comes back as nothing under
//! `cargo run` — a blank widget that only shows up in one of the two ways the
//! panel is ever started. GTK4 decodes PNG itself, with no loader module in the
//! picture, so a texture built here works under both.
//!
//! `assets/crypto/SOURCES.md` records where each logo came from, what its
//! licence is, and the one command that regenerates every PNG.
//!
//! Textures are built once per asset and size and then kept: the bar redraws
//! its icons on every price change, and decoding three PNGs every half hour to
//! produce the same three textures would be pure waste.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk4::{Image, gdk, glib};
use topbar_services::Asset;
use tracing::warn;

/// The sizes `assets/crypto` has PNGs for.
///
/// Anything else is rounded up to the nearest of these and scaled down by GTK,
/// which is what the 12px badge on a pair does.
const SIZES: [i32; 4] = [16, 24, 32, 48];

/// Every embedded logo: asset, size, bytes.
///
/// A flat table rather than a build-time macro so the test at the bottom can
/// walk it and prove each entry decodes to the size it claims.
const LOGOS: [(Asset, i32, &[u8]); 12] = [
    (
        Asset::Btc,
        16,
        include_bytes!("../../../../../assets/crypto/bitcoin-16.png"),
    ),
    (
        Asset::Btc,
        24,
        include_bytes!("../../../../../assets/crypto/bitcoin-24.png"),
    ),
    (
        Asset::Btc,
        32,
        include_bytes!("../../../../../assets/crypto/bitcoin-32.png"),
    ),
    (
        Asset::Btc,
        48,
        include_bytes!("../../../../../assets/crypto/bitcoin-48.png"),
    ),
    (
        Asset::Eth,
        16,
        include_bytes!("../../../../../assets/crypto/ethereum-16.png"),
    ),
    (
        Asset::Eth,
        24,
        include_bytes!("../../../../../assets/crypto/ethereum-24.png"),
    ),
    (
        Asset::Eth,
        32,
        include_bytes!("../../../../../assets/crypto/ethereum-32.png"),
    ),
    (
        Asset::Eth,
        48,
        include_bytes!("../../../../../assets/crypto/ethereum-48.png"),
    ),
    (
        Asset::Xmr,
        16,
        include_bytes!("../../../../../assets/crypto/monero-16.png"),
    ),
    (
        Asset::Xmr,
        24,
        include_bytes!("../../../../../assets/crypto/monero-24.png"),
    ),
    (
        Asset::Xmr,
        32,
        include_bytes!("../../../../../assets/crypto/monero-32.png"),
    ),
    (
        Asset::Xmr,
        48,
        include_bytes!("../../../../../assets/crypto/monero-48.png"),
    ),
];

thread_local! {
    /// Decoded logos, keyed by asset and the size they were rendered at.
    static TEXTURES: RefCell<HashMap<(Asset, i32), gdk::Texture>> =
        RefCell::new(HashMap::new());
}

/// The PNG for `asset` at the smallest embedded size that is not smaller than
/// `wanted`.
///
/// Scaling a logo down is invisible; scaling one up is not, which is why this
/// rounds the way it does. Anything above the largest embedded size gets that.
fn best_fit(wanted: i32) -> i32 {
    SIZES
        .into_iter()
        .find(|size| *size >= wanted)
        .unwrap_or_else(|| SIZES[SIZES.len() - 1])
}

/// The bytes for one asset at one embedded size.
fn bytes(asset: Asset, size: i32) -> Option<&'static [u8]> {
    LOGOS
        .iter()
        .find(|(logo, logo_size, _)| *logo == asset && *logo_size == size)
        .map(|(_, _, bytes)| *bytes)
}

/// The decoded logo for `asset`, suited to being drawn at `wanted` pixels.
///
/// `None` only if a PNG in `assets/crypto` is corrupt, which the test below
/// makes a build failure rather than something a user finds out about.
pub fn texture(asset: Asset, wanted: i32) -> Option<gdk::Texture> {
    let size = best_fit(wanted);
    TEXTURES.with_borrow_mut(|cache| {
        if let Some(texture) = cache.get(&(asset, size)) {
            return Some(texture.clone());
        }
        let texture = decode(asset, size)?;
        cache.insert((asset, size), texture.clone());
        Some(texture)
    })
}

/// Decode one embedded PNG.
fn decode(asset: Asset, size: i32) -> Option<gdk::Texture> {
    let bytes = bytes(asset, size)?;
    match gdk::Texture::from_bytes(&glib::Bytes::from_static(bytes)) {
        Ok(texture) => Some(texture),
        Err(error) => {
            warn!(
                "the {} logo at {size}px would not decode: {error}",
                asset.key()
            );
            None
        }
    }
}

/// An [`Image`] of `asset`'s logo, drawn at `size` pixels.
///
/// The pixel size is set here rather than left to CSS because the same logo is
/// drawn at four different sizes on two surfaces, and a class per size would be
/// four classes saying a number.
pub fn image(asset: Asset, size: i32) -> Image {
    let image = match texture(asset, size) {
        Some(texture) => Image::from_paintable(Some(&texture)),
        // A missing logo must not cost the price beside it. The generic
        // "something financial" glyph keeps the row the right shape.
        None => Image::from_icon_name("emblem-documents-symbolic"),
    };
    image.set_pixel_size(size);
    image
}

#[cfg(test)]
mod tests {
    use gtk4::prelude::TextureExt;

    use super::*;

    #[test]
    fn there_is_a_logo_for_every_asset_at_every_size() {
        assert_eq!(LOGOS.len(), Asset::ALL.len() * SIZES.len());
        for asset in Asset::ALL {
            for size in SIZES {
                assert!(
                    bytes(asset, size).is_some(),
                    "no {} logo at {size}px",
                    asset.key()
                );
            }
        }
    }

    /// The regeneration guard.
    ///
    /// A bad `rsvg-convert` run — a wrong flag, an SVG that failed to load, a
    /// truncated write — produces a file that is committed and then draws
    /// nothing. Decoding all twelve here turns that into a failing test.
    ///
    /// `gdk::Texture::from_bytes` uses GDK's own PNG loader, so this needs no
    /// display and no `gtk4::init` — which is exactly why the widget loads its
    /// logos this way in the first place.
    #[test]
    fn every_embedded_logo_decodes_at_the_size_it_claims() {
        for (asset, size, bytes) in LOGOS {
            let texture = gdk::Texture::from_bytes(&glib::Bytes::from_static(bytes))
                .unwrap_or_else(|error| {
                    panic!("the {} logo at {size}px is not a PNG: {error}", asset.key())
                });
            assert_eq!(
                (texture.width(), texture.height()),
                (size, size),
                "the {} logo is not {size}x{size}",
                asset.key()
            );
        }
    }

    #[test]
    fn a_size_between_two_logos_takes_the_larger() {
        assert_eq!(best_fit(12), 16, "the pair badge scales 16 down, not 16 up");
        assert_eq!(best_fit(16), 16);
        assert_eq!(best_fit(20), 24);
        assert_eq!(best_fit(24), 24);
        assert_eq!(best_fit(64), 48, "there is nothing bigger to reach for");
    }
}
