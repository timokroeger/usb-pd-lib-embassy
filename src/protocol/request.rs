use bilge::prelude::*;
use defmt::Format;

#[bitsize(32)]
#[derive(FromBits, DebugBits, Format, Clone, Copy)]
pub struct Request {
    pub min_operating_current: u10, // 10mA units
    pub operating_curent: u10,      // 10mA units
    _reserved1: u4,
    pub no_usb_suspend: bool,
    pub usb_communications_capable: bool,
    pub capability_mismatch: bool,
    pub give_back_flag: bool,
    pub object_position: u3,
    _reserved2: bool,
}
