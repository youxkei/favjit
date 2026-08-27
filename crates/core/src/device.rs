//! Keyboards, and the scopes layout rules are written against.

/// A keyboard, as the host chose to number it.
///
/// Opaque on purpose: nothing in `core` may derive meaning from the value, so a
/// host is free to hand out whatever its capture API gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub u64);

/// What the host could learn about a keyboard when it attached.
///
/// `vendor_id` / `product_id` are optional because ADR-0003 makes per-device
/// rules contingent on the capture API exposing the originating device: a
/// keyboard the host cannot identify still converts, it just cannot be singled
/// out by [`DeviceMatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub is_built_in: bool,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
}

impl DeviceInfo {
    pub const fn built_in(id: DeviceId) -> Self {
        Self {
            id,
            is_built_in: true,
            vendor_id: None,
            product_id: None,
        }
    }

    pub const fn external(id: DeviceId, vendor_id: u16, product_id: u16) -> Self {
        Self {
            id,
            is_built_in: false,
            vendor_id: Some(vendor_id),
            product_id: Some(product_id),
        }
    }
}

/// One external keyboard, named by USB identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceMatch {
    pub vendor_id: u16,
    pub product_id: u16,
}

impl DeviceMatch {
    pub const fn new(vendor_id: u16, product_id: u16) -> Self {
        Self {
            vendor_id,
            product_id,
        }
    }

    pub fn matches(&self, info: &DeviceInfo) -> bool {
        info.vendor_id == Some(self.vendor_id) && info.product_id == Some(self.product_id)
    }
}

/// Which keyboards a rule applies to.
///
/// The split is by how a keyboard is *typed*, not by how it is attached. An
/// external keyboard the user types in Dudrack belongs with the built-in one,
/// which is why there is no plain "external" scope: `RawJis` is every external
/// keyboard the configuration has not claimed for Dudrack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every keyboard.
    Any,
    /// The Mac's own keyboard.
    BuiltIn,
    /// Everything typed in Dudrack: the built-in keyboard plus the external
    /// keyboards named in the configuration.
    Dudrack,
    /// The Dudrack-typed external keyboards alone — for the JIS keys the
    /// built-in keyboard does not have.
    DudrackExternal,
    /// External keyboards that keep their JIS labels.
    RawJis,
    /// Whatever arrives over the link, and nothing attached here.
    ///
    /// Its own scope rather than a case of `RawJis`, because a keyboard at the other
    /// machine and a keyboard under the person's other hand are different keyboards
    /// however alike they are printed: one is where the person is typing *from* and the
    /// other is beside where they are typing *into*. What identifies it is the number
    /// every forwarded event carries ([`crate::link::is_from_source`]) rather than a
    /// vendor and product, which is what makes it nameable at all — a hook on the
    /// Windows side says which key and not which keyboard.
    Forwarded,
}
