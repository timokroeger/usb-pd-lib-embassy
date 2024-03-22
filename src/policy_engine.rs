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
}

impl<'d, T: ucpd::Instance> PolicyEngine<'d, T> {
    pub fn new(pe: ProtocolEngine<'d, T>) -> Self {
        Self { pe }
    }

    pub async fn run(&mut self) -> Result<(), HardReset> {
        loop {
            let mut obj_buf = [0; 7];
            let msg = self.pe.receive(&mut obj_buf).await?;
            match msg {
                Message::Data(DataMessageType::SourceCapabilites, _) => {
                    match self.power_negotiation().await {
                        Ok(true) => info!("Power Negotiation finished"),
                        Ok(false) => info!("Power Negotiation unsuccessful"),
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
                // TODO: Reject unsupported messages
                msg => info!("Ignoring message {}", msg),
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
        // default 5V, operating and max current = 50mA
        let obj = Request::new(
            u10::new(5),
            u10::new(5),
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
            Message::Control(ControlMessageType::Reject) => return Ok(false),
            _ => return Err(Error::UnexpectedMessage(msg)),
        };

        let msg = with_timeout(TIMEOUT_PS_TRANSITION, self.receive(&mut [])).await??;
        if msg != Message::Control(ControlMessageType::PsRdy) {
            return Err(Error::UnexpectedMessage(msg));
        };

        Ok(true)
    }
}
