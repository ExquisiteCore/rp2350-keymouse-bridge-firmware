//! SPDX-License-Identifier: MIT OR Apache-2.0
//!
//! Copyright (c) 2021-2024 The rp-rs Developers
//! Copyright (c) 2021 rp-rs organization
//! Copyright (c) 2025 Raspberry Pi Ltd.
//!
//! 配置 RP2350 链接脚本。

use std::path::PathBuf;

const DEFAULT_USB_VENDOR_ID: u16 = 0xCAFE;
const DEFAULT_USB_PRODUCT_ID: u16 = 0x2350;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(rp2350)");

    // 把 memory.x 写到链接器能够搜索到的目录。
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    println!("cargo:rustc-link-search={}", out.display());

    std::fs::write(out.join("memory.x"), include_bytes!("rp2350.x"))
        .expect("write RP2350 memory.x");

    let vendor_id = usb_id_from_env("RP2350_USB_VID", DEFAULT_USB_VENDOR_ID);
    let product_id = usb_id_from_env("RP2350_USB_PID", DEFAULT_USB_PRODUCT_ID);
    let usb_ids = format!(
        "pub const USB_VENDOR_ID: u16 = 0x{vendor_id:04X};\n\
         pub const USB_PRODUCT_ID: u16 = 0x{product_id:04X};\n"
    );
    std::fs::write(out.join("usb_ids.rs"), usb_ids).expect("write generated USB IDs");

    println!("cargo::rustc-cfg=rp2350");
    println!("cargo:rerun-if-changed=rp2350.x");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RP2350_USB_VID");
    println!("cargo:rerun-if-env-changed=RP2350_USB_PID");
}

fn usb_id_from_env(name: &str, default: u16) -> u16 {
    let Some(raw) = std::env::var_os(name) else {
        return default;
    };
    let raw = raw.to_string_lossy();
    let value = raw.trim();
    let parsed = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or_else(|| value.parse::<u16>(), |hex| u16::from_str_radix(hex, 16));
    parsed.unwrap_or_else(|_| {
        panic!("invalid {name} value `{raw}`: expected a decimal or 0x-prefixed hexadecimal u16")
    })
}
