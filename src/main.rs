//! SPDX-License-Identifier: MIT OR Apache-2.0
//!
//! RP2350 USB HID 键鼠桥接固件。
//!
//! 设备枚举为 USB 复合设备：CDC 串口用于接收命令，HID Keyboard 和
//! HID Mouse 用于向操作系统发送标准键盘/鼠标报告。

#![no_std]
#![no_main]

mod command_executor;
mod commands;
mod error;
mod firmware_config;
mod frame_stream;
mod hid_report;
mod led;
mod protocol;
mod response_writer;
mod static_resources;
mod usb_device;
mod usb_identity;

use core::sync::atomic::{AtomicU8, Ordering};

use defmt::{info, warn};
use embassy_futures::join::join;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::USB;
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_time::Timer;
use embassy_usb::Builder;
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::class::hid::HidWriter;
use embassy_usb::driver::EndpointError;
use {defmt_rtt as _, panic_probe as _};

use command_executor::{DeviceResponse, InputState, execute_frame, reset_inputs};
use error::ErrorCode;
use frame_stream::{FrameAction, next_frame_action, sequence_from_partial, shift_left};
use led::{
    LED_MODE_DISCONNECTED, LED_SIGNAL_ACTIVITY, LED_SIGNAL_ERROR, LED_SIGNAL_NONE, LED_TICK_MS,
    LedAnimator, LedMode, LedSignal,
};
use protocol::{MAX_FRAME_SIZE, decode_frame};
use response_writer::{send_ack, send_nack, send_status};
use static_resources::{
    static_buf_64, static_buf_256, static_buf_512, static_cdc_state, static_hid_state_keyboard,
    static_hid_state_mouse,
};
use usb_device::{
    CdcClass, KeyboardWriter, MouseWriter, keyboard_config, mouse_config, usb_config,
};

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

    let mut builder = Builder::new(
        driver,
        usb_config(),
        static_buf_512(),
        static_buf_256(),
        &mut [],
        static_buf_64(),
    );

    let cdc_state = static_cdc_state();
    let mut cdc = CdcAcmClass::new(&mut builder, cdc_state, 64);

    let keyboard =
        HidWriter::<_, 8>::new(&mut builder, static_hid_state_keyboard(), keyboard_config());

    let mouse = HidWriter::<_, 8>::new(&mut builder, static_hid_state_mouse(), mouse_config());

    let mut usb = builder.build();

    let usb_fut = usb.run();
    let control_fut = control_loop(&mut cdc, keyboard, mouse);

    join(usb_fut, control_fut).await;
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

async fn control_loop(
    cdc: &mut CdcClass,
    mut keyboard: KeyboardWriter,
    mut mouse: MouseWriter,
) -> ! {
    let mut rx_packet = [0u8; 64];
    loop {
        set_led_mode(LedMode::Disconnected);
        cdc.wait_connection().await;
        info!("CDC connected");
        set_led_mode(LedMode::Connected);
        let mut frame_buf = [0u8; MAX_FRAME_SIZE];
        let mut frame_len = 0usize;
        let mut input_state = InputState::new();
        let _ = reset_inputs(&mut keyboard, &mut mouse, &mut input_state).await;

        loop {
            let read_len = match cdc.read_packet(&mut rx_packet).await {
                Ok(read_len) => read_len,
                Err(EndpointError::Disabled) => {
                    info!("CDC disconnected");
                    set_led_mode(LedMode::Disconnected);
                    break;
                }
                Err(EndpointError::BufferOverflow) => {
                    warn!("CDC packet overflow");
                    signal_led(LedSignal::Error);
                    let _ = send_nack(cdc, 0, ErrorCode::Transport).await;
                    frame_len = 0;
                    continue;
                }
            };

            if read_len == 0 {
                continue;
            }

            if frame_len + read_len > frame_buf.len() {
                warn!("frame buffer overflow");
                signal_led(LedSignal::Error);
                let _ = send_nack(cdc, 0, ErrorCode::FrameTooLong).await;
                frame_len = 0;
                continue;
            }

            frame_buf[frame_len..frame_len + read_len].copy_from_slice(&rx_packet[..read_len]);
            frame_len += read_len;

            while let Some(action) = next_frame_action(&frame_buf[..frame_len]) {
                match action {
                    FrameAction::NeedMore => break,
                    FrameAction::DropPrefix(count) => {
                        signal_led(LedSignal::Error);
                        shift_left(&mut frame_buf, &mut frame_len, count);
                    }
                    FrameAction::Reject {
                        len,
                        sequence,
                        error,
                    } => {
                        warn!("reject frame");
                        signal_led(LedSignal::Error);
                        let _ = send_nack(cdc, sequence, ErrorCode::from_decode(error)).await;
                        shift_left(&mut frame_buf, &mut frame_len, len);
                    }
                    FrameAction::Process(len) => {
                        let result = decode_frame(&frame_buf[..len]);
                        match result {
                            Ok(frame) => {
                                let sequence = frame.sequence;
                                match execute_frame(
                                    &frame,
                                    &mut keyboard,
                                    &mut mouse,
                                    &mut input_state,
                                )
                                .await
                                {
                                    Ok(DeviceResponse::Ack) => {
                                        let _ = send_ack(cdc, sequence).await;
                                        signal_led(LedSignal::Activity);
                                    }
                                    Ok(DeviceResponse::Info(payload)) => {
                                        let _ = send_status(cdc, sequence, &payload).await;
                                        signal_led(LedSignal::Activity);
                                    }
                                    Ok(DeviceResponse::Caps(payload)) => {
                                        let _ = send_status(cdc, sequence, &payload).await;
                                        signal_led(LedSignal::Activity);
                                    }
                                    Err(error) => {
                                        warn!("command failed: {}", error as u8);
                                        signal_led(LedSignal::Error);
                                        let _ = send_nack(cdc, sequence, error).await;
                                    }
                                }
                            }
                            Err(error) => {
                                warn!("decode failed");
                                signal_led(LedSignal::Error);
                                let sequence = sequence_from_partial(&frame_buf[..len]);
                                let _ =
                                    send_nack(cdc, sequence, ErrorCode::from_decode(error)).await;
                            }
                        }
                        shift_left(&mut frame_buf, &mut frame_len, len);
                    }
                }
            }
        }
    }
}
