//! SPDX-License-Identifier: MIT OR Apache-2.0
//!
//! RP2350 USB HID 键鼠桥接固件。
//!
//! 设备枚举为 USB 复合设备：CDC 串口用于接收命令，HID Keyboard 和
//! HID Mouse 用于向操作系统发送标准键盘/鼠标报告。

#![no_std]
#![no_main]

#[allow(dead_code)]
mod batch;
#[allow(dead_code)]
mod command_executor;
mod commands;
#[allow(dead_code)]
mod coordinator;
mod error;
mod execution_core;
mod firmware_config;
#[allow(dead_code)]
mod frame_stream;
mod hid_report;
mod input_state;
#[allow(dead_code)]
mod led;
mod owned_command;
mod protocol;
mod response_writer;
mod runtime;
#[allow(dead_code)]
mod safety;
mod static_resources;
#[allow(dead_code)]
mod usb_device;
mod usb_identity;

use core::sync::atomic::{AtomicU8, Ordering};

use defmt::{info, warn};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::USB;
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_time::Timer;
use embassy_usb::Builder;
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::class::hid::{HidReaderWriter, HidWriter};
use {defmt_rtt as _, panic_probe as _};

use led::{
    LED_MODE_DISCONNECTED, LED_SIGNAL_ACTIVITY, LED_SIGNAL_ERROR, LED_SIGNAL_NONE, LED_TICK_MS,
    LedAnimator, LedMode, LedSignal,
};
use runtime::{
    cdc_control_task, cdc_receive_task, dispatcher_task, executor_task, keyboard_led_task,
    lease_task, response_task,
};
use static_resources::{
    static_buf_64, static_buf_256, static_buf_512, static_cdc_state, static_hid_state_keyboard,
    static_hid_state_mouse, static_keyboard_led_handler, static_runtime_usb_handler,
    static_usb_serial_buffer,
};
use usb_device::{keyboard_config, mouse_config, usb_config};
use usb_identity::format_usb_serial;

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

static LED_MODE: AtomicU8 = AtomicU8::new(LED_MODE_DISCONNECTED);
static LED_SIGNAL: AtomicU8 = AtomicU8::new(LED_SIGNAL_NONE);

#[embassy_executor::main]
async fn main(spawner: embassy_executor::Spawner) {
    info!("RP2350 USB HID bridge start");

    let p = embassy_rp::init(Default::default());
    match led_task(p.PIN_25) {
        Ok(task) => spawner.spawn(task),
        Err(_) => warn!("LED task spawn failed"),
    }
    let driver = Driver::new(p.USB, Irqs);
    let chip_id = embassy_rp::otp::get_chipid().expect("read RP2350 chip ID");
    let serial_number = format_usb_serial(chip_id, static_usb_serial_buffer())
        .expect("format RP2350 USB serial number");

    let mut builder = Builder::new(
        driver,
        usb_config(serial_number),
        static_buf_512(),
        static_buf_256(),
        &mut [],
        static_buf_64(),
    );

    builder.handler(static_runtime_usb_handler());

    let cdc = CdcAcmClass::new(&mut builder, static_cdc_state(), 64);

    let keyboard = HidReaderWriter::<_, 1, 8>::new(
        &mut builder,
        static_hid_state_keyboard(),
        keyboard_config(static_keyboard_led_handler()),
    );

    let mouse = HidWriter::<_, 8>::new(&mut builder, static_hid_state_mouse(), mouse_config());

    let (sender, receiver, control) = cdc.split_with_control();
    let (keyboard_reader, keyboard_writer) = keyboard.split();
    let mut usb = builder.build();

    match dispatcher_task() {
        Ok(task) => spawner.spawn(task),
        Err(_) => warn!("dispatcher task spawn failed"),
    }
    match executor_task(keyboard_writer, mouse) {
        Ok(task) => spawner.spawn(task),
        Err(_) => warn!("executor task spawn failed"),
    }
    match keyboard_led_task(keyboard_reader) {
        Ok(task) => spawner.spawn(task),
        Err(_) => warn!("keyboard LED task spawn failed"),
    }
    match response_task(sender) {
        Ok(task) => spawner.spawn(task),
        Err(_) => warn!("response task spawn failed"),
    }
    match cdc_receive_task(receiver) {
        Ok(task) => spawner.spawn(task),
        Err(_) => warn!("CDC receive task spawn failed"),
    }
    match cdc_control_task(control) {
        Ok(task) => spawner.spawn(task),
        Err(_) => warn!("CDC control task spawn failed"),
    }
    match lease_task() {
        Ok(task) => spawner.spawn(task),
        Err(_) => warn!("lease task spawn failed"),
    }

    usb.run().await;
}

#[embassy_executor::task]
async fn led_task(pin: embassy_rp::Peri<'static, embassy_rp::peripherals::PIN_25>) -> ! {
    let mut led = Output::new(pin, Level::Low);
    let mut animator = LedAnimator::new(LedMode::Disconnected);

    loop {
        animator.set_mode(LedMode::from_u8(LED_MODE.load(Ordering::Relaxed)));
        animator.signal(LedSignal::from_u8(
            LED_SIGNAL.swap(LED_SIGNAL_NONE, Ordering::AcqRel),
        ));

        if animator.next_output() {
            led.set_high();
        } else {
            led.set_low();
        }

        Timer::after_millis(LED_TICK_MS).await;
    }
}

fn set_led_mode(mode: LedMode) {
    LED_MODE.store(mode.as_u8(), Ordering::Relaxed);
}

fn signal_led(signal: LedSignal) {
    match signal {
        LedSignal::None => {}
        LedSignal::Activity => {
            let _ = LED_SIGNAL.compare_exchange(
                LED_SIGNAL_NONE,
                LED_SIGNAL_ACTIVITY,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        LedSignal::Error => {
            LED_SIGNAL.store(LED_SIGNAL_ERROR, Ordering::Release);
        }
    }
}
