#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

mod policy_engine;
mod protocol;
mod protocol_engine;

use core::pin::pin;

use defmt::{panic, *};
use embassy_futures::select::select;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::rcc::{Hse, HseMode, Pll, PllMul, PllPreDiv, PllRDiv, PllSource, Sysclk};
use embassy_stm32::time::mhz;
use embassy_stm32::ucpd::{CcPhy, CcPull, CcSel, CcVState, Ucpd};
use embassy_stm32::{bind_interrupts, peripherals, ucpd, Config};
use embassy_time::{with_timeout, Duration};
use policy_engine::PolicyEngine;
use protocol_engine::ProtocolEngine;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    UCPD1 => ucpd::InterruptHandler<peripherals::UCPD1>;
});

#[derive(Debug, Format)]
enum CableOrientation {
    Normal,
    Flipped,
    DebugAccessoryMode,
}

// Returns true when the cable
async fn wait_attached<T: ucpd::Instance>(cc_phy: &mut CcPhy<'_, T>) -> CableOrientation {
    loop {
        let (cc1, cc2) = cc_phy.vstate();
        if cc1 == CcVState::LOWEST && cc2 == CcVState::LOWEST {
            // Detached, wait until attached by monitoring the CC lines.
            cc_phy.wait_for_vstate_change().await;
            continue;
        }

        // Attached, wait for CC lines to be stable for tCCDebounce (100..200ms).
        if with_timeout(Duration::from_millis(100), cc_phy.wait_for_vstate_change())
            .await
            .is_ok()
        {
            // State has changed, restart detection procedure.
            continue;
        };

        // State was stable for the complete debounce period, check orientation.
        return match (cc1, cc2) {
            (_, CcVState::LOWEST) => CableOrientation::Normal, // CC1 connected
            (CcVState::LOWEST, _) => CableOrientation::Flipped, // CC2 connected
            _ => CableOrientation::DebugAccessoryMode,         // Both connected (special cable)
        };
    }
}

// Using the CC lines to detect cable detach is not spec compliant.
// The correct approach is be to monitor VBUS using an additional pin
// Use the CC lines nevertheless to keep the example simple.
async fn wait_detach<T: ucpd::Instance>(cc_phy: &mut CcPhy<'_, T>) {
    while !matches!(
        cc_phy.wait_for_vstate_change().await,
        (CcVState::LOWEST, CcVState::LOWEST)
    ) {}
    info!("USB cable detached");
}

#[cortex_m_rt::entry]
fn main() -> ! {
    let mut config = Config::default();
    config.enable_ucpd1_dead_battery = true;
    config.rcc.hse = Some(Hse {
        freq: mhz(8),
        mode: HseMode::Oscillator,
    });
    config.rcc.pll = Some(Pll {
        source: PllSource::HSE,
        prediv: PllPreDiv::DIV1,
        mul: PllMul::MUL42,
        divp: None,
        divq: None,
        divr: Some(PllRDiv::DIV2),
    });
    config.rcc.boost = true;
    config.rcc.sys = Sysclk::PLL1_R;
    let mut p = embassy_stm32::init(config);
    info!("Hello World!");

    let mut led = Output::new(p.PC6, Level::High, Speed::High);
    //let mut button = ExtiInput::new(p.PC13, p.EXTI13, Pull::Down);

    let my_task = pin!(async {
        loop {
            let mut ucpd = Ucpd::new(&mut p.UCPD1, Irqs {}, &mut p.PB6, &mut p.PB4);
            ucpd.cc_phy().set_pull(CcPull::Sink);

            info!("Waiting for USB connection...");
            let cable_orientation = wait_attached(ucpd.cc_phy()).await;
            info!("USB cable attached, orientation: {}", cable_orientation);

            let cc_sel = match cable_orientation {
                CableOrientation::Normal => {
                    info!("Starting PD communication on CC1 pin");
                    CcSel::CC1
                }
                CableOrientation::Flipped => {
                    info!("Starting PD communication on CC2 pin");
                    CcSel::CC2
                }
                CableOrientation::DebugAccessoryMode => panic!("No PD communication in DAM"),
            };

            let (mut cc_phy, pd_phy) = ucpd.split_pd_phy(&p.DMA1_CH1, &mut p.DMA1_CH2, cc_sel);
            let protocol_engine = ProtocolEngine::new(pd_phy);
            let mut policy_engine = PolicyEngine::new(protocol_engine, 100);

            select(wait_detach(&mut cc_phy), async {
                policy_engine.run_sink().await
            })
            .await;
            //wait_detach(&mut cc_phy).await;

            led.toggle();
        }
    });

    lilos::exec::run_tasks(&mut [my_task], lilos::exec::ALL_TASKS)
}

#[defmt::panic_handler]
fn defmt_panic() -> ! {
    cortex_m::asm::udf();
}
