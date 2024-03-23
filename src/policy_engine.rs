use bilge::arbitrary_int::*;
use defmt::*;
use embassy_stm32::ucpd;
use embassy_time::{with_timeout, Duration, TimeoutError};

use crate::protocol::*;
use crate::protocol_engine::{HardReset, Message, ProtocolEngine};

/// Time to wait for a response.
const TIMEOUT_SENDER_RESPONSE: Duration = Duration::from_millis(30);

/// Time to wait for a PS_RDY message.
const TIMEOUT_PS_TRANSITION: Duration = Duration::from_millis(500);

#[derive(Debug, Format)]
pub enum Error<'m> {
    HardReset,
    Timeout,
    UnexpectedMessage(Message<'m>),
}

impl From<HardReset> for Error<'_> {
    fn from(_: HardReset) -> Self {
        Self::HardReset
    }
}

impl From<TimeoutError> for Error<'_> {
    fn from(_: TimeoutError) -> Self {
        Self::Timeout
    }
}

pub struct PolicyEngine<'d, T: ucpd::Instance> {
    pe: ProtocolEngine<'d, T>,
    operating_current: u10, // 10mA resoultion
}

impl<'d, T: ucpd::Instance> PolicyEngine<'d, T> {
    pub fn new(pe: ProtocolEngine<'d, T>, operating_current_ma: u16) -> Self {
        Self {
            pe,
            // Round up to next 10mA step
            operating_current: u10::new((operating_current_ma + 9) / 10),
        }
    }

    pub async fn run(&mut self) -> Result<(), HardReset> {
        loop {
            let mut obj_buf = [0; 7];
            let msg = self.pe.receive(&mut obj_buf).await?;
            match msg {
                Message::Control(ControlMessageType::Ping) => info!("Ignoring {}", msg),
                Message::Control(ControlMessageType::GetSinkCap) => {
                    info!("Sending sink capabilites");
                    self.sink_capabilities().await?;
                }
                Message::Data(DataMessageType::SourceCapabilites, _) => {
                    info!("Source capablities received, starting power negotiation");
                    match self.power_negotiation().await {
                        Ok(true) => info!("Power negotiation finished"),
                        Ok(false) => info!("Power negotiation unsuccessful"),
                        Err(Error::HardReset) => return Err(HardReset),
                        Err(Error::Timeout) => {
                            error!("timeout");
                            self.transmit_hard_reset().await;
                            return Err(HardReset);
                        }
                        Err(Error::UnexpectedMessage(msg)) => {
                            error!(
                                "Received unexpected message {} during power negotiation",
                                msg
                            );
                            self.transmit_soft_reset().await?;
                        }
                    }
                }
                Message::Data(DataMessageType::VendorDefined, _) => info!("Ignoring {}", msg),
                msg => {
                    info!("Rejecting unsupported message {}", msg);
                    self.pe
                        .transmit(&Message::Control(ControlMessageType::Reject))
                        .await?;
                }
            };
        }
    }

    async fn receive<'o>(&mut self, obj_buf: &'o mut [u32]) -> Result<Message<'o>, HardReset> {
        let msg = self.pe.receive(obj_buf).await?;
        if msg == Message::Control(ControlMessageType::SoftReset) {
            warn!("Received SoftReset, sending Accept");
            self.pe
                .transmit(&Message::Control(ControlMessageType::Accept))
                .await?;
        }
        Ok(msg)
    }

    async fn transmit_soft_reset(&mut self) -> Result<(), HardReset> {
        self.pe
            .transmit(&Message::Control(ControlMessageType::SoftReset))
            .await?;
        let msg = with_timeout(TIMEOUT_SENDER_RESPONSE, self.pe.receive(&mut []))
            .await
            .map_err(|_| HardReset)??;
        if msg != Message::Control(ControlMessageType::Accept) {
            error!(
                "Expected Accept message in renspone to SoftReset, received {} instead",
                msg
            );
            self.transmit_hard_reset().await;
            return Err(HardReset);
        };
        Ok(())
    }

    async fn transmit_hard_reset(&mut self) {
        // TODO: implement hard reset counter
        self.pe.transmit_hard_reset().await;
    }

    async fn power_negotiation(&mut self) -> Result<bool, Error> {
        // TODO: simple constructor in protocol module.
        // default 5V
        let obj = Request::new(
            self.operating_current,
            self.operating_current,
            u4::new(0),
            false,
            false,
            false,
            false,
            u3::new(1),
            false,
        );
        self.pe
            .transmit(&Message::Data(DataMessageType::Request, &[obj.into()]))
            .await?;

        let msg = with_timeout(TIMEOUT_SENDER_RESPONSE, self.receive(&mut [])).await??;
        match msg {
            Message::Control(ControlMessageType::Accept) => {}
            Message::Control(ControlMessageType::Reject | ControlMessageType::Wait) => {
                return Ok(false)
            }
            _ => return Err(Error::UnexpectedMessage(msg)),
        };

        let msg = with_timeout(TIMEOUT_PS_TRANSITION, self.receive(&mut [])).await??;
        if msg != Message::Control(ControlMessageType::PsRdy) {
            return Err(Error::UnexpectedMessage(msg));
        };

        Ok(true)
    }

    async fn sink_capabilities(&mut self) -> Result<(), HardReset> {
        // default 5V
        let obj = sink_capabilities::FixedSupply::new(
            self.operating_current,
            u10::new(10), // 50mV resolution
            u5::new(0),
            false,
            false,
            false,
            false,
            false,
            u2::new(0),
        );
        self.pe
            .transmit(&Message::Data(
                DataMessageType::SinkCapabilities,
                &[obj.into()],
            ))
            .await?;
        Ok(())
    }
}
