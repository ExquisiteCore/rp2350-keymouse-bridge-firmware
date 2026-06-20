//! Embassy USB 需要的静态缓冲和 class 状态。

use embassy_usb::class::cdc_acm::State as CdcState;
use static_cell::StaticCell;

use crate::usb_device::UsbHidState;

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
