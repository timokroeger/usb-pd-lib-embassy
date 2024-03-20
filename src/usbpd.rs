use bilge::prelude::*;
use embassy_stm32::ucpd::{Instance, PdPhy, RxError, TxError};

#[bitsize(4)]
#[derive(FromBits, Debug, Clone, Copy, PartialEq)]
pub enum ControlMessageType {
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
pub enum DataMessageType {
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
pub enum PortDataRole {
    UpstreamFacingPort,
    DownstreamFacingPort,
}

#[bitsize(2)]
#[derive(FromBits, Debug, Clone, Copy, PartialEq)]
pub enum SpecificationRevision {
    Revision1_0,
    Revision2_0,
    #[fallback]
    Reserved,
}

#[bitsize(1)]
#[derive(FromBits, Debug, Clone, Copy, PartialEq)]
pub enum PortPowerRole {
    Sink,
    Source,
}

#[bitsize(16)]
#[derive(FromBits, DebugBits, Clone, Copy)]
pub struct Header {
    message_type: u4,
    _reserved1: bool,
    port_data_role: PortDataRole,
    specification_revision: SpecificationRevision,
    port_power_role: PortPowerRole,
    message_id: u3,
    number_of_data_objects: u3,
    _reserved2: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Error {
    HardReset,
}

pub struct ProtocolEngine<'d, T: Instance> {
    phy: PdPhy<'d, T>,
    buf: [u8; 30],
    message_id: Option<u3>,
    header_template: Header,
}

impl<'d, T: Instance> ProtocolEngine<'d, T> {
    pub fn new(phy: PdPhy<'d, T>) -> Self {
        Self {
            phy,
            buf: [0; 30],
            message_id: None,
            // TODO: make configurable
            header_template: Header::new(
                u4::new(0),
                false,
                PortDataRole::UpstreamFacingPort,
                SpecificationRevision::Revision2_0,
                PortPowerRole::Sink,
                u3::new(0),
                u3::new(0),
                false,
            ),
        }
    }

    pub async fn receive(&mut self) -> Result<&[u8], Error> {
        loop {
            let n = match self.phy.receive(&mut self.buf).await {
                // Good reception, save received size.
                Ok(n) => n,
                // Ignore incomplete messages and messages with invalid CRC.
                Err(RxError::Crc | RxError::Overrun) => continue,
                // Forward hard reset errors to caller.
                Err(RxError::HardReset) => {
                    self.reset();
                    return Err(Error::HardReset);
                }
            };

            // Check message length.
            if n < 2 {
                continue;
            }
            let rx_header = Header::from(u16::from_le_bytes([self.buf[0], self.buf[1]]));
            if n != 2 + 4 * usize::from(rx_header.number_of_data_objects().value()) {
                continue;
            }

            if n == 2
                && ControlMessageType::from(rx_header.message_type())
                    == ControlMessageType::SoftReset
            {
                self.reset();
            }

            // Construct and transmit a GoodCRC response with a matching message id.
            let mut goodcrc_header = self.header_template;
            goodcrc_header.set_message_type(ControlMessageType::GoodCRC.into());
            goodcrc_header.set_message_id(rx_header.message_id());

            match self
                .phy
                .transmit(goodcrc_header.value.to_le_bytes().as_slice())
                .await
            {
                // Cannot send GoodCRC, ignore received data and wait for retransmission.
                Err(TxError::Discarded) => continue,
                // Forward hard reset errors to caller.
                Err(TxError::HardReset) => {
                    self.reset();
                    return Err(Error::HardReset);
                }
                // Good transmission
                Ok(()) => {}
            }

            // Perform message deduplicated based on message id.
            if self.message_id == Some(rx_header.message_id()) {
                continue;
            }
            self.message_id = Some(rx_header.message_id());

            return Ok(&self.buf[..n]);
        }
    }

    fn reset(&mut self) {
        self.message_id = None;
    }
}
