//! USB 复合设备配置和固件内使用的 USB 类型别名。

use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver;
use embassy_usb::Config;
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::class::hid::{
    Config as HidConfig, HidBootProtocol, HidSubclass, HidWriter, State as HidState,
};
use usbd_hid::descriptor::{KeyboardReport, MouseReport, SerializedDescriptor};

use crate::firmware_config::{USB_PRODUCT_ID, USB_VENDOR_ID};
use crate::usb_identity::{USB_MANUFACTURER, USB_PRODUCT, USB_SERIAL_NUMBER};

pub type UsbDriver = Driver<'static, USB>;
pub type CdcClass = CdcAcmClass<'static, UsbDriver>;
pub type KeyboardWriter = HidWriter<'static, UsbDriver, 8>;
pub type MouseWriter = HidWriter<'static, UsbDriver, 8>;
pub type UsbHidState = HidState<'static>;

pub fn usb_config() -> Config<'static> {
    let mut config = Config::new(USB_VENDOR_ID, USB_PRODUCT_ID);
    config.manufacturer = Some(USB_MANUFACTURER);
    config.product = Some(USB_PRODUCT);
    config.serial_number = Some(USB_SERIAL_NUMBER);
    config.max_power = 100;
    config.max_packet_size_0 = 64;
    config.composite_with_iads = true;
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config
}

pub fn keyboard_config() -> HidConfig<'static> {
    HidConfig {
        report_descriptor: KeyboardReport::desc(),
        request_handler: None,
        poll_ms: 1,
        max_packet_size: 64,
        hid_subclass: HidSubclass::Boot,
        hid_boot_protocol: HidBootProtocol::Keyboard,
    }
}

pub fn mouse_config() -> HidConfig<'static> {
    HidConfig {
        report_descriptor: MouseReport::desc(),
        request_handler: None,
        poll_ms: 1,
        max_packet_size: 64,
        hid_subclass: HidSubclass::Boot,
        hid_boot_protocol: HidBootProtocol::Mouse,
    }
}
