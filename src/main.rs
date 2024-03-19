#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

mod usbpd;

use core::pin::pin;

use defmt::*;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::{bind_interrupts, peripherals, ucpd, Config};
use embassy_time::{Duration, Ticker};
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    UCPD1 => ucpd::InterruptHandler<peripherals::UCPD1>;
});

#[cortex_m_rt::entry]
fn main() -> ! {
    let p = embassy_stm32::init(Config::default());
    info!("Hello World!");

    let mut led = Output::new(p.PC6, Level::High, Speed::High);
    //let mut button = ExtiInput::new(p.PC13, p.EXTI13, Pull::Down);

    let my_task = pin!(async {
        let mut ticker = Ticker::every(Duration::from_millis(500));
        loop {
            led.toggle();
            ticker.next().await;
        }
    });

    lilos::exec::run_tasks(&mut [my_task], lilos::exec::ALL_TASKS)
}

#[defmt::panic_handler]
fn defmt_panic() -> ! {
    cortex_m::asm::udf();
}
