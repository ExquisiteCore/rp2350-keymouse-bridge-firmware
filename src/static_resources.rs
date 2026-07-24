//! Embassy USB 需要的静态缓冲和 class 状态。

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_usb::Handler;
use embassy_usb::class::cdc_acm::State as CdcState;
use static_cell::StaticCell;

use crate::coordinator::{CachedResponse, OwnedRequest, SafetyEvent};
use crate::runtime::{ExecutionJob, ExecutionResult};
use crate::runtime_safety::LeaseRefresh;
use crate::usb_device::{KeyboardLedHandler, UsbHidState};
use crate::usb_identity::USB_SERIAL_CAPACITY;

pub static REQUESTS: Channel<CriticalSectionRawMutex, OwnedRequest, 8> = Channel::new();
pub static JOBS: Channel<CriticalSectionRawMutex, ExecutionJob, 1> = Channel::new();
pub static RESULTS: Channel<CriticalSectionRawMutex, ExecutionResult, 2> = Channel::new();
pub static RESPONSES: Channel<CriticalSectionRawMutex, CachedResponse, 8> = Channel::new();
pub static SAFETY_EVENTS: Channel<CriticalSectionRawMutex, SafetyEvent, 4> = Channel::new();
pub static CANCEL: Signal<CriticalSectionRawMutex, u32> = Signal::new();
pub static LEASE_REFRESH: Signal<CriticalSectionRawMutex, LeaseRefresh> = Signal::new();
pub static REQUEST_RESET: Signal<CriticalSectionRawMutex, u32> = Signal::new();
pub static RESPONSE_RESET: Signal<CriticalSectionRawMutex, u32> = Signal::new();

pub struct RuntimeUsbHandler;

impl Handler for RuntimeUsbHandler {
    fn enabled(&mut self, enabled: bool) {
        if !enabled {
            crate::runtime::begin_session_reset();
            let _ = SAFETY_EVENTS.try_send(SafetyEvent::UsbDisabled);
        }
    }
}

pub fn static_runtime_usb_handler() -> &'static mut RuntimeUsbHandler {
    static CELL: StaticCell<RuntimeUsbHandler> = StaticCell::new();
    CELL.init(RuntimeUsbHandler)
}

pub fn static_keyboard_led_handler() -> &'static mut KeyboardLedHandler {
    static CELL: StaticCell<KeyboardLedHandler> = StaticCell::new();
    CELL.init(KeyboardLedHandler)
}

pub fn static_usb_serial_buffer() -> &'static mut [u8; USB_SERIAL_CAPACITY] {
    static CELL: StaticCell<[u8; USB_SERIAL_CAPACITY]> = StaticCell::new();
    CELL.init([0; USB_SERIAL_CAPACITY])
}

pub fn static_buf_512() -> &'static mut [u8; 512] {
    static CELL: StaticCell<[u8; 512]> = StaticCell::new();
    CELL.init([0; 512])
}

pub fn static_buf_256() -> &'static mut [u8; 256] {
    static CELL: StaticCell<[u8; 256]> = StaticCell::new();
    CELL.init([0; 256])
}

pub fn static_buf_64() -> &'static mut [u8; 64] {
    static CELL: StaticCell<[u8; 64]> = StaticCell::new();
    CELL.init([0; 64])
}

pub fn static_cdc_state() -> &'static mut CdcState<'static> {
    static CELL: StaticCell<CdcState<'static>> = StaticCell::new();
    CELL.init(CdcState::new())
}

pub fn static_hid_state_keyboard() -> &'static mut UsbHidState {
    static CELL: StaticCell<UsbHidState> = StaticCell::new();
    CELL.init(UsbHidState::new())
}

pub fn static_hid_state_mouse() -> &'static mut UsbHidState {
    static CELL: StaticCell<UsbHidState> = StaticCell::new();
    CELL.init(UsbHidState::new())
}
