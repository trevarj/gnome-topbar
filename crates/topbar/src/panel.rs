//! The GTK panel.
//!
//! M0 only pins down the toolchain: the binary links GTK4 and
//! gtk4-layer-shell (see `build.rs` for the link-order shim) and can report
//! which versions it is bound to. The bar itself lands in M1.

/// Report the GTK4 and gtk4-layer-shell versions this binary is linked against.
///
/// Calling into GTK for the version numbers is deliberate: it proves the
/// dynamic link path — including the `libgtk4-layer-shell.so`-before-
/// `libwayland-client.so` ordering — without needing a display.
pub fn linked_stack() -> String {
    // Taking the address of a gtk4-layer-shell entry point forces the linker to
    // resolve the library without calling into it (which would need a display).
    let probe = gtk4_layer_shell::is_supported as fn() -> bool;

    format!(
        "gtk4 {}.{}.{}, gtk4-layer-shell entry {:p}",
        gtk4::major_version(),
        gtk4::minor_version(),
        gtk4::micro_version(),
        probe as *const (),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_gtk4_version() {
        let stack = linked_stack();
        assert!(stack.starts_with("gtk4 4."), "{stack}");
    }
}
