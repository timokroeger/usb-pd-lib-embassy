use bilge::prelude::*;

#[bitsize(4)]
#[derive(FromBits, Debug, Clone, Copy, PartialEq)]
enum ControlMessageType {
    GoodCRC = 0x1,
    GotoMin = 0x2,
    Accept = 0x3,
    Reject = 0x4,
    Ping = 0x5,
    PsRdy = 0x6,
    GetSourceCap = 0x7,
    GetSinkCap = 0x8,
    DrSwap = 0x9,
    PrSwap = 0xA,
    VconnSwap = 0xB,
    Wait = 0xC,
    SoftReset = 0xD,
    #[fallback]
    Reserved,
}

#[bitsize(4)]
#[derive(FromBits, Debug, Clone, Copy, PartialEq)]
enum DataMessageType {
    SourceCapabilites = 0x1,
    Request = 0x2,
    Bist = 0x3,
    SinkCapabilities = 0x4,
    VenderDefined = 0xF,
    #[fallback]
    Reserved,
}

#[bitsize(1)]
#[derive(FromBits, Debug, Clone, Copy, PartialEq)]
enum PortDataRole {
    UpstreamFacingPort,
    DownstreamFacingPort,
}

#[bitsize(2)]
#[derive(FromBits, Debug, Clone, Copy, PartialEq)]
enum SpecificationRevision {
    Revision1_0,
    Revision2_0,
    #[fallback]
    Reserved,
}

#[bitsize(1)]
#[derive(FromBits, Debug, Clone, Copy, PartialEq)]
enum PortPowerRole {
    Sink,
    Source,
}

#[bitsize(16)]
#[derive(FromBits, Clone, Copy)]
struct Header {
    message_type: u4,
    _reserved1: bool,
    port_data_role: PortDataRole,
    specification_revision: SpecificationRevision,
    port_power_role: PortPowerRole,
    message_id: u3,
    number_of_data_objects: u3,
    _reserved2: bool,
}
