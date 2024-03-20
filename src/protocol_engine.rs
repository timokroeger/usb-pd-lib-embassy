use bilge::prelude::*;
use defmt::{debug, trace, warn, Format};
use embassy_stm32::ucpd::{Instance, PdPhy, RxError, TxError};
use safe_transmute::transmute_to_bytes_mut;

use crate::protocol::*;

#[derive(Debug, Format)]
pub enum Message<'a> {
    Control(ControlMessageType),
    Data(DataMessageType, &'a [u32]),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Error {
    HardReset,
}

pub struct ProtocolEngine<'d, T: Instance> {
    phy: PdPhy<'d, T>,
    buf: [u32; 8],
    message_id: Option<u3>,
    header_template: Header,
}

impl<'d, T: Instance> ProtocolEngine<'d, T> {
    pub fn new(phy: PdPhy<'d, T>) -> Self {
        Self {
            phy,
            buf: [0; 8],
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

    pub async fn receive(&mut self) -> Result<Message, Error> {
        loop {
            // Skip the first to bytes so that the header goes into byte 3 and 4
            // and the data starts at a 4 byte alignment which allows it to be
            // transmuted to &[u32].
            let buf = &mut transmute_to_bytes_mut(&mut self.buf)[2..];

            let n = match self.phy.receive(buf).await {
                // Good reception, save received size.
                Ok(n) => n,
                // Ignore incomplete messages and messages with invalid CRC.
                Err(RxError::Crc | RxError::Overrun) => continue,
                // Forward hard reset errors to caller.
                Err(RxError::HardReset) => {
                    self.handle_hard_reset()?;
                    unreachable!()
                }
            };

            // Check message length.
            if n < 2 {
                warn!("RX {=[u8]:x} message too short", buf);
                continue;
            }
            let rx_header = Header::from(u16::from_le_bytes([buf[0], buf[1]]));
            let num_objects = usize::from(rx_header.number_of_data_objects().value());
            let expected_len = 2 + 4 * num_objects;
            if n != expected_len {
                warn!(
                    "RX {=[u8]:x} invalid message length, expected {=usize} bytes",
                    buf[..n],
                    expected_len,
                );
                continue;
            }

            trace!("RX {=[u8]:x}", buf[..n]);

            // Construct and transmit a GoodCRC response with a matching message id.
            let mut goodcrc_header = self.header_template;
            goodcrc_header.set_message_type(ControlMessageType::GoodCRC.into());
            goodcrc_header.set_message_id(rx_header.message_id());

            let tx_buf = u16::from(goodcrc_header).to_le_bytes();
            match self.phy.transmit(&tx_buf).await {
                // Cannot send GoodCRC, ignore received data and wait for retransmission.
                Err(TxError::Discarded) => warn!("TX {=[u8]:x} GoodCRC Discarded", tx_buf),
                // Forward hard reset errors to caller.
                Err(TxError::HardReset) => self.handle_hard_reset()?,
                // Good transmission
                Ok(()) => trace!("TX {=[u8]:x} GoodCRC", tx_buf),
            }

            let is_softreset = num_objects == 0
                && rx_header.message_type() == ControlMessageType::SoftReset.into();

            // Perform message deduplicated based on message id.
            // Skip check for soft reset messages which have a message id of 0.
            if !is_softreset && self.message_id == Some(rx_header.message_id()) {
                debug!("RX duplicate message");
                continue;
            }
            self.message_id = Some(rx_header.message_id());

            return Ok(if num_objects == 0 {
                Message::Control(ControlMessageType::from(rx_header.message_type()))
            } else {
                let objects = &mut self.buf[1..1 + num_objects];
                objects.iter_mut().map(|obj| *obj = obj.to_le());
                Message::Data(
                    DataMessageType::from(rx_header.message_type()),
                    objects,
                )
            });
        }
    }

    fn handle_hard_reset(&mut self) -> Result<(), Error> {
        debug!("HardReset");
        self.message_id = None;
        Err(Error::HardReset)
    }
}
