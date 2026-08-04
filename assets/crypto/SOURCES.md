# Crypto asset provenance

The three logos the built-in `crypto` widget draws, where each came from, and
what its Wikimedia Commons file page says about its licence. Downloaded
**2026-08-04**.

Every file below is stated to be **public domain** on its Commons page, which is
why these three were chosen over the several attribution-only alternatives in
the same categories. The Monero entry carries an extra note because its Commons
page records a second, stricter claim by the upstream project; topbar credits
the Monero Project regardless, which satisfies that claim as well.

## bitcoin.svg

- Commons file: <https://commons.wikimedia.org/wiki/File:Bitcoin.svg>
- Download URL: <https://upload.wikimedia.org/wikipedia/commons/4/46/Bitcoin.svg>
- Licence as stated on the file page: **Public domain**
- Author as stated: Grayliptrot, derived from
  [File:Bitcoin logo.svg](https://commons.wikimedia.org/wiki/File:Bitcoin_logo.svg)
- Committed verbatim, 64x64.

## ethereum.svg

- Commons file: <https://commons.wikimedia.org/wiki/File:Ethereum-icon-purple.svg>
- Download URL: <https://upload.wikimedia.org/wikipedia/commons/6/6f/Ethereum-icon-purple.svg>
- Licence as stated on the file page: **Public domain** (own work, released by
  the uploader)
- Author as stated: GitR0n1n
- Committed with its CRLF line endings normalised to LF and nothing else
  changed; the rendered PNGs are byte-identical either way.
  `viewBox="0 0 1920 1920"`.
- Chosen over [File:Ethereum logo 2014.svg](https://commons.wikimedia.org/wiki/File:Ethereum_logo_2014.svg),
  which is the Ethereum Foundation's own file and is **CC BY 3.0** rather than
  public domain. Both draw the same diamond; the public-domain one avoids an
  attribution obligation on a 16px panel icon.

## monero.svg

- Commons file: <https://commons.wikimedia.org/wiki/File:Monero-Logo.svg>
- Download URL: <https://upload.wikimedia.org/wikipedia/commons/2/2d/Monero-Logo.svg>
- Licence as stated on the file page: **Public domain** — the page carries
  `{{PD-textlogo}}` (below the threshold of originality).
- Author as stated: The Monero Project, from
  <https://downloads.getmonero.org/resources/branding.zip>
- **Licence caveat, recorded honestly:** the same Commons page's *permission*
  field quotes the Monero Project as saying the mark "is made available under
  the CC BY 3.0 license"
  (<https://getmonero.org/legal/copyright>). The two claims disagree; the
  stricter of them is CC BY 3.0, so this file is credited to the Monero Project
  here and in the widget's documentation. Nothing else CC BY 3.0 asks for
  applies to an unmodified logo.
- Committed verbatim, 282x75 — it is the **full** lockup (symbol plus the
  "monero" wordmark). Only the leading 75x75 square, which is the circular
  symbol, is rasterised; see the crop below.

## Rasterisation

The dev shell's `cargo`-built binary has no gdk-pixbuf loader cache wired up, so
runtime SVG decoding cannot be relied on outside the wrapped Nix binary. The
PNGs beside these files are therefore pre-rendered, committed, and embedded with
`include_bytes!`; GTK4 decodes PNG natively with no loader module involved.

Regenerate every PNG from the SVGs with, from this directory:

```sh
for s in 16 24 32 48; do
  nix run nixpkgs#librsvg -- -w $s -h $s -o bitcoin-$s.png  bitcoin.svg
  nix run nixpkgs#librsvg -- -w $s -h $s -o ethereum-$s.png ethereum.svg
  # monero.svg is the 282x75 lockup: render it 3.76x as wide as it is tall and
  # let a square page clip everything but the leading symbol.
  nix run nixpkgs#librsvg -- -w $((s * 376 / 100)) -h $s \
    --page-width $s --page-height $s -o monero-$s.png monero.svg
done
```

`crates/topbar/src/widgets/crypto/icons.rs` has a test that decodes all twelve,
so a bad regeneration fails `cargo test` rather than showing up as a blank panel.
