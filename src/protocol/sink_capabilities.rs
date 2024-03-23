use bilge::prelude::*;
use defmt::Format;

#[bitsize(32)]
#[derive(FromBits, DebugBits, Format, Clone, Copy)]
pub struct FixedSupply {
    pub operating_current: u10, // 10mA units
    pub voltage: u10,           // 150mV units
    _reserved1: u5,
    pub dual_role_data: bool,
    pub usb_communications_capable: bool,
    pub unconstrained_power: bool,
    pub higher_capabilty: bool,
    pub dual_power_role: bool,
    fixed_supply: u2,
}
