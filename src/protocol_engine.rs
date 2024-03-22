use bilge::prelude::*;
use defmt::{debug, trace, warn, Format};
use embassy_stm32::ucpd::{Instance, PdPhy, RxError, TxError};
use embassy_time::{with_timeout, Duration, TimeoutError};
use safe_transmute::transmute_to_bytes_mut;

use crate::protocol::*;

const RETRY_COUNT: usize = 3;

/// Time to wait for a GoodCRC messages
const TIMEOUT_RECEIVE: Duration = Duration::from_millis(3);

#[derive(Debug, Format, PartialEq)]
pub enum Message<'o> {
    Control(ControlMessageType),
    Data(DataMessageType, &'o [u32]),
}

#[derive(Debug, Format, Clone, Copy)]
pub struct HardReset;

pub struct ProtocolEngine<'d, T: Instance> {
    phy: PdPhy<'d, T>,
    rx_message_id: Option<u3>,
    tx_message_id: u3,
    header_template: Header,
}

impl<'d, T: Instance> ProtocolEngine<'d, T> {
    pub fn new(phy: PdPhy<'d, T>) -> Self {
        Self {
            phy,
            rx_message_id: None,
            tx_message_id: u3::new(0),
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

    pub async fn receive<'o>(&mut self, obj_buf: &'o mut [u32]) -> Result<Message<'o>, HardReset> {
        loop {
            // Skip the first to bytes so that the header goes into byte 3 and 4
            // and the data starts at a 4 byte alignment which allows it to be
            // transmuted to &[u32].
            let mut raw_buf = [0_u32; 8];
            let buf = &mut transmute_to_bytes_mut(&mut raw_buf)[2..];

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

            // Handle soft reset.
            if num_objects == 0 && rx_header.message_type() == ControlMessageType::SoftReset.into()
            {
                self.rx_message_id = None;
                self.tx_message_id = u3::new(0);
            }

            // Perform message deduplicated based on message id.
            if self.rx_message_id == Some(rx_header.message_id()) {
                debug!("RX duplicate message");
                continue;
            }
            self.rx_message_id = Some(rx_header.message_id());

            let msg = if num_objects == 0 {
                Message::Control(ControlMessageType::from(rx_header.message_type()))
            } else {
                let truncated_obj_len = obj_buf.len().min(num_objects);
                for i in 0..obj_buf.len().min(num_objects) {
                    obj_buf[i] = raw_buf[i + 1].to_le();
                }
                Message::Data(
                    DataMessageType::from(rx_header.message_type()),
                    &obj_buf[..truncated_obj_len],
                )
            };
            debug!("Received {}", msg);
            return Ok(msg);
        }
    }

    pub async fn transmit(&mut self, msg: &Message<'_>) -> Result<bool, HardReset> {
        debug!("Transmitting {}", msg);

        if let Message::Control(ControlMessageType::SoftReset) = msg {
            self.rx_message_id = None;
        }

        let mut raw_buf = [0_u32; 8];
        let (msg_type, num_objects): (u4, usize) = match *msg {
            Message::Control(hdr) => (hdr.into(), 0),
            Message::Data(hdr, data) => {
                raw_buf[1..1 + data.len()].copy_from_slice(data);
                (hdr.into(), data.len())
            }
        };

        let mut tx_header = self.header_template;
        tx_header.set_message_id(self.tx_message_id);
        tx_header.set_message_type(msg_type);
        tx_header.set_number_of_data_objects(u3::new(num_objects as _));

        let mut ok = false;
        for _retry in 0..=RETRY_COUNT {
            // Skip the first to bytes to put the header right before the data objects.
            // Transmuting must be done inside the loop to please the borrow checker.
            let buf = &mut transmute_to_bytes_mut(&mut raw_buf)[2..2 + 2 + 4 * num_objects];
            [buf[0], buf[1]] = u16::from(tx_header).to_le_bytes();

            trace!("TX {=[u8]:x} retry={=usize}", buf, _retry);
            match self.phy.transmit(buf).await {
                Ok(()) => {}
                // Retry when line not idle.
                Err(TxError::Discarded) => {
                    warn!("TX {=[u8]:x} retry={=usize} discarded", buf, _retry);
                    continue;
                }
                // Forward hard reset to caller.
                Err(TxError::HardReset) => self.handle_hard_reset()?,
            }

            let mut goodcrc_buf = [0_u8; 2];
            match with_timeout(TIMEOUT_RECEIVE, self.phy.receive(&mut goodcrc_buf)).await {
                Ok(Ok(2)) => {
                    let goodcrc =
                        Header::from(u16::from_le_bytes([goodcrc_buf[0], goodcrc_buf[1]]));
                    if goodcrc.number_of_data_objects() != u3::new(0)
                        || goodcrc.message_type() != ControlMessageType::GoodCRC.into()
                        || goodcrc.message_id() != self.tx_message_id
                    {
                        warn!(
                            "TX retry={=usize} Received invalid GoodCRC message {=[u8]:x}",
                            _retry, goodcrc_buf
                        );
                        continue;
                    }
                    trace!("RX {=[u8]:x} GoodCRC", goodcrc_buf);
                    ok = true;
                    break;
                }
                Ok(Ok(_)) | Ok(Err(RxError::Crc | RxError::Overrun)) => {
                    warn!(
                        "TX retry={=usize} Expected GoodCRC but received invalid data",
                        _retry
                    );
                    continue;
                }
                Ok(Err(RxError::HardReset)) => self.handle_hard_reset()?,
                Err(TimeoutError) => {
                    warn!("TX retry={=usize} GoodCRC timeout", _retry);
                    continue;
                }
            }
        }

        self.tx_message_id.wrapping_add(u3::new(1));
        Ok(ok)
    }

    pub async fn transmit_hard_reset(&mut self) {
        debug!("Transmitting HardReset");
        let _ = self.phy.transmit_hardreset().await;
    }

    fn handle_hard_reset(&mut self) -> Result<(), HardReset> {
        debug!("Received HardReset");
        self.rx_message_id = None;
        self.tx_message_id = u3::new(0);
        Err(HardReset)
    }
}
