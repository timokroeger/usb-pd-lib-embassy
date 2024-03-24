use bilge::arbitrary_int::*;
use defmt::*;
use embassy_stm32::ucpd;
use embassy_time::{with_timeout, Duration};

use crate::protocol::*;
use crate::protocol_engine::{HardReset, Message, ProtocolEngine};

/// Time to wait for a response.
const TIMEOUT_SENDER_RESPONSE: Duration = Duration::from_millis(30);

/// Time to wait for a PS_RDY message.
const TIMEOUT_PS_TRANSITION: Duration = Duration::from_millis(500);

pub struct PolicyEngine<'d, T: ucpd::Instance> {
    protocol_engine: ProtocolEngine<'d, T>,
    operating_current: u10, // 10mA resoultion
}

enum Error {
    HardReset,
    SoftReset,
}

impl From<HardReset> for Error {
    fn from(_: HardReset) -> Self {
        Self::HardReset
    }
}

impl<'d, T: ucpd::Instance> PolicyEngine<'d, T> {
    pub fn new(protocol_engine: ProtocolEngine<'d, T>, operating_current_ma: u16) -> Self {
        Self {
            protocol_engine,
            // Round up to next 10mA step
            operating_current: u10::new((operating_current_ma + 9) / 10),
        }
    }

    pub async fn run(&mut self) -> Result<(), HardReset> {
        let mut ready = false;
        loop {
            let mut obj_buf = [0; 7];
            match self.receive(&mut obj_buf).await {
                Ok(msg) => match self.handle_message(msg, ready).await {
                    Ok(r) => ready = r,
                    Err(Error::HardReset) => return Err(HardReset),
                    Err(Error::SoftReset) => ready = false,
                },
                Err(Error::HardReset) => return Err(HardReset),
                Err(Error::SoftReset) => ready = false,
            }
        }
    }

    async fn handle_message(&mut self, msg: Message<'_>, was_ready: bool) -> Result<bool, Error> {
        let mut ready = was_ready;
        match msg {
            Message::Control(ControlMessageType::Ping) => info!("Ignoring {}", msg),
            Message::Control(ControlMessageType::GetSinkCap) => {
                info!("Sending sink capabilites");
                self.sink_capabilities().await?;
            }
            Message::Data(DataMessageType::SourceCapabilites, _) => {
                info!("Source capablities received, starting power negotiation");
                if self.power_negotiation(was_ready).await? {
                    info!("Power negotiation finished");
                    ready = true;
                } else {
                    info!("Power negotiation unsuccessful");
                }
            }
            Message::Data(DataMessageType::VendorDefined, _) => info!("Ignoring {}", msg),
            msg => {
                info!("Rejecting unsupported message {}", msg);
                self.transmit(&Message::Control(ControlMessageType::Reject))
                    .await?;
            }
        }
        Ok(ready)
    }

    async fn receive<'m>(&mut self, obj_buf: &'m mut [u32]) -> Result<Message<'m>, Error> {
        match self.protocol_engine.receive(obj_buf).await? {
            Message::Control(ControlMessageType::SoftReset) => {
                warn!("Received SoftReset, sending Accept");
                self.transmit(&Message::Control(ControlMessageType::Accept))
                    .await?;
                Err(Error::SoftReset)
            }
            msg => Ok(msg),
        }
    }

    async fn receive_timeout<'m>(&mut self, timeout: Duration) -> Result<Message<'m>, Error> {
        let msg = with_timeout(timeout, self.receive(&mut []))
            .await
            .map_err(|_| {
                error!("Receive timeout");
                HardReset
            })??;
        Ok(msg)
    }

    async fn transmit(&mut self, msg: &Message<'_>) -> Result<(), Error> {
        if self.protocol_engine.transmit(msg).await? {
            Ok(())
        } else {
            self.transmit_soft_reset().await?;
            Err(Error::SoftReset)
        }
    }

    async fn transmit_soft_reset(&mut self) -> Result<(), HardReset> {
        if !self
            .protocol_engine
            .transmit(&Message::Control(ControlMessageType::SoftReset))
            .await?
        {
            error!("Error during SoftReset transmission");
            self.transmit_hard_reset().await;
            return Err(HardReset);
        }
        let msg = with_timeout(
            TIMEOUT_SENDER_RESPONSE,
            self.protocol_engine.receive(&mut []),
        )
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
        self.protocol_engine.transmit_hard_reset().await;
    }

    async fn power_negotiation(&mut self, _was_ready: bool) -> Result<bool, Error> {
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
        self.transmit(&Message::Data(DataMessageType::Request, &[obj.into()]))
            .await?;

        match self.receive_timeout(TIMEOUT_SENDER_RESPONSE).await? {
            Message::Control(ControlMessageType::Accept) => {}
            Message::Control(ControlMessageType::Reject | ControlMessageType::Wait) => {
                return Ok(false)
            }
            msg => {
                error!(
                    "Expected Reject or Wait message in renspone to Request, received {} instead",
                    msg
                );
                self.transmit_soft_reset().await?;
                return Err(Error::SoftReset);
            }
        };

        match self.receive_timeout(TIMEOUT_PS_TRANSITION).await? {
            Message::Control(ControlMessageType::PsRdy) => Ok(true),
            msg => {
                error!("Expected PS_RDY message, received {} instead", msg);
                self.transmit_soft_reset().await?;
                Err(Error::SoftReset)
            }
        }
    }

    async fn sink_capabilities(&mut self) -> Result<(), Error> {
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
        self.transmit(&Message::Data(
            DataMessageType::SinkCapabilities,
            &[obj.into()],
        ))
        .await
    }
}
